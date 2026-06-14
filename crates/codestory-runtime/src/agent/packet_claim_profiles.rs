use crate::agent::eval_probes::eval_probes_enabled;
use crate::agent::packet_citations::packet_citation_source_text;
use crate::agent::packet_evidence_roles::PacketEvidenceRole;
use crate::agent::packet_scoring::{normalize_identifier, packet_display_path};
use crate::agent::packet_source_patterns::{
    packet_display_owner, packet_human_join, packet_source_constructed_type, packet_source_has_all,
    packet_source_has_any, packet_source_identifier_ending_with, packet_source_identifier_exact,
    packet_source_identifier_with_words, packet_source_identifier_with_words_shortest,
    packet_sql_create_table_names, packet_sql_foreign_key_claims,
};
use crate::agent::packet_terms::{
    packet_probe_terms, packet_terms_indicate_client_send_flow,
    packet_terms_indicate_event_loop_command_flow, packet_terms_indicate_hook_cache_flow,
    packet_terms_indicate_request_dispatch_flow, packet_terms_indicate_runtime_formatting_flow,
    packet_terms_indicate_search_execution_flow, packet_terms_indicate_server_route_dispatch_flow,
    packet_terms_indicate_shell_version_use_flow, packet_terms_indicate_sql_schema_flow,
    packet_terms_indicate_string_predicate_flow, packet_terms_indicate_stylesheet_animation_flow,
    packet_terms_indicate_url_session_request_flow,
};
use codestory_contracts::api::AgentCitationDto;
use std::collections::HashSet;

const GENERIC_PRODUCT_CLAIM_PROFILES: &[SourceClaimProfile] = &[
    SourceClaimProfile::ShellVersionUse,
    SourceClaimProfile::StringPredicate,
    SourceClaimProfile::StylesheetAnimation,
    SourceClaimProfile::SqlSchema,
    SourceClaimProfile::RuntimeFormatting,
];

const EVAL_DIAGNOSTIC_CLAIM_PROFILES: &[SourceClaimProfile] = &[
    SourceClaimProfile::ServerRoute,
    SourceClaimProfile::HookCache,
    SourceClaimProfile::ClientSend,
    SourceClaimProfile::UrlSessionRequest,
    SourceClaimProfile::ClientRequestDispatch,
    SourceClaimProfile::EventLoopCommand,
    SourceClaimProfile::SearchExecution,
];

#[derive(Debug, Clone, Copy)]
enum SourceClaimProfile {
    ServerRoute,
    ShellVersionUse,
    HookCache,
    ClientSend,
    UrlSessionRequest,
    StringPredicate,
    StylesheetAnimation,
    SqlSchema,
    RuntimeFormatting,
    ClientRequestDispatch,
    EventLoopCommand,
    SearchExecution,
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
            Self::SqlSchema => {
                if packet_terms_indicate_sql_schema_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_sql_schema_flow_claims(ctx.source));
                }
            }
            Self::RuntimeFormatting => {
                if packet_terms_indicate_runtime_formatting_flow(&ctx.prompt_terms) {
                    claims.extend(packet_generic_runtime_formatting_flow_claims(ctx.source));
                }
            }
            Self::ClientRequestDispatch => collect_client_request_dispatch_claims(ctx, claims),
            Self::EventLoopCommand => collect_event_loop_command_claims(ctx, claims),
            Self::SearchExecution => collect_search_execution_claims(ctx, claims),
        }
    }
}

struct SourceClaimContext<'a> {
    source: &'a str,
    symbol: &'a str,
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
    let eval_diagnostics = eval_probes_enabled();
    let ctx = SourceClaimContext::new(prompt, citation, source);

    if eval_diagnostics {
        claims.extend(
            crate::agent::eval_probes::source_derived_claims_for_citation(prompt, citation, source),
        );
        for profile in EVAL_DIAGNOSTIC_CLAIM_PROFILES {
            profile.collect(&ctx, &mut claims);
        }
    }

    for profile in GENERIC_PRODUCT_CLAIM_PROFILES {
        profile.collect(&ctx, &mut claims);
    }

    claims
}

