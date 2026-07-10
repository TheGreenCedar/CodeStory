#[cfg(test)]
use crate::agent::eval_probes::{
    eval_probes_enabled, push_eval_architecture_flow_probe_terms,
    push_eval_flow_hint_packet_queries, push_prompt_named_file_probe_queries,
};
use crate::agent::packet_command_profiles::{
    packet_command_exact_probe_queries, packet_command_role_probe_queries,
};
use crate::agent::packet_flow_requirements::packet_flow_requirement_queries_for_terms;
use crate::agent::packet_required_probes::{
    packet_concrete_file_probe_queries_from_required, packet_prompt_exact_symbol_probe_queries,
    packet_sufficiency_required_probe_queries_from_terms,
    push_command_loop_source_probe_queries_for_terms, push_indexing_flow_required_probe_queries,
    push_search_flow_probe_queries, push_sql_schema_required_probe_queries,
};
use crate::agent::packet_scoring::{
    normalize_identifier, packet_adjacent_query_stop_term, packet_query_stop_term,
};
use crate::agent::packet_terms::{
    packet_probe_terms, packet_terms_have, packet_terms_have_any,
    packet_terms_indicate_buffered_io_flow, packet_terms_indicate_client_send_flow,
    packet_terms_indicate_command_dispatch_flow, packet_terms_indicate_command_event_loop_flow,
    packet_terms_indicate_event_loop_command_flow, packet_terms_indicate_form_validation_flow,
    packet_terms_indicate_html_css_template_structure_flow, packet_terms_indicate_indexing_flow,
    packet_terms_indicate_javascript_route_source_flow,
    packet_terms_indicate_log_record_handler_flow,
    packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_network_command_input_flow,
    packet_terms_indicate_prepared_session_adapter_flow,
    packet_terms_indicate_request_dispatch_flow, packet_terms_indicate_route_tree_dispatch_flow,
    packet_terms_indicate_runtime_formatting_flow, packet_terms_indicate_search_execution_flow,
    packet_terms_indicate_server_request_dispatch_flow,
    packet_terms_indicate_server_route_dispatch_flow,
    packet_terms_indicate_shell_install_dispatch_flow, packet_terms_indicate_site_build_phase_flow,
    packet_terms_indicate_sql_schema_flow, packet_terms_indicate_string_predicate_flow,
    packet_terms_indicate_stylesheet_animation_flow,
    packet_terms_indicate_url_session_request_flow, prompt_search_terms,
};
use crate::agent::planning::dedupe_packet_plan_queries;
use crate::{
    exact_symbol_query_terms, is_non_primary_source_term, looks_like_standalone_symbol_query,
    query_mentions_non_primary_source,
};
use codestory_contracts::api::{
    PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto, PacketTaskClassDto,
};
#[cfg(test)]
pub(crate) fn build_packet_plan(
    question: &str,
    requested: Option<PacketTaskClassDto>,
    budget: PacketBudgetModeDto,
) -> PacketPlanDto {
    build_packet_plan_with_extra(question, requested, budget, &[])
}

pub(crate) fn build_packet_plan_with_extra(
    question: &str,
    requested: Option<PacketTaskClassDto>,
    budget: PacketBudgetModeDto,
    extra_probes: &[String],
) -> PacketPlanDto {
    let task_class = requested.unwrap_or_else(|| infer_packet_task_class(question));
    let question_terms = packet_probe_terms(question);
    let shell_install_dispatch_flow =
        packet_terms_indicate_shell_install_dispatch_flow(&question_terms);
    let url_session_request_flow = packet_terms_indicate_url_session_request_flow(&question_terms);
    let sql_schema_flow = packet_terms_indicate_sql_schema_flow(&question_terms);
    let mut queries = Vec::new();
    push_packet_query(
        &mut queries,
        question,
        "original task phrasing for sidecar-primary source-backed retrieval",
    );
    for term in extract_packet_query_terms(question) {
        push_packet_query(
            &mut queries,
            &term,
            "concrete symbol, file, route, or code term",
        );
    }
    for query in extra_probes {
        push_packet_query(
            &mut queries,
            query,
            "explicit symbol probe from packet request",
        );
    }
    for query in packet_symbol_probe_queries(question, task_class, budget) {
        push_packet_query(
            &mut queries,
            &query,
            "symbol probe expanded from task wording",
        );
    }
    for query in task_class_seed_queries(
        task_class,
        shell_install_dispatch_flow,
        url_session_request_flow,
        sql_schema_flow,
    ) {
        push_packet_query(&mut queries, query, "task-class retrieval seed");
    }
    for query in packet_concept_queries(question) {
        push_packet_query(
            &mut queries,
            &query,
            "natural-language concept from task wording",
        );
    }
    let query_cap = packet_plan_query_cap(budget);
    queries.truncate(query_cap);

    let mut trace = vec![format!(
        "task_class={:?} source={}",
        task_class,
        if requested.is_some() {
            "request"
        } else {
            "heuristic"
        }
    )];
    trace.push(format!("planned_queries={}", queries.len()));
    if !extra_probes.is_empty() {
        trace.push(format!(
            "explicit_extra_probes={} source=request",
            extra_probes.len()
        ));
    }

    let mut plan = PacketPlanDto {
        task_class,
        inferred_task_class: requested.is_none(),
        queries,
        trace,
    };
    dedupe_packet_plan_queries(&mut plan);
    #[cfg(test)]
    let eval_probes = eval_probes_enabled();
    #[cfg(not(test))]
    let eval_probes = false;
    plan.trace.push(format!(
        "deduped_queries={} eval_probes={eval_probes}",
        plan.queries.len()
    ));
    plan
}

