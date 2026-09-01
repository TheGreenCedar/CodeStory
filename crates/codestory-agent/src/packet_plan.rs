#[cfg(any(test, feature = "test-support"))]
use crate::eval_probes::{eval_probes_enabled, push_prompt_named_file_probe_queries};
use crate::packet_obligations::build_packet_obligation_plan;
use crate::packet_required_probes::{
    packet_concrete_file_probe_queries_from_required, packet_prompt_exact_symbol_probe_queries,
    packet_prompt_explicit_source_path_queries,
};
use crate::packet_scoring::{packet_adjacent_query_stop_term, packet_query_stop_term};
use crate::packet_terms::{packet_probe_terms, prompt_search_terms};
use crate::planning::{
    PACKET_ADJACENT_VARIANT_QUERY_PURPOSE, PACKET_CONCRETE_FILE_QUERY_PURPOSE,
    PACKET_EXACT_SYMBOL_QUERY_PURPOSE, PACKET_GENERIC_TERM_QUERY_PURPOSE,
    dedupe_packet_plan_queries, packet_plan_query_is_exact_symbol_identity,
};
use crate::text::{
    exact_symbol_query_terms, is_non_primary_source_term, looks_like_standalone_symbol_query,
    query_mentions_non_primary_source,
};
use codestory_contracts::api::{
    AgentCitationDto, PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto, PacketTaskClassDto,
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
    let task_class = PacketTaskClassDto::ArchitectureExplanation;
    let _ = requested;
    // Task class is retained only as a skip-serializing diagnostic on the
    // internal plan. It must not steer queries or obligations.
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
    for path in packet_prompt_explicit_source_path_queries(question) {
        push_packet_query(&mut queries, &path, PACKET_CONCRETE_FILE_QUERY_PURPOSE);
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
    for (query, purpose) in packet_symbol_probe_query_specs(question, task_class, budget) {
        push_packet_query(&mut queries, &query, purpose);
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

    let mut trace = vec!["retrieval=generic source=unspecified".to_string()];
    trace.push(format!("planned_queries={}", queries.len()));
    if !extra_probes.is_empty() {
        trace.push(format!(
            "explicit_extra_probes={} source=request",
            extra_probes.len()
        ));
    }

    let mut plan = PacketPlanDto {
        task_class,
        inferred_task_class: false,
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

pub fn packet_rank_terms(_question: &str) -> Vec<String> {
    Vec::new()
}

/// Owner/member probe synthesis is deleted. Ordinary wording reaches generic
/// retrieval only; planner code must not invent `Owner.member` identities.
pub fn packet_owner_member_probe_queries(
    question: &str,
    anchor_citations: &[AgentCitationDto],
    limit: usize,
) -> Vec<String> {
    let _ = (question, anchor_citations, limit);
    Vec::new()
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
    packet_symbol_probe_query_specs(question, task_class, budget)
        .into_iter()
        .map(|(query, _)| query)
        .collect()
}

fn packet_symbol_probe_query_specs(
    question: &str,
    task_class: PacketTaskClassDto,
    budget: PacketBudgetModeDto,
) -> Vec<(String, &'static str)> {
    let terms = packet_probe_terms(question);
    let mut queries = Vec::<(String, &'static str)>::new();
    let compact = matches!(
        budget,
        PacketBudgetModeDto::Compact | PacketBudgetModeDto::Tiny
    );

    push_unique_query_specs(
        &mut queries,
        &packet_prompt_exact_symbol_probe_queries(question, &terms, task_class),
        PACKET_EXACT_SYMBOL_QUERY_PURPOSE,
    );
    #[cfg(any(test, feature = "test-support"))]
    if eval_probes_enabled() {
        let mut named_files = Vec::new();
        push_prompt_named_file_probe_queries(&terms, &mut named_files);
        push_unique_query_specs(
            &mut queries,
            &named_files,
            PACKET_CONCRETE_FILE_QUERY_PURPOSE,
        );
    }
    let query_values = queries
        .iter()
        .map(|(query, _)| query.clone())
        .collect::<Vec<_>>();
    let concrete_file_queries = packet_concrete_file_probe_queries_from_required(&query_values);
    push_unique_query_specs(
        &mut queries,
        &concrete_file_queries,
        PACKET_CONCRETE_FILE_QUERY_PURPOSE,
    );
    if !compact {
        let mut adjacent = Vec::new();
        push_adjacent_packet_term_queries(&terms, &mut adjacent, 8);
        push_unique_query_specs(
            &mut queries,
            &adjacent,
            PACKET_ADJACENT_VARIANT_QUERY_PURPOSE,
        );
    }
    let mut generic = Vec::new();
    push_generic_symbol_probe_queries(&terms, &mut generic, compact);
    push_unique_query_specs(&mut queries, &generic, PACKET_GENERIC_TERM_QUERY_PURPOSE);

    queries.truncate(packet_plan_query_cap(budget));
    queries
}

fn push_unique_query_specs(
    queries: &mut Vec<(String, &'static str)>,
    values: &[String],
    purpose: &'static str,
) {
    for value in values {
        let value = value.trim();
        if value.len() < 3
            || queries
                .iter()
                .any(|(query, _)| query.eq_ignore_ascii_case(value))
        {
            continue;
        }
        queries.push((value.to_string(), purpose));
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
    format!("packet_plan retrieval=generic queries={queries}")
}

#[cfg(test)]
mod owner_member_probe_tests {
    use super::packet_owner_member_probe_queries;
    use codestory_contracts::api::{AgentCitationDto, NodeId, NodeKind, SearchHitOrigin};

    fn citation(display_name: &str, kind: NodeKind) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(display_name.to_string()),
            display_name: display_name.to_string(),
            kind,
            file_path: Some("src/example.rs".to_string()),
            line: Some(1),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            source_excerpt: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
        }
    }

    #[test]
    fn owner_member_probe_synthesis_is_deleted() {
        let citations = vec![
            citation("Build.build", NodeKind::FUNCTION),
            citation("Site.posts", NodeKind::FUNCTION),
            citation("AutoMapper.Mapper", NodeKind::CLASS),
            citation("Monolog.Logger.addRecord", NodeKind::FUNCTION),
        ];
        let queries = packet_owner_member_probe_queries(
            "Trace how Jekyll's build command creates a site and runs the read, generate, render, and write phases. Cite the source files and name the supporting symbols.",
            &citations,
            10,
        );
        assert!(
            queries.is_empty(),
            "owner/member synthesis must not mint queries: {queries:?}"
        );
    }
}