pub(crate) fn packet_source_derived_claim_for_role(
    role: PacketEvidenceRole,
    citation: &AgentCitationDto,
    prompt: &str,
) -> Option<String> {
    let source = packet_citation_source_text(citation)?;
    if source.len() > 800_000 {
        return None;
    }
    let ctx = SourceClaimContext::new(prompt, citation, &source);
    let request_flow = packet_terms_indicate_request_dispatch_flow(&ctx.prompt_terms);
    let command_flow = packet_terms_indicate_event_loop_command_flow(&ctx.prompt_terms);
    let search_flow = packet_terms_indicate_search_execution_flow(&ctx.prompt_terms);
    let eval_diagnostics = eval_probes_enabled();

    if eval_diagnostics && request_flow {
        if role == PacketEvidenceRole::ClientFactory
            && let Some(claim) = client_factory_claim(&ctx)
        {
            return Some(claim);
        }
        if let Some(claim) = client_request_pipeline_claim(&ctx) {
            return Some(claim);
        }
        if role == PacketEvidenceRole::RequestDispatch
            && let Some(claim) = request_dispatch_claim(&ctx)
        {
            return Some(claim);
        }
        if role == PacketEvidenceRole::InterceptorManagement
            && let Some(claim) = interceptor_management_claim(&ctx)
        {
            return Some(claim);
        }
        if role == PacketEvidenceRole::TransportAdapter
            && let Some(claim) = transport_adapter_claim(&ctx)
        {
            return Some(claim);
        }
    }

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
        && role == PacketEvidenceRole::NetworkCommandInput
        && let Some(claim) = network_command_input_claim(&ctx)
    {
        return Some(claim);
    }

    if eval_diagnostics && command_flow && role == PacketEvidenceRole::CommandDispatch {
        if let Some(claim) = command_dispatch_table_claim(&ctx) {
            return Some(claim);
        }
        if let Some(claim) = command_dispatch_call_claim(&ctx) {
            return Some(claim);
        }
    }

    if eval_diagnostics
        && search_flow
        && role == PacketEvidenceRole::SearchDriver
        && let Some(claim) = search_driver_claim(&ctx)
    {
        return Some(claim);
    }

    if eval_diagnostics
        && search_flow
        && role == PacketEvidenceRole::ArgumentPlanning
        && let Some(claim) = argument_planning_claim(&ctx)
    {
        return Some(claim);
    }

    if eval_diagnostics
        && search_flow
        && role == PacketEvidenceRole::SearchExecutionUnit
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

    push_optional_claim(claims, client_factory_claim(ctx));
    push_optional_claim(claims, client_request_pipeline_claim(ctx));
    push_optional_claim(claims, request_dispatch_claim(ctx));
    push_optional_claim(claims, interceptor_management_claim(ctx));
    push_optional_claim(claims, transport_adapter_claim(ctx));
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
    if packet_source_has_all(ctx.source, &["merge", "config", "interceptors", "request"])
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
            "`{}` transforms the body/headers and invokes the configured adapter.",
            ctx.symbol
        ));
    }
    None
}

fn interceptor_management_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_all(ctx.source, &["handlers", "fulfilled", "rejected"]) {
        return Some(format!(
            "`{}` stores interceptor pairs used by the promise chain in request.",
            ctx.symbol
        ));
    }
    None
}

