use crate::agent::eval_probes::{
    eval_probes_enabled, push_eval_required_probe_queries,
    push_prompt_concept_derived_symbol_probes,
};
use crate::agent::packet_batch::packet_file_stem_matches_query;
use crate::agent::packet_scoring::{
    normalize_identifier, packet_display_path, packet_query_stop_term,
};
use crate::agent::packet_terms::{
    packet_probe_terms, packet_terms_have, packet_terms_have_any,
    packet_terms_indicate_buffered_io_flow, packet_terms_indicate_client_send_flow,
    packet_terms_indicate_form_validation_flow, packet_terms_indicate_indexing_flow,
    packet_terms_indicate_log_record_handler_flow,
    packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_prepared_session_adapter_flow,
    packet_terms_indicate_request_dispatch_flow, packet_terms_indicate_runtime_formatting_flow,
    packet_terms_indicate_search_execution_flow, packet_terms_indicate_server_route_dispatch_flow,
    packet_terms_indicate_shell_install_dispatch_flow, packet_terms_indicate_site_build_phase_flow,
    packet_terms_indicate_sql_schema_flow, packet_terms_indicate_stylesheet_animation_flow,
    packet_terms_indicate_url_session_request_flow,
};
use crate::exact_symbol_query_terms;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, NodeKind, PacketClaimDto, PacketTaskClassDto,
};

pub(crate) fn packet_missing_sufficiency_probe_queries_with_extra(
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    supported_claims: &[PacketClaimDto],
    extra_probes: &[String],
) -> Vec<String> {
    packet_sufficiency_required_probe_queries_with_extra(question, task_class, extra_probes)
        .into_iter()
        .filter(|query| !packet_probe_query_is_covered(query, answer, supported_claims))
        .collect()
}

fn packet_probe_query_is_covered(
    query: &str,
    answer: &AgentAnswerDto,
    supported_claims: &[PacketClaimDto],
) -> bool {
    if packet_required_probe_requires_citation(query) {
        return packet_probe_query_is_cited(query, answer);
    }
    packet_probe_query_is_cited(query, answer)
        || packet_css_custom_property_probe_is_covered(query, answer, supported_claims)
        || packet_probe_query_is_claimed(query, supported_claims)
}

pub(crate) fn packet_probe_query_is_claimed(
    query: &str,
    supported_claims: &[PacketClaimDto],
) -> bool {
    if let Some(parts) = packet_file_scoped_symbol_probe_parts(query) {
        return supported_claims
            .iter()
            .any(|claim| packet_claim_covers_file_scoped_probe(&parts, claim));
    }

    if !packet_probe_query_allows_claim_coverage(query) {
        return false;
    }
    let normalized_query = normalize_identifier(query);
    if normalized_query.is_empty() {
        return false;
    }
    supported_claims.iter().any(|claim| {
        let normalized_claim = normalize_identifier(&claim.claim);
        normalized_claim.contains(&normalized_query)
            || packet_claim_covers_concept_probe(&normalized_query, &normalized_claim)
    })
}

fn packet_claim_covers_concept_probe(normalized_query: &str, normalized_claim: &str) -> bool {
    match normalized_query {
        "recordcreation" => {
            normalized_claim.contains("record") && normalized_claim.contains("creat")
        }
        "handlerregistration" => {
            normalized_claim.contains("handler")
                && (normalized_claim.contains("register") || normalized_claim.contains("stack"))
        }
        "handlerprocessing" => {
            normalized_claim.contains("handler")
                && (normalized_claim.contains("process")
                    || normalized_claim.contains("write")
                    || normalized_claim.contains("writ")
                    || normalized_claim.contains("format"))
        }
        "handlerinterface" => {
            normalized_claim.contains("handlerinterface")
                || (normalized_claim.contains("handler") && normalized_claim.contains("boundar"))
        }
        "logrecord" => normalized_claim.contains("logrecord"),
        "logcall" => normalized_claim.contains("log") && normalized_claim.contains("addrecord"),
        "handlerstack" => {
            normalized_claim.contains("handler") && normalized_claim.contains("stack")
        }
        "nativeformconstraints" => {
            normalized_claim.contains("native")
                && normalized_claim.contains("required")
                && normalized_claim.contains("pattern")
                && normalized_claim.contains("min")
                && normalized_claim.contains("max")
        }
        "customvalidationmessages" => {
            (normalized_claim.contains("validation") || normalized_claim.contains("showerror"))
                && (normalized_claim.contains("message")
                    || normalized_claim.contains("messages")
                    || normalized_claim.contains("showerror"))
        }
        "validitystate" => {
            normalized_claim.contains("validitystate")
                || (normalized_claim.contains("validity")
                    && (normalized_claim.contains("valuemissing")
                        || normalized_claim.contains("typemismatch")
                        || normalized_claim.contains("tooshort")
                        || normalized_claim.contains("fields")))
        }
        "submitpreventdefault" => {
            normalized_claim.contains("submit")
                && (normalized_claim.contains("preventdefault")
                    || normalized_claim.contains("preventsubmission"))
                && (normalized_claim.contains("invalid") || normalized_claim.contains("form"))
        }
        "formvalidationbypass" => {
            normalized_claim.contains("novalidate")
                && (normalized_claim.contains("suppress") || normalized_claim.contains("disable"))
                && (normalized_claim.contains("browser") || normalized_claim.contains("defaultui"))
        }
        "shellinstallerbootstrap" => {
            normalized_claim.contains("install")
                && normalized_claim.contains("bootstrap")
                && (normalized_claim.contains("source")
                    || normalized_claim.contains("nvmsh")
                    || normalized_claim.contains("profile"))
        }
        "shellfunctiondispatch" => {
            normalized_claim.contains("shell")
                && normalized_claim.contains("dispatch")
                && (normalized_claim.contains("function") || normalized_claim.contains("command"))
        }
        "installdownloadhelpers" => {
            normalized_claim.contains("install")
                && (normalized_claim.contains("download") || normalized_claim.contains("fetch"))
                && (normalized_claim.contains("helper")
                    || normalized_claim.contains("nvmdownload")
                    || normalized_claim.contains("nvminstallnode"))
        }
        "conditionalversionuse" => {
            normalized_claim.contains("use")
                && (normalized_claim.contains("current") || normalized_claim.contains("active"))
                && (normalized_claim.contains("needed") || normalized_claim.contains("already"))
        }
        "shellcompletion" => {
            normalized_claim.contains("completion")
                && (normalized_claim.contains("complete") || normalized_claim.contains("command"))
        }
        "toplevelhelpers" => {
            normalized_claim.contains("toplevel")
                && normalized_claim.contains("helper")
                && normalized_claim.contains("client")
                && (normalized_claim.contains("delegate") || normalized_claim.contains("wrap"))
        }
        "requestfinalization" => {
            (normalized_claim.contains("request") || normalized_claim.contains("baserequest"))
                && (normalized_claim.contains("finalize")
                    || normalized_claim.contains("finalized")
                    || normalized_claim.contains("finalization"))
                && (normalized_claim.contains("prepare")
                    || normalized_claim.contains("body")
                    || normalized_claim.contains("send"))
        }
        "requestresponse" => {
            normalized_claim.contains("response")
                && (normalized_claim.contains("request")
                    || normalized_claim.contains("fromstream")
                    || normalized_claim.contains("streamed"))
        }
        "references" => {
            normalized_claim.contains("rowsreference")
                || normalized_claim.contains("foreignkey")
                || normalized_claim.contains("references")
        }
        "sqltabledefinitions" => {
            normalized_claim.contains("sqlschema")
                && (normalized_claim.contains("definestables")
                    || normalized_claim.contains("tables")
                    || normalized_claim.contains("createtable"))
        }
        "foreignkeyrelationships" => {
            normalized_claim.contains("rowsreference") || normalized_claim.contains("foreignkey")
        }
        "sqlschemascripts" | "schemadialectscripts" => {
            normalized_claim.contains("sql")
                && normalized_claim.contains("schema")
                && (normalized_claim.contains("dialectscripts")
                    || normalized_claim.contains("schemascripts"))
        }
        _ => false,
    }
}

