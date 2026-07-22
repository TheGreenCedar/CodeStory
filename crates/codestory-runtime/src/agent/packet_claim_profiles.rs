#[cfg(test)]
use crate::agent::eval_probes::eval_probes_enabled;
use crate::agent::packet_citations::packet_citation_source_text;
use crate::agent::packet_evidence_roles::{
    PacketEvidenceRole, packet_citation_owns_interceptor_management,
    packet_citation_owns_request_pipeline, packet_evidence_role,
};
use crate::agent::packet_flow_requirements::{CoverageMode, FlowRole};
use crate::agent::packet_scoring::{normalize_identifier, packet_display_path};
use crate::agent::packet_source_patterns::{
    packet_display_owner, packet_human_join, packet_source_constructed_type, packet_source_has_all,
    packet_source_has_any, packet_source_identifier_ending_with, packet_source_identifier_exact,
    packet_source_identifier_with_words, packet_source_identifier_with_words_shortest,
    packet_sql_create_table_names, packet_sql_foreign_key_claims,
};
use crate::agent::packet_terms::{
    packet_probe_terms, packet_terms_indicate_buffered_io_flow,
    packet_terms_indicate_client_send_flow, packet_terms_indicate_event_loop_command_flow,
    packet_terms_indicate_form_validation_flow, packet_terms_indicate_hook_cache_flow,
    packet_terms_indicate_html_css_template_structure_flow,
    packet_terms_indicate_log_record_handler_flow,
    packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_request_dispatch_flow, packet_terms_indicate_runtime_formatting_flow,
    packet_terms_indicate_search_execution_flow,
    packet_terms_indicate_server_request_dispatch_flow,
    packet_terms_indicate_server_route_dispatch_flow,
    packet_terms_indicate_shell_install_dispatch_flow,
    packet_terms_indicate_shell_version_use_flow, packet_terms_indicate_site_build_phase_flow,
    packet_terms_indicate_sql_schema_flow, packet_terms_indicate_string_predicate_flow,
    packet_terms_indicate_stylesheet_animation_flow,
    packet_terms_indicate_url_session_request_flow,
};
use codestory_contracts::api::{AgentCitationDto, NodeKind};
use std::collections::HashSet;

const GENERIC_PRODUCT_CLAIM_PROFILES: &[SourceClaimProductProfile] = &[
    SourceClaimProductProfile::pending(SourceClaimProfile::ServerRoute),
    SourceClaimProductProfile::pending(SourceClaimProfile::ServerRequestDispatch),
    SourceClaimProductProfile::contracted(
        SourceClaimProfile::ShellInstallDispatch,
        SourceClaimProfileContract {
            domain: "shell-install-dispatch",
            scope: SourceClaimProfileScope::Product,
            allowed_evidence_tier: CoverageMode::AllowsLexicalSource,
            allowed_proof_roles: &[
                FlowRole::Entrypoint,
                FlowRole::Dispatch,
                FlowRole::TerminalBoundary,
            ],
            positive_fixture_id: "generic-shell-install-dispatch-positive",
            false_positive_fixture_id: "generic-shell-install-dispatch-helper-negative",
        },
    ),
    SourceClaimProductProfile::contracted(
        SourceClaimProfile::ShellVersionUse,
        SourceClaimProfileContract {
            domain: "shell-version-use",
            scope: SourceClaimProfileScope::Product,
            allowed_evidence_tier: CoverageMode::AllowsLexicalSource,
            allowed_proof_roles: &[FlowRole::Dispatch],
            positive_fixture_id: "generic-shell-version-use-positive",
            false_positive_fixture_id: "generic-shell-version-use-helper-negative",
        },
    ),
    SourceClaimProductProfile::pending(SourceClaimProfile::HookCache),
    SourceClaimProductProfile::pending(SourceClaimProfile::ClientSend),
    SourceClaimProductProfile::pending(SourceClaimProfile::UrlSessionRequest),
    SourceClaimProductProfile::pending(SourceClaimProfile::StringPredicate),
    SourceClaimProductProfile::pending(SourceClaimProfile::StylesheetAnimation),
    SourceClaimProductProfile::pending(SourceClaimProfile::HtmlCssTemplateStructure),
    SourceClaimProductProfile::pending(SourceClaimProfile::SqlSchema),
    SourceClaimProductProfile::pending(SourceClaimProfile::RuntimeFormatting),
    SourceClaimProductProfile::pending(SourceClaimProfile::LoggerHandlerFlow),
    SourceClaimProductProfile::pending(SourceClaimProfile::SiteBuildPhase),
    SourceClaimProductProfile::contracted(
        SourceClaimProfile::MappingConfigurationPlan,
        SourceClaimProfileContract {
            domain: "object-mapping-plan",
            scope: SourceClaimProfileScope::Product,
            allowed_evidence_tier: CoverageMode::RequiresResolvedSourceOrGraph,
            allowed_proof_roles: &[FlowRole::Configuration, FlowRole::Dispatch],
            positive_fixture_id: "generic-object-mapping-plan-positive",
            false_positive_fixture_id: "generic-object-mapping-cache-helper-negative",
        },
    ),
    SourceClaimProductProfile::pending(SourceClaimProfile::FormValidation),
    SourceClaimProductProfile::contracted(
        SourceClaimProfile::ClientRequestDispatch,
        SourceClaimProfileContract {
            domain: "session-request-dispatch",
            scope: SourceClaimProfileScope::Product,
            allowed_evidence_tier: CoverageMode::RequiresResolvedSourceOrGraph,
            allowed_proof_roles: &[
                FlowRole::Entrypoint,
                FlowRole::Dispatch,
                FlowRole::TransformOrValidate,
                FlowRole::TerminalBoundary,
            ],
            positive_fixture_id: "generic-session-request-dispatch-positive",
            false_positive_fixture_id: "generic-session-request-transport-negative",
        },
    ),
    SourceClaimProductProfile::pending(SourceClaimProfile::EventLoopCommand),
    SourceClaimProductProfile::pending(SourceClaimProfile::SearchExecution),
    SourceClaimProductProfile::pending(SourceClaimProfile::BufferedIo),
];

#[derive(Debug, Clone, Copy)]
struct SourceClaimProductProfile {
    profile: SourceClaimProfile,
    contract: SourceClaimProfileContractStatus,
}

impl SourceClaimProductProfile {
    const fn pending(profile: SourceClaimProfile) -> Self {
        Self {
            profile,
            contract: SourceClaimProfileContractStatus::PendingMigration,
        }
    }

    const fn contracted(profile: SourceClaimProfile, contract: SourceClaimProfileContract) -> Self {
        Self {
            profile,
            contract: SourceClaimProfileContractStatus::Contracted(contract),
        }
    }

    fn collect(&self, ctx: &SourceClaimContext<'_>, claims: &mut Vec<String>) {
        self.contract.assert_valid();
        self.profile.collect(ctx, claims);
    }
}

#[derive(Debug, Clone, Copy)]
enum SourceClaimProfileContractStatus {
    Contracted(SourceClaimProfileContract),
    PendingMigration,
}

impl SourceClaimProfileContractStatus {
    fn assert_valid(self) {
        if let Self::Contracted(contract) = self {
            debug_assert!(matches!(contract.scope, SourceClaimProfileScope::Product));
            debug_assert!(matches!(
                contract.allowed_evidence_tier,
                CoverageMode::RequiresResolvedSourceOrGraph
                    | CoverageMode::AllowsSourceRange
                    | CoverageMode::AllowsLexicalSource
                    | CoverageMode::DiagnosticOnly
            ));
            debug_assert!(!contract.domain.is_empty());
            debug_assert!(!contract.allowed_proof_roles.is_empty());
            debug_assert!(!contract.positive_fixture_id.is_empty());
            debug_assert!(!contract.false_positive_fixture_id.is_empty());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceClaimProfileContract {
    domain: &'static str,
    scope: SourceClaimProfileScope,
    allowed_evidence_tier: CoverageMode,
    allowed_proof_roles: &'static [FlowRole],
    positive_fixture_id: &'static str,
    false_positive_fixture_id: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceClaimProfileScope {
    Product,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceClaimProfile {
    ServerRoute,
    ServerRequestDispatch,
    ShellInstallDispatch,
    ShellVersionUse,
    HookCache,
    ClientSend,
    UrlSessionRequest,
    StringPredicate,
    StylesheetAnimation,
    HtmlCssTemplateStructure,
    SqlSchema,
    RuntimeFormatting,
    LoggerHandlerFlow,
    SiteBuildPhase,
    MappingConfigurationPlan,
    FormValidation,
    ClientRequestDispatch,
    EventLoopCommand,
    SearchExecution,
    BufferedIo,
}

impl SourceClaimProfile {
    fn collect(self, ctx: &SourceClaimContext<'_>, claims: &mut Vec<String>) {
        match self {
            Self::ServerRoute => {
                if packet_terms_indicate_server_route_dispatch_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_server_route_flow_claims(
                        ctx.symbol, ctx.source,
                    ));
                }
            }
            Self::ServerRequestDispatch => {
                if packet_terms_indicate_server_request_dispatch_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_server_request_dispatch_flow_claims(
                        ctx.symbol, ctx.source, &ctx.path,
                    ));
                }
            }
            Self::ShellInstallDispatch => {
                if packet_terms_indicate_shell_install_dispatch_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_shell_install_dispatch_flow_claims(
                        ctx.symbol,
                        &ctx.file_name,
                        ctx.source,
                    ));
                }
            }
            Self::ShellVersionUse => {
                if packet_terms_indicate_shell_version_use_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_shell_version_use_flow_claims(
                        ctx.symbol, ctx.source,
                    ));
                }
            }
            Self::HookCache => {
                if packet_terms_indicate_hook_cache_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_hook_cache_flow_claims(
                        ctx.symbol, ctx.source,
                    ));
                }
            }
            Self::ClientSend => {
                if packet_terms_indicate_client_send_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_client_send_flow_claims(
                        ctx.symbol, ctx.source,
                    ));
                }
            }
            Self::UrlSessionRequest => {
                if packet_terms_indicate_url_session_request_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_url_session_request_flow_claims(
                        ctx.symbol, ctx.source,
                    ));
                }
            }
            Self::StringPredicate => {
                if packet_terms_indicate_string_predicate_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_string_predicate_flow_claims(
                        ctx.symbol, ctx.source,
                    ));
                }
            }
            Self::StylesheetAnimation => {
                if packet_terms_indicate_stylesheet_animation_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_css_animation_flow_claims(ctx.source));
                }
            }
            Self::HtmlCssTemplateStructure => {
                if packet_terms_indicate_html_css_template_structure_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_html_css_template_structure_claims(
                        &ctx.file_name,
                        ctx.source,
                    ));
                }
            }
            Self::SqlSchema => {
                if packet_terms_indicate_sql_schema_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_sql_schema_flow_claims(ctx.source));
                }
            }
            Self::RuntimeFormatting => {
                if packet_terms_indicate_runtime_formatting_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_runtime_formatting_flow_claims(
                        ctx.symbol,
                        &ctx.file_name,
                        ctx.source,
                    ));
                }
            }
            Self::LoggerHandlerFlow => {
                if packet_terms_indicate_log_record_handler_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_log_record_handler_flow_claims(
                        ctx.symbol, ctx.source,
                    ));
                }
            }
            Self::SiteBuildPhase => {
                if packet_terms_indicate_site_build_phase_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_site_build_phase_flow_claims(ctx.source));
                }
            }
            Self::MappingConfigurationPlan => {
                if packet_terms_indicate_mapper_configuration_plan_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_mapper_configuration_plan_claims(
                        ctx.symbol,
                        ctx.kind,
                        &ctx.file_name,
                        ctx.source,
                    ));
                }
            }
            Self::FormValidation => {
                if packet_terms_indicate_form_validation_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_form_validation_flow_claims(
                        ctx.symbol, ctx.source,
                    ));
                }
            }
            Self::ClientRequestDispatch => collect_client_request_dispatch_claims(ctx, claims),
            Self::EventLoopCommand => collect_event_loop_command_claims(ctx, claims),
            Self::SearchExecution => collect_search_execution_claims(ctx, claims),
            Self::BufferedIo => collect_buffered_io_claims(ctx, claims),
        }
    }
}

struct SourceClaimContext<'a> {
    source: &'a str,
    symbol: &'a str,
    kind: NodeKind,
    evidence_role: Option<PacketEvidenceRole>,
    owns_request_pipeline: bool,
    owns_interceptor_management: bool,
    path: String,
    file_name: String,
    normalized_prompt: String,
    prompt_terms: Vec<String>,
}

impl<'a> SourceClaimContext<'a> {
    fn new(prompt: &str, citation: &'a AgentCitationDto, source: &'a str) -> Self {
        let symbol = citation.display_name.as_str();
        let path = citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .unwrap_or_default();
        let file_name = path
            .rsplit(['/', '\\'])
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(symbol)
            .to_string();
        Self {
            source,
            symbol,
            kind: citation.kind,
            evidence_role: packet_evidence_role(citation),
            owns_request_pipeline: packet_citation_owns_request_pipeline(citation),
            owns_interceptor_management: packet_citation_owns_interceptor_management(citation),
            path,
            file_name,
            normalized_prompt: normalize_identifier(prompt),
            prompt_terms: packet_probe_terms(prompt),
        }
    }
}

pub(crate) fn packet_source_derived_claims_for_citation(
    prompt: &str,
    citation: &AgentCitationDto,
    source: &str,
) -> Vec<String> {
    let mut claims = Vec::new();
    let ctx = SourceClaimContext::new(prompt, citation, source);

    #[cfg(test)]
    if eval_probes_enabled() {
        claims.extend(
            crate::agent::eval_probes::source_derived_claims_for_citation(prompt, citation, source),
        );
    }

    for profile in GENERIC_PRODUCT_CLAIM_PROFILES {
        profile.collect(&ctx, &mut claims);
    }

    claims
}