pub(crate) fn packet_rank_terms(question: &str) -> Vec<String> {
    let mut terms = prompt_search_terms(question);
    for term in extract_packet_query_terms(question) {
        push_unique_term(&mut terms, &term);
    }
    for query in packet_symbol_probe_queries(
        question,
        infer_packet_task_class(question),
        PacketBudgetModeDto::Standard,
    ) {
        push_unique_term(&mut terms, &normalize_identifier(&query));
    }
    terms
}

pub(crate) fn packet_request_extra_probes(extra_probes: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for probe in extra_probes {
        let probe = probe.trim();
        if probe.is_empty() || probe.len() > 240 {
            continue;
        }
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(probe))
        {
            normalized.push(probe.to_string());
        }
        if normalized.len() >= 16 {
            break;
        }
    }
    normalized
}

pub(crate) fn packet_explicit_request_probe_queries(plan: &PacketPlanDto) -> Vec<String> {
    plan.queries
        .iter()
        .filter(|query| query.purpose.contains("explicit symbol probe"))
        .map(|query| query.query.clone())
        .collect()
}

fn packet_plan_query_cap(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 20,
        PacketBudgetModeDto::Compact => 32,
        PacketBudgetModeDto::Standard => 48,
        PacketBudgetModeDto::Deep => 56,
    }
}

pub(crate) fn packet_symbol_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
    budget: PacketBudgetModeDto,
) -> Vec<String> {
    let terms = packet_probe_terms(question);
    let mut queries = Vec::new();
    let compact = matches!(
        budget,
        PacketBudgetModeDto::Compact | PacketBudgetModeDto::Tiny
    );

    push_unique_owned_terms(
        &mut queries,
        &packet_command_role_probe_queries(question, task_class),
    );
    push_unique_owned_terms(
        &mut queries,
        &packet_command_exact_probe_queries(question, task_class),
    );
    push_unique_owned_terms(
        &mut queries,
        &packet_prompt_exact_symbol_probe_queries(question, &terms, task_class),
    );
    #[cfg(test)]
    if eval_probes_enabled() {
        push_prompt_named_file_probe_queries(&terms, &mut queries);
    }
    push_unique_owned_terms(
        &mut queries,
        &packet_flow_requirement_queries_for_terms(&terms, task_class),
    );
    push_prompt_derived_exact_flow_anchor_queries(&terms, &mut queries);
    push_unique_owned_terms(
        &mut queries,
        &packet_sufficiency_required_probe_queries_from_terms(&terms, task_class),
    );
    let concrete_file_queries = packet_concrete_file_probe_queries_from_required(&queries);
    push_unique_owned_terms(&mut queries, &concrete_file_queries);
    push_predicate_symbol_probe_queries(&terms, &mut queries);
    push_flow_hint_packet_queries(&terms, &mut queries);
    push_task_class_symbol_probe_queries(task_class, &terms, &mut queries);
    if !compact {
        push_adjacent_packet_term_queries(&terms, &mut queries, 8);
    } else if matches!(task_class, PacketTaskClassDto::ArchitectureExplanation) {
        push_adjacent_packet_term_queries(&terms, &mut queries, 12);
    }
    push_generic_symbol_probe_queries(&terms, &mut queries, compact);

    queries.truncate(packet_plan_query_cap(budget));
    queries
}

fn push_flow_hint_packet_queries(terms: &[String], queries: &mut Vec<String>) {
    push_prompt_derived_flow_hint_packet_queries(terms, queries);
    #[cfg(test)]
    {
        push_eval_flow_hint_packet_queries(terms, queries);
    }
    #[cfg(test)]
    let use_index_derived = !eval_probes_enabled();
    #[cfg(not(test))]
    let use_index_derived = true;
    if use_index_derived {
        push_index_derived_architecture_probes(
            PacketTaskClassDto::ArchitectureExplanation,
            terms,
            queries,
        );
    }
}