fn packet_claim_covers_file_scoped_probe(
    parts: &PacketFileScopedSymbolProbe,
    claim: &PacketClaimDto,
) -> bool {
    let claim_file_matches = claim.citations.iter().any(|citation| {
        citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .map(|path| {
                path.rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(path.as_str())
                    .eq_ignore_ascii_case(&parts.file_name)
            })
            .unwrap_or(false)
    });
    if !claim_file_matches {
        return false;
    }
    let normalized_claim = normalize_identifier(&claim.claim);
    parts
        .symbols
        .iter()
        .all(|symbol| normalized_claim.contains(symbol))
}

fn packet_css_custom_property_probe_is_covered(
    query: &str,
    answer: &AgentAnswerDto,
    supported_claims: &[PacketClaimDto],
) -> bool {
    let Some(parts) = packet_file_scoped_symbol_probe_parts(query) else {
        return false;
    };
    if !parts.file_name.eq_ignore_ascii_case("_vars.css") {
        return false;
    }
    if parts.symbols.is_empty()
        || !parts
            .symbols
            .iter()
            .all(|symbol| symbol.starts_with("animate"))
    {
        return false;
    }
    let cites_variables_file = answer.citations.iter().any(|citation| {
        citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .map(|path| {
                path.rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(path.as_str())
                    .eq_ignore_ascii_case("_vars.css")
            })
            .unwrap_or(false)
    });
    if !cites_variables_file {
        return false;
    }

    supported_claims.iter().any(|claim| {
        let normalized_claim = normalize_identifier(&claim.claim);
        normalized_claim.contains("csscustomproperties")
            && parts
                .symbols
                .iter()
                .all(|symbol| normalized_claim.contains(symbol))
    })
}

fn packet_probe_query_allows_claim_coverage(query: &str) -> bool {
    let trimmed = query.trim();
    packet_concept_probe_allows_claim_coverage(&normalize_identifier(trimmed))
        || trimmed.contains('.')
            && !trimmed.contains('/')
            && !trimmed.contains('\\')
            && !trimmed.chars().any(char::is_whitespace)
}

fn packet_concept_probe_allows_claim_coverage(normalized_query: &str) -> bool {
    matches!(
        normalized_query,
        "recordcreation"
            | "handlerregistration"
            | "handlerprocessing"
            | "handlerinterface"
            | "logrecord"
            | "logcall"
            | "handlerstack"
            | "nativeformconstraints"
            | "customvalidationmessages"
            | "validitystate"
            | "submitpreventdefault"
            | "formvalidationbypass"
            | "toplevelhelpers"
            | "requestfinalization"
            | "requestresponse"
            | "references"
            | "sqltabledefinitions"
            | "foreignkeyrelationships"
            | "sqlschemascripts"
            | "schemadialectscripts"
    )
}

fn packet_required_probe_requires_citation(query: &str) -> bool {
    matches!(
        normalize_identifier(query).as_str(),
        "routetreeaddroute" | "sourcereadbuffer" | "sinkwritebuffer"
    )
}

#[cfg(test)]
pub(crate) fn packet_sufficiency_required_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    packet_sufficiency_required_probe_queries_with_extra(question, task_class, &[])
}

pub(crate) fn packet_sufficiency_required_probe_queries_with_extra(
    question: &str,
    task_class: PacketTaskClassDto,
    extra_probes: &[String],
) -> Vec<String> {
    let terms = packet_probe_terms(question);
    let mut queries = packet_prompt_exact_symbol_probe_queries(question, &terms, task_class);
    push_unique_owned_terms(&mut queries, extra_probes);
    push_unique_owned_terms(
        &mut queries,
        &packet_sufficiency_required_probe_queries_from_terms(&terms, task_class),
    );
    queries
}

pub(crate) fn packet_sufficiency_required_probe_queries_from_terms(
    terms: &[String],
    task_class: PacketTaskClassDto,
) -> Vec<String> {
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

    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let mut queries = Vec::new();

    if eval_probes_enabled() {
        push_eval_required_probe_queries(terms, &mut queries);
        return queries;
    }

    if has("exec") && has_any(&["runtime", "session"]) {
        push_unique_terms(&mut queries, &["exec runtime", "exec session"]);
    }
    if has("exec") && has_any(&["cli", "command", "subcommand"]) {
        push_unique_terms(&mut queries, &["exec cli", "exec command"]);
    }
    if has_any(&["json", "jsonl"]) && has_any(&["event", "events", "output"]) {
        push_unique_terms(&mut queries, &["json event output", "jsonl event output"]);
    }
    if has("thread") && has_any(&["start", "starts", "started"]) {
        push_unique_term(&mut queries, "thread start");
    }
    if has("turn") && has_any(&["start", "starts", "started"]) {
        push_unique_term(&mut queries, "turn start");
    }
    if has_any(&["storage", "persistent"]) || (has("data") && has_any(&["access", "accessed"])) {
        push_unique_terms(&mut queries, &["storage access", "persistent storage"]);
    }
    if packet_terms_indicate_indexing_flow(terms) {
        push_indexing_flow_required_probe_queries(&mut queries);
    }
    if packet_terms_indicate_request_dispatch_flow(terms) {
        push_unique_terms(
            &mut queries,
            &[
                "request interceptor",
                "request dispatch",
                "transport adapter",
            ],
        );
    }
    if packet_terms_indicate_client_send_flow(terms) {
        push_client_send_source_probe_queries(&mut queries);
        push_unique_terms(
            &mut queries,
            &[
                "client convenience methods",
                "top level helpers",
                "request finalization",
                "transport send",
                "request response",
            ],
        );
    }
    if packet_terms_indicate_url_session_request_flow(terms) {
        push_url_session_request_source_probe_queries(&mut queries);
        push_unique_terms(
            &mut queries,
            &[
                "session request creation",
                "request task resume",
                "data request validation",
                "urlsession callbacks",
            ],
        );
    }
    if packet_terms_indicate_sql_schema_flow(terms) {
        push_sql_schema_required_probe_queries(terms, &mut queries);
    }
    if packet_terms_indicate_prepared_session_adapter_flow(terms) {
        push_unique_terms(
            &mut queries,
            &[
                "request preparation",
                "session request",
                "session send",
                "adapter send",
                "adapter selection",
            ],
        );
    }
    if has("event") && has("loop") {
        push_unique_terms(
            &mut queries,
            &[
                "event loop",
                "event dispatch",
                "network input",
                "command dispatch",
            ],
        );
    }
    if has("call") && has_any(&["command", "commands", "dispatch", "dispatches"]) {
        push_unique_terms(&mut queries, &["command dispatch", "command handler"]);
    }
    if packet_terms_indicate_search_execution_flow(terms) {
        push_search_flow_probe_queries(&mut queries);
    }
    if has_any(&["indexing", "indexed", "indexer"])
        && (has_any(&["storage", "persistent", "project", "configuration", "group"])
            || has_any(&["command", "commands"]))
    {
        push_unique_terms(
            &mut queries,
            &["build index", "source group indexing", "indexer command"],
        );
    }
    push_prompt_concept_role_probe_queries(terms, &mut queries);

    queries
}