pub(crate) fn packet_source_derived_claim_for_role(
    _role: PacketEvidenceRole,
    citation: &AgentCitationDto,
    prompt: &str,
) -> Option<String> {
    let source = packet_citation_source_text(citation)?;
    if source.len() > 800_000 {
        return None;
    }
    let ctx = SourceClaimContext::new(prompt, citation, &source);
    let request_flow = packet_terms_indicate_request_dispatch_flow(&ctx.prompt_terms);

    if request_flow && citation_can_own_behavior(&ctx) {
        if let Some(claim) = python_like_request_dispatch_claim_for_role(&ctx) {
            return Some(claim);
        }
        if let Some(claim) = client_request_claim_for_citation(&ctx) {
            return Some(claim);
        }
    }

    #[cfg(test)]
    {
        let eval_diagnostics = eval_probes_enabled();
        let command_flow = packet_terms_indicate_event_loop_command_flow(&ctx.prompt_terms);
        let search_flow = packet_terms_indicate_search_execution_flow(&ctx.prompt_terms);

        if eval_diagnostics && command_flow && event_loop_prompt(&ctx) {
            if let Some(claim) = event_loop_entry_claim(&ctx) {
                return Some(claim);
            }
            if let Some(claim) = event_loop_process_events_claim(&ctx) {
                return Some(claim);
            }
        }

        if eval_diagnostics
            && command_flow
            && _role == PacketEvidenceRole::NetworkCommandInput
            && let Some(claim) = network_command_input_claim(&ctx)
        {
            return Some(claim);
        }

        if eval_diagnostics && command_flow && _role == PacketEvidenceRole::CommandDispatch {
            if let Some(claim) = command_dispatch_table_claim(&ctx) {
                return Some(claim);
            }
            if let Some(claim) = command_dispatch_call_claim(&ctx) {
                return Some(claim);
            }
        }

        if eval_diagnostics
            && search_flow
            && _role == PacketEvidenceRole::SearchDriver
            && let Some(claim) = search_driver_claim(&ctx)
        {
            return Some(claim);
        }

        if eval_diagnostics
            && search_flow
            && _role == PacketEvidenceRole::ArgumentPlanning
            && let Some(claim) = argument_planning_claim(&ctx)
        {
            return Some(claim);
        }

        if eval_diagnostics
            && search_flow
            && _role == PacketEvidenceRole::SearchExecutionUnit
            && let Some(claim) = search_execution_state_claim(&ctx)
        {
            return Some(claim);
        }

        if eval_diagnostics
            && search_flow
            && let Some(claim) = search_walk_claim(&ctx)
        {
            return Some(claim);
        }

        if eval_diagnostics
            && search_flow
            && let Some(claim) = parallel_search_claim(&ctx)
        {
            return Some(claim);
        }

        if eval_diagnostics
            && search_flow
            && let Some(claim) = search_execution_method_claim(&ctx)
        {
            return Some(claim);
        }
    }

    None
}

fn push_optional_claim(claims: &mut Vec<String>, claim: Option<String>) {
    if let Some(claim) = claim {
        claims.push(claim);
    }
}

fn collect_client_request_dispatch_claims(ctx: &SourceClaimContext<'_>, claims: &mut Vec<String>) {
    if !packet_terms_indicate_request_dispatch_flow(&ctx.prompt_terms) {
        return;
    }
    if !citation_can_own_behavior(ctx) {
        return;
    }

    claims.extend(python_like_request_dispatch_claims(ctx));
    push_optional_claim(claims, client_request_claim_for_citation(ctx));
}

fn collect_buffered_io_claims(ctx: &SourceClaimContext<'_>, claims: &mut Vec<String>) {
    if !packet_terms_indicate_buffered_io_flow(&ctx.prompt_terms) {
        return;
    }

    push_optional_claim(claims, buffered_io_buffer_storage_claim(ctx));
    push_optional_claim(claims, buffered_io_source_wrapper_claim(ctx));
    push_optional_claim(claims, buffered_io_sink_wrapper_claim(ctx));
    push_optional_claim(claims, buffered_io_helper_claim(ctx));
}

fn buffered_io_buffer_storage_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    let normalized_symbol = normalize_identifier(ctx.symbol);
    let normalized_source = normalize_identifier(ctx.source);
    let lower = ctx.source.to_ascii_lowercase();
    if (normalized_symbol == "buffer"
        || normalized_symbol.ends_with("buffer")
        || normalized_symbol.contains("bytebuffer"))
        && (lower.contains("class buffer")
            || lower.contains("struct buffer")
            || lower.contains("interface buffer")
            || lower.contains("expect class buffer")
            || lower.contains("actual class buffer")
            || lower.contains("typealias buffer"))
        && (normalized_source.contains("read") || normalized_source.contains("write"))
        && (normalized_source.contains("byte") || normalized_source.contains("segment"))
    {
        return Some(
            "Buffer is the in-memory byte store used by buffered reads and writes.".to_string(),
        );
    }
    None
}

fn buffered_io_source_wrapper_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    let normalized_symbol = normalize_identifier(ctx.symbol);
    let normalized_source = normalize_identifier(ctx.source);
    let lower = ctx.source.to_ascii_lowercase();
    let source_wrapper_symbol = normalized_symbol.contains("bufferedsource")
        || normalized_symbol.ends_with("source")
        || (normalized_symbol.contains("buffered") && normalized_symbol.contains("reader"));
    let source_wrapper_body = normalized_source.contains("source")
        && normalized_source.contains("buffer")
        && (normalized_source.contains("read") || normalized_source.contains("request"))
        && (lower.contains("override fun")
            || lower.contains("class ")
            || lower.contains("struct "));
    if source_wrapper_symbol && source_wrapper_body {
        return Some(
            "A buffered source wrapper reads from an upstream Source into a Buffer.".to_string(),
        );
    }
    None
}

fn buffered_io_sink_wrapper_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    let normalized_symbol = normalize_identifier(ctx.symbol);
    let normalized_source = normalize_identifier(ctx.source);
    let lower = ctx.source.to_ascii_lowercase();
    let sink_wrapper_symbol = normalized_symbol.contains("bufferedsink")
        || normalized_symbol.ends_with("sink")
        || (normalized_symbol.contains("buffered") && normalized_symbol.contains("writer"));
    let sink_wrapper_body = normalized_source.contains("sink")
        && normalized_source.contains("buffer")
        && (normalized_source.contains("write")
            || normalized_source.contains("emit")
            || normalized_source.contains("flush"))
        && (lower.contains("override fun")
            || lower.contains("class ")
            || lower.contains("struct "));
    if sink_wrapper_symbol && sink_wrapper_body {
        return Some(
            "A buffered sink wrapper writes buffered bytes to an upstream Sink.".to_string(),
        );
    }
    None
}

fn buffered_io_helper_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    let normalized_source = normalize_identifier(ctx.source);
    let lower = ctx.source.to_ascii_lowercase();
    let source_buffer_helper = lower.contains("fun source.buffer")
        || normalized_source.contains("sourcebuffer")
        || (normalized_source.contains("source")
            && normalized_source.contains("bufferedsource")
            && normalized_source.contains("buffer"));
    let sink_buffer_helper = lower.contains("fun sink.buffer")
        || normalized_source.contains("sinkbuffer")
        || (normalized_source.contains("sink")
            && normalized_source.contains("bufferedsink")
            && normalized_source.contains("buffer"));
    if source_buffer_helper && sink_buffer_helper {
        return Some(
            "Buffering helpers wrap Source and Sink instances with buffered implementations."
                .to_string(),
        );
    }
    None
}

fn client_factory_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_all(ctx.source, &["new ", "prototype", "request", "extend"]) {
        let context = packet_source_constructed_type(ctx.source).unwrap_or_else(|| "client".into());
        return Some(format!(
            "`{}` wraps a {context} context and exposes verb helpers bound to request.",
            ctx.symbol
        ));
    }
    None
}

fn client_request_pipeline_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if ctx.owns_request_pipeline
        && packet_source_has_all(ctx.source, &["merge", "config", "interceptors", "request"])
        && packet_source_has_any(ctx.source, &["dispatch", "adapter"])
        && let Some(owner) = packet_display_owner(ctx.symbol)
    {
        let dispatch = packet_source_identifier_with_words(ctx.source, &["dispatch", "request"])
            .unwrap_or_else(|| "request dispatch".to_string());
        return Some(format!(
            "{owner}.request merges defaults, runs request interceptors, then calls {dispatch}."
        ));
    }
    None
}

fn request_dispatch_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_all(ctx.source, &["adapter", "transform"])
        && packet_source_has_any(ctx.source, &["headers", "data", "body"])
    {
        return Some(format!(
            "`{}` transforms the body/headers and invokes the configured adapter, sending the request through that transport.",
            ctx.symbol
        ));
    }
    None
}

fn interceptor_management_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if ctx.owns_interceptor_management
        && packet_source_has_any(ctx.source, &["handler", "handlers"])
    {
        return Some(format!(
            "`{}` stores request interceptor handler pairs for chained execution.",
            ctx.symbol
        ));
    }
    None
}

fn transport_adapter_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_all(
        ctx.source,
        &[
            "knownadapters",
            "getadapter",
            "return adapter",
            "xhr",
            "http",
        ],
    ) {
        return Some(format!(
            "`{}` selects xhr or http transport based on environment capabilities.",
            ctx.file_name
        ));
    }
    None
}

fn client_request_claim_for_citation(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if let Some(claim) = client_request_pipeline_claim(ctx) {
        return Some(claim);
    }
    if ctx.kind == NodeKind::FILE {
        return transport_adapter_claim(ctx);
    }
    match ctx.evidence_role {
        Some(PacketEvidenceRole::ClientFactory) => client_factory_claim(ctx),
        Some(PacketEvidenceRole::RequestDispatch) => request_dispatch_claim(ctx),
        Some(PacketEvidenceRole::InterceptorManagement) => interceptor_management_claim(ctx),
        Some(PacketEvidenceRole::TransportAdapter) => transport_adapter_claim(ctx),
        _ => None,
    }
}

fn citation_can_own_behavior(ctx: &SourceClaimContext<'_>) -> bool {
    !matches!(
        ctx.kind,
        NodeKind::MODULE | NodeKind::NAMESPACE | NodeKind::PACKAGE
    )
}

fn python_like_request_dispatch_claim_for_role(ctx: &SourceClaimContext<'_>) -> Option<String> {
    let normalized_symbol = normalize_identifier(ctx.symbol);
    if normalized_symbol == "request" {
        return top_level_session_request_delegate_claim(ctx);
    }
    if normalized_symbol.contains("sessionrequest") {
        return session_request_prepares_claim(ctx);
    }
    if normalized_symbol.contains("sessionsend") {
        return session_send_adapter_claim(ctx);
    }
    if normalized_symbol.contains("adaptersend") {
        return http_adapter_send_claim(ctx);
    }
    if let Some(claim) = prepared_request_prepare_claim(ctx) {
        return Some(claim);
    }
    if let Some(claim) = http_adapter_send_claim(ctx) {
        return Some(claim);
    }
    None
}

fn python_like_request_dispatch_claims(ctx: &SourceClaimContext<'_>) -> Vec<String> {
    let mut claims = Vec::new();
    push_optional_claim(&mut claims, top_level_session_request_delegate_claim(ctx));
    push_optional_claim(&mut claims, session_request_prepares_claim(ctx));
    push_optional_claim(&mut claims, prepared_request_prepare_claim(ctx));
    push_optional_claim(&mut claims, session_send_adapter_claim(ctx));
    push_optional_claim(&mut claims, http_adapter_send_claim(ctx));
    claims
}

fn top_level_session_request_delegate_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    let lower = ctx.source.to_ascii_lowercase();
    if packet_source_has_any(
        ctx.source,
        &["session() as session", "sessions.session() as session"],
    ) && lower.contains("session.")
        && lower.contains("request(")
    {
        return Some(
            "The top-level request helper opens a session object and delegates to the session request method."
                .to_string(),
        );
    }
    None
}

fn session_request_prepares_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    let lower = ctx.source.to_ascii_lowercase();
    if lower.contains("def request(")
        && lower.contains("request(")
        && lower.contains("self.prepare_request(")
    {
        return Some(
            "The session request method creates a request object and prepares it into a transport-ready request object."
                .to_string(),
        );
    }
    None
}

fn prepared_request_prepare_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    let lower = ctx.source.to_ascii_lowercase();
    if lower.contains("def prepare(")
        && lower.contains("prepare_method(")
        && lower.contains("prepare_url(")
        && lower.contains("prepare_body(")
    {
        return Some(
            "Request preparation builds the method, URL, headers, cookies, body, auth, and hooks."
                .to_string(),
        );
    }
    None
}

fn session_send_adapter_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    let lower = ctx.source.to_ascii_lowercase();
    if lower.contains("def send(")
        && lower.contains("get_adapter(")
        && lower.contains("adapter.send(")
    {
        return Some(
            "The session send method chooses an adapter and calls the adapter send method."
                .to_string(),
        );
    }
    None
}

fn http_adapter_send_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    let normalized_source = normalize_identifier(ctx.source);
    if normalized_source.contains("class")
        && normalized_source.contains("adapter")
        && normalized_source.contains("defsend")
        && normalized_source.contains("connurlopen")
        && normalized_source.contains("buildresponse")
    {
        return Some("The transport adapter send path is the response boundary.".to_string());
    }
    None
}

fn collect_event_loop_command_claims(ctx: &SourceClaimContext<'_>, claims: &mut Vec<String>) {
    if !packet_terms_indicate_event_loop_command_flow(&ctx.prompt_terms) {
        return;
    }

    if event_loop_prompt(ctx) {
        push_optional_claim(claims, event_loop_entry_claim(ctx));
        push_optional_claim(claims, event_loop_process_events_claim(ctx));
    }

    push_optional_claim(claims, network_command_input_claim(ctx));
    push_optional_claim(claims, command_dispatch_table_claim(ctx));
    push_optional_claim(claims, command_dispatch_call_claim(ctx));
}

fn event_loop_prompt(ctx: &SourceClaimContext<'_>) -> bool {
    ctx.normalized_prompt.contains("eventloop")
        || (ctx.normalized_prompt.contains("event") && ctx.normalized_prompt.contains("loop"))
}

fn event_loop_entry_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_all(ctx.source, &["init", "event"])
        && let Some(loop_entry) = packet_source_identifier_ending_with(ctx.source, "Main", "main")
        && packet_source_identifier_exact(ctx.source, "main").is_some()
    {
        return Some(format!(
            "main initializes the server and enters {loop_entry} on the shared event loop."
        ));
    }
    None
}

fn event_loop_process_events_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if let Some(process_events) =
        packet_source_identifier_with_words(ctx.source, &["process", "events"])
        && packet_source_has_any(ctx.source, &["readable", "writable"])
    {
        return Some(format!(
            "{process_events} polls readable/writable fds and invokes registered file event handlers."
        ));
    }
    None
}

fn network_command_input_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if let Some(read_client) = packet_source_identifier_with_words(ctx.source, &["read", "client"])
        && let Some(process_input) =
            packet_source_identifier_with_words(ctx.source, &["process", "input", "buffer"])
    {
        return Some(format!(
            "{read_client} appends socket input and drives {process_input} when a full command is available."
        ));
    }
    None
}

fn command_dispatch_table_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if let Some(process_command) =
        packet_source_identifier_with_words(ctx.source, &["process", "command"])
        && packet_source_has_any(ctx.source, &["lookup", "arity", "acl", "cluster"])
    {
        return Some(format!(
            "{process_command} resolves the command table entry and enforces ACL, arity, and cluster checks."
        ));
    }
    None
}

fn command_dispatch_call_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if let Some(call) = packet_source_identifier_exact(ctx.source, "call")
        && packet_source_has_all(ctx.source, &["proc", "propagat"])
        && packet_source_has_any(ctx.source, &["slowlog", "monitor"])
    {
        return Some(format!(
            "{call} executes the command proc and handles propagation, monitoring, and slowlog accounting."
        ));
    }
    None
}

fn collect_search_execution_claims(ctx: &SourceClaimContext<'_>, claims: &mut Vec<String>) {
    if !packet_terms_indicate_search_execution_flow(&ctx.prompt_terms) {
        return;
    }

    push_optional_claim(claims, search_driver_claim(ctx));
    push_optional_claim(claims, argument_planning_claim(ctx));
    push_optional_claim(claims, search_execution_state_claim(ctx));
    push_optional_claim(claims, search_walk_claim(ctx));
    push_optional_claim(claims, parallel_search_claim(ctx));
    push_optional_claim(claims, search_execution_method_claim(ctx));
}