fn push_index_derived_architecture_probes(
    _task_class: PacketTaskClassDto,
    terms: &[String],
    queries: &mut Vec<String>,
) {
    for term in terms.iter().filter(|term| term.len() >= 5).take(8) {
        if term.contains('/') || term.contains('.') {
            push_unique_term(queries, term);
        }
    }
}

fn push_prompt_derived_exact_flow_anchor_queries(terms: &[String], queries: &mut Vec<String>) {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);

    if has("exec") && has_any(&["runtime", "session"]) {
        push_unique_terms(queries, &["exec runtime", "exec session"]);
    }
    if has("exec") && has_any(&["cli", "command", "subcommand"]) {
        push_unique_terms(queries, &["exec cli", "exec command"]);
    }
    if has_any(&["json", "jsonl"]) && has_any(&["event", "events", "output"]) {
        push_unique_terms(queries, &["json event output", "event output processor"]);
    }
    if has("exec") && has_any(&["event", "events", "json", "jsonl"]) {
        push_unique_term(queries, "exec event output");
    }
    if has("thread") && has_any(&["start", "starts", "started"]) {
        push_unique_term(queries, "thread start");
    }
    if has("turn") && has_any(&["start", "starts", "started"]) {
        push_unique_term(queries, "turn start");
    }
    if packet_terms_indicate_indexing_flow(terms) {
        push_indexing_flow_required_probe_queries(queries);
    }
    if packet_terms_indicate_request_dispatch_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "request interceptor",
                "request dispatch",
                "transport adapter",
            ],
        );
    }
    if packet_terms_indicate_server_request_dispatch_flow(terms) {
        push_server_request_dispatch_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "server request dispatch",
                "request context",
                "view function dispatch",
                "response finalization",
            ],
        );
    }
    if packet_terms_indicate_client_send_flow(terms) {
        push_client_send_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "client convenience methods",
                "top level helpers",
                "public client facade",
                "client interface helper",
                "request finalization",
                "transport send",
                "request response",
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
    if packet_terms_indicate_form_validation_flow(terms) {
        push_form_validation_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "native form constraints",
                "custom validation flow",
                "custom error rendering",
                "validity state",
                "submit prevent default",
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
    if packet_terms_indicate_html_css_template_structure_flow(terms) {
        push_html_css_template_structure_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "html app shell",
                "module script entry",
                "css theme defaults",
                "css layout selectors",
                "interactive element styles",
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
    if packet_terms_indicate_javascript_route_source_flow(terms) {
        push_javascript_route_source_probe_queries(queries);
    }
    if packet_terms_indicate_server_route_dispatch_flow(terms) {
        push_unique_terms(
            queries,
            &["route registration", "request handler", "handler chain"],
        );
    }
    if packet_terms_indicate_route_tree_dispatch_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "router group",
                "route tree",
                "route tree add route",
                "router group handle route",
                "engine request handler",
                "context next handler chain",
                "engine creation",
                "engine creation router state",
            ],
        );
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
                "logger record",
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
    if has_any(&["adapter", "adapters", "transport"]) {
        push_unique_terms(queries, &["transport adapter", "adapter selection"]);
    }
    if packet_terms_indicate_event_loop_command_flow(terms) {
        push_command_loop_source_probe_queries_for_terms(terms, queries);
        if packet_terms_indicate_command_event_loop_flow(terms) {
            push_unique_terms(queries, &["event loop", "event dispatch"]);
        }
        if packet_terms_indicate_network_command_input_flow(terms) {
            push_unique_terms(queries, &["network input"]);
        }
        if packet_terms_indicate_command_dispatch_flow(terms) {
            push_unique_terms(queries, &["command dispatch"]);
        }
    } else if has("event") && has("loop") {
        push_unique_terms(queries, &["event loop", "event dispatch"]);
    }
    if has_any(&["client", "network", "reads", "socket"]) {
        push_unique_terms(queries, &["client input", "network input"]);
    }
    if has("call") && has_any(&["command", "commands", "dispatch", "dispatches"]) {
        push_unique_terms(queries, &["command dispatch", "command handler"]);
    }
    if packet_terms_indicate_search_execution_flow(terms) {
        push_search_flow_probe_queries(queries);
    }
}