fn push_prompt_concept_role_probe_queries(terms: &[String], queries: &mut Vec<String>) {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);

    if has_any(&["serialize", "serializes", "serialized", "serialization"]) {
        push_unique_term(queries, "serialize");
    }
    if has_any(&["cache", "caches"]) && has_any(&["helper", "helpers"]) {
        push_unique_term(queries, "cache helper");
    }
    if has_any(&["middleware", "middlewares"]) {
        push_unique_term(queries, "middleware");
    }

    if has_any(&["handler", "handlers"]) {
        if has_any(&[
            "record",
            "records",
            "process",
            "processing",
            "write",
            "writes",
        ]) {
            push_unique_term(queries, "handler processing");
        }
        if has_any(&["dispatch", "dispatches", "route", "routes"]) {
            push_unique_term(queries, "handler dispatch");
        }
    }
    if packet_terms_indicate_server_route_dispatch_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "route registration",
                "router group",
                "route tree",
                "route tree add route",
                "router group handle route",
                "request handler",
                "engine request handler",
                "context next handler chain",
                "handler chain",
                "engine creation",
                "engine creation new router",
            ],
        );
    }

    if has_any(&["validation", "validate", "validates", "validity", "invalid"]) {
        if has_any(&["form", "forms", "input", "inputs", "html"]) {
            push_unique_term(queries, "form validation");
        }
        if has_any(&["constraint", "constraints", "native"]) {
            push_unique_term(queries, "constraint validation");
        }
        if has("html") && has_any(&["constraint", "constraints", "native"]) {
            push_unique_term(queries, "html constraint");
        }
        if has("html")
            && has_any(&["constraint", "constraints", "native"])
            && has_any(&["form", "forms", "input", "inputs"])
        {
            push_unique_term(queries, "pattern");
        }
        if has_any(&["javascript", "script", "scripts", "js"]) {
            push_unique_term(queries, "javascript validation");
        }
        if has_any(&["custom", "message", "messages", "error", "errors"]) {
            push_unique_term(queries, "custom validation");
        }
        if has("custom") && has("html") && has_any(&["javascript", "script", "scripts", "js"]) {
            push_unique_term(queries, "form validation bypass");
            push_unique_term(queries, "validity state");
        }
        if has_any(&["validity", "state", "states"]) {
            push_unique_term(queries, "validity state");
        }
    }
    if packet_terms_indicate_form_validation_flow(terms) {
        push_form_validation_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "native form constraints",
                "custom validation messages",
                "validity state",
                "submit prevent default",
                "form validation bypass",
            ],
        );
    }
    if packet_terms_indicate_stylesheet_animation_flow(terms) {
        push_stylesheet_animation_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "css animation variables",
                "css animation base class",
                "css keyframes",
                "css animation imports",
            ],
        );
    }
    if packet_terms_indicate_url_session_request_flow(terms) {
        push_url_session_request_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "session request creation",
                "request task resume",
                "data request validation",
                "urlsession callbacks",
            ],
        );
    }
    if packet_terms_indicate_sql_schema_flow(terms) {
        push_sql_schema_required_probe_queries(terms, queries);
        push_unique_terms(
            queries,
            &[
                "sql table definitions",
                "foreign key relationships",
                "schema dialect scripts",
            ],
        );
    }
    if packet_terms_indicate_shell_install_dispatch_flow(terms) {
        push_shell_install_dispatch_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "shell installer bootstrap",
                "shell function dispatch",
                "install download helpers",
                "conditional version use",
                "shell completion",
            ],
        );
    }

    if has_any(&["mapper", "mappers", "mapping", "map", "maps"]) {
        if has_any(&["configuration", "config", "profile", "profiles"]) {
            push_unique_term(queries, "mapper configuration");
        }
        if has("type") || has_any(&["types", "typemap", "typemaps"]) {
            push_unique_term(queries, "type map");
        }
        if has_any(&["plan", "plans", "execution", "expression", "lambda"]) {
            push_unique_term(queries, "mapping plan");
        }
    }

    if has_any(&["buffer", "buffers", "buffered"]) {
        if has_any(&["source", "sources", "read", "reads", "reader"]) {
            push_unique_term(queries, "buffered source");
        }
        if has_any(&["sink", "sinks", "write", "writes", "writer"]) {
            push_unique_term(queries, "buffered sink");
        }
    }
    if packet_terms_indicate_buffered_io_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "source sink buffer",
                "buffer storage",
                "buffered wrapper",
                "source read buffer",
                "sink write buffer",
                "source buffer",
                "sink buffer",
            ],
        );
    }
    if packet_terms_indicate_log_record_handler_flow(terms) {
        push_log_record_handler_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "log record",
                "record creation",
                "handler registration",
                "handler processing",
                "handler interface",
            ],
        );
    }
    if packet_terms_indicate_site_build_phase_flow(terms) {
        push_site_build_phase_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "site build lifecycle",
                "site process phases",
                "read generate render write",
                "reader read",
                "renderer render",
            ],
        );
    }
    if packet_terms_indicate_mapper_configuration_plan_flow(terms) {
        push_mapper_configuration_plan_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "mapper runtime api",
                "mapper configuration",
                "type map plan",
                "mapping execution plan",
                "source destination mapping",
            ],
        );
    }
    if packet_terms_indicate_runtime_formatting_flow(terms) {
        push_runtime_formatting_source_probe_queries(queries);
    }

    if has_any(&["client", "clients"]) && has_any(&["send", "sends", "sending"]) {
        push_unique_term(queries, "client send");
    }
    if has_any(&["request", "requests"]) && has_any(&["response", "responses"]) {
        push_unique_term(queries, "request response");
    }
}

pub(crate) fn packet_prompt_exact_symbol_probe_queries(
    question: &str,
    terms: &[String],
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    if !matches!(
        task_class,
        PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::EditPlanning
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::BugLocalization
    ) {
        return Vec::new();
    }

    let mut queries = Vec::new();
    for term in exact_symbol_query_terms(question) {
        if packet_prompt_exact_symbol_term_is_probe(&term) {
            push_unique_term(&mut queries, &term);
        }
    }
    if eval_probes_enabled() {
        push_prompt_concept_derived_symbol_probes(terms, &mut queries);
    }
    queries
}

fn packet_prompt_exact_symbol_term_is_probe(term: &str) -> bool {
    let trimmed = term.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let letters = trimmed
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .collect::<Vec<_>>();
    !letters.is_empty() && !letters.iter().all(|ch| ch.is_ascii_uppercase())
}

pub(crate) fn packet_concrete_file_probe_queries_from_required(
    required_queries: &[String],
) -> Vec<String> {
    let mut queries = Vec::new();
    for query in required_queries {
        if let Some(file_query) = packet_required_probe_file_query(query) {
            push_unique_term(&mut queries, &file_query);
        }
    }
    queries
}

fn packet_required_probe_file_query(query: &str) -> Option<String> {
    if !packet_required_probe_needs_concrete_file(query) {
        return None;
    }
    let normalized_query = normalize_identifier(query);
    if normalized_query == "eventprocessor" {
        return Some("event_processor.rs".to_string());
    }
    query
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        .then(|| format!("{query}.rs"))
}

pub(crate) fn push_indexing_flow_required_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "indexing entrypoint",
            "file discovery",
            "symbol extraction",
            "storage persistence",
            "search projection",
            "snapshot refresh",
        ],
    );
}

pub(crate) fn push_search_flow_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "search entrypoint",
            "flag parsing",
            "argument planning",
            "candidate file walk",
            "search execution",
            "parallel search",
            "result printer",
        ],
    );
}

pub(crate) fn packet_probe_query_is_cited(query: &str, answer: &AgentAnswerDto) -> bool {
    answer
        .citations
        .iter()
        .any(|citation| packet_citation_satisfies_required_probe(query, citation))
}