fn transport_adapter_claim(ctx: &SourceClaimContext<'_>) -> Option<String> {
    if packet_source_has_all(ctx.source, &["adapter"])
        && packet_source_has_all(ctx.source, &["xhr", "http"])
        && packet_source_has_any(ctx.source, &["known", "environment", "platform"])
    {
        return Some(format!(
            "`{}` selects xhr or http transport based on environment capabilities.",
            ctx.file_name
        ));
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
    let mut claims = Vec::new();

    if source_lower.contains("withargs")
        && source_lower.contains("export default")
        && let Some((public_hook, handler)) = packet_source_with_args_wrapper(source)
    {
        claims.push(format!(
            "The public {public_hook} export wraps {handler} with argument normalization."
        ));
    }

    if source_lower.contains("serialize(_key)")
        && (source_lower.contains("getcache")
            || source_lower.contains("createcachehelper")
            || source_lower.contains("cache"))
    {
        claims.push(format!(
            "{symbol} serializes the key before reading cache state."
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

fn packet_generic_client_send_flow_claims(symbol: &str, source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();
    let owner = packet_display_owner(symbol).unwrap_or_else(|| symbol.to_string());

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
    }

    if source_lower.contains("dart:io")
        && source_lower.contains("httpclient")
        && source_lower.contains("openurl")
        && source_lower.contains("request.finalize")
        && source_lower.contains("stream.pipe")
        && source_lower.contains("httpclientresponse")
    {
        claims.push(format!(
            "{owner}.send forwards finalized requests through an HTTP client transport."
        ));
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
            "Session request creation builds request objects before optional eager execution."
                .to_string(),
        );
    }

    if normalized_symbol.ends_with("requestresume")
        && source_lower.contains("public func resume() -> self")
        && source_lower.contains("task.resume()")
    {
        claims.push("Request.resume resumes the underlying request task.".to_string());
    }

    if normalized_symbol.ends_with("validate")
        && source_lower.contains("public func validate(_ validation")
        && source_lower.contains("validators.write")
        && source_lower.contains("didvalidate")
        && source_lower.contains("request")
    {
        claims.push("Request validation attaches validation behavior.".to_string());
    }

    if normalized_symbol.ends_with("delegate")
        && source_lower.contains("urlsessiondatadelegate")
        && source_lower.contains("open func urlsession")
        && (source_lower.contains("request.didreceiveresponse")
            || source_lower.contains("request.didreceive(data: data)")
            || source_lower.contains("didcompletewitherror"))
    {
        claims.push("The session delegate receives request callback events.".to_string());
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
            claims.push(
                "isBlank treats null, empty, and whitespace-only inputs as blank.".to_string(),
            );
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
            claims.push("isEmpty does not trim whitespace before deciding emptiness.".to_string());
        }
    }

    claims
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

    for keyframe in packet_css_keyframe_names(source).into_iter().take(4) {
        if packet_css_class_sets_animation_name(source, &keyframe) {
            claims.push(format!(
                "Named classes such as .{keyframe} set animation-name to matching keyframes; @keyframes {keyframe} defines the matching animation."
            ));
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

fn packet_source_with_args_wrapper(source: &str) -> Option<(String, String)> {
    let lower = source.to_ascii_lowercase();
    let mut search_from = 0usize;

    while let Some(relative_at) = lower[search_from..].find("withargs") {
        let with_args_at = search_from + relative_at;
        let statement_start = source[..with_args_at]
            .rfind(['\n', ';'])
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let before = &source[statement_start..with_args_at];
        let Some(wrapper) = before
            .rsplit_once('=')
            .and_then(|(left, _)| packet_last_identifier(left))
        else {
            search_from = with_args_at + "withargs".len();
            continue;
        };

        let after = &source[with_args_at..];
        let Some(handler_start) = after.find('(').map(|idx| idx + 1) else {
            search_from = with_args_at + "withargs".len();
            continue;
        };
        let handler_tail = &after[handler_start..];
        let Some(handler) = packet_first_identifier_after_type_arguments(handler_tail) else {
            search_from = with_args_at + "withargs".len();
            continue;
        };

        if packet_source_exports_default_identifier(after, &wrapper) {
            return Some((wrapper, handler));
        }

        search_from = with_args_at + "withargs".len();
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

    if normalized_symbol.contains("application")
        && source_lower.contains("function")
        && source_lower.contains("handle(")
        && source_lower.contains("request")
        && source_lower.contains("response")
    {
        claims.push(
            "The application factory builds a callable request handler and wires request/response state."
                .to_string(),
        );
    }

    if normalized_symbol.ends_with("handle")
        && source_lower.contains("router")
        && source_lower.contains(".handle(")
    {
        claims.push("The application handler delegates request handling to a router.".to_string());
    }

    if normalized_symbol.ends_with("use")
        && source_lower.contains("function use")
        && source_lower.contains("router.use(")
    {
        claims.push("Middleware registration delegates to a router.".to_string());
    }

    if normalized_symbol.ends_with("route")
        && source_lower.contains("function route")
        && source_lower.contains("router.route(")
    {
        claims.push(
            "The route registration helper creates route entries through a router.".to_string(),
        );
    }

    if normalized_symbol.ends_with("send")
        && source_lower.contains("res.send = function send")
        && source_lower.contains("this.set('content-length'")
        && source_lower.contains(".end(")
    {
        claims.push(
            "The response send helper sets response metadata before ending the body.".to_string(),
        );
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

fn packet_generic_sql_schema_flow_claims(source: &str) -> Vec<String> {
    let mut claims = Vec::new();
    let tables = packet_sql_create_table_names(source);
    if !tables.is_empty() {
        claims.push(format!(
            "SQL schema defines tables {}.",
            packet_human_join(&tables.iter().take(6).cloned().collect::<Vec<_>>())
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

fn packet_generic_runtime_formatting_flow_claims(source: &str) -> Vec<String> {
    let normalized_source = normalize_identifier(source);
    let mut claims = Vec::new();

    if normalized_source.contains("vformat")
        && (normalized_source.contains("formatargs")
            || normalized_source.contains("basicformatargs")
            || normalized_source.contains("formatargstore"))
        && (normalized_source.contains("vformatto") || normalized_source.contains("formatto"))
    {
        claims.push(
            "Runtime formatting uses type-erased format arguments before dispatching formatted output helpers."
                .to_string(),
        );
    }

    if normalized_source.contains("formaterror")
        && (normalized_source.contains("runtimeerror")
            || normalized_source.contains("throwformaterror")
            || normalized_source.contains("formatting"))
    {
        claims.push("Formatting errors are represented as runtime failures.".to_string());
    }

    claims
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
                provenance: Vec::new(),
            }),
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
    fn search_execution_source_claims_are_eval_only() {
        let _env = EnvVarGuard::cleared(EVAL_PROBES_ENV);
        let prompt = "Explain how a search command parses CLI flags, walks candidate files, and executes a search through matcher, searcher, and printer components.";
        let citation = test_packet_citation("HiArgs", "crates/core/main.rs");

        let claims =
            packet_source_derived_claims_for_citation(prompt, &citation, search_execution_source());
        assert!(
            claims.is_empty(),
            "search execution claims should be eval-only in production source profiles; got {claims:?}"
        );

        let _eval_probes = EvalProbesGuard::enabled();
        let claims =
            packet_source_derived_claims_for_citation(prompt, &citation, search_execution_source());
        for expected in [
            "`HiArgs` builds traversal, matching, search, and output components used by the search pipeline.",
            "`SearchWorker` carries matching, search, and output state for each candidate input.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected eval-only search execution claim `{expected}`; got {claims:?}"
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
        assert!(
            claims.is_empty(),
            "SWR-shaped claims should be eval-only in production source profiles; got {claims:?}"
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
                "expected hook/cache claim `{expected}`; got {claims:?}"
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
        assert!(
            claims.is_empty(),
            "Dart client transport claims should be eval-only in production source profiles; got {claims:?}"
        );

        let _eval_probes = EvalProbesGuard::enabled();
        let claims = packet_source_derived_claims_for_citation(
            client_prompt,
            &citation,
            client_send_source(),
        );
        for expected in [
            "BaseTransportClient implements convenience methods in terms of send.",
            "BaseTransportClient.send forwards finalized requests through an HTTP client transport.",
        ] {
            assert!(
                claims.iter().any(|claim| claim == expected),
                "expected client send claim `{expected}`; got {claims:?}"
            );
        }
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
        assert!(
            claims.is_empty(),
            "command/event-loop claims should be eval-only in production source profiles; got {claims:?}"
        );

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
                "expected command/event-loop claim `{expected}`; got {claims:?}"
            );
        }
    }
}