fn search_driver_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_all(ctx.source, &["flags", "parse", "search"])
        && let Some(main) = packet_source_identifier_exact(ctx.source, "main")
    {
        let run = packet_source_identifier_exact(ctx.source, "run").unwrap_or_else(|| "run".into());
        return Some(format!(
            "{main} delegates parsed search options into {run} for search execution."
        ));
    }
    None
}

fn argument_planning_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_all(ctx.source, &["walk", "matcher", "searcher", "printer"]) {
        let owner = packet_display_owner(ctx.symbol)
            .or_else(|| packet_source_identifier_with_words_shortest(ctx.source, &["args"]))
            .unwrap_or_else(|| ctx.symbol.to_string());
        return Some(format!(
            "`{owner}` builds traversal, matching, search, and output components used by the search pipeline."
        ));
    }
    None
}

fn search_execution_state_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_all(ctx.source, &["matcher", "searcher", "printer"])
        && packet_source_has_any(ctx.source, &["candidate", "file", "input", "path"])
    {
        let execution_unit =
            packet_source_identifier_with_words_shortest(ctx.source, &["search", "worker"])
                .unwrap_or_else(|| ctx.symbol.to_string());
        return Some(format!(
            "`{execution_unit}` carries matching, search, and output state for each candidate input."
        ));
    }
    None
}

fn search_walk_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_all(ctx.source, &["searcher", "search"])
        && packet_source_has_any(ctx.source, &["candidate", "file", "path", "walk"])
        && let Some(execution_unit) =
            packet_source_identifier_with_words_shortest(ctx.source, &["search", "worker"])
    {
        return Some(format!(
            "candidate traversal invokes {execution_unit} for each file selected by the search walk."
        ));
    }
    None
}

fn parallel_search_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_any(ctx.source, &["parallel", "concurrent", "rayon", "thread"])
        && packet_source_has_any(ctx.source, &["candidate", "file", "path", "walk"])
        && let Some(parallel_search) =
            packet_source_identifier_with_words_shortest(ctx.source, &["search", "parallel"])
    {
        return Some(format!(
            "{parallel_search} uses parallel traversal to search candidate files concurrently."
        ));
    }
    None
}

fn search_execution_method_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_all(ctx.source, &["matcher", "searcher", "printer"])
        && let Some(execution_unit) =
            packet_source_identifier_with_words_shortest(ctx.source, &["search", "worker"])
        && let Some(search_method) = packet_source_identifier_exact(ctx.source, "search")
    {
        return Some(format!(
            "{execution_unit}::{search_method} executes one candidate search with matching, search, and output state."
        ));
    }
    None
}

fn packet_generic_hook_cache_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let normalized_source = normalize_identifier(source);
    let mut claims = Vec::new();
    let cache_helper_call = source_shape_has_cache_helper_call(&normalized_source);

    if source_lower.contains("export default")
        && let Some((public_hook, handler)) = packet_source_argument_wrapper(source)
    {
        claims.push(format!(
            "The public {public_hook} export wraps {handler} with argument normalization."
        ));
    }

    if normalized_source.contains("stablehash") && normalized_source.contains("returnkeyargs") {
        claims.push(format!("{symbol} serializes hook keys into cache keys."));
    }

    if source_lower.contains("serialize(_key)")
        && (source_lower.contains("getcache")
            || (source_lower.contains("create")
                && source_lower.contains("cache")
                && source_lower.contains("helper"))
            || source_lower.contains("cache"))
    {
        claims.push(format!(
            "{symbol} serializes the key before reading cache state."
        ));
    }

    if normalized_source.contains("exportasyncfunction")
        && normalized_source.contains("serialize")
        && cache_helper_call
        && normalized_source.contains("mutatebykey")
    {
        claims.push(format!(
            "{symbol} routes mutate behavior through the mutation helper."
        ));
    }

    if normalized_source.contains("middleware")
        && normalized_source.contains("hook")
        && normalized_source.contains("configuse")
        && source_shape_has_hook_return_call(&normalized_source)
    {
        claims.push(format!(
            "{symbol} composes middleware around a public hook."
        ));
    }

    if source_lower.contains("cache.get(key)")
        && source_lower.contains("return [")
        && (source_lower.contains("cache.set(key")
            || source_lower.contains("state[5]")
            || source_lower.contains("setter"))
        && (source_lower.contains("subscribe")
            || source_lower.contains("state[6]")
            || source_lower.contains("subscriber"))
        && (source_lower.contains("snapshot")
            || source_lower.contains("initial_cache")
            || source_lower.contains("initial cache"))
    {
        claims.push(format!(
            "{symbol} provides cache get, set, subscribe, and snapshot helpers."
        ));
    }

    claims
}

fn source_shape_has_cache_helper_call(normalized_source: &str) -> bool {
    normalized_source.contains("cachehelper") && normalized_source.contains("create")
}

fn source_shape_has_hook_return_call(normalized_source: &str) -> bool {
    normalized_source.contains("returnuse") && normalized_source.contains("hook")
}

fn packet_generic_client_send_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let normalized_source = normalize_identifier(source);
    let mut claims = Vec::new();
    let owner = packet_display_owner(symbol).unwrap_or_else(|| symbol.to_string());

    if source_lower.contains("_withclient")
        && source_lower.contains("client()")
        && source_lower.contains("client.")
        && packet_source_has_any(source, &["get(", "post(", "put("])
        && source_lower.contains("future<response>")
    {
        claims.push("Top-level HTTP helpers delegate to a Client.".to_string());
    }

    if normalized_source.contains("interfaceclassclient")
        && normalized_source.contains("futureresponse")
        && normalized_source.contains("futurestreamedresponsesend")
        && normalized_source.contains("request")
    {
        claims.push(
            "Client interface helper methods declare convenience request helpers and send(request)."
                .to_string(),
        );
    }

    if source_lower.contains("_sendunstreamed")
        && source_lower.contains("response.fromstream")
        && source_lower.contains("send(request)")
        && (source_lower.contains("future<response>")
            || source_lower.contains("response>")
            || source_lower.contains("response "))
        && packet_source_has_any(source, &["get(", "post(", "put(", "patch(", "delete("])
    {
        claims.push(format!(
            "{owner} implements convenience methods in terms of send."
        ));
        claims.push("Response.fromStream materializes the response stream boundary.".to_string());
    }

    if normalized_source.contains("classrequestextends")
        && normalized_source.contains("bytestreamfinalize")
        && normalized_source.contains("frombytesbodybytes")
    {
        claims.push(format!(
            "{owner}.finalize prepares the request body for sending."
        ));
    }

    if normalized_source.contains("classresponseextendsbaseresponse")
        && normalized_source.contains("fromstreamstreamedresponseresponse")
        && normalized_source.contains("responsestreamtobytes")
    {
        claims.push("Response.fromStream materializes the response stream boundary.".to_string());
    }

    if source_lower.contains("dart:io")
        && source_lower.contains("httpclient")
        && source_lower.contains("openurl")
        && source_lower.contains("request.finalize")
        && source_lower.contains("stream.pipe")
        && source_lower.contains("httpclientresponse")
    {
        claims.push(format!(
            "{owner}.send is the dart:io transport implementation that forwards finalized requests through an HTTP client."
        ));
    }

    if source_lower.contains("bytestream finalize()")
        && source_lower.contains("_finalized = true")
        && source_lower.contains("request body")
    {
        claims.push(format!(
            "{owner}.finalize prepares the request body for sending."
        ));
    }

    claims
}

fn packet_generic_form_validation_flow_claims(_symbol: &str, source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    let has_native_constraints = source_lower.contains("<form")
        && source_lower.contains("required")
        && source_lower.contains("pattern")
        && source_lower.contains("min=")
        && source_lower.contains("max=");
    if has_native_constraints {
        claims.push(
            "The form validation examples use native required, pattern, min, and max constraints."
                .to_string(),
        );
    }

    if source_lower.contains("<form")
        && source_lower.contains("validity")
        && (source_lower.contains("addeventlistener")
            || source_lower.contains("checkvalidity")
            || source_lower.contains("setcustomvalidity"))
    {
        claims.push(
            "A custom validation example applies script-driven validity checks before rendering messages."
                .to_string(),
        );
    }

    if source_lower.contains("validity.valuemissing")
        && source_lower.contains("validity.typemismatch")
        && source_lower.contains("validity.tooshort")
    {
        claims.push(
            "Custom error rendering branches on ValidityState fields to choose messages."
                .to_string(),
        );
    }

    if source_lower.contains("addeventlistener('submit'")
        && source_lower.contains("validity.valid")
        && source_lower.contains("preventdefault")
    {
        claims.push("Submit handling prevents invalid form submission.".to_string());
    }

    claims
}

fn packet_generic_url_session_request_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if (normalized_symbol == "session" || normalized_symbol.ends_with("sessionrequest"))
        && source_lower.contains("open func request")
        && source_lower.contains("let request =")
        && source_lower.contains("performeagerlyifnecessary")
    {
        claims.push(
            "The session request API creates request objects before optional eager execution."
                .to_string(),
        );
    }

    if normalized_symbol.ends_with("requestresume")
        && source_lower.contains("public func resume() -> self")
        && source_lower.contains("task.resume()")
    {
        claims.push("The request resume API resumes the underlying URL session task.".to_string());
    }

    if normalized_symbol.ends_with("validate")
        && source_lower.contains("public func validate(_ validation")
        && source_lower.contains("validators.write")
        && source_lower.contains("didvalidate")
        && source_lower.contains("request")
    {
        claims.push("Request validation methods attach validation behavior.".to_string());
    }

    if normalized_symbol.ends_with("delegate")
        && source_lower.contains("urlsessiondatadelegate")
        && source_lower.contains("open func urlsession")
        && (source_lower.contains("request.didreceiveresponse")
            || source_lower.contains("request.didreceive(data: data)")
            || source_lower.contains("didcompletewitherror"))
    {
        claims.push("Session delegate callbacks receive URLSession task events.".to_string());
    }

    claims
}

pub(crate) fn packet_generic_string_predicate_flow_claims(
    symbol: &str,
    source: &str,
) -> Vec<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_symbol.ends_with("isblank")
        && let Some(method) = packet_source_method_block(source, "boolean", "isBlank")
    {
        let method_lower = method.to_ascii_lowercase();
        let null_empty_whitespace_documented = source_lower.contains("null, empty or whitespace")
            || source_lower.contains("null, empty, or whitespace")
            || source_lower.contains("null, empty and whitespace");
        if method_lower.contains("character.iswhitespace")
            && (method_lower.contains("null") || null_empty_whitespace_documented)
            && method_lower.contains("length")
        {
            let symbol = packet_predicate_claim_symbol(symbol, "isBlank");
            claims.push(format!(
                "{symbol} treats null, empty, and whitespace-only inputs as blank."
            ));
        }
    }

    if normalized_symbol.ends_with("isempty")
        && let Some(method) = packet_source_method_block(source, "boolean", "isEmpty")
    {
        let method_lower = method.to_ascii_lowercase();
        if method_lower.contains("null")
            && method_lower.contains("length()")
            && !method_lower.contains("trim(")
            && !method_lower.contains(".trim")
            && !method_lower.contains("strip(")
            && !method_lower.contains(".strip")
        {
            let symbol = packet_predicate_claim_symbol(symbol, "isEmpty");
            claims.push(format!(
                "{symbol} does not trim whitespace before deciding emptiness."
            ));
        }
    }

    if normalized_source_contains_region_match_delegate(source) {
        let owner = packet_display_owner(symbol).unwrap_or_else(|| "String comparison".to_string());
        claims.push(format!(
            "{owner} delegates region matching work to a shared character-sequence helper."
        ));
    }

    claims
}

fn packet_predicate_claim_symbol(symbol: &str, method: &str) -> String {
    packet_display_owner(symbol)
        .map(|owner| format!("{owner}.{method}"))
        .unwrap_or_else(|| method.to_string())
}

fn normalized_source_contains_region_match_delegate(source: &str) -> bool {
    let normalized_source = normalize_identifier(source);
    normalized_source.contains("charsequence")
        && normalized_source.contains("regionmatches")
        && (normalized_source.contains("return")
            || normalized_source.contains("delegate")
            || normalized_source.contains("ignorecase"))
}

fn packet_source_method_block(
    source: &str,
    return_type: &str,
    method_name: &str,
) -> Option<String> {
    let lower = source.to_ascii_lowercase();
    let method_lower = method_name.to_ascii_lowercase();
    let return_lower = return_type.to_ascii_lowercase();
    let patterns = [
        format!("{return_lower} {method_lower}("),
        format!("{return_lower}\n{method_lower}("),
    ];
    let method_start = patterns
        .iter()
        .filter_map(|pattern| lower.find(pattern))
        .min()?;
    let brace_start = lower[method_start..].find('{')? + method_start;
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    for index in brace_start..bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(source[method_start..=index].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn packet_generic_css_animation_flow_claims(source: &str) -> Vec<String> {
    let mut claims = Vec::new();
    let custom_properties = packet_css_custom_property_names(source);
    let duration = packet_css_custom_property_with_fragment(&custom_properties, "duration");
    let delay = packet_css_custom_property_with_fragment(&custom_properties, "delay");
    let repeat = packet_css_custom_property_with_fragment(&custom_properties, "repeat");

    if let (Some(duration), Some(delay), Some(repeat)) = (duration, delay, repeat) {
        claims.push(format!(
            "Shared CSS custom properties {duration}, {delay}, and {repeat} define animation duration, delay, and repeat defaults."
        ));
    }

    if let Some(base_class) =
        packet_css_class_with_properties(source, &["animation-duration", "animation-fill-mode"])
    {
        claims.push(format!(
            ".{base_class} is the base class that applies animation duration and fill mode."
        ));
    }

    let lower = source.to_ascii_lowercase();
    let imports_css = lower.contains("@import") && lower.contains(".css");
    let imports_variables = lower.contains("var") && lower.contains(".css");
    let imports_base = lower.contains("base") && lower.contains(".css");
    let imports_named_animation = lower
        .lines()
        .filter(|line| line.contains("@import") && line.contains(".css"))
        .any(|line| line.contains('/') && !line.contains("docs"));
    if imports_css && imports_variables && imports_base && imports_named_animation {
        claims.push(
            "The animation stylesheet entrypoint imports the variable, base, and individual animation files."
                .to_string(),
        );
    }

    for keyframe in packet_css_keyframe_names(source).into_iter().take(4) {
        if packet_css_class_sets_animation_name(source, &keyframe) {
            claims.push(format!(
                "Named classes such as .{keyframe} set animation-name to matching keyframes; @keyframes {keyframe} defines the matching animation."
            ));
        }
    }

    claims
}

pub(crate) fn packet_generic_html_css_template_structure_claims(
    file_name: &str,
    source: &str,
) -> Vec<String> {
    let lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if (file_name.ends_with(".html") || (lower.contains("<html") && lower.contains("<body")))
        && lower.contains("id=\"app\"")
        && lower.contains("type=\"module\"")
        && lower.contains("viewport")
    {
        claims.push(format!(
            "{file_name} provides the app shell with viewport metadata, div#app, and a script[type=\"module\"] module script entry."
        ));
    }

    if file_name.ends_with(".css") {
        if lower.contains(":root")
            && lower.contains("font-family")
            && lower.contains("color-scheme")
            && lower.contains("font-smoothing")
            && lower.contains("body")
            && lower.contains("min-height")
        {
            claims.push(format!(
                "{file_name} owns :root typography, color-scheme, smoothing, and body layout defaults."
            ));
        }

        if lower.contains("#app")
            && lower.contains("max-width")
            && lower.contains("margin: 0 auto")
            && lower.contains("padding")
        {
            claims.push(
                "CSS app container rules constrain mounted content and center it with padding."
                    .to_string(),
            );
        }

        if lower.contains(".logo")
            && lower.contains("transition")
            && lower.contains("button")
            && lower.contains(":hover")
            && (lower.contains(":focus") || lower.contains(":focus-visible"))
        {
            claims.push(
                "CSS interaction selectors define hover, focus, and transition behavior."
                    .to_string(),
            );
        }

        if lower.contains("@media")
            && lower.contains("prefers-color-scheme: light")
            && lower.contains(":root")
            && lower.contains("button")
        {
            claims.push(
                "Light color-scheme media query rules override root, link-hover, and button colors."
                    .to_string(),
            );
        }
    }

    claims
}

fn packet_css_custom_property_names(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut properties = Vec::new();
    let mut seen = HashSet::new();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes[index] != b'-' || bytes[index + 1] != b'-' {
            index += 1;
            continue;
        }
        let start = index;
        index += 2;
        while index < bytes.len() && packet_css_identifier_byte(bytes[index]) {
            index += 1;
        }
        if index > start + 2 {
            let property = source[start..index].to_string();
            if seen.insert(property.to_ascii_lowercase()) {
                properties.push(property);
            }
        }
    }
    properties
}

fn packet_css_custom_property_with_fragment<'a>(
    properties: &'a [String],
    fragment: &str,
) -> Option<&'a str> {
    properties
        .iter()
        .find(|property| normalize_identifier(property).contains(fragment))
        .map(String::as_str)
}