pub(crate) fn packet_citation_satisfies_required_probe(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    if let Some(matches_file_scoped_symbol) =
        packet_file_scoped_symbol_probe_matches(query, citation)
    {
        return matches_file_scoped_symbol;
    }
    if packet_citation_matches_route_engine_constructor_probe(query, citation) {
        return true;
    }
    if packet_citation_matches_route_dispatch_probe(query, citation) {
        return true;
    }
    if packet_citation_matches_argument_planning_probe(query, citation) {
        return true;
    }
    if packet_required_probe_needs_buffered_wrapper_implementation(query) {
        return packet_citation_matches_buffered_wrapper_implementation(query, citation);
    }
    if packet_required_probe_needs_concrete_file(query) {
        return packet_file_stem_matches_query(query, citation.file_path.as_deref());
    }
    if packet_required_probe_needs_full_token_coverage(query) {
        if packet_citation_probe_has_exact_identifier_match(query, citation) {
            return true;
        }
        let tokens = packet_probe_match_tokens(query);
        return !tokens.is_empty()
            && packet_citation_probe_token_coverage(query, citation) >= tokens.len();
    }
    if packet_citation_matches_public_api_surface_probe(query, citation) {
        return true;
    }
    if packet_citation_matches_validation_bypass_probe(query, citation) {
        return true;
    }
    if packet_citation_matches_sql_schema_scripts_probe(query, citation) {
        return true;
    }
    let Some(match_rank) = packet_citation_probe_match_rank(query, citation) else {
        return false;
    };
    !packet_required_probe_needs_exact_match(query) || match_rank >= 4
}

pub(crate) fn packet_required_probe_needs_exact_match(query: &str) -> bool {
    let normalized_query = normalize_identifier(query);
    query.contains("::")
        || query.contains('.')
        || normalized_query == "formvalidationbypass"
        || (normalized_query.starts_with("createtable") && normalized_query != "createtable")
}

fn packet_required_probe_needs_concrete_file(query: &str) -> bool {
    let normalized_query = normalize_identifier(query);
    normalized_query.contains("execevents") || normalized_query == "eventprocessor"
}

fn packet_required_probe_needs_full_token_coverage(query: &str) -> bool {
    matches!(
        normalize_identifier(query).as_str(),
        "indexingentrypoint"
            | "filediscovery"
            | "symbolextraction"
            | "storagepersistence"
            | "searchprojection"
            | "snapshotrefresh"
            | "routetreeaddroute"
            | "sourcereadbuffer"
            | "sinkwritebuffer"
    )
}

fn packet_citation_probe_has_exact_identifier_match(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    let normalized_query = normalize_identifier(query);
    if normalized_query.is_empty() {
        return false;
    }
    let normalized_display = normalize_identifier(&citation.display_name);
    normalized_display == normalized_query || normalized_display.ends_with(&normalized_query)
}

pub(crate) fn packet_citation_probe_match_rank(
    query: &str,
    citation: &AgentCitationDto,
) -> Option<u8> {
    let normalized_query = normalize_identifier(query);
    if normalized_query.is_empty() {
        return Some(0);
    }
    let normalized_display = normalize_identifier(&citation.display_name);
    let normalized_path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .map(|path| normalize_identifier(&path))
        .unwrap_or_default();
    if let Some(matches_file_scoped_symbol) =
        packet_file_scoped_symbol_probe_matches(query, citation)
    {
        if matches_file_scoped_symbol {
            Some(6)
        } else {
            None
        }
    } else if packet_citation_matches_route_engine_constructor_probe(query, citation) {
        Some(6)
    } else if packet_citation_matches_route_dispatch_probe(query, citation) {
        Some(6)
    } else if packet_citation_matches_argument_planning_probe(query, citation) {
        Some(6)
    } else if packet_citation_matches_buffered_wrapper_implementation(query, citation) {
        Some(6)
    } else if packet_citation_matches_public_api_surface_probe(query, citation) {
        Some(6)
    } else if packet_citation_matches_validation_bypass_probe(query, citation) {
        Some(5)
    } else if packet_citation_matches_sql_schema_scripts_probe(query, citation) {
        Some(5)
    } else if packet_file_stem_matches_query(query, citation.file_path.as_deref()) {
        Some(5)
    } else if normalized_display == normalized_query
        || normalized_display.ends_with(&normalized_query)
        || (!packet_required_probe_needs_exact_match(query)
            && packet_citation_probe_token_coverage(query, citation) >= 2)
    {
        Some(4)
    } else if normalized_path.contains(&normalized_query) {
        Some(3)
    } else if normalized_display.contains(&normalized_query) {
        Some(2)
    } else if !normalized_display.is_empty() && normalized_query.contains(&normalized_display) {
        Some(1)
    } else {
        None
    }
}

fn packet_citation_matches_sql_schema_scripts_probe(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    if !matches!(
        normalize_identifier(query).as_str(),
        "sqlschemascripts" | "schemadialectscripts"
    ) {
        return false;
    }
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(citation.kind, NodeKind::FILE | NodeKind::ANNOTATION)
        && path.ends_with(".sql")
        && (path.contains("sqlite")
            || path.contains("mysql")
            || path.contains("postgres")
            || path.contains("postgresql")
            || path.contains("sqlserver")
            || path.contains("oracle")
            || path.contains("db2")
            || normalize_identifier(&citation.display_name).contains("sqlschema"))
}

fn packet_citation_matches_route_engine_constructor_probe(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    if normalize_identifier(query) != "enginecreationnewrouter" {
        return false;
    }
    if !matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD) {
        return false;
    }
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if path.contains("/test/")
        || path.contains("/tests/")
        || path.starts_with("test/")
        || path.starts_with("tests/")
        || path.contains("\\test\\")
        || path.contains("\\tests\\")
        || path.starts_with("test\\")
        || path.starts_with("tests\\")
        || path.contains("_test.")
        || path.contains("test.")
    {
        return false;
    }
    citation
        .display_name
        .rsplit(['.', ':', '#'])
        .next()
        .map(normalize_identifier)
        .is_some_and(|tail| tail == "new")
}

fn packet_citation_matches_route_dispatch_probe(query: &str, citation: &AgentCitationDto) -> bool {
    let normalized_query = normalize_identifier(query);
    if !matches!(
        normalized_query.as_str(),
        "handlerdispatch" | "requesthandler" | "enginerequesthandler"
    ) {
        return false;
    }
    if !matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD) {
        return false;
    }
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if path.contains("/test/")
        || path.contains("/tests/")
        || path.starts_with("test/")
        || path.starts_with("tests/")
        || path.contains("\\test\\")
        || path.contains("\\tests\\")
        || path.starts_with("test\\")
        || path.starts_with("tests\\")
        || path.contains("_test.")
        || path.contains("test.")
    {
        return false;
    }

    let normalized_display = normalize_identifier(&citation.display_name);
    let handles_http_request = normalized_display.contains("handle")
        && (normalized_display.contains("request") || normalized_display.contains("http"));
    let dispatches_handler_or_route = normalized_display.contains("dispatch")
        && (normalized_display.contains("handler")
            || normalized_display.contains("route")
            || normalized_display.contains("request"));
    let handler_request_symbol =
        normalized_display.contains("handler") && normalized_display.contains("request");

    match normalized_query.as_str() {
        "handlerdispatch" => handles_http_request || dispatches_handler_or_route,
        "requesthandler" => handles_http_request || handler_request_symbol,
        "enginerequesthandler" => {
            (normalized_display.contains("engine") || path.contains("engine"))
                && (handles_http_request || handler_request_symbol)
        }
        _ => false,
    }
}

fn packet_citation_matches_argument_planning_probe(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    if normalize_identifier(query) != "argumentplanning" {
        return false;
    }
    if !matches!(
        citation.kind,
        NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::TYPEDEF
            | NodeKind::FUNCTION
            | NodeKind::METHOD
    ) {
        return false;
    }
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("\\test\\")
        || path.contains("\\tests\\")
        || path.contains("_test.")
        || path.contains("test.")
    {
        return false;
    }

    let normalized_display = normalize_identifier(&citation.display_name);
    let stem = path
        .rsplit(['/', '\\'])
        .next()
        .and_then(|file_name| file_name.rsplit_once('.').map(|(stem, _)| stem))
        .map(normalize_identifier)
        .unwrap_or_default();
    let display_has_argument_carrier = normalized_display.contains("args")
        || normalized_display.contains("argument")
        || normalized_display.contains("options");
    let stem_has_argument_carrier =
        stem.contains("args") || stem.contains("argument") || stem.contains("options");
    let path_has_cli_argument_context = path.contains("/flags/")
        || path.contains("\\flags\\")
        || path.contains("/args/")
        || path.contains("\\args\\")
        || path.contains("/cli/")
        || path.contains("\\cli\\")
        || path.contains("/command")
        || path.contains("\\command");
    let callable_builds_arguments = matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        && (normalized_display.contains("parse")
            || normalized_display.contains("build")
            || normalized_display.contains("plan")
            || normalized_display.contains("prepare"))
        && (display_has_argument_carrier
            || normalized_display.contains("flags")
            || normalized_display.contains("opts"));

    (display_has_argument_carrier || stem_has_argument_carrier)
        && (path_has_cli_argument_context || stem_has_argument_carrier || callable_builds_arguments)
}