fn push_prompt_derived_flow_hint_packet_queries(terms: &[String], queries: &mut Vec<String>) {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);

    if packet_terms_indicate_indexing_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "index service",
                "workspace execution plan",
                "workspace indexer",
                "symbol extraction indexer",
                "projection batch",
                "search projection",
                "snapshot refresh",
            ],
        );
    }
    if has("exec") && has_any(&["runtime", "session"]) {
        push_unique_terms(queries, &["exec runtime", "exec session", "run exec"]);
    }
    if has("exec") && has_any(&["cli", "command", "subcommand"]) {
        push_unique_terms(queries, &["exec cli", "exec command", "subcommand"]);
    }
    if has_any(&["cli", "command", "subcommand"]) && has_any(&["runtime", "exec"]) {
        push_unique_term(queries, "command runtime");
    }
    if has_any(&["json", "jsonl"]) && has_any(&["event", "events", "output"]) {
        push_unique_terms(
            queries,
            &[
                "json event output",
                "jsonl event output",
                "event output processor",
            ],
        );
    }
    if has("exec") && has_any(&["event", "events", "json", "jsonl"]) {
        push_unique_terms(queries, &["exec event output", "exec events"]);
    }
    if has("thread") && has_any(&["start", "starts", "started"]) {
        push_unique_terms(queries, &["thread start", "start thread"]);
    }
    if has("turn") && has_any(&["start", "starts", "started"]) {
        push_unique_terms(queries, &["turn start", "start turn"]);
    }
    if packet_terms_indicate_request_dispatch_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "request interceptor",
                "interceptor manager",
                "dispatch request",
            ],
        );
    }
    if packet_terms_indicate_server_request_dispatch_flow(terms) {
        push_server_request_dispatch_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "server request dispatch",
                "request context",
                "view function dispatch",
                "response finalization",
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
    if packet_terms_indicate_javascript_route_source_flow(terms) {
        push_javascript_route_source_probe_queries(queries);
    }
    if packet_terms_indicate_server_route_dispatch_flow(terms) {
        push_unique_terms(
            queries,
            &["route registration", "request handler", "handler chain"],
        );
    }
    if packet_terms_indicate_route_tree_dispatch_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "router group",
                "route tree",
                "route tree add route",
                "router group handle route",
                "engine request handler",
                "context next handler chain",
                "engine creation",
                "engine creation router state",
            ],
        );
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
                "buffer read write",
            ],
        );
    }
    if packet_terms_indicate_log_record_handler_flow(terms) {
        push_log_record_handler_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "log call",
                "logger record",
                "record creation",
                "handler stack",
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
                "site read",
                "site render",
                "site write",
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
    if packet_terms_indicate_form_validation_flow(terms) {
        push_form_validation_source_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "native form constraints",
                "custom validation flow",
                "custom error rendering",
                "validity state",
                "submit prevent default",
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
    if packet_terms_indicate_html_css_template_structure_flow(terms) {
        push_html_css_template_structure_probe_queries(queries);
        push_unique_terms(
            queries,
            &[
                "html app shell",
                "module script entry",
                "css theme defaults",
                "css layout selectors",
                "interactive element styles",
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
    if packet_terms_indicate_prepared_session_adapter_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "request preparation",
                "prepared request prepare method",
                "transport-ready request object",
                "session request",
                "session send",
                "adapter send",
                "adapter send method",
                "transport adapter send method",
                "adapter selection",
            ],
        );
    }
    if has_any(&["adapter", "adapters", "transport"]) {
        push_unique_terms(queries, &["transport adapter", "adapter selection"]);
    }
    if has("event") && has("loop") {
        push_unique_terms(queries, &["event loop", "main event loop"]);
    }
    if has_any(&["client", "network", "reads", "socket"]) {
        push_unique_terms(
            queries,
            &["client command input", "networking command read"],
        );
    }
    if has("command") && has_any(&["dispatch", "dispatches"]) {
        push_unique_term(queries, "command dispatch");
    }
    if packet_terms_indicate_search_execution_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "search entrypoint",
                "flag parsing",
                "search pipeline",
                "argument planning",
                "candidate file walk",
                "search execution",
                "parallel search",
                "result printer",
            ],
        );
    }
}

fn push_generic_symbol_probe_queries(terms: &[String], queries: &mut Vec<String>, _compact: bool) {
    let term_cap = 12;
    for term in terms
        .iter()
        .filter(|term| term.len() >= 4 && !packet_query_stop_term(term.as_str()))
        .take(term_cap)
    {
        push_unique_term(queries, term);
        push_unique_term(queries, &packet_camel_case(&[term.as_str()]));
    }
}

fn push_javascript_route_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "app initialization",
            "application factory",
            "callable app object",
            "middleware registration",
            "middleware use registration",
            "route registration",
            "request handler",
            "router handle dispatch",
            "response send",
            "response send helper",
            "request response prototype",
        ],
    );
}

fn push_server_request_dispatch_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "wsgi app",
            "request dispatch wrapper",
            "dispatch request view function",
            "request context",
            "route decorator",
            "route add url rule",
            "response finalization",
        ],
    );
}