fn packet_css_class_with_properties(source: &str, required_properties: &[&str]) -> Option<String> {
    let lower = source.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut index = 0usize;
    while let Some(dot_offset) = lower[index..].find('.') {
        let dot = index + dot_offset;
        let name_start = dot + 1;
        if name_start >= bytes.len() || !packet_css_identifier_byte(bytes[name_start]) {
            index = name_start.saturating_add(1);
            continue;
        }
        let mut name_end = name_start;
        while name_end < bytes.len() && packet_css_identifier_byte(bytes[name_end]) {
            name_end += 1;
        }
        let Some(block_start_offset) = lower[name_end..].find('{') else {
            break;
        };
        let block_start = name_end + block_start_offset + 1;
        let Some(block_end_offset) = lower[block_start..].find('}') else {
            break;
        };
        let block = &lower[block_start..block_start + block_end_offset];
        if required_properties
            .iter()
            .all(|property| block.contains(&property.to_ascii_lowercase()))
        {
            return Some(source[name_start..name_end].to_string());
        }
        index = name_end;
    }
    None
}

fn packet_css_keyframe_names(source: &str) -> Vec<String> {
    let lower = source.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    let mut search_from = 0usize;
    while let Some(offset) = lower[search_from..].find("@keyframes") {
        let mut index = search_from + offset + "@keyframes".len();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let name_start = index;
        while index < bytes.len() && packet_css_identifier_byte(bytes[index]) {
            index += 1;
        }
        if index > name_start {
            let name = source[name_start..index].to_string();
            if seen.insert(name.to_ascii_lowercase()) {
                names.push(name);
            }
        }
        search_from = index;
    }
    names
}

fn packet_css_class_sets_animation_name(source: &str, class_name: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    let class_name = class_name.to_ascii_lowercase();
    let class_selector = format!(".{class_name}");
    if !lower.contains(&class_selector) {
        return false;
    }
    let compact = lower
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    compact.contains(&format!("animation-name:{class_name}"))
}

fn packet_css_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn packet_source_argument_wrapper(source: &str) -> Option<(String, String)> {
    let mut search_from = 0usize;

    while let Some(relative_at) = source[search_from..].find('=') {
        let assignment_at = search_from + relative_at;
        let statement_start = source[..assignment_at]
            .rfind(['\n', ';'])
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let Some(wrapper) = packet_last_identifier(&source[statement_start..assignment_at]) else {
            search_from = assignment_at + 1;
            continue;
        };

        let after = &source[assignment_at + 1..];
        let Some(wrapper_factory) = packet_first_identifier(after) else {
            search_from = assignment_at + 1;
            continue;
        };
        let normalized_factory = normalize_identifier(&wrapper_factory);
        if !normalized_factory.contains("args") && !normalized_factory.contains("argument") {
            search_from = assignment_at + 1;
            continue;
        }
        let Some(handler_start) = after.find('(').map(|idx| idx + 1) else {
            search_from = assignment_at + 1;
            continue;
        };
        let handler_tail = &after[handler_start..];
        let Some(handler) = packet_first_identifier_after_type_arguments(handler_tail) else {
            search_from = assignment_at + 1;
            continue;
        };

        if packet_source_exports_default_identifier(source, &wrapper) {
            return Some((wrapper, handler));
        }

        search_from = assignment_at + 1;
    }

    None
}

fn packet_source_exports_default_identifier(source: &str, identifier: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    let mut search_from = 0usize;

    while let Some(relative_at) = lower[search_from..].find("export default") {
        let export_at = search_from + relative_at + "export default".len();
        if packet_first_identifier(&source[export_at..]).as_deref() == Some(identifier) {
            return true;
        }
        search_from = export_at;
    }

    false
}

fn packet_first_identifier_after_type_arguments(value: &str) -> Option<String> {
    let mut start = 0usize;
    let trimmed = value.trim_start();
    if trimmed.starts_with('<') {
        let mut depth = 0usize;
        for (idx, ch) in trimmed.char_indices() {
            match ch {
                '<' => depth += 1,
                '>' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        start = idx + ch.len_utf8();
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    packet_first_identifier(&trimmed[start..])
}

fn packet_first_identifier(value: &str) -> Option<String> {
    let mut chars = value
        .char_indices()
        .skip_while(|(_, ch)| !is_ident_start(*ch));
    let (start, _) = chars.next()?;
    let mut end = value.len();
    for (idx, ch) in value[start..].char_indices().skip(1) {
        if !is_ident_continue(ch) {
            end = start + idx;
            break;
        }
    }
    Some(value[start..end].to_string())
}

fn packet_last_identifier(value: &str) -> Option<String> {
    value
        .split(|ch: char| !is_ident_continue(ch))
        .rfind(|part| part.chars().next().is_some_and(is_ident_start))
        .map(str::to_string)
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn packet_generic_shell_install_dispatch_flow_claims(
    symbol: &str,
    file_name: &str,
    source: &str,
) -> Vec<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let normalized_file = normalize_identifier(file_name);
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if (normalized_file.contains("install") || normalized_symbol.contains("install"))
        && source_lower.contains("source")
        && (source_lower.contains(".sh") || source_lower.contains("shell"))
    {
        claims.push("The installer bootstraps shell runtime sourcing.".to_string());
    }

    if (normalized_file.contains("install") || normalized_symbol.contains("download"))
        && (source_lower.contains("download")
            || source_lower.contains("curl ")
            || source_lower.contains("wget "))
        && (source_lower.contains("completion") || source_lower.contains(".sh"))
    {
        claims.push(
            "The installer fetches shell support assets and completion/runtime files.".to_string(),
        );
    }

    if normalized_symbol.contains("install")
        && source_lower.contains(" install")
        && (source_lower.contains("version") || source_lower.contains("${"))
    {
        claims.push("The install helper invokes a version-aware install command.".to_string());
    }

    if (source_lower.contains("case $command") || source_lower.contains("case \"$command\""))
        || (source_lower.contains("case \"$1\"") || source_lower.contains("case ${1"))
    {
        claims.push("The shell dispatcher branches on command arguments.".to_string());
    }

    if normalized_file.contains("completion")
        && (source_lower.contains("complete -") || source_lower.contains("compdef "))
    {
        claims
            .push("Shell completion registers a completion function for the command.".to_string());
    }

    claims
}

fn packet_generic_shell_version_use_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if (normalized_symbol.contains("ifneeded") || normalized_symbol.contains("needed"))
        && source_lower.contains("if ")
        && source_lower.contains("${1-}")
        && source_lower.contains("current")
        && source_lower.contains("return")
        && source_lower.contains("$@")
        && source_lower.contains(" use ")
    {
        claims.push(format!(
            "{symbol} switches versions only when the requested version is not already active."
        ));
    }

    claims
}

fn packet_generic_server_route_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    let callable_factory_source =
        source_lower.contains("mixin(app") || source_lower.contains("var app = function");
    if source_lower.contains("function")
        && source_lower.contains("handle(")
        && source_lower.contains("request")
        && source_lower.contains("response")
        && (normalized_symbol.contains("application") || callable_factory_source)
    {
        if callable_factory_source {
            claims.push(
                "The application factory builds a callable app object and mixes in request and response prototypes."
                    .to_string(),
            );
        } else {
            claims.push(
                "The application factory builds a callable request handler and wires request/response state."
                    .to_string(),
            );
        }
    }

    if source_lower.contains("router")
        && source_lower.contains(".handle(")
        && (normalized_symbol.ends_with("handle")
            || (source_lower.contains("handle = function")
                && source_lower.contains("this.router.handle")))
    {
        if symbol.contains('.') {
            claims.push(format!(
                "{symbol} delegates request handling to the router."
            ));
        } else {
            claims.push(
                "The application handler delegates request handling to a router.".to_string(),
            );
        }
    }

    if source_lower.contains("function use")
        && source_lower.contains("router")
        && source_lower.contains(".use(")
        && (normalized_symbol.ends_with("use") || source_lower.contains("use = function"))
    {
        if symbol.contains('.') {
            claims.push(format!("{symbol} registers middleware on the router."));
        } else {
            claims.push("Middleware registration delegates to a router.".to_string());
        }
    }

    if source_lower.contains("function route")
        && source_lower.contains("router.route(")
        && (normalized_symbol.ends_with("route") || source_lower.contains("route = function"))
    {
        claims.push(
            "The route registration helper creates route entries through a router.".to_string(),
        );
    }

    if packet_symbol_is_response_send_helper(symbol, &normalized_symbol)
        && packet_source_sets_response_metadata(&source_lower)
        && packet_source_ends_or_writes_response(&source_lower)
    {
        if symbol.contains('.') {
            claims.push(format!(
                "{symbol} sets response metadata before ending the response."
            ));
        } else {
            claims.push(
                "The response send helper sets response metadata before ending the body."
                    .to_string(),
            );
        }
    }

    if normalized_symbol.contains("handle")
        && source_lower.contains("handlers")
        && source_lower.contains("relativepath")
        && (source_lower.contains(".handle(") || source_lower.contains(" handle("))
        && source_lower.contains("return")
    {
        claims.push(format!(
            "{symbol} registers routes by delegating to the group handle path."
        ));
    }

    if normalized_symbol == "new"
        && source_lower.contains("&engine")
        && source_lower.contains("routergroup")
        && (source_lower.contains("trees") || source_lower.contains("methodtree"))
    {
        claims.push(
            "Engine construction creates route registration state and method trees.".to_string(),
        );
    }

    if source_lower.contains("addroute")
        && source_lower.contains("handlers")
        && (source_lower.contains("methodtree")
            || source_lower.contains("methodtrees")
            || source_lower.contains("root"))
    {
        claims.push(
            "Route registration inserts handlers into the per-method route tree.".to_string(),
        );
    }

    if source_lower.contains("getvalue")
        && source_lower.contains("handlers")
        && (source_lower.contains(".next(") || source_lower.contains("next()"))
    {
        claims.push(
            "Request dispatch finds a route, installs handlers on the context, and advances into the handler chain."
                .to_string(),
        );
    }

    if normalized_symbol.ends_with("next")
        && source_lower.contains("handlers")
        && source_lower.contains("index")
        && source_lower.contains("++")
        && source_lower.contains("for ")
    {
        claims.push(format!("{symbol} advances through the handler chain."));
    }

    claims
}

fn packet_symbol_is_response_send_helper(symbol: &str, normalized_symbol: &str) -> bool {
    if let Some((owner, method)) = packet_receiver_method_parts(symbol)
        && packet_response_receiver_owner(&owner)
        && packet_response_terminal_method(&method)
    {
        return true;
    }
    (normalized_symbol.contains("response") || normalized_symbol.starts_with("res"))
        && packet_response_terminal_method(normalized_symbol)
}

fn packet_receiver_method_parts(symbol: &str) -> Option<(String, String)> {
    let trimmed = symbol.trim();
    for separator in ['.', '#', ':'] {
        if let Some(index) = trimmed.rfind(separator) {
            let owner = normalize_identifier(&trimmed[..index]);
            let method = normalize_identifier(&trimmed[index + separator.len_utf8()..]);
            if !owner.is_empty() && !method.is_empty() {
                return Some((owner, method));
            }
        }
    }
    None
}

fn packet_response_receiver_owner(owner: &str) -> bool {
    matches!(owner, "res" | "response" | "reply")
}

fn packet_response_terminal_method(method: &str) -> bool {
    matches!(method, "send" | "json" | "end" | "respond")
}

fn packet_source_sets_response_metadata(source_lower: &str) -> bool {
    source_lower.contains("content-length")
        || source_lower.contains("contentlength")
        || source_lower.contains("content-type")
        || source_lower.contains("contenttype")
        || source_lower.contains("setheader")
        || source_lower.contains(".set(")
        || source_lower.contains("header(")
}

fn packet_source_ends_or_writes_response(source_lower: &str) -> bool {
    source_lower.contains(".end(")
        || source_lower.contains(".write(")
        || source_lower.contains("writehead(")
}

fn packet_generic_server_request_dispatch_flow_claims(
    symbol: &str,
    source: &str,
    path: &str,
) -> Vec<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let normalized_source = normalize_identifier(source);
    let symbol_has = |parts: &[&str]| parts.iter().all(|part| normalized_symbol.contains(part));
    let source_has = |parts: &[&str]| parts.iter().all(|part| normalized_source.contains(part));
    let mut claims = Vec::new();

    if !packet_claim_primary_source_path(path) {
        return claims;
    }

    if symbol_has(&["wsgi", "app"]) && source_has(&["request", "context"]) {
        claims.push(format!(
            "{symbol} is the WSGI entry point and creates or uses request context before dispatch."
        ));
    }

    if symbol_has(&["full", "dispatch", "request"])
        && source_has(&["preprocess", "request"])
        && source_has(&["dispatch", "request"])
        && source_has(&["finalize", "request"])
    {
        claims.push(format!(
            "{symbol} wraps preprocessing, dispatch, exception handling, and response finalization."
        ));
    }

    if symbol_has(&["dispatch", "request"])
        && !symbol_has(&["full", "dispatch", "request"])
        && source_has(&["view", "function"])
        && source_has(&["view", "args"])
    {
        claims.push(format!(
            "{symbol} invokes the view function selected by URL matching."
        ));
    }

    if normalized_symbol.ends_with("route") && source_has(&["add", "url", "rule"]) {
        claims.push(
            "Route registration decorator adds URL rules without performing request dispatch itself."
                .to_string(),
        );
    }

    claims
}