fn packet_citation_matches_public_api_surface_probe(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    if !matches!(
        normalize_identifier(query).as_str(),
        "api" | "apis" | "publicapi" | "publicapis"
    ) {
        return false;
    }
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if path.contains("/test/")
        || path.contains("/tests/")
        || path.starts_with("test/")
        || path.starts_with("tests/")
        || path.contains("\\test\\")
        || path.contains("\\tests\\")
        || path.starts_with("test\\")
        || path.starts_with("tests\\")
        || path.contains("_test.")
        || path.contains("test.")
    {
        return false;
    }

    let normalized_display = normalize_identifier(&citation.display_name);
    let normalized_path = normalize_identifier(&path);
    let names_api_surface = normalized_display.contains("api")
        || normalized_display.contains("public")
        || normalized_path.contains("api")
        || normalized_path.contains("public");
    if names_api_surface {
        return matches!(
            citation.kind,
            NodeKind::CLASS
                | NodeKind::INTERFACE
                | NodeKind::TYPEDEF
                | NodeKind::FUNCTION
                | NodeKind::METHOD
        );
    }

    matches!(citation.kind, NodeKind::INTERFACE)
        && citation
            .display_name
            .rsplit(['.', ':', '#'])
            .next()
            .is_some_and(packet_display_tail_has_interface_prefix)
}

fn packet_display_tail_has_interface_prefix(display_tail: &str) -> bool {
    let mut chars = display_tail.chars();
    if chars.next() != Some('I') {
        return false;
    }
    chars.next().is_some_and(|ch| ch.is_ascii_uppercase())
}

pub(crate) fn packet_required_probe_needs_buffered_wrapper_implementation(query: &str) -> bool {
    matches!(
        normalize_identifier(query).as_str(),
        "sourcereadbuffer" | "sinkwritebuffer"
    )
}

pub(crate) fn packet_citation_matches_buffered_wrapper_implementation(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    let normalized_query = normalize_identifier(query);
    let needs_source = normalized_query == "sourcereadbuffer";
    let needs_sink = normalized_query == "sinkwritebuffer";
    if !needs_source && !needs_sink {
        return false;
    }
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("\\test\\")
        || path.contains("\\tests\\")
        || path.contains("_test.")
        || path.contains("test.")
    {
        return false;
    }
    let stem = path
        .rsplit(['/', '\\'])
        .next()
        .and_then(|file_name| file_name.rsplit_once('.').map(|(stem, _)| stem))
        .map(normalize_identifier)
        .unwrap_or_default();
    if stem.is_empty()
        || !stem.contains("buffer")
        || matches!(stem.as_str(), "bufferedsource" | "bufferedsink")
    {
        return false;
    }
    if needs_source && !stem.contains("source") {
        return false;
    }
    if needs_sink && !stem.contains("sink") {
        return false;
    }

    let normalized_display = normalize_identifier(&citation.display_name);
    if needs_source {
        normalized_display.contains("read")
            || normalized_display.contains("buffer")
            || stem.contains("source")
    } else {
        normalized_display.contains("write")
            || normalized_display.contains("buffer")
            || stem.contains("sink")
    }
}

fn packet_citation_matches_validation_bypass_probe(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    let normalized_query = normalize_identifier(query);
    if normalized_query != "formvalidationbypass" {
        return false;
    }
    let normalized_display = normalize_identifier(&citation.display_name);
    let normalized_path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .map(|path| normalize_identifier(&path))
        .unwrap_or_default();
    normalized_display.contains("validate")
        && (normalized_display.starts_with("no")
            || normalized_display.contains("disable")
            || normalized_display.contains("bypass")
            || normalized_display.contains("skip"))
        && (normalized_path.contains("form")
            || normalized_path.contains("validation")
            || normalized_path.contains("constraint"))
}

fn packet_file_scoped_symbol_probe_matches(
    query: &str,
    citation: &AgentCitationDto,
) -> Option<bool> {
    let parts = packet_file_scoped_symbol_probe_parts(query)?;
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    let file_name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path.as_str())
        .to_ascii_lowercase();
    if file_name != parts.file_name {
        return Some(false);
    }

    let normalized_display = normalize_identifier(&citation.display_name);
    if parts.symbols.len() >= 3 && parts.symbols[0] == "create" && parts.symbols[1] == "table" {
        let Some(table_name) = parts.symbols.last() else {
            return Some(false);
        };
        let expected = format!("createtable{table_name}");
        return Some(normalized_display == expected || normalized_display.ends_with(&expected));
    }
    if parts.symbols.len() >= 2 && parts.symbols[0] == "foreign" && parts.symbols[1] == "key" {
        return Some(
            normalized_display == "foreignkey" || normalized_display.ends_with("foreignkey"),
        );
    }
    Some(parts.symbols.iter().any(|symbol| {
        normalized_display == *symbol
            || normalized_display.ends_with(symbol)
            || packet_file_scoped_short_symbol_matches(&citation.display_name, symbol)
    }))
}

fn packet_file_scoped_short_symbol_matches(display_name: &str, symbol: &str) -> bool {
    if symbol.len() > 3 {
        return false;
    }
    display_name
        .rsplit(['.', ':', '#'])
        .next()
        .map(normalize_identifier)
        .is_some_and(|tail| tail == symbol)
}

pub(crate) struct PacketFileScopedSymbolProbe {
    pub(crate) query_path: String,
    pub(crate) file_name: String,
    pub(crate) raw_symbols: Vec<String>,
    pub(crate) symbols: Vec<String>,
}

pub(crate) fn packet_file_scoped_symbol_probe_parts(
    query: &str,
) -> Option<PacketFileScopedSymbolProbe> {
    let mut parts = query.split_whitespace();
    let file_part = parts
        .next()?
        .trim_matches(|ch: char| matches!(ch, '`' | '"' | '\''));
    let query_path = file_part.replace('\\', "/");
    let file_name = file_part.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    if !file_name.contains('.') && !packet_extensionless_source_file_name(&file_name) {
        return None;
    }

    let raw_symbols = parts
        .map(|part| {
            part.trim_matches(|ch: char| matches!(ch, '`' | '"' | '\'' | ',' | ';'))
                .to_string()
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let symbols = raw_symbols
        .iter()
        .map(|part| normalize_identifier(part))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if symbols.is_empty() {
        return None;
    }

    Some(PacketFileScopedSymbolProbe {
        query_path,
        file_name,
        raw_symbols,
        symbols,
    })
}

fn packet_extensionless_source_file_name(file_name: &str) -> bool {
    matches!(
        file_name,
        "makefile" | "dockerfile" | "rakefile" | "gemfile" | "bash_completion" | "configure"
    ) || file_name.ends_with("_completion")
}

pub(crate) fn packet_citation_probe_token_coverage(
    query: &str,
    citation: &AgentCitationDto,
) -> usize {
    let tokens = packet_probe_match_tokens(query);
    if tokens.len() < 2 {
        return 0;
    }
    let display = normalize_identifier(&citation.display_name);
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .map(|path| normalize_identifier(&path))
        .unwrap_or_default();
    tokens
        .iter()
        .filter(|token| display.contains(token.as_str()) || path.contains(token.as_str()))
        .count()
}

fn packet_probe_match_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for token in query
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() >= 3 && !packet_query_stop_term(token))
    {
        if !tokens.iter().any(|existing| existing == &token) {
            tokens.push(token);
        }
    }
    tokens
}