fn push_client_send_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "http top level helper",
            "public client facade",
            "client convenience method",
            "client interface helper",
            "client send implementation",
            "request finalization",
            "request preparation",
            "prepared request prepare method",
            "transport-ready request object",
            "adapter send method",
            "transport adapter send method",
            "io transport client send",
            "response stream boundary",
        ],
    );
}

fn push_url_session_request_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "session request creation",
            "request object creation",
            "request resume dispatch",
            "request validation pipeline",
            "delegate callback handling",
            "url session callback boundary",
        ],
    );
}

fn push_form_validation_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "html form required constraint",
            "html form pattern constraint",
            "html form min max constraints",
            "custom form validation input",
            "custom validation validity state",
            "custom validation error rendering",
            "submit prevent default",
        ],
    );
}

fn push_stylesheet_animation_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "animation custom property duration",
            "animation custom property delay",
            "animation custom property repeat",
            "animation variables file",
            "animation base class",
            "animation stylesheet import",
            "named animation class",
            "named keyframes animation",
            "attention animation keyframes",
            "attention seeker animation",
        ],
    );
}

fn push_html_css_template_structure_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "html app root element",
            "html module script entry",
            "css root selector",
            "css body layout selector",
            "css app container selector",
            "css color scheme theme",
            "css button hover focus",
            "css light color scheme media query",
            "css logo hover transition",
        ],
    );
}

fn push_shell_install_dispatch_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "shell installer bootstrap",
            "install download helpers",
            "shell function dispatch",
            "conditional version use",
            "shell completion",
        ],
    );
}

fn push_runtime_formatting_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "format argument store",
            "format arg store",
            "dynamic format argument collection",
            "dynamic format arg store",
            "format error type",
            "format failure type",
            "format source buffer append",
            "buffer append",
            "system source vformat",
            "format runtime source",
            "output formatting function",
            "system output formatting",
            "system error formatting",
            "format error code",
        ],
    );
}

fn push_log_record_handler_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "logger handler stack",
            "handler registration",
            "logger record creation",
            "log method record handoff",
            "record handler interface",
            "processing handler write boundary",
        ],
    );
}

fn push_site_build_phase_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "build process entrypoint",
            "build lifecycle method",
            "site lifecycle process phases",
            "site read phase",
            "site render phase",
            "site write phase",
            "content reader read phase",
            "page renderer render phase",
        ],
    );
}

fn push_mapper_configuration_plan_source_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "mapper public api",
            "mapping runtime entrypoint",
            "mapping configuration source",
            "type map source",
            "mapping lambda plan",
            "mapping plan builder",
            "mapping execution plan",
        ],
    );
}

fn push_predicate_symbol_probe_queries(terms: &[String], queries: &mut Vec<String>) {
    if !packet_terms_indicate_predicate_probe_flow(terms) {
        return;
    }

    let scopes = packet_predicate_probe_scopes(terms, queries);
    let mut method_names = Vec::new();

    for term in terms.iter().take(16) {
        if packet_predicate_probe_single_term(term) {
            push_predicate_method_name(&mut method_names, &[term.as_str()]);
            push_predicate_identifier_variants(queries, &[term.as_str()]);
        }
    }

    for window in terms.windows(2).take(16) {
        if let [left, right] = window
            && packet_predicate_probe_term_pair(left, right)
        {
            push_predicate_method_name(&mut method_names, &[left.as_str(), right.as_str()]);
            push_predicate_identifier_variants(queries, &[left.as_str(), right.as_str()]);
        }
    }

    push_string_region_matching_probe_queries(terms, queries, &scopes);
    for scope in scopes.iter().take(4) {
        for method_name in method_names.iter().take(4) {
            push_unique_term(queries, &format!("{scope} {method_name}"));
            if packet_predicate_method_source_probe_allowed(method_name)
                && let Some(source_file) = packet_predicate_scope_source_file(scope)
            {
                push_unique_term(queries, &format!("{source_file} {method_name}"));
            }
        }
    }
}

fn packet_terms_indicate_predicate_probe_flow(terms: &[String]) -> bool {
    packet_terms_indicate_string_predicate_flow(terms)
        || (packet_terms_have_any(
            terms,
            &[
                "check",
                "checks",
                "checking",
                "predicate",
                "predicates",
                "validate",
                "validates",
                "validation",
            ],
        ) && terms
            .iter()
            .any(|term| packet_predicate_probe_single_term(term)))
}

fn packet_predicate_probe_single_term(term: &str) -> bool {
    matches!(
        normalize_identifier(term).as_str(),
        "blank"
            | "empty"
            | "whitespace"
            | "valid"
            | "invalid"
            | "enabled"
            | "disabled"
            | "active"
            | "available"
            | "ready"
            | "present"
    )
}

