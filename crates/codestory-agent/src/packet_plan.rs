#[cfg(any(test, feature = "test-support"))]
use crate::eval_probes::{
    eval_probes_enabled, push_eval_architecture_flow_probe_terms,
    push_prompt_named_file_probe_queries,
};
use crate::packet_flow_requirements::packet_flow_requirement_queries_for_terms;
use crate::packet_obligations::build_packet_obligation_plan;
use crate::packet_required_probes::{
    packet_concrete_file_probe_queries_from_required, packet_prompt_exact_symbol_probe_queries,
};
use crate::packet_scoring::{
    normalize_identifier, packet_adjacent_query_stop_term, packet_query_stop_term,
};
use crate::packet_terms::{
    packet_probe_terms, packet_terms_indicate_shell_install_dispatch_flow,
    packet_terms_indicate_sql_schema_flow, packet_terms_indicate_url_session_request_flow,
    prompt_search_terms,
};
use crate::planning::{
    PACKET_EXACT_SYMBOL_QUERY_PURPOSE, dedupe_packet_plan_queries,
    packet_plan_query_is_exact_symbol_identity,
};
use crate::text::{
    exact_symbol_query_terms, is_non_primary_source_term, looks_like_standalone_symbol_query,
    query_mentions_non_primary_source,
};
use codestory_contracts::api::{
    PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto, PacketTaskClassDto,
};
#[cfg(any(test, feature = "test-support"))]
pub fn build_packet_plan(
    question: &str,
    requested: Option<PacketTaskClassDto>,
    budget: PacketBudgetModeDto,
) -> PacketPlanDto {
    build_packet_plan_with_extra(question, requested, budget, &[])
}

pub fn build_packet_plan_with_extra(
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
    if looks_like_standalone_symbol_query(question) {
        push_exact_symbol_packet_query(&mut queries, question);
    } else {
        push_packet_query(
            &mut queries,
            question,
            "original task phrasing for sidecar-primary source-backed retrieval",
        );
    }
    for term in exact_symbol_query_terms(question) {
        push_exact_symbol_packet_query(&mut queries, &term);
    }
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
        probe_resolutions: Vec::new(),
        obligations: Default::default(),
        trace,
    };
    dedupe_packet_plan_queries(&mut plan);
    plan.obligations = build_packet_obligation_plan(question, task_class, &plan.queries);
    #[cfg(any(test, feature = "test-support"))]
    let eval_probes = eval_probes_enabled();
    #[cfg(not(any(test, feature = "test-support")))]
    let eval_probes = false;
    plan.trace.push(format!(
        "deduped_queries={} eval_probes={eval_probes}",
        plan.queries.len()
    ));
    plan.trace.push(format!(
        "obligation_plan_version={} claim_obligations={} query_obligations={}",
        plan.obligations.version,
        plan.obligations.claim_obligations.len(),
        plan.obligations.query_obligations.len()
    ));
    plan
}

pub fn packet_rank_terms(question: &str) -> Vec<String> {
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

pub fn packet_explicit_request_probe_queries(plan: &PacketPlanDto) -> Vec<String> {
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

pub fn packet_symbol_probe_queries(
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
        &packet_prompt_exact_symbol_probe_queries(question, &terms, task_class),
    );
    #[cfg(any(test, feature = "test-support"))]
    if eval_probes_enabled() {
        push_prompt_named_file_probe_queries(&terms, &mut queries);
    }
    push_unique_owned_terms(
        &mut queries,
        &packet_flow_requirement_queries_for_terms(&terms, task_class),
    );
    let concrete_file_queries = packet_concrete_file_probe_queries_from_required(&queries);
    push_unique_owned_terms(&mut queries, &concrete_file_queries);
    if !compact {
        push_adjacent_packet_term_queries(&terms, &mut queries, 8);
    }
    push_generic_symbol_probe_queries(&terms, &mut queries, compact);

    queries.truncate(packet_plan_query_cap(budget));
    queries
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

pub fn packet_concept_queries(question: &str) -> Vec<String> {
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

pub fn infer_packet_task_class(question: &str) -> PacketTaskClassDto {
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

pub fn extract_packet_query_terms(question: &str) -> Vec<String> {
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

pub fn push_unique_term(terms: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.len() < 3 {
        return;
    }
    let duplicate = terms.iter().any(|term| term.eq_ignore_ascii_case(value));
    if !duplicate {
        terms.push(value.to_string());
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
            "referential relationships",
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
    let duplicate = queries
        .iter()
        .any(|existing| existing.query.eq_ignore_ascii_case(query));
    if duplicate {
        return;
    }
    queries.push(PacketPlanQueryDto {
        query: query.to_string(),
        purpose: purpose.to_string(),
    });
}

fn push_exact_symbol_packet_query(queries: &mut Vec<PacketPlanQueryDto>, query: &str) {
    let query = query.trim();
    if query.is_empty()
        || queries.iter().any(|existing| {
            packet_plan_query_is_exact_symbol_identity(existing) && existing.query == query
        })
    {
        return;
    }
    queries.push(PacketPlanQueryDto {
        query: query.to_string(),
        purpose: PACKET_EXACT_SYMBOL_QUERY_PURPOSE.to_string(),
    });
}

pub fn packet_plan_annotation(plan: &PacketPlanDto) -> String {
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
    #[cfg(any(test, feature = "test-support"))]
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