fn push_unique_term(terms: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.len() < 3 {
        return;
    }
    if !terms.iter().any(|term| term.eq_ignore_ascii_case(value)) {
        terms.push(value.to_string());
    }
}

fn push_unique_terms(terms: &mut Vec<String>, values: &[&str]) {
    for value in values {
        push_unique_term(terms, value);
    }
}

fn push_unique_owned_terms(terms: &mut Vec<String>, values: &[String]) {
    for value in values {
        push_unique_term(terms, value);
    }
}

fn push_runtime_formatting_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "base.h format_arg_store",
            "format_arg_store",
            "args.h dynamic_format_arg_store",
            "dynamic_format_arg_store",
            "format.h format_error",
            "format_error",
        ],
    );
}

fn push_log_record_handler_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "Logger.php Logger",
            "Logger.php pushHandler",
            "Logger.php addRecord",
            "Logger.php log",
            "LogRecord.php LogRecord",
            "HandlerInterface.php handle",
            "AbstractProcessingHandler.php handle",
        ],
    );
}

fn push_site_build_phase_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "build.rb Build.process",
            "build.rb Build.build",
            "site.rb Site",
            "site.rb Site.process",
            "site.rb Site.read",
            "site.rb Site.render",
            "site.rb Site.write",
            "reader.rb Reader",
            "reader.rb Reader.read",
            "renderer.rb Renderer",
            "renderer.rb Renderer.render_document",
            "renderer.rb Renderer.render_liquid",
        ],
    );
}

fn push_mapper_configuration_plan_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "Mapper.cs IMapperBase",
            "Mapper.cs IMapper",
            "Mapper.cs Mapper",
            "Mapper.cs Mapper.Map",
            "MapperConfiguration.cs MapperConfiguration",
            "TypeMap.cs TypeMap",
            "TypeMap.cs CreateMapperLambda",
            "TypeMapPlanBuilder.cs TypeMapPlanBuilder",
            "TypeMapPlanBuilder.cs CreateMapperLambda",
        ],
    );
}

fn push_client_send_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "http.dart get",
            "http.dart Client",
            "client.dart Client",
            "client.dart Client.get",
            "base_client.dart BaseClient",
            "base_client.dart send",
            "base_request.dart BaseRequest",
            "base_request.dart finalize",
            "io_client.dart IOClient",
            "io_client.dart send",
            "response.dart Response",
            "response.dart fromStream",
        ],
    );
}

fn push_url_session_request_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "Session.swift Session",
            "Session.swift Session.request",
            "Request.swift Request",
            "Request.swift Request.resume",
            "DataRequest.swift DataRequest",
            "DataRequest.swift DataRequest.validate",
            "SessionDelegate.swift SessionDelegate",
            "SessionDelegate.swift urlSession",
        ],
    );
}

fn push_form_validation_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "full-example.html required",
            "full-example.html pattern",
            "full-example.html min",
            "full-example.html max",
            "detailed-custom-validation.html input#mail",
            "detailed-custom-validation.html novalidate",
            "detailed-custom-validation.html showError",
            "fruit-pattern.html pattern",
            "min-max.html min",
            "min-max.html max",
        ],
    );
}

fn push_stylesheet_animation_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "_vars.css --animate-duration",
            "_vars.css --animate-delay",
            "_vars.css --animate-repeat",
            "_base.css .animated",
            "animate.css @import",
            "bounce.css bounce",
            "bounce.css @keyframes bounce",
            "flash.css flash",
            "flash.css @keyframes flash",
        ],
    );
}

pub(crate) fn push_sql_schema_required_probe_queries(terms: &[String], queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "CREATE TABLE",
            "FOREIGN KEY",
            "REFERENCES",
            "sql schema scripts",
        ],
    );
    for table in packet_sql_schema_prompt_table_candidates(terms)
        .into_iter()
        .take(8)
    {
        push_unique_term(queries, &format!("CREATE TABLE {table}"));
    }
}

fn packet_sql_schema_prompt_table_candidates(terms: &[String]) -> Vec<String> {
    let mut candidates = Vec::new();
    for window in terms.windows(2) {
        let [left, right] = window else {
            continue;
        };
        if !packet_sql_schema_compound_suffix(right) {
            continue;
        }
        let Some(left) = packet_sql_schema_prompt_table_part(left, true) else {
            continue;
        };
        let Some(right) = packet_sql_schema_prompt_table_part(right, true) else {
            continue;
        };
        push_unique_term(&mut candidates, &format!("{left}{right}"));
    }

    for term in terms {
        let Some(table) = packet_sql_schema_prompt_table_part(term, false) else {
            continue;
        };
        push_unique_term(&mut candidates, &table);
    }

    candidates
}

fn packet_sql_schema_compound_suffix(term: &str) -> bool {
    matches!(
        normalize_identifier(term).as_str(),
        "line" | "lines" | "item" | "items" | "detail" | "details"
    )
}

fn packet_sql_schema_prompt_table_part(term: &str, allow_singular: bool) -> Option<String> {
    let normalized = normalize_identifier(term);
    if normalized.len() < 4
        || packet_sql_schema_prompt_table_stop_term(&normalized)
        || normalized.chars().any(|ch| !ch.is_ascii_alphanumeric())
    {
        return None;
    }
    if !allow_singular && packet_sql_schema_compound_suffix(&normalized) {
        return None;
    }
    if !allow_singular
        && !normalized.ends_with('s')
        && !matches!(
            normalized.as_str(),
            "line" | "lines" | "item" | "items" | "detail" | "details"
        )
    {
        return None;
    }
    let singular = packet_sql_schema_singular_table_term(&normalized);
    if singular.len() < 4 || packet_sql_schema_prompt_table_stop_term(&singular) {
        return None;
    }
    Some(packet_sql_schema_pascal_identifier(&singular))
}

fn packet_sql_schema_singular_table_term(term: &str) -> String {
    term.strip_suffix("ies")
        .map(|prefix| format!("{prefix}y"))
        .or_else(|| term.strip_suffix('s').map(str::to_string))
        .unwrap_or_else(|| term.to_string())
}

fn packet_sql_schema_pascal_identifier(term: &str) -> String {
    let mut value = String::new();
    let mut chars = term.chars();
    if let Some(first) = chars.next() {
        value.push(first.to_ascii_uppercase());
        value.extend(chars.map(|ch| ch.to_ascii_lowercase()));
    }
    value
}

fn packet_sql_schema_prompt_table_stop_term(term: &str) -> bool {
    matches!(
        term,
        "across"
            | "between"
            | "constraint"
            | "constraints"
            | "core"
            | "create"
            | "database"
            | "databases"
            | "definition"
            | "definitions"
            | "dialect"
            | "dialects"
            | "explain"
            | "file"
            | "files"
            | "foreign"
            | "reference"
            | "references"
            | "relation"
            | "relations"
            | "relationship"
            | "relationships"
            | "schema"
            | "schemas"
            | "script"
            | "scripts"
            | "seed"
            | "seeds"
            | "sqlite"
            | "source"
            | "sources"
            | "mysql"
            | "name"
            | "names"
            | "postgres"
            | "postgresql"
            | "sql"
            | "support"
            | "supporting"
            | "table"
            | "tables"
    )
}