fn packet_claim_primary_source_path(path: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    !(path.contains("/tests/")
        || path.contains("/test/")
        || path.starts_with("tests/")
        || path.starts_with("test/")
        || path.contains("/examples/")
        || path.starts_with("examples/"))
}

fn packet_generic_sql_schema_flow_claims(source: &str) -> Vec<String> {
    let mut claims = Vec::new();
    let tables = packet_sql_create_table_names(source);
    if !tables.is_empty() {
        claims.push(format!(
            "SQL schema defines tables {}.",
            packet_human_join(&tables.iter().take(12).cloned().collect::<Vec<_>>())
        ));
    }
    for claim in packet_sql_foreign_key_claims(source) {
        if !claims.iter().any(|existing| existing == &claim) {
            claims.push(claim);
        }
        if claims.len() >= 18 {
            break;
        }
    }
    claims
}

fn packet_generic_runtime_formatting_flow_claims(
    symbol: &str,
    file_name: &str,
    source: &str,
) -> Vec<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let normalized_file_name = normalize_identifier(file_name);
    let normalized_source = normalize_identifier(source);
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_symbol.contains("formatargstore") || normalized_source.contains("formatargstore")
    {
        claims.push(
            "Runtime formatting builds type-erased format argument stores before dispatching formatting."
                .to_string(),
        );
    }

    if normalized_symbol == "vformat" || normalized_symbol.ends_with("vformat") {
        claims.push(
            "Runtime formatting routes format calls through a central runtime argument path."
                .to_string(),
        );
    }

    if normalized_source.contains("vformat")
        && (normalized_source.contains("formatargs")
            || normalized_source.contains("basicformatargs")
            || normalized_source.contains("formatargstore"))
        && (normalized_source.contains("vformatto") || normalized_source.contains("formatto"))
    {
        claims.push(
            "Runtime formatting uses type-erased arguments before dispatching formatted output helpers."
                .to_string(),
        );
    }

    if normalized_source.contains("formaterror")
        && (normalized_source.contains("runtimeerror")
            || normalized_source.contains("throwformaterror")
            || normalized_source.contains("formatting"))
    {
        claims
            .push("Runtime formatting defines an error type for formatting failures.".to_string());
    }

    if (normalized_symbol.contains("formatto") || normalized_source.contains("formatto"))
        && (normalized_source.contains("outputit")
            || normalized_source.contains("outputiterator")
            || normalized_source.contains("appender")
            || normalized_source.contains("vformatto"))
    {
        claims.push(
            "Runtime formatting writes formatted output through output iterator helpers."
                .to_string(),
        );
    }

    if normalized_source.contains("buffer")
        && normalized_source.contains("append")
        && (normalized_file_name.starts_with("format")
            || normalized_source.contains("formatted")
            || normalized_source.contains("format"))
    {
        claims.push(
            "Runtime formatting source instantiates buffer append paths for formatted output."
                .to_string(),
        );
    }

    if (normalized_file_name.starts_with("os")
        || normalized_source.contains("systemerror")
        || normalized_source.contains("formaterrorcode"))
        && normalized_source.contains("vformat")
        && (normalized_source.contains("formaterrorcode")
            || normalized_source.contains("formatwindowserror")
            || source_lower.contains("std::system_error"))
    {
        claims.push(
            "Runtime formatting error-boundary code formats system errors through shared formatting helpers."
                .to_string(),
        );
    }

    claims
}

fn packet_generic_log_record_handler_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let normalized_symbol = normalize_identifier(symbol);
    let source_lower = source.to_ascii_lowercase();
    let creates_record_object = source_lower.contains("new ") && source_lower.contains("record(");
    let typed_record_handle =
        source_lower.contains("function handle(") && source_lower.contains("$record");
    let mut claims = Vec::new();

    if source_lower.contains("class logger")
        && source_lower.contains("$handlers")
        && source_lower.contains("function pushhandler")
        && source_lower.contains("array_unshift($this->handlers")
    {
        claims
            .push("The logger owns a handler stack populated by handler registration.".to_string());
    }

    if normalized_symbol.ends_with("log")
        && source_lower.contains("function log(")
        && source_lower.contains("$this->addrecord(")
    {
        claims.push("The logger log method delegates into record creation.".to_string());
    }

    if (normalized_symbol.ends_with("addrecord") || source_lower.contains("function addrecord("))
        && creates_record_object
        && (source_lower.contains("$handler->handle($record)")
            || source_lower.contains("$handler->handle(clone $record)")
            || source_lower.contains("->handle($record)")
            || source_lower.contains("->handle(clone $record)"))
    {
        claims.push("addRecord creates a log record before passing it to handlers.".to_string());
    }

    if source_lower.contains("interface handlerinterface")
        && typed_record_handle
        && source_lower.contains("function handlebatch(")
    {
        claims.push(
            "HandlerInterface defines record handling and batch handling boundaries.".to_string(),
        );
    }

    if typed_record_handle
        && source_lower.contains("$this->processrecord($record)")
        && source_lower.contains("$this->write($record)")
    {
        claims.push(
            "The processing handler handles records by processing and writing them.".to_string(),
        );
    }

    claims
}

fn packet_generic_site_build_phase_flow_claims(source: &str) -> Vec<String> {
    let normalized_source = normalize_identifier(source);
    let mut claims = Vec::new();

    if normalized_source.contains("defprocess") && normalized_source.contains("sitenew") {
        claims.push("Build.process constructs or processes a site.".to_string());
    }

    if normalized_source.contains("defprocess")
        && normalized_source.contains("reset")
        && normalized_source.contains("read")
        && normalized_source.contains("generate")
        && normalized_source.contains("render")
        && normalized_source.contains("cleanup")
        && normalized_source.contains("write")
    {
        claims.push(
            "The site lifecycle method runs reset, read, generate, render, cleanup, and write phases."
                .to_string(),
        );
    }

    if normalized_source.contains("classreader") && normalized_source.contains("defread") {
        claims.push("Content reading source owns the site content read phase.".to_string());
    }

    if normalized_source.contains("classrenderer")
        && (normalized_source.contains("defrender")
            || normalized_source.contains("renderdocument")
            || normalized_source.contains("renderliquid"))
    {
        claims.push("Page rendering source handles page and document rendering.".to_string());
    }

    claims
}

fn packet_generic_mapper_configuration_plan_claims(
    symbol: &str,
    kind: NodeKind,
    file_name: &str,
    source: &str,
) -> Vec<String> {
    let file_name = file_name.to_ascii_lowercase();
    let normalized_source = normalize_identifier(source);
    let mut claims = Vec::new();
    let owner = packet_display_owner(symbol).unwrap_or_else(|| symbol.to_string());

    if packet_mapper_public_api_symbol(symbol, kind)
        && normalized_source.contains("interface")
        && normalized_source.contains("map")
        && normalized_source.contains("source")
        && normalized_source.contains("destination")
        && (normalized_source.contains("mapper") || normalized_source.contains("mapping"))
    {
        claims.push(format!(
            "{owner} exposes public runtime mapper APIs for source-to-destination mapping."
        ));
    }

    if (normalize_identifier(symbol).contains("configuration")
        || normalize_identifier(&file_name).contains("configuration"))
        && normalized_source.contains("configuration")
        && (normalized_source.contains("configuredmaps")
            || normalized_source.contains("resolvedmaps")
            || normalized_source.contains("typemaps")
            || normalized_source.contains("executionplans"))
        && (normalized_source.contains("buildexecutionplan")
            || normalized_source.contains("createmapper")
            || normalized_source.contains("compilemappings"))
    {
        claims.push(
            "Mapping configuration source builds and owns runtime mapping plans.".to_string(),
        );
    }

    if packet_mapper_public_api_symbol(symbol, kind)
        && normalized_source.contains("class")
        && normalized_source.contains("mapper")
        && normalized_source.contains("mapcore")
        && normalized_source.contains("getexecutionplan")
    {
        claims.push(
            "Mapper runtime source exposes the public object-mapping entry point.".to_string(),
        );
    }

    if packet_mapper_plan_symbol(symbol, &file_name)
        && normalized_source.contains("lambda")
        && normalized_source.contains("map")
        && (normalized_source.contains("planbuilder")
            || normalized_source.contains("mapexpression")
            || normalized_source.contains("expression"))
    {
        claims.push(
            "Type-map source contributes lambda plans used by the mapping execution pipeline."
                .to_string(),
        );
    }

    if file_name.ends_with("planbuilder.cs")
        && normalized_source.contains("lambda")
        && normalized_source.contains("map")
        && normalized_source.contains("createdestinationfunc")
        && normalized_source.contains("createassignmentfunc")
        && normalized_source.contains("createmapperfunc")
    {
        claims.push(
            "The mapping plan builder participates in building expression plans for mappings."
                .to_string(),
        );
    }

    claims
}

fn packet_mapper_public_api_symbol(symbol: &str, kind: NodeKind) -> bool {
    let normalized = normalize_identifier(symbol);
    matches!(
        kind,
        NodeKind::INTERFACE | NodeKind::CLASS | NodeKind::METHOD | NodeKind::FUNCTION
    ) && normalized.contains("mapper")
        && ![
            "action",
            "configuration",
            "convention",
            "destinationname",
            "expression",
            "member",
            "operation",
            "options",
            "projection",
            "source",
        ]
        .iter()
        .any(|needle| normalized.contains(needle))
}