fn packet_predicate_probe_term_pair(left: &str, right: &str) -> bool {
    matches!(
        (
            normalize_identifier(left).as_str(),
            normalize_identifier(right).as_str()
        ),
        ("case", "sensitive")
            | ("case", "insensitive")
            | ("white", "space")
            | ("non", "empty")
            | ("not", "empty")
    )
}

fn packet_predicate_probe_scopes(terms: &[String], queries: &[String]) -> Vec<String> {
    let mut scopes: Vec<String> = Vec::new();
    for value in queries
        .iter()
        .map(String::as_str)
        .chain(terms.iter().map(String::as_str))
    {
        if packet_predicate_probe_scope_term(value) {
            let normalized_value = normalize_identifier(value);
            if !scopes
                .iter()
                .any(|scope| normalize_identifier(scope.as_str()) == normalized_value)
            {
                scopes.push(value.to_string());
            }
        }
    }
    scopes
}

fn packet_predicate_probe_scope_term(term: &str) -> bool {
    let trimmed = term.trim();
    let normalized = normalize_identifier(trimmed);
    if normalized == "strings"
        && trimmed
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
    {
        return true;
    }
    if trimmed.len() < 4
        || trimmed.chars().any(char::is_whitespace)
        || trimmed.contains('.')
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || packet_query_stop_term(trimmed)
        || packet_predicate_probe_single_term(trimmed)
    {
        return false;
    }
    if matches!(
        normalized.as_str(),
        "check"
            | "checks"
            | "commons"
            | "explain"
            | "implements"
            | "input"
            | "inputs"
            | "lang"
            | "name"
            | "string"
            | "strings"
            | "supporting"
            | "symbols"
            | "text"
    ) {
        return false;
    }
    trimmed.chars().any(|ch| ch.is_ascii_uppercase())
        || normalized.ends_with("utils")
        || normalized.ends_with("helper")
        || normalized.ends_with("helpers")
        || normalized.ends_with("checks")
        || normalized.contains("charsequence")
}

fn push_string_region_matching_probe_queries(
    terms: &[String],
    queries: &mut Vec<String>,
    scopes: &[String],
) {
    if !packet_terms_indicate_string_predicate_flow(terms)
        || !packet_terms_have_any(
            terms,
            &[
                "case",
                "sensitive",
                "insensitive",
                "ignore",
                "ignores",
                "comparison",
                "compare",
                "matching",
            ],
        )
    {
        return;
    }
    push_unique_term(queries, "regionMatches");
    if packet_terms_have(terms, "strings") {
        push_unique_term(queries, "Strings regionMatches");
    }
    for scope in scopes
        .iter()
        .filter(|scope| normalize_identifier(scope).contains("charsequence"))
        .take(2)
    {
        push_unique_term(queries, &format!("{scope} regionMatches"));
    }
    for scope in scopes.iter().take(4) {
        if let Some(source_file) = packet_predicate_scope_source_file(scope) {
            push_unique_term(queries, &format!("{source_file} regionMatches"));
        }
    }
}

fn packet_predicate_scope_source_file(scope: &str) -> Option<String> {
    let trimmed = scope.trim();
    if !packet_predicate_probe_scope_term(trimmed)
        || trimmed.contains('.')
        || trimmed.contains('/')
        || trimmed.contains('\\')
    {
        return None;
    }
    trimmed
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
        .then(|| format!("{trimmed}.java"))
}

fn packet_predicate_method_source_probe_allowed(method_name: &str) -> bool {
    matches!(
        normalize_identifier(method_name).as_str(),
        "isblank" | "isempty"
    )
}

fn push_predicate_identifier_variants(queries: &mut Vec<String>, terms: &[&str]) {
    push_predicate_method_name(queries, terms);
    let words = packet_identifier_words(terms);
    if words.is_empty() {
        return;
    }
    let snake = words.join("_");
    push_unique_term(queries, &format!("is_{snake}"));
}

fn push_predicate_method_name(queries: &mut Vec<String>, terms: &[&str]) {
    let words = packet_identifier_words(terms);
    if words.is_empty() {
        return;
    }
    let pascal = words
        .iter()
        .map(|word| packet_capitalize_identifier_word(word))
        .collect::<String>();
    push_unique_term(queries, &format!("is{pascal}"));
}