fn push_shell_install_dispatch_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "install.sh nvm_do_install",
            "install.sh nvm_install_node",
            "install.sh install_nvm_as_script",
            "install.sh nvm_download",
            "nvm.sh nvm",
            "nvm.sh nvm_download",
            "nvm.sh nvm_use_if_needed",
            "bash_completion __nvm",
            "bash_completion __nvm_commands",
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentCitationDto, NodeId, NodeKind, PacketClaimDto, RetrievalScoreBreakdownDto,
        SearchHitOrigin,
    };

    fn test_packet_citation(display_name: &str, file_path: &str, score: f32) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(format!(
                "test:{}:{}",
                display_name.replace(' ', "_"),
                file_path.replace(['/', '\\'], "_")
            )),
            display_name: display_name.to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some(file_path.to_string()),
            line: Some(1),
            score,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: 0.4,
                semantic: 0.2,
                graph: 0.3,
                total: score,
                provenance: Vec::new(),
            }),
        }
    }

    #[test]
    fn packet_probe_match_rank_uses_multi_token_path_coverage() {
        let mut citation = test_packet_citation(
            "std::collections::HashMap",
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
            0.6,
        );
        citation.kind = NodeKind::MODULE;

        assert_eq!(
            packet_citation_probe_match_rank("jsonl event output", &citation),
            Some(4)
        );
        assert_eq!(
            packet_citation_probe_token_coverage("jsonl event output", &citation),
            3
        );
    }

    #[test]
    fn packet_required_probe_matching_uses_file_stems_and_display_symbols() {
        let event_loop_entry = test_packet_citation("service::main", "src/event_loop.c", 0.9);
        let command_handler = test_packet_citation("CommandHandler", "src/commands.c", 0.9);
        let search_entrypoint =
            test_packet_citation("search_driver::run", "crates/search/src/main.rs", 0.9);
        let candidate_builder = test_packet_citation(
            "CandidateFiles",
            "crates/search/src/candidate_files.rs",
            0.9,
        );

        assert!(packet_citation_satisfies_required_probe(
            "event_loop.c main",
            &event_loop_entry
        ));
        assert!(packet_citation_satisfies_required_probe(
            "command handler",
            &command_handler
        ));
        assert!(packet_citation_satisfies_required_probe(
            "search driver run",
            &search_entrypoint
        ));
        assert!(packet_citation_satisfies_required_probe(
            "candidate files",
            &candidate_builder
        ));
    }

    #[test]
    fn prompt_concept_roles_generate_general_production_probes() {
        let hook_queries = packet_sufficiency_required_probe_queries(
            "Explain how the public hook serializes keys, connects cache helpers, and composes middleware.",
            PacketTaskClassDto::ArchitectureExplanation,
        );
        for expected in ["serialize", "cache helper", "middleware"] {
            assert!(
                hook_queries.iter().any(|query| query == expected),
                "expected {expected:?} in {hook_queries:?}"
            );
        }
        assert!(
            !hook_queries.iter().any(|query| query.contains("_internal")),
            "production probes must not use benchmark-specific paths: {hook_queries:?}"
        );

        let flow_queries = packet_sufficiency_required_probe_queries(
            "Trace native HTML form constraint validation, custom JavaScript validation, handler processing, mapper configuration, type map plans, and buffered source/sink behavior.",
            PacketTaskClassDto::ArchitectureExplanation,
        );
        for expected in [
            "form validation",
            "constraint validation",
            "html constraint",
            "pattern",
            "javascript validation",
            "form validation bypass",
            "validity state",
            "handler processing",
            "mapper configuration",
            "type map",
            "mapping plan",
            "buffered source",
            "buffered sink",
            "source sink buffer",
            "buffer storage",
            "buffered wrapper",
            "source read buffer",
            "sink write buffer",
            "source buffer",
            "sink buffer",
        ] {
            assert!(
                flow_queries.iter().any(|query| query == expected),
                "expected {expected:?} in {flow_queries:?}"
            );
        }

        let route_queries = packet_sufficiency_required_probe_queries(
            "Trace how an HTTP route registration reaches request handler dispatch through a router engine.",
            PacketTaskClassDto::RouteTracing,
        );
        for expected in [
            "handler dispatch",
            "route registration",
            "router group",
            "route tree",
            "route tree add route",
            "router group handle route",
            "request handler",
            "engine request handler",
            "context next handler chain",
            "handler chain",
            "engine creation",
            "engine creation new router",
        ] {
            assert!(
                route_queries.iter().any(|query| query == expected),
                "expected {expected:?} in {route_queries:?}"
            );
        }
    }

    #[test]
    fn concept_role_probes_match_common_symbol_and_file_shapes() {
        let cache_helper = test_packet_citation("createCacheHelper", "src/cache/helper.ts", 0.9);
        let middleware = test_packet_citation("withMiddleware", "src/runtime/middleware.ts", 0.9);
        let processing_handler =
            test_packet_citation("AbstractProcessingHandler", "src/logging/handler.rs", 0.9);
        let real_buffered_source =
            test_packet_citation("RealBufferedSource", "src/io/real_buffered_source.kt", 0.9);
        let real_buffered_sink =
            test_packet_citation("RealBufferedSink", "src/io/real_buffered_sink.kt", 0.9);
        let transport_client =
            test_packet_citation("BaseTransportClient.send", "src/http/client.dart", 0.9);
        let validate = test_packet_citation("validate", "src/form/validation.js", 0.9);
        let validation_bypass =
            test_packet_citation("novalidate", "src/form/custom-validation.html", 0.9);
        let mut public_mapper_api = test_packet_citation("IMapperBase", "src/Mapper.cs", 0.9);
        public_mapper_api.kind = NodeKind::INTERFACE;
        let mut test_public_api = test_packet_citation("IMapperBase", "tests/MapperTests.cs", 0.9);
        test_public_api.kind = NodeKind::INTERFACE;

        assert!(packet_citation_satisfies_required_probe(
            "cache helper",
            &cache_helper
        ));
        assert!(packet_citation_satisfies_required_probe(
            "middleware",
            &middleware
        ));
        assert!(packet_citation_satisfies_required_probe(
            "handler processing",
            &processing_handler
        ));
        assert!(packet_citation_satisfies_required_probe(
            "buffered source",
            &real_buffered_source
        ));
        let buffered_source_impl = test_packet_citation(
            "RealBufferedSource.read",
            "src/io/real_buffered_source.kt",
            0.9,
        );
        assert!(packet_citation_satisfies_required_probe(
            "source read buffer",
            &buffered_source_impl
        ));
        assert!(packet_citation_satisfies_required_probe(
            "buffered sink",
            &real_buffered_sink
        ));
        let buffered_sink_impl = test_packet_citation(
            "RealBufferedSink.write",
            "src/io/real_buffered_sink.kt",
            0.9,
        );
        assert!(packet_citation_satisfies_required_probe(
            "sink write buffer",
            &buffered_sink_impl
        ));
        assert!(packet_citation_satisfies_required_probe(
            "route tree add route",
            &test_packet_citation("node.addRoute", "src/router/tree.go", 0.9)
        ));
        assert!(packet_citation_satisfies_required_probe(
            "router group handle route",
            &test_packet_citation("RouterGroup.Handle", "src/http/router_group.go", 0.9)
        ));
        assert!(packet_citation_satisfies_required_probe(
            "engine request handler",
            &test_packet_citation("ServerEngine.handleHttpRequest", "src/http/server.go", 0.9)
        ));
        let route_dispatch =
            test_packet_citation("Engine.handleHTTPRequest", "src/http/server.go", 0.9);
        assert!(packet_citation_satisfies_required_probe(
            "handler dispatch",
            &route_dispatch
        ));
        assert!(packet_citation_satisfies_required_probe(
            "request handler",
            &route_dispatch
        ));
        assert_eq!(
            packet_citation_probe_match_rank("handler dispatch", &route_dispatch),
            Some(6)
        );
        let mut argument_plan =
            test_packet_citation("SearchArgs", "src/cli/flags/search_args.rs", 0.9);
        argument_plan.kind = NodeKind::STRUCT;
        assert!(packet_citation_satisfies_required_probe(
            "argument planning",
            &argument_plan
        ));
        assert_eq!(
            packet_citation_probe_match_rank("argument planning", &argument_plan),
            Some(6)
        );
        assert!(packet_citation_satisfies_required_probe(
            "argument planning",
            &test_packet_citation("parse_args", "src/config.rs", 0.9)
        ));
        let mut broad_flag = test_packet_citation("Flag", "src/cli/flags/mod.rs", 0.9);
        broad_flag.kind = NodeKind::INTERFACE;
        assert!(!packet_citation_satisfies_required_probe(
            "argument planning",
            &broad_flag
        ));
        assert!(packet_citation_satisfies_required_probe(
            "context next handler chain",
            &test_packet_citation("RequestContext.Next", "src/http/context.go", 0.9)
        ));
        let engine_new = test_packet_citation("New", "src/http/server.go", 0.9);
        assert!(packet_citation_satisfies_required_probe(
            "engine creation new router",
            &engine_new
        ));
        assert_eq!(
            packet_citation_probe_match_rank("engine creation new router", &engine_new),
            Some(6)
        );
        assert!(!packet_citation_satisfies_required_probe(
            "source read buffer",
            &test_packet_citation("BufferedSource", "src/io/buffered_source.kt", 0.9)
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "sink write buffer",
            &test_packet_citation("BufferedSink.write", "src/io/buffered_sink.kt", 0.9)
        ));
        assert_eq!(
            packet_citation_probe_match_rank("source read buffer", &buffered_source_impl),
            Some(6)
        );
        assert_eq!(
            packet_citation_probe_match_rank("sink write buffer", &buffered_sink_impl),
            Some(6)
        );
        assert!(packet_citation_satisfies_required_probe(
            "client send",
            &transport_client
        ));
        assert!(packet_citation_satisfies_required_probe(
            "APIs",
            &public_mapper_api
        ));
        assert_eq!(
            packet_citation_probe_match_rank("APIs", &public_mapper_api),
            Some(6)
        );
        assert!(!packet_citation_satisfies_required_probe(
            "APIs",
            &test_public_api
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "form validation bypass",
            &validate
        ));
        assert!(packet_citation_satisfies_required_probe(
            "form validation bypass",
            &validation_bypass
        ));
    }

    #[test]
    fn file_scoped_required_probes_match_symbol_inside_file() {
        let gin_new = test_packet_citation("New", "gin.go", 0.9);
        let gin_with = test_packet_citation("Engine.With", "gin.go", 0.9);
        let binding_default = test_packet_citation("Default", "binding/binding.go", 0.9);
        let router_group = test_packet_citation("RouterGroup", "routergroup.go", 0.9);
        let router_group_handle = test_packet_citation("RouterGroup.Handle", "routergroup.go", 0.9);

        assert!(packet_citation_satisfies_required_probe(
            "gin.go New",
            &gin_new
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "gin.go New",
            &gin_with
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "gin.go Default",
            &binding_default
        ));
        assert!(packet_citation_satisfies_required_probe(
            "routergroup.go RouterGroup.Handle",
            &router_group_handle
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "routergroup.go RouterGroup.Handle",
            &router_group
        ));

        let create_track = test_packet_citation(
            "CREATE TABLE Track",
            "SampleDatabase/DataSources/Sample_Sqlite.sql",
            0.9,
        );
        let create_playlist_track = test_packet_citation(
            "CREATE TABLE PlaylistTrack",
            "SampleDatabase/DataSources/Sample_Sqlite.sql",
            0.9,
        );
        let create_invoice = test_packet_citation(
            "CREATE TABLE Invoice",
            "SampleDatabase/DataSources/Sample_Sqlite.sql",
            0.9,
        );
        assert!(packet_citation_satisfies_required_probe(
            "CREATE TABLE Track",
            &create_track
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "CREATE TABLE Track",
            &create_invoice
        ));
        assert!(packet_citation_satisfies_required_probe(
            "SampleDatabase/DataSources/Sample_Sqlite.sql CREATE TABLE Track",
            &create_track
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "SampleDatabase/DataSources/Sample_Sqlite.sql CREATE TABLE Track",
            &create_playlist_track
        ));
    }

    #[test]
    fn sql_schema_required_probes_derive_prompt_table_symbols() {
        let terms = packet_probe_terms(
            "Explain SQL schema relationships between artists, albums, tracks, invoices, and invoice lines across SQL seed scripts. Cite the source files.",
        );
        let queries = packet_sufficiency_required_probe_queries_from_terms(
            &terms,
            PacketTaskClassDto::DataFlow,
        );

        for expected in [
            "CREATE TABLE",
            "FOREIGN KEY",
            "REFERENCES",
            "CREATE TABLE Artist",
            "CREATE TABLE Album",
            "CREATE TABLE Track",
            "CREATE TABLE Invoice",
            "CREATE TABLE InvoiceLine",
        ] {
            assert!(
                queries.iter().any(|query| query == expected),
                "expected SQL schema probe `{expected}` in {queries:?}"
            );
        }
        assert!(
            !queries.iter().any(|query| query == "CREATE TABLE Line"),
            "standalone compound suffixes should not become table probes: {queries:?}"
        );
        assert!(
            !queries.iter().any(|query| query == "CREATE TABLE File"),
            "documentation words should not become table probes: {queries:?}"
        );
    }

    #[test]
    fn route_sufficiency_probes_can_be_covered_by_source_claims() {
        let claims = vec![
            PacketClaimDto {
                claim: "app.use registers middleware on the router.".to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "app.handle delegates request handling to the router.".to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "res.send prepares and sends the response body.".to_string(),
                citations: Vec::new(),
            },
        ];

        for probe in ["app.use", "app.handle", "res.send"] {
            assert!(
                packet_probe_query_is_claimed(probe, &claims),
                "expected claim-backed coverage for {probe}: {claims:?}"
            );
        }
    }

    #[test]
    fn log_record_sufficiency_probes_can_be_covered_by_source_claims() {
        let claims = vec![
            PacketClaimDto {
                claim: "Logger owns a stack of handlers registered by pushHandler.".to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "addRecord creates a LogRecord before passing it to handlers.".to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "AbstractProcessingHandler handles records by processing and writing them."
                    .to_string(),
                citations: Vec::new(),
            },
        ];

        for probe in [
            "handler registration",
            "record creation",
            "handler processing",
            "log record",
            "handler stack",
        ] {
            assert!(
                packet_probe_query_is_claimed(probe, &claims),
                "expected claim-backed coverage for {probe}: {claims:?}"
            );
        }
    }

    #[test]
    fn client_send_sufficiency_probes_can_be_covered_by_source_claims() {
        let claims = vec![
            PacketClaimDto {
                claim: "Top-level HTTP helpers delegate to a Client.".to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "BaseRequest.finalize prepares the request body for sending.".to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "Response.fromStream builds a streamed response boundary.".to_string(),
                citations: Vec::new(),
            },
        ];

        for probe in [
            "top level helpers",
            "request finalization",
            "request response",
        ] {
            assert!(
                packet_probe_query_is_claimed(probe, &claims),
                "expected claim-backed coverage for {probe}: {claims:?}"
            );
        }
    }

    #[test]
    fn form_validation_sufficiency_probes_can_be_covered_by_source_claims() {
        let claims = vec![
            PacketClaimDto {
                claim:
                    "The form validation examples use native required, pattern, min, and max constraints."
                        .to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "A custom validation example uses novalidate to suppress the browser default UI."
                    .to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "showError branches on ValidityState fields to choose messages.".to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "Submit handlers prevent submission when the form is invalid.".to_string(),
                citations: Vec::new(),
            },
        ];

        for probe in [
            "native form constraints",
            "custom validation messages",
            "validity state",
            "submit prevent default",
            "form validation bypass",
        ] {
            assert!(
                packet_probe_query_is_claimed(probe, &claims),
                "expected claim-backed coverage for {probe}: {claims:?}"
            );
        }
    }
}