fn packet_mapper_plan_symbol(symbol: &str, file_name: &str) -> bool {
    let normalized_symbol = normalize_identifier(symbol);
    let normalized_file = normalize_identifier(file_name);
    normalized_symbol.contains("lambda")
        || normalized_symbol.contains("typemap")
        || normalized_symbol.contains("mappingplan")
        || normalized_file.contains("typemap")
        || normalized_file.contains("mappingplan")
        || normalized_file.contains("planbuilder")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::eval_probes::EVAL_PROBES_ENV;
    use codestory_contracts::api::{NodeId, NodeKind, RetrievalScoreBreakdownDto, SearchHitOrigin};

    fn test_packet_citation(display_name: &str, file_path: &str) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(display_name.to_string()),
            display_name: display_name.to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some(file_path.to_string()),
            line: Some(10),
            score: 0.9,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: 0.4,
                semantic: 0.2,
                graph: 0.3,
                total: 0.9,
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

    struct EvalProbesGuard;

    impl EvalProbesGuard {
        fn enabled() -> Self {
            crate::agent::eval_probes::push_eval_probes_test_override();
            Self
        }
    }

    impl Drop for EvalProbesGuard {
        fn drop(&mut self) {
            crate::agent::eval_probes::pop_eval_probes_test_override();
        }
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn cleared(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: tests use this guard to isolate one env var for this process-local
            // regression and restore it on drop.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: restores the process-local env var captured by this guard.
            unsafe {
                if let Some(previous) = self.previous.take() {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    fn contracted_product_profile(
        profile: SourceClaimProfile,
    ) -> Option<SourceClaimProfileContract> {
        GENERIC_PRODUCT_CLAIM_PROFILES
            .iter()
            .find(|entry| entry.profile == profile)
            .and_then(|entry| match entry.contract {
                SourceClaimProfileContractStatus::Contracted(contract) => Some(contract),
                SourceClaimProfileContractStatus::PendingMigration => None,
            })
    }

    struct ContractFixture {
        prompt: &'static str,
        symbol: &'static str,
        path: &'static str,
        source: &'static str,
        expected_claim: Option<&'static str>,
    }

    fn contract_fixture(id: &str) -> ContractFixture {
        match id {
            "generic-shell-install-dispatch-positive" => ContractFixture {
                prompt: "Trace how an install script bootstraps the shell function and dispatches install, download, and use commands.",
                symbol: "install_runtime",
                path: "tools/install-runtime.sh",
                source: r#"
                install_runtime() {
                  SOURCE_STR='[ -s "$TOOL_DIR/runtime.sh" ] && source "$TOOL_DIR/runtime.sh"'
                  download_file "$RUNTIME_SOURCE" "$TOOL_DIR/runtime.sh"
                }
                "#,
                expected_claim: Some("The installer bootstraps shell runtime sourcing."),
            },
            "generic-shell-install-dispatch-helper-negative" => ContractFixture {
                prompt: "Trace how an install script bootstraps the shell function and dispatches install, download, and use commands.",
                symbol: "helper_cache",
                path: "tools/helpers.sh",
                source: "helper_cache() { echo cache; }",
                expected_claim: None,
            },
            "generic-shell-version-use-positive" => ContractFixture {
                prompt: "Explain how a shell command switches versions only when the requested version is not already active.",
                symbol: "use_if_needed",
                path: "tools/runtime.sh",
                source: r#"
                use_if_needed() {
                  current="$(current_version)"
                  if [ "${1-}" = "$current" ]; then
                    return
                  fi
                  tool use "$@"
                }
                "#,
                expected_claim: Some(
                    "use_if_needed switches versions only when the requested version is not already active.",
                ),
            },
            "generic-shell-version-use-helper-negative" => ContractFixture {
                prompt: "Explain how a shell command switches versions only when the requested version is not already active.",
                symbol: "use_version",
                path: "tools/runtime.sh",
                source: "use_version() { echo current; }",
                expected_claim: None,
            },
            "generic-object-mapping-plan-positive" => ContractFixture {
                prompt: "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects through type-map lambda plans.",
                symbol: "MappingPlan.BuildMapperLambda",
                path: "src/ObjectMapping/MappingPlan.cs",
                source: r#"
                public sealed class MappingPlan
                {
                    public LambdaExpression MapExpression { get; private set; }
                    internal LambdaExpression BuildMapperLambda(IGlobalMappingConfiguration configuration) =>
                        Types.ContainsGenericParameters ? null : new MappingPlanBuilder(configuration, this).BuildMapperLambda();
                }
                "#,
                expected_claim: Some(
                    "Type-map source contributes lambda plans used by the mapping execution pipeline.",
                ),
            },
            "generic-object-mapping-cache-helper-negative" => ContractFixture {
                prompt: "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects through type-map lambda plans.",
                symbol: "CacheConfig",
                path: "src/ObjectMapping/CacheConfig.cs",
                source: "public sealed class CacheConfig { public string Key { get; set; } }",
                expected_claim: None,
            },
            "generic-session-request-dispatch-positive" => ContractFixture {
                prompt: "Explain how a session request call creates a prepared request and sends it through an adapter.",
                symbol: "Session.request",
                path: "src/http/session_flow.py",
                source: "def request(self, method, url, **kwargs):\n    req = Request(method=method, url=url)\n    prep = self.prepare_request(req)\n    return self.send(prep, **kwargs)\n",
                expected_claim: Some(
                    "The session request method creates a request object and prepares it into a transport-ready request object.",
                ),
            },
            "generic-session-request-transport-negative" => ContractFixture {
                prompt: "Explain how a session request call creates a prepared request and sends it through an adapter.",
                symbol: "Session",
                path: "src/http/session_flow.py",
                source: "adapter = self.get_adapter(url=request.url)\n# http proxy environment settings\n",
                expected_claim: None,
            },
            other => panic!("unknown source-claim profile fixture id {other}"),
        }
    }

    #[test]
    fn high_risk_product_profiles_have_anti_overfit_contracts() {
        let expectations = [
            (
                SourceClaimProfile::ShellInstallDispatch,
                "shell-install-dispatch",
                CoverageMode::AllowsLexicalSource,
                &[
                    FlowRole::Entrypoint,
                    FlowRole::Dispatch,
                    FlowRole::TerminalBoundary,
                ][..],
                "generic-shell-install-dispatch-positive",
                "generic-shell-install-dispatch-helper-negative",
            ),
            (
                SourceClaimProfile::ShellVersionUse,
                "shell-version-use",
                CoverageMode::AllowsLexicalSource,
                &[FlowRole::Dispatch][..],
                "generic-shell-version-use-positive",
                "generic-shell-version-use-helper-negative",
            ),
            (
                SourceClaimProfile::MappingConfigurationPlan,
                "object-mapping-plan",
                CoverageMode::RequiresResolvedSourceOrGraph,
                &[FlowRole::Configuration, FlowRole::Dispatch][..],
                "generic-object-mapping-plan-positive",
                "generic-object-mapping-cache-helper-negative",
            ),
            (
                SourceClaimProfile::ClientRequestDispatch,
                "session-request-dispatch",
                CoverageMode::RequiresResolvedSourceOrGraph,
                &[
                    FlowRole::Entrypoint,
                    FlowRole::Dispatch,
                    FlowRole::TransformOrValidate,
                    FlowRole::TerminalBoundary,
                ][..],
                "generic-session-request-dispatch-positive",
                "generic-session-request-transport-negative",
            ),
        ];

        for (
            profile,
            domain,
            allowed_evidence_tier,
            allowed_proof_roles,
            positive_fixture_id,
            false_positive_fixture_id,
        ) in expectations
        {
            let contract = contracted_product_profile(profile)
                .unwrap_or_else(|| panic!("expected anti-overfit contract for {profile:?}"));
            assert_eq!(contract.domain, domain);
            assert_eq!(contract.scope, SourceClaimProfileScope::Product);
            assert_eq!(contract.allowed_evidence_tier, allowed_evidence_tier);
            assert_eq!(contract.allowed_proof_roles, allowed_proof_roles);
            assert_eq!(contract.positive_fixture_id, positive_fixture_id);
            assert_eq!(
                contract.false_positive_fixture_id,
                false_positive_fixture_id
            );

            for value in [
                contract.domain,
                contract.positive_fixture_id,
                contract.false_positive_fixture_id,
            ] {
                let lower = value.to_ascii_lowercase();
                for blocked in ["requests", "nvm", "automapper", "axios"] {
                    assert!(
                        !lower.contains(blocked),
                        "contract value `{value}` must stay generic"
                    );
                }
            }
        }
    }

    #[test]
    fn non_high_risk_product_profiles_stay_pending_until_risk_evidence() {
        let pending = [
            SourceClaimProfile::ServerRoute,
            SourceClaimProfile::ServerRequestDispatch,
            SourceClaimProfile::HookCache,
            SourceClaimProfile::ClientSend,
            SourceClaimProfile::UrlSessionRequest,
            SourceClaimProfile::StringPredicate,
            SourceClaimProfile::StylesheetAnimation,
            SourceClaimProfile::HtmlCssTemplateStructure,
            SourceClaimProfile::SqlSchema,
            SourceClaimProfile::RuntimeFormatting,
            SourceClaimProfile::LoggerHandlerFlow,
            SourceClaimProfile::SiteBuildPhase,
            SourceClaimProfile::FormValidation,
            SourceClaimProfile::EventLoopCommand,
            SourceClaimProfile::SearchExecution,
            SourceClaimProfile::BufferedIo,
        ];

        let actual_pending: Vec<_> = GENERIC_PRODUCT_CLAIM_PROFILES
            .iter()
            .filter_map(|entry| match entry.contract {
                SourceClaimProfileContractStatus::PendingMigration => Some(entry.profile),
                SourceClaimProfileContractStatus::Contracted(_) => None,
            })
            .collect();

        assert_eq!(actual_pending.len(), pending.len());
        for profile in pending {
            assert!(
                actual_pending.contains(&profile),
                "expected {profile:?} to remain pending migration"
            );
            assert!(
                contracted_product_profile(profile).is_none(),
                "{profile:?} needs concrete overfit-risk evidence before contract migration"
            );
        }
    }

    #[test]
    fn contracted_product_profile_fixtures_execute_positive_and_negative_paths() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);

        for entry in GENERIC_PRODUCT_CLAIM_PROFILES {
            let SourceClaimProfileContractStatus::Contracted(contract) = entry.contract else {
                continue;
            };

            let positive = contract_fixture(contract.positive_fixture_id);
            let citation = test_packet_citation(positive.symbol, positive.path);
            let claims = packet_source_derived_claims_for_citation(
                positive.prompt,
                &citation,
                positive.source,
            );
            let expected = positive
                .expected_claim
                .expect("positive contract fixture must name an expected claim");
            assert!(
                claims.iter().any(|claim| claim == expected),
                "positive fixture `{}` for {} should emit `{expected}`; got {claims:?}",
                contract.positive_fixture_id,
                contract.domain
            );

            let negative = contract_fixture(contract.false_positive_fixture_id);
            let citation = test_packet_citation(negative.symbol, negative.path);
            let claims = packet_source_derived_claims_for_citation(
                negative.prompt,
                &citation,
                negative.source,
            );
            assert!(
                claims.is_empty(),
                "negative fixture `{}` for {} should not emit product claims; got {claims:?}",
                contract.false_positive_fixture_id,
                contract.domain
            );
        }
    }

    #[test]
    fn role_safe_request_claims_require_behavior_owning_citations() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let prompt = "Explain how a client instance sends requests through interceptors, dispatch, and a transport adapter.";
        let source = r#"
            Client.prototype.request = function request(config) {
              config = merge(defaults, config)
              this.interceptors.request.forEach(run)
              return dispatchTransport(config)
            }
        "#;

        let mut import_label = test_packet_citation("../helpers.js", "src/adapters/client.js");
        import_label.kind = NodeKind::MODULE;
        let import_claims =
            packet_source_derived_claims_for_citation(prompt, &import_label, source);
        assert!(
            import_claims.is_empty(),
            "an import/module label must not inherit file-wide behavior: {import_claims:?}"
        );

        let mut retry_method =
            test_packet_citation("Client.prototype.retryRequest", "src/client.js");
        retry_method.kind = NodeKind::METHOD;
        let retry_claims = packet_source_derived_claims_for_citation(prompt, &retry_method, source);
        assert!(
            retry_claims.is_empty(),
            "a request-suffixed method must not inherit the request declaration: {retry_claims:?}"
        );

        let mut private_request_method =
            test_packet_citation("Client.prototype._request", "src/client.js");
        private_request_method.kind = NodeKind::METHOD;
        let private_request_claims = packet_source_derived_claims_for_citation(
            prompt,
            &private_request_method,
            r#"
            class Client {
              _request(config) { return config }
              request(config) {
                config = merge(defaults, config)
                this.interceptors.request.forEach(run)
                return dispatchTransport(config)
              }
            }
            "#,
        );
        assert!(
            private_request_claims.is_empty(),
            "_request must not inherit a co-located request pipeline: {private_request_claims:?}"
        );

        let mut request_method = test_packet_citation("Client.prototype.request", "src/client.js");
        request_method.kind = NodeKind::METHOD;
        let request_claims =
            packet_source_derived_claims_for_citation(prompt, &request_method, source);
        assert!(
            request_claims
                .iter()
                .any(|claim| claim.contains("runs request interceptors")),
            "the behavior-owning request method should retain its pipeline claim: {request_claims:?}"
        );

        let mut interceptor_owner = test_packet_citation(
            "RequestInterceptorRegistry.constructor",
            "src/interceptors.ts",
        );
        interceptor_owner.kind = NodeKind::METHOD;
        let interceptor_claims = packet_source_derived_claims_for_citation(
            prompt,
            &interceptor_owner,
            "constructor() { this.handlers = [] }",
        );
        assert!(
            interceptor_claims
                .iter()
                .any(|claim| claim.contains("interceptor handler pairs")),
            "an interceptor-owner constructor should retain its management claim: {interceptor_claims:?}"
        );

        let adapter_source = "const knownAdapters = { http, xhr }; function getAdapter(name) { const adapter = knownAdapters[name]; return adapter; }";
        let adapter_helper = test_packet_citation("isResolvedHandle", "src/adapters/adapters.js");
        let adapter_helper_claims =
            packet_source_derived_claims_for_citation(prompt, &adapter_helper, adapter_source);
        assert!(
            adapter_helper_claims.is_empty(),
            "a helper in an adapter directory must not inherit file-wide selection behavior: {adapter_helper_claims:?}"
        );

        let adapter_owner = test_packet_citation("getAdapter", "src/adapters/adapters.js");
        let adapter_owner_claims =
            packet_source_derived_claims_for_citation(prompt, &adapter_owner, adapter_source);
        assert!(
            adapter_owner_claims
                .iter()
                .any(|claim| claim.contains("transport based on environment capabilities")),
            "the adapter selector should retain its selection claim: {adapter_owner_claims:?}"
        );

        let mut adapter_file =
            test_packet_citation("src/adapters/adapters.js", "src/adapters/adapters.js");
        adapter_file.kind = NodeKind::FILE;
        let adapter_file_claims =
            packet_source_derived_claims_for_citation(prompt, &adapter_file, adapter_source);
        assert!(
            adapter_file_claims
                .iter()
                .any(|claim| claim.contains("transport based on environment capabilities")),
            "an exact adapter file should retain its file-level selection claim: {adapter_file_claims:?}"
        );
    }

    fn hook_cache_source() -> &'static str {
        r#"
        export const useSWRHandler = (_key, fetcher, config) => {
          const [key, fnArg] = serialize(_key)
          const [getCache, setCache, subscribeCache, getInitialCache] =
            createCacheHelper(cache, key)
          const cachedData = getCache()
          return { data: cachedData.data }
        }
        const useSWR = withArgs<SWRHook>(useSWRHandler)
        export default useSWR
        "#
    }

    fn client_send_source() -> &'static str {
        r#"
        import 'dart:io';

        abstract mixin class BaseTransportClient implements Client {
          Future<Response> get(Uri url) => _sendUnstreamed('GET', url);
          Future<Response> post(Uri url, {Object? body}) =>
              _sendUnstreamed('POST', url, body);
          Future<StreamedResponse> send(BaseRequest request);
          Future<Response> _sendUnstreamed(String method, Uri url) async {
            var request = Request(method, url);
            return Response.fromStream(await send(request));
          }
        }

        class NativeClient extends BaseTransportClient {
          HttpClient? _inner;
          Future<NativeStreamedResponse> send(BaseRequest request) async {
            var stream = request.finalize();
            var ioRequest = await _inner!.openUrl(request.method, request.url);
            final response = await stream.pipe(ioRequest) as HttpClientResponse;
            return NativeStreamedResponse(response);
          }
        }
        "#
    }

    fn command_dispatch_source() -> &'static str {
        r#"
        void readQueryFromClient(client *c) {
          processInputBuffer(c);
        }

        void processInputBuffer(client *c) {
          processCommand(c);
        }

        int processCommand(client *c) {
          lookupCommand(c->argv[0]);
          aclCheckCommandPerm(c);
          if (arity) return C_ERR;
          if (cluster) return C_ERR;
          call(c, 0);
        }

        void call(client *c, int flags) {
          c->cmd->proc(c);
          propagate(c);
          slowlogPushEntryIfNeeded(c);
        }
        "#
    }

    fn search_execution_source() -> &'static str {
        r#"
        fn main() {
            let flags = parse_flags();
            run(flags);
        }

        fn run(flags: Flags) {
            let args = HiArgs::from(flags);
            search_parallel(args);
        }

        struct HiArgs {
            walk: WalkBuilder,
            matcher: Matcher,
            searcher: Searcher,
            printer: Printer,
        }

        struct SearchWorker {
            matcher: Matcher,
            searcher: Searcher,
            printer: Printer,
            candidate_path: PathBuf,
        }

        impl SearchWorker {
            fn search(&mut self) {
                self.searcher.search_path(&self.matcher, &self.candidate_path, &mut self.printer);
            }
        }
        "#
    }

    fn route_engine_source() -> &'static str {
        r#"
        func New() *Engine {
            engine := &Engine{
                RouterGroup: RouterGroup{},
                trees: make(methodTrees, 0, 9),
            }
            return engine
        }

        func (registrar *RouteRegistrar) addRoute(method string, path string, handlers HandlersChain) {
            root := registrar.trees.get(method)
            root.addRoute(path, handlers)
        }

        func (engine *Engine) dispatch(c *Context) {
            value := engine.trees.get(c.Request.Method).getValue(c.Request.URL.Path, c.params)
            if value.handlers != nil {
                c.handlers = value.handlers
                c.Next()
            }
        }
        "#
    }

    fn buffered_io_source() -> &'static str {
        r#"
        class Buffer {
            var size: Long = 0
            fun read(byteCount: Long): ByteArray {
                val segment = head ?: return ByteArray(0)
                return segment.read(byteCount)
            }
            fun write(bytes: ByteArray) {
                writableSegment().write(bytes)
            }
        }

        class BufferedReaderImpl(private val source: Source) : BufferedSource {
            private val buffer = Buffer()
            override fun read(byteCount: Long): ByteArray {
                if (buffer.size == 0L) source.read(buffer, Segment.SIZE)
                return buffer.read(byteCount)
            }
        }

        class BufferedWriterImpl(private val sink: Sink) : BufferedSink {
            private val buffer = Buffer()
            override fun write(bytes: ByteArray) {
                buffer.write(bytes)
                emit()
            }
            fun emit() {
                sink.write(buffer, buffer.size)
            }
        }

        fun Source.buffer(): BufferedSource = BufferedReaderImpl(this)
        fun Sink.buffer(): BufferedSink = BufferedWriterImpl(this)
        "#
    }

    #[test]
    fn source_claims_do_not_activate_product_profiles_for_codestory_packet_audit_prompt() {
        let prompt = "Audit CodeStory packet and orchestrator sufficiency for generic public helper cache source text.";
        let cases = [
            (
                "useSWRHandler",
                "src/index/use-swr.ts",
                hook_cache_source(),
                &[
                    "The public useSWR export wraps useSWRHandler with argument normalization.",
                    "useSWRHandler serializes the key before reading cache state.",
                ][..],
            ),
            (
                "BaseTransportClient",
                "src/base_client.dart",
                client_send_source(),
                &[
                    "BaseTransportClient implements convenience methods in terms of send.",
                    "BaseTransportClient.send forwards finalized requests through an HTTP client transport.",
                ][..],
            ),
            (
                "processCommand",
                "src/server.c",
                command_dispatch_source(),
                &[
                    "readQueryFromClient appends socket input and drives processInputBuffer when a full command is available.",
                    "processCommand resolves the command table entry and enforces ACL, arity, and cluster checks.",
                    "call executes the command proc and handles propagation, monitoring, and slowlog accounting.",
                ][..],
            ),
            (
                "HiArgs",
                "crates/core/main.rs",
                search_execution_source(),
                &[
                    "`HiArgs` builds traversal, matching, search, and output components used by the search pipeline.",
                    "`SearchWorker` carries matching, search, and output state for each candidate input.",
                    "SearchWorker::search executes one candidate search with matching, search, and output state.",
                ][..],
            ),
        ];

        for (symbol, path, source, blocked_claims) in cases {
            let citation = test_packet_citation(symbol, path);
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            for blocked_claim in blocked_claims {
                assert!(
                    !claims.iter().any(|claim| claim == blocked_claim),
                    "CodeStory packet audit prompt must not activate unrelated product claim `{blocked_claim}`; got {claims:?}"
                );
            }
        }
    }

    #[test]
    fn server_route_source_claims_cover_registration_tree_and_dispatch_without_eval() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let prompt =
            "Trace HTTP route registration through router engine request handler dispatch.";
        let cases = [
            (
                "New",
                "Engine construction creates route registration state and method trees.",
            ),
            (
                "RouteRegistrar.addRoute",
                "Route registration inserts handlers into the per-method route tree.",
            ),
            (
                "Engine.dispatch",
                "Request dispatch finds a route, installs handlers on the context, and advances into the handler chain.",
            ),
        ];

        for (symbol, expected) in cases {
            let citation = test_packet_citation(symbol, "src/router.go");
            let claims =
                packet_source_derived_claims_for_citation(prompt, &citation, route_engine_source());
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected production route claim `{expected}` for {symbol}; got {claims:?}"
            );
        }

        let express_prompt = "Trace how Express creates an application, registers middleware/routes, and handles an incoming request through the router and response helpers.";
        let express_cases = [
            (
                "createApplication",
                "lib/express.js",
                "function createApplication() { var app = function(req, res, next) { app.handle(req, res, next); }; mixin(app, proto, false); app.request = Object.create(req); app.response = Object.create(res); app.init(); return app; }",
                "The application factory builds a callable app object and mixes in request and response prototypes.",
            ),
            (
                "app.use",
                "lib/application.js",
                "app.use = function use(fn) { return router.use(path, fn); }",
                "app.use registers middleware on the router.",
            ),
            (
                "app.handle",
                "lib/application.js",
                "app.handle = function handle(req, res, callback) { this.router.handle(req, res, done); }",
                "app.handle delegates request handling to the router.",
            ),
            (
                "res.send",
                "lib/response.js",
                "res.send = function send(body) { this.set('Content-Length', len); this.end(chunk, encoding); return this; }",
                "res.send sets response metadata before ending the response.",
            ),
            (
                "reply.respond",
                "src/http/reply.js",
                "reply.respond = function writePayload(payload) { this.setHeader('Content-Type', 'text/plain'); return this.end(payload); }",
                "reply.respond sets response metadata before ending the response.",
            ),
        ];
        for (symbol, path, source, expected) in express_cases {
            let citation = test_packet_citation(symbol, path);
            let claims =
                packet_source_derived_claims_for_citation(express_prompt, &citation, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected production Express source claim `{expected}` for {symbol}; got {claims:?}"
            );
        }

        let metadata_only = test_packet_citation("reply.append", "src/http/reply.js");
        let metadata_only_claims = packet_source_derived_claims_for_citation(
            express_prompt,
            &metadata_only,
            "reply.append = function append(body) { this.setHeader('Content-Type', 'text/plain'); return body; }",
        );
        assert!(
            !metadata_only_claims
                .iter()
                .any(|claim| claim.contains("ending the response")),
            "metadata-only helpers must not claim terminal response writes: {metadata_only_claims:?}"
        );
    }

    #[test]
    fn mapper_plan_source_claims_use_generic_object_mapping_shapes() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let prompt = "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects through type-map lambda plans.";
        let cases = [
            (
                "IRuntimeMapper",
                NodeKind::INTERFACE,
                "src/ObjectMapping/RuntimeMapper.cs",
                r#"
                public interface IRuntimeMapperBase
                {
                    TDestination Map<TSource, TDestination>(TSource source);
                    object Map(object source, Type sourceType, Type destinationType);
                }
                public interface IRuntimeMapper : IRuntimeMapperBase
                {
                    IConfigurationProvider ConfigurationProvider { get; }
                }
                public sealed class RuntimeMapper : IRuntimeMapper
                {
                    TDestination MapCore<TSource, TDestination>(TSource source, TDestination destination) =>
                        _configuration.GetExecutionPlan<TSource, TDestination>()(source, destination);
                }
                "#,
                "IRuntimeMapper exposes public runtime mapper APIs for source-to-destination mapping.",
            ),
            (
                "MappingConfiguration",
                NodeKind::CLASS,
                "src/ObjectMapping/Configuration/MappingConfiguration.cs",
                r#"
                public sealed class MappingConfiguration
                {
                    private readonly Dictionary<TypePair, MappingPlan> _configuredMaps = new();
                    private readonly Dictionary<TypePair, MappingPlan> _resolvedMaps = new();
                    private readonly Dictionary<MapRequest, Delegate> _executionPlans = new();
                    public RuntimeMapper CreateMapper() => new(this);
                    public LambdaExpression BuildExecutionPlan(Type sourceType, Type destinationType) =>
                        _resolvedMaps[new(sourceType, destinationType)].MapExpression;
                }
                "#,
                "Mapping configuration source builds and owns runtime mapping plans.",
            ),
            (
                "MappingPlan.BuildMapperLambda",
                NodeKind::METHOD,
                "src/ObjectMapping/MappingPlan.cs",
                r#"
                public sealed class MappingPlan
                {
                    public Type SourceType { get; }
                    public Type DestinationType { get; }
                    public LambdaExpression MapExpression { get; private set; }
                    internal LambdaExpression BuildMapperLambda(IGlobalMappingConfiguration configuration) =>
                        Types.ContainsGenericParameters ? null : new MappingPlanBuilder(configuration, this).BuildMapperLambda();
                }
                "#,
                "Type-map source contributes lambda plans used by the mapping execution pipeline.",
            ),
        ];

        for (symbol, kind, path, source, expected) in cases {
            let mut citation = test_packet_citation(symbol, path);
            citation.kind = kind;
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected generic mapper source claim `{expected}` for {symbol}; got {claims:?}"
            );
        }
    }

    #[test]
    fn buffered_io_source_claims_cover_state_wrappers_and_helpers_without_eval() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let prompt =
            "Explain how buffered Source and Sink wrappers use Buffer state for reads and writes.";
        let cases = [
            (
                "Buffer",
                "src/io/buffer.kt",
                "Buffer is the in-memory byte store used by buffered reads and writes.",
            ),
            (
                "BufferedReaderImpl",
                "src/io/buffered_reader_impl.kt",
                "A buffered source wrapper reads from an upstream Source into a Buffer.",
            ),
            (
                "BufferedWriterImpl",
                "src/io/buffered_writer_impl.kt",
                "A buffered sink wrapper writes buffered bytes to an upstream Sink.",
            ),
            (
                "Buffering",
                "src/io/buffering.kt",
                "Buffering helpers wrap Source and Sink instances with buffered implementations.",
            ),
        ];

        for (symbol, path, expected) in cases {
            let citation = test_packet_citation(symbol, path);
            let claims =
                packet_source_derived_claims_for_citation(prompt, &citation, buffered_io_source());
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected production buffered-IO claim `{expected}` for {symbol}; got {claims:?}"
            );
        }
    }

    #[test]
    fn log_record_handler_source_claims_cover_logger_record_and_handler_flow() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let prompt = "Explain how a logger turns a log call into a LogRecord and passes it through handlers.";
        let cases = [
            (
                "Logger",
                "src/logging/Logger.php",
                r#"
                class Logger {
                    protected array $handlers = [];
                    public function pushHandler(HandlerInterface $handler): self {
                        array_unshift($this->handlers, $handler);
                        return $this;
                    }
                    public function log($level, string $message, array $context = []): void {
                        $this->addRecord($level, $message, $context);
                    }
                    public function addRecord($level, string $message, array $context = []): bool {
                        $record = new LogRecord(message: $message, context: $context);
                        foreach ($this->handlers as $handler) {
                            if (true === $handler->handle(clone $record)) {
                                break;
                            }
                        }
                        return true;
                    }
                }
                "#,
                &[
                    "The logger owns a handler stack populated by handler registration.",
                    "addRecord creates a log record before passing it to handlers.",
                ][..],
            ),
            (
                "Logger.log",
                "src/logging/Logger.php",
                "class Logger { public function log($level, string $message): void { $this->addRecord($level, $message); } }",
                &["The logger log method delegates into record creation."][..],
            ),
            (
                "HandlerInterface",
                "src/logging/HandlerInterface.php",
                "interface HandlerInterface { public function handle(LogRecord $record): bool; public function handleBatch(array $records): void; }",
                &["HandlerInterface defines record handling and batch handling boundaries."][..],
            ),
            (
                "AbstractProcessingHandler.handle",
                "src/logging/AbstractProcessingHandler.php",
                r#"
                abstract class AbstractProcessingHandler {
                    public function handle(LogRecord $record): bool {
                        $record = $this->processRecord($record);
                        $record->formatted = $this->getFormatter()->format($record);
                        $this->write($record);
                        return true;
                    }
                }
                "#,
                &["The processing handler handles records by processing and writing them."][..],
            ),
        ];

        for (symbol, path, source, expected_claims) in cases {
            let citation = test_packet_citation(symbol, path);
            let claims = packet_source_derived_claims_for_citation(prompt, &citation, source);
            for expected in expected_claims {
                assert!(
                    claims.iter().any(|claim| claim == expected),
                    "expected production log-record claim `{expected}` for {symbol}; got {claims:?}"
                );
            }
        }
    }

    #[test]
    fn search_execution_source_claims_activate_with_search_intent() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let prompt = "Explain how a search command parses CLI flags, walks candidate files, and executes a search through matcher, searcher, and printer components.";
        let citation = test_packet_citation("HiArgs", "crates/core/main.rs");

        let claims =
            packet_source_derived_claims_for_citation(prompt, &citation, search_execution_source());
        for expected in [
            "main delegates parsed search options into run for search execution.",
            "`HiArgs` builds traversal, matching, search, and output components used by the search pipeline.",
            "`SearchWorker` carries matching, search, and output state for each candidate input.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected production search execution claim `{expected}`; got {claims:?}"
            );
        }

        let _eval_probes = EvalProbesGuard::enabled();
        let claims =
            packet_source_derived_claims_for_citation(prompt, &citation, search_execution_source());
        for expected in [
            "`HiArgs` builds traversal, matching, search, and output components used by the search pipeline.",
            "`SearchWorker` carries matching, search, and output state for each candidate input.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected search execution claim `{expected}` with eval probes enabled; got {claims:?}"
            );
        }
    }

    #[test]
    fn role_search_execution_claims_are_eval_only() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let temp = tempfile::tempdir().expect("temp dir");
        let source_path = temp.path().join("main.rs");
        std::fs::write(&source_path, search_execution_source()).expect("write source");
        let citation = test_packet_citation("HiArgs", &source_path.to_string_lossy());
        let prompt = "Explain how a search command parses CLI flags, walks candidate files, and executes a search through matcher, searcher, and printer components.";

        assert_eq!(
            packet_source_derived_claim_for_role(
                PacketEvidenceRole::ArgumentPlanning,
                &citation,
                prompt
            ),
            None,
            "role-specific search claims should be eval-only in production"
        );

        let _eval_probes = EvalProbesGuard::enabled();
        assert_eq!(
            packet_source_derived_claim_for_role(
                PacketEvidenceRole::ArgumentPlanning,
                &citation,
                prompt
            )
            .as_deref(),
            Some(
                "`HiArgs` builds traversal, matching, search, and output components used by the search pipeline."
            )
        );
    }

    #[test]
    fn source_claims_activate_hook_cache_only_with_hook_or_swr_intent() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let generic_prompt = "Explain public helper cache behavior.";
        let citation = test_packet_citation("useSWRHandler", "src/index/use-swr.ts");
        let claims = packet_source_derived_claims_for_citation(
            generic_prompt,
            &citation,
            hook_cache_source(),
        );
        assert!(
            claims.is_empty(),
            "generic cache words must not activate SWR hook/cache claims; got {claims:?}"
        );

        let swr_prompt =
            "Explain how SWR exposes a public hook, serializes keys, and connects cache helpers.";
        let claims =
            packet_source_derived_claims_for_citation(swr_prompt, &citation, hook_cache_source());
        for expected in [
            "The public useSWR export wraps useSWRHandler with argument normalization.",
            "useSWRHandler serializes the key before reading cache state.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected production hook/cache claim `{expected}`; got {claims:?}"
            );
        }

        let unrelated_wrapper_claims = packet_source_derived_claims_for_citation(
            swr_prompt,
            &citation,
            r#"
            const useData = target(useDataHandler)
            export default useData
            "#,
        );
        assert!(
            unrelated_wrapper_claims
                .iter()
                .all(|claim| !claim.contains("argument normalization")),
            "unrelated wrapper factories containing `arg` must not imply argument normalization; got {unrelated_wrapper_claims:?}"
        );

        let _eval_probes = EvalProbesGuard::enabled();
        let claims =
            packet_source_derived_claims_for_citation(swr_prompt, &citation, hook_cache_source());
        for expected in [
            "The public useSWR export wraps useSWRHandler with argument normalization.",
            "useSWRHandler serializes the key before reading cache state.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected hook/cache claim `{expected}` with eval probes enabled; got {claims:?}"
            );
        }

        let swr_only_prompt = "Explain SWR cache behavior and its public API.";
        let claims = packet_source_derived_claims_for_citation(
            swr_only_prompt,
            &citation,
            hook_cache_source(),
        );
        for expected in [
            "The public useSWR export wraps useSWRHandler with argument normalization.",
            "useSWRHandler serializes the key before reading cache state.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "SWR-specific prompts without the word hook should still activate `{expected}`; got {claims:?}"
            );
        }

        let helper_claims = packet_source_derived_claims_for_citation(
            swr_prompt,
            &test_packet_citation("makeCacheHelper", "src/runtime/cache-helper.ts"),
            r#"
            export const makeCacheHelper = (cache, key) => {
              const state = runtimeState.get(cache)
              return [
                () => cache.get(key) || EMPTY_CACHE,
                info => {
                  const prev = cache.get(key)
                  cache.set(key, info)
                  state[5](key, info, prev)
                },
                state[6],
                () => snapshot[key] || cache.get(key)
              ] as const
            }
            "#,
        );
        assert!(
            helper_claims.iter().any(|claim| {
                claim == "makeCacheHelper provides cache get, set, subscribe, and snapshot helpers."
            }),
            "expected generic cache helper claim; got {helper_claims:?}"
        );

        let serialize_claims = packet_source_derived_claims_for_citation(
            swr_prompt,
            &test_packet_citation("normalizeKey", "src/runtime/serialize.ts"),
            r#"
            export const normalizeKey = key => {
              const args = key
              key = typeof key == 'string' ? key : stableHash(key)
              return [key, args]
            }
            "#,
        );
        assert!(
            serialize_claims
                .iter()
                .any(|claim| claim == "normalizeKey serializes hook keys into cache keys."),
            "expected generic key serialization claim; got {serialize_claims:?}"
        );

        let mutation_claims = packet_source_derived_claims_for_citation(
            "Explain how a public hook serializes keys, connects cache helpers, and routes mutate behavior.",
            &test_packet_citation("applyMutation", "src/runtime/mutate.ts"),
            r#"
            export async function applyMutation(cache, _key, data) {
              return mutateByKey(_key)
              async function mutateByKey(_k) {
                const [key] = serialize(_k)
                const [get, set] = createCacheHelper(cache, key)
                set({ data })
              }
            }
            "#,
        );
        assert!(
            mutation_claims.iter().any(|claim| claim
                == "applyMutation routes mutate behavior through the mutation helper."),
            "expected generic mutation helper claim; got {mutation_claims:?}"
        );

        let middleware_claims = packet_source_derived_claims_for_citation(
            "Explain how a public hook composes middleware around cache behavior.",
            &test_packet_citation("withMiddleware", "src/runtime/middleware.ts"),
            r#"
            export const withMiddleware = (useHook: SWRHook, middleware) => {
              return (...args) => {
                const config = { use: [] }
                const uses = (config.use || []).concat(middleware)
                return useHook(args[0], args[1], { ...config, use: uses })
              }
            }
            "#,
        );
        assert!(
            middleware_claims
                .iter()
                .any(|claim| claim == "withMiddleware composes middleware around a public hook."),
            "expected generic middleware composition claim; got {middleware_claims:?}"
        );
    }

    #[test]
    fn source_claims_activate_client_send_only_with_client_request_send_intent() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let generic_prompt = "Explain helper cache architecture.";
        let citation = test_packet_citation("BaseTransportClient", "src/base_client.dart");
        let claims = packet_source_derived_claims_for_citation(
            generic_prompt,
            &citation,
            client_send_source(),
        );
        assert!(
            claims.is_empty(),
            "generic helper/cache words must not activate Dart client send claims; got {claims:?}"
        );

        let client_prompt =
            "Explain how a client request helper routes send behavior through the transport.";
        let claims = packet_source_derived_claims_for_citation(
            client_prompt,
            &citation,
            client_send_source(),
        );
        for expected in [
            "BaseTransportClient implements convenience methods in terms of send.",
            "Response.fromStream materializes the response stream boundary.",
            "BaseTransportClient.send is the dart:io transport implementation that forwards finalized requests through an HTTP client.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected production client send claim `{expected}`; got {claims:?}"
            );
        }

        let _eval_probes = EvalProbesGuard::enabled();
        let claims = packet_source_derived_claims_for_citation(
            client_prompt,
            &citation,
            client_send_source(),
        );
        for expected in [
            "BaseTransportClient implements convenience methods in terms of send.",
            "Response.fromStream materializes the response stream boundary.",
            "BaseTransportClient.send is the dart:io transport implementation that forwards finalized requests through an HTTP client.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected client send claim `{expected}` with eval probes enabled; got {claims:?}"
            );
        }

        let top_level_claims = packet_source_derived_claims_for_citation(
            client_prompt,
            &test_packet_citation("get", "lib/http.dart"),
            r#"
            Future<Response> get(Uri url) =>
                _withClient((client) => client.get(url));
            Future<T> _withClient<T>(Future<T> Function(Client) fn) async {
              var client = Client();
              return await fn(client);
            }
            "#,
        );
        assert!(
            top_level_claims
                .iter()
                .any(|claim| claim == "Top-level HTTP helpers delegate to a Client."),
            "expected top-level helper claim; got {top_level_claims:?}"
        );

        let client_interface_claims = packet_source_derived_claims_for_citation(
            client_prompt,
            &test_packet_citation("Client.get", "src/client.dart"),
            r#"
            abstract interface class Client {
              Future<Response> get(Uri url);
              Future<Response> post(Uri url);
              Future<StreamedResponse> send(BaseRequest request);
            }
            "#,
        );
        assert!(
            client_interface_claims.iter().any(|claim| {
                claim
                    == "Client interface helper methods declare convenience request helpers and send(request)."
            }),
            "expected client interface helper claim; got {client_interface_claims:?}"
        );

        let finalize_claims = packet_source_derived_claims_for_citation(
            client_prompt,
            &test_packet_citation("BaseRequest", "src/base_request.dart"),
            r#"
            abstract class BaseRequest {
              bool _finalized = false;
              /// Finalizes the HTTP request in preparation for it being sent.
              /// Freezes all mutable fields and returns a ByteStream that emits the request body.
              ByteStream finalize() {
                _finalized = true;
                return const ByteStream(Stream.empty());
              }
            }
            "#,
        );
        assert!(
            finalize_claims
                .iter()
                .any(|claim| claim == "BaseRequest.finalize prepares the request body for sending."),
            "expected request finalization claim; got {finalize_claims:?}"
        );

        let request_claims = packet_source_derived_claims_for_citation(
            client_prompt,
            &test_packet_citation("Request.finalize", "src/request.dart"),
            r#"
            class Request extends BaseRequest {
              ByteStream finalize() {
                return ByteStream.fromBytes(bodyBytes);
              }
            }
            "#,
        );
        assert!(
            request_claims
                .iter()
                .any(|claim| claim == "Request.finalize prepares the request body for sending."),
            "expected concrete request finalization claim; got {request_claims:?}"
        );

        let response_claims = packet_source_derived_claims_for_citation(
            client_prompt,
            &test_packet_citation("Response.fromStream", "src/response.dart"),
            r#"
            class Response extends BaseResponse {
              static Future<Response> fromStream(StreamedResponse response) async {
                final body = await response.stream.toBytes();
                return Response.bytes(body, response.statusCode);
              }
            }
            "#,
        );
        assert!(
            response_claims
                .iter()
                .any(|claim| claim
                    == "Response.fromStream materializes the response stream boundary."),
            "expected response materialization claim; got {response_claims:?}"
        );
    }

    #[test]
    fn source_claims_activate_form_validation_flow_from_html_examples() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let generic_prompt = "Explain helper cache architecture.";
        let citation = test_packet_citation("showError", "form-validation/example.html");
        let claims = packet_source_derived_claims_for_citation(
            generic_prompt,
            &citation,
            r#"<form novalidate><input id="mail" required></form>"#,
        );
        assert!(
            claims.is_empty(),
            "generic prompt must not activate form validation claims; got {claims:?}"
        );

        let validation_prompt = "Explain how form validation examples combine native HTML constraints with custom JavaScript validation.";
        let full_example_claims = packet_source_derived_claims_for_citation(
            validation_prompt,
            &test_packet_citation("required", "full-example.html"),
            r#"
            <form>
              <input required pattern="\d+" min="12" max="120">
            </form>
            "#,
        );
        assert!(
            full_example_claims.iter().any(|claim| {
                claim == "The form validation examples use native required, pattern, min, and max constraints."
            }),
            "expected native constraint claim; got {full_example_claims:?}"
        );

        let custom_claims = packet_source_derived_claims_for_citation(
            validation_prompt,
            &test_packet_citation("showError", "detailed-custom-validation.html"),
            r#"
            <form novalidate>
              <input id="mail" type="email" required>
            </form>
            <script>
              const email = document.getElementById('mail');
              const emailError = document.querySelector('#mail + span.error');
              form.addEventListener('submit', function (event) {
                if (!email.validity.valid) {
                  showError();
                  event.preventDefault();
                }
              });
              function showError() {
                if(email.validity.valueMissing) {
                  emailError.textContent = 'missing';
                } else if(email.validity.typeMismatch) {
                  emailError.textContent = 'type';
                } else if(email.validity.tooShort) {
                  emailError.textContent = 'short';
                }
              }
            </script>
            "#,
        );
        for expected in [
            "A custom validation example applies script-driven validity checks before rendering messages.",
            "Custom error rendering branches on ValidityState fields to choose messages.",
            "Submit handling prevents invalid form submission.",
        ] {
            assert!(
                custom_claims.iter().any(|claim| claim == expected),
                "expected form validation claim `{expected}`; got {custom_claims:?}"
            );
        }
    }

    #[test]
    fn source_claims_activate_server_request_dispatch_flow_without_client_transport() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let generic_prompt = "Explain client request adapter behavior.";
        let claims = packet_source_derived_claims_for_citation(
            generic_prompt,
            &test_packet_citation("wsgi_app", "src/app.py"),
            "def wsgi_app(self, environ, start_response): ctx = self.request_context(environ); response = self.full_dispatch_request()",
        );
        assert!(
            claims.is_empty(),
            "generic client prompt must not activate server request claims; got {claims:?}"
        );

        let server_prompt = "Trace how a WSGI app receives a request, opens request handling, dispatches to a view, finalizes the response, and returns control to the server.";
        let entry_claims = packet_source_derived_claims_for_citation(
            server_prompt,
            &test_packet_citation("wsgi_app", "src/app.py"),
            "def wsgi_app(self, environ, start_response): ctx = self.request_context(environ); response = self.full_dispatch_request()",
        );
        assert!(
            entry_claims.iter().any(|claim| {
                claim == "wsgi_app is the WSGI entry point and creates or uses request context before dispatch."
            }),
            "expected WSGI entry claim; got {entry_claims:?}"
        );

        let dispatch_claims = packet_source_derived_claims_for_citation(
            server_prompt,
            &test_packet_citation("dispatch_request", "src/app.py"),
            "def dispatch_request(self): return self.ensure_sync(self.view_functions[rule.endpoint])(**view_args)",
        );
        assert!(
            dispatch_claims.iter().any(|claim| {
                claim == "dispatch_request invokes the view function selected by URL matching."
            }),
            "expected view dispatch claim; got {dispatch_claims:?}"
        );
    }

    #[test]
    fn source_claims_activate_html_css_template_structure_without_fixed_filenames() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let prompt = "Explain how an HTML app shell and CSS structure split template selectors, theme defaults, and interactive element styling.";
        let html_claims = packet_source_derived_claims_for_citation(
            prompt,
            &test_packet_citation("app", "templates/home.html"),
            r#"
            <html>
              <head>
                <meta name="viewport" content="width=device-width">
              </head>
              <body>
                <div id="app"></div>
                <script type="module" src="/main.js"></script>
              </body>
            </html>
            "#,
        );
        assert!(
            html_claims.iter().any(|claim| {
                claim == "home.html provides the app shell with viewport metadata, div#app, and a script[type=\"module\"] module script entry."
            }),
            "expected generic HTML app-shell claim; got {html_claims:?}"
        );

        let css_claims = packet_source_derived_claims_for_citation(
            prompt,
            &test_packet_citation("app", "assets/main.css"),
            r#"
            :root { font-family: system-ui; color-scheme: light dark; -webkit-font-smoothing: antialiased; }
            body { margin: 0; min-height: 100vh; }
            #app { max-width: 64rem; margin: 0 auto; padding: 2rem; }
            .logo:hover { transition: filter 300ms; }
            button:hover { color: blue; }
            button:focus-visible { outline: auto; }
            @media (prefers-color-scheme: light) { :root { color: #111; } a:hover { color: #333; } button { background: white; } }
            "#,
        );
        for expected in [
            "main.css owns :root typography, color-scheme, smoothing, and body layout defaults.",
            "CSS app container rules constrain mounted content and center it with padding.",
            "CSS interaction selectors define hover, focus, and transition behavior.",
            "Light color-scheme media query rules override root, link-hover, and button colors.",
        ] {
            assert!(
                css_claims.iter().any(|claim| claim == expected),
                "expected structural CSS claim `{expected}`; got {css_claims:?}"
            );
        }
    }

    #[test]
    fn source_claims_activate_shell_install_dispatch_flow() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let generic_prompt = "Explain helper cache architecture.";
        let claims = packet_source_derived_claims_for_citation(
            generic_prompt,
            &test_packet_citation("nvm_do_install", "install.sh"),
            "nvm_do_install() { nvm_install_node; }",
        );
        assert!(
            claims.is_empty(),
            "generic prompt must not activate shell install-dispatch claims; got {claims:?}"
        );

        let shell_prompt = "Trace how an install script bootstraps the shell function and dispatches install, download, and use commands.";
        let install_claims = packet_source_derived_claims_for_citation(
            shell_prompt,
            &test_packet_citation("nvm_do_install", "install.sh"),
            r#"
            nvm_do_install() {
              SOURCE_STR='[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"'
              COMPLETION_STR='[ -s "$NVM_DIR/bash_completion" ] && . "$NVM_DIR/bash_completion"'
              nvm_install_node
            }
            install_nvm_as_script() {
              nvm_download -s "$NVM_SOURCE_LOCAL" -o "$INSTALL_DIR/nvm.sh"
              nvm_download -s "$NVM_BASH_COMPLETION_SOURCE" -o "$INSTALL_DIR/bash_completion"
            }
            "#,
        );
        for expected in [
            "The installer bootstraps shell runtime sourcing.",
            "The installer fetches shell support assets and completion/runtime files.",
        ] {
            assert!(
                install_claims.iter().any(|claim| claim == expected),
                "expected shell install claim `{expected}`; got {install_claims:?}"
            );
        }

        let dispatcher_claims = packet_source_derived_claims_for_citation(
            shell_prompt,
            &test_packet_citation("nvm", "nvm.sh"),
            r#"
            nvm() {
              local COMMAND
              COMMAND="${1-}"
              case $COMMAND in
                "install") nvm_install_node ;;
                "use") nvm_use_if_needed "$@" ;;
              esac
            }
            "#,
        );
        assert!(
            dispatcher_claims
                .iter()
                .any(|claim| claim == "The shell dispatcher branches on command arguments."),
            "expected shell dispatcher claim; got {dispatcher_claims:?}"
        );

        let completion_claims = packet_source_derived_claims_for_citation(
            shell_prompt,
            &test_packet_citation("__nvm", "bash_completion"),
            r#"
            __nvm() { __nvm_commands; }
            complete -o default -F __nvm nvm
            "#,
        );
        assert!(
            completion_claims.iter().any(|claim| {
                claim == "Shell completion registers a completion function for the command."
            }),
            "expected shell completion claim; got {completion_claims:?}"
        );
    }

    #[test]
    fn source_claims_activate_command_claims_only_with_command_event_loop_intent() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let generic_prompt = "Audit packet helper cache source shapes.";
        let citation = test_packet_citation("processCommand", "src/server.c");
        let claims = packet_source_derived_claims_for_citation(
            generic_prompt,
            &citation,
            command_dispatch_source(),
        );
        assert!(
            claims.is_empty(),
            "generic prompt must not activate command dispatch claims; got {claims:?}"
        );

        let command_prompt = "Explain Redis command dispatch from network command input through the command table and slowlog call accounting.";
        let claims = packet_source_derived_claims_for_citation(
            command_prompt,
            &citation,
            command_dispatch_source(),
        );
        for expected in [
            "readQueryFromClient appends socket input and drives processInputBuffer when a full command is available.",
            "processCommand resolves the command table entry and enforces ACL, arity, and cluster checks.",
            "call executes the command proc and handles propagation, monitoring, and slowlog accounting.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected production command/event-loop claim `{expected}`; got {claims:?}"
            );
        }

        let _eval_probes = EvalProbesGuard::enabled();
        let claims = packet_source_derived_claims_for_citation(
            command_prompt,
            &citation,
            command_dispatch_source(),
        );
        for expected in [
            "readQueryFromClient appends socket input and drives processInputBuffer when a full command is available.",
            "processCommand resolves the command table entry and enforces ACL, arity, and cluster checks.",
            "call executes the command proc and handles propagation, monitoring, and slowlog accounting.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected command/event-loop claim `{expected}` with eval probes enabled; got {claims:?}"
            );
        }
    }
}