fn packet_identifier_words(terms: &[&str]) -> Vec<String> {
    terms
        .iter()
        .flat_map(|term| {
            term.split(|ch: char| !ch.is_ascii_alphanumeric())
                .filter(|part| !part.is_empty())
                .map(|part| part.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn packet_capitalize_identifier_word(word: &str) -> String {
    let mut value = String::new();
    let mut chars = word.chars();
    if let Some(first) = chars.next() {
        value.push(first.to_ascii_uppercase());
        value.extend(chars.map(|ch| ch.to_ascii_lowercase()));
    }
    value
}

fn push_task_class_symbol_probe_queries(
    task_class: PacketTaskClassDto,
    terms: &[String],
    queries: &mut Vec<String>,
) {
    if matches!(task_class, PacketTaskClassDto::RouteTracing)
        && (packet_terms_indicate_shell_install_dispatch_flow(terms)
            || packet_terms_indicate_url_session_request_flow(terms))
    {
        return;
    }
    let class_queries = match task_class {
        PacketTaskClassDto::RouteTracing => {
            &["router", "handler", "route", "middleware", "dispatch"][..]
        }
        PacketTaskClassDto::BugLocalization => &["error", "validate"],
        PacketTaskClassDto::ChangeImpact => &["affected", "references"],
        PacketTaskClassDto::SymbolOwnership => &["references", "callers"],
        PacketTaskClassDto::EditPlanning => &["tests", "config"],
        PacketTaskClassDto::ArchitectureExplanation | PacketTaskClassDto::DataFlow => &[],
    };
    push_unique_terms(queries, class_queries);
}

fn push_adjacent_packet_term_queries(
    terms: &[String],
    queries: &mut Vec<String>,
    window_cap: usize,
) {
    for window in terms.windows(2).take(window_cap) {
        if let [left, right] = window {
            if packet_adjacent_query_stop_term(left) || packet_adjacent_query_stop_term(right) {
                continue;
            }
            push_unique_term(queries, &format!("{left}_{right}"));
            push_unique_term(
                queries,
                &packet_camel_case(&[left.as_str(), right.as_str()]),
            );
        }
    }
}

pub(crate) fn packet_concept_queries(question: &str) -> Vec<String> {
    let include_non_primary_terms = query_mentions_non_primary_source(question);
    prompt_search_terms(question)
        .into_iter()
        .filter(|term| {
            term.len() >= 4
                && (include_non_primary_terms || !is_non_primary_source_term(term.as_str()))
                && !packet_query_stop_term(term.as_str())
                && !matches!(
                    term.as_str(),
                    "answer"
                        | "cite"
                        | "cites"
                        | "explain"
                        | "files"
                        | "full"
                        | "into"
                        | "moves"
                        | "support"
                        | "through"
                )
        })
        .take(8)
        .collect()
}

fn packet_camel_case(words: &[&str]) -> String {
    let mut value = String::new();
    for word in words {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            value.push(first.to_ascii_uppercase());
            value.extend(chars.map(|ch| ch.to_ascii_lowercase()));
        }
    }
    value
}

pub(crate) fn infer_packet_task_class(question: &str) -> PacketTaskClassDto {
    let lower = question.to_ascii_lowercase();
    if contains_any(
        &lower,
        &["bug", "error", "failing", "failed", "broken", "crash"],
    ) {
        PacketTaskClassDto::BugLocalization
    } else if contains_any(
        &lower,
        &["impact", "affected", "regression", "blast radius"],
    ) || risk_of_change_prompt(&lower)
    {
        PacketTaskClassDto::ChangeImpact
    } else if contains_any(&lower, &["route", "endpoint", "handler", "api path"]) {
        PacketTaskClassDto::RouteTracing
    } else if contains_any(&lower, &["owner", "owns", "who calls", "references"]) {
        PacketTaskClassDto::SymbolOwnership
    } else if contains_any(
        &lower,
        &[
            "data flow",
            "flow from",
            "flow into",
            "flows from",
            "flows into",
            "pipeline",
            "through",
        ],
    ) {
        PacketTaskClassDto::DataFlow
    } else if contains_any(
        &lower,
        &[
            "where to edit",
            "edit",
            "change",
            "modify",
            "implement",
            "add ",
        ],
    ) {
        PacketTaskClassDto::EditPlanning
    } else {
        PacketTaskClassDto::ArchitectureExplanation
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn risk_of_change_prompt(lower: &str) -> bool {
    lower.contains("risk if")
        && contains_any(lower, &[" change", " changing", " modify", " modifying"])
        || lower.contains("risk of changing")
        || lower.contains("risk from changing")
        || lower.contains("risk in changing")
}

pub(crate) fn extract_packet_query_terms(question: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut quoted = false;
    let mut quote = '\0';
    let mut start = 0usize;
    for (index, ch) in question.char_indices() {
        if matches!(ch, '`' | '"' | '\'') {
            if quoted && ch == quote {
                push_unique_term(&mut terms, question[start..index].trim());
                quoted = false;
            } else if !quoted {
                quoted = true;
                quote = ch;
                start = index + ch.len_utf8();
            }
        }
    }

    for term in exact_symbol_query_terms(question) {
        push_unique_term(&mut terms, &term);
    }
    for term in packet_architecture_flow_probe_terms(question) {
        push_unique_term(&mut terms, &term);
    }

    for token in question.split_whitespace() {
        let token = token.trim_matches(|ch: char| {
            matches!(
                ch,
                ',' | '.' | ';' | ':' | '?' | '!' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '`'
            )
        });
        if is_packet_code_like_term(token)
            || (looks_like_standalone_symbol_query(token)
                && token.len() >= 4
                && !packet_extract_query_stop_term(token))
        {
            push_unique_term(&mut terms, token);
        }
    }
    terms.truncate(16);
    terms
}

fn packet_extract_query_stop_term(token: &str) -> bool {
    packet_query_stop_term(token)
        || matches!(
            token.to_ascii_lowercase().as_str(),
            "cite"
                | "cites"
                | "file"
                | "files"
                | "path"
                | "paths"
                | "that"
                | "them"
                | "they"
                | "their"
                | "your"
                | "into"
                | "from"
                | "with"
                | "have"
                | "been"
                | "will"
                | "also"
                | "only"
                | "over"
                | "under"
                | "than"
                | "then"
                | "each"
                | "such"
                | "some"
                | "more"
                | "most"
                | "many"
                | "much"
                | "very"
                | "just"
                | "like"
                | "make"
                | "made"
                | "used"
                | "uses"
                | "using"
                | "work"
                | "works"
                | "working"
        )
}

fn is_packet_code_like_term(token: &str) -> bool {
    if token.len() < 3 {
        return false;
    }
    token.contains("::")
        || token.contains('/')
        || token.contains('\\')
        || token.contains('.')
        || token.contains('_')
        || token.contains('-')
        || token.chars().skip(1).any(|ch| ch.is_ascii_uppercase())
}

pub(crate) fn push_unique_term(terms: &mut Vec<String>, value: &str) {
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

fn task_class_seed_queries(
    task_class: PacketTaskClassDto,
    shell_install_dispatch_flow: bool,
    url_session_request_flow: bool,
    sql_schema_flow: bool,
) -> &'static [&'static str] {
    match task_class {
        PacketTaskClassDto::ArchitectureExplanation => &[
            "architecture entrypoint",
            "runtime flow",
            "main",
            "run",
            "entrypoint",
        ],
        PacketTaskClassDto::BugLocalization => &["error path", "failure handling"],
        PacketTaskClassDto::ChangeImpact => &["affected symbols", "impacted tests"],
        PacketTaskClassDto::RouteTracing if shell_install_dispatch_flow => &["references"],
        PacketTaskClassDto::RouteTracing if url_session_request_flow => {
            &["request lifecycle", "references"]
        }
        PacketTaskClassDto::RouteTracing => &["route handler endpoint", "references"],
        PacketTaskClassDto::SymbolOwnership => &["definition references", "callers"],
        PacketTaskClassDto::DataFlow if sql_schema_flow => &[
            "table definitions",
            "foreign key relationships",
            "schema dialect scripts",
        ],
        PacketTaskClassDto::DataFlow => &["pipeline flow", "storage handoff"],
        PacketTaskClassDto::EditPlanning => &["edit candidates", "test coverage"],
    }
}

fn push_packet_query(queries: &mut Vec<PacketPlanQueryDto>, query: &str, purpose: &str) {
    let query = query.trim();
    if query.is_empty() {
        return;
    }
    if queries
        .iter()
        .any(|existing| existing.query.eq_ignore_ascii_case(query))
    {
        return;
    }
    queries.push(PacketPlanQueryDto {
        query: query.to_string(),
        purpose: purpose.to_string(),
    });
}

pub(crate) fn packet_plan_annotation(plan: &PacketPlanDto) -> String {
    let queries = plan
        .queries
        .iter()
        .map(|query| query.query.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    format!(
        "packet_plan task_class={:?} inferred={} queries={}",
        plan.task_class, plan.inferred_task_class, queries
    )
}

fn packet_architecture_flow_probe_terms(prompt: &str) -> Vec<String> {
    let lower = prompt.to_ascii_lowercase();
    let mut terms = Vec::new();
    if prompt_mentions_indexing_flow(&lower) {
        for term in [
            "index service",
            "workspace execution plan",
            "workspace indexer",
            "symbol extraction indexer",
            "search projection",
            "snapshot refresh",
        ] {
            push_unique_term(&mut terms, term);
        }
    }
    #[cfg(test)]
    push_eval_architecture_flow_probe_terms(&lower, &mut terms);
    terms
}

fn prompt_mentions_indexing_flow(lower: &str) -> bool {
    contains_any(lower, &["indexing", "indexer", "indexed", " index "])
        && contains_any(
            lower,
            &[
                "cli",
                "command",
                "discovery",
                "extraction",
                "file",
                "persistence",
                "projection",
                "refresh",
                "runtime",
                "search",
                "snapshot",
                "storage",
                "store",
                "symbol",
                "workspace",
            ],
        )
}
