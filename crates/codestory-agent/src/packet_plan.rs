#[cfg(any(test, feature = "test-support"))]
use crate::eval_probes::{eval_probes_enabled, push_prompt_named_file_probe_queries};
use crate::packet_obligations::build_packet_obligation_plan;
use crate::packet_required_probes::{
    packet_concrete_file_probe_queries_from_required, packet_prompt_exact_symbol_probe_queries,
    packet_prompt_explicit_source_path_queries,
};
use crate::packet_scoring::{
    normalize_identifier, packet_adjacent_query_stop_term, packet_query_stop_term,
};
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
    AgentCitationDto, NodeKind, PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto,
    PacketTaskClassDto, SearchHitOrigin,
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
    for query in task_class_seed_queries(task_class) {
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

/// Build bounded owner/member probes from owners explicitly named in the task or already present
/// in the first retrieval, plus action words in the task. Broad semantic search is good at finding
/// a relevant type but can miss its exact lifecycle members, so qualified probes combine the
/// retained owner with the task's verbs without adding repository-specific vocabulary.
pub fn packet_owner_member_probe_queries(
    question: &str,
    anchor_citations: &[AgentCitationDto],
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let normalized_question = normalize_identifier(question);
    let prompt_terms = prompt_search_terms(question);
    let prompt_term_keys = prompt_terms
        .iter()
        .map(|term| normalize_identifier(term))
        .collect::<std::collections::HashSet<_>>();
    let anchor_owners = anchor_citations
        .iter()
        .filter_map(packet_owner_member_anchor_owner)
        .collect::<Vec<_>>();
    let mut owners = Vec::<(usize, String)>::new();
    let mut seen_owners = std::collections::HashSet::<String>::new();
    let exact_owner_candidates = exact_symbol_query_terms(question);
    for candidate in &exact_owner_candidates {
        if candidate.contains(['.', ':', '/', '\\'])
            || packet_camel_identifier_words(candidate).is_empty()
        {
            continue;
        }
        let key = normalize_identifier(candidate);
        if key.len() < 3 || !seen_owners.insert(key.clone()) {
            continue;
        }
        let position = normalized_question.rfind(&key).unwrap_or_default();
        owners.push((position, candidate.clone()));
    }
    for owner in anchor_owners {
        let owner_key = normalize_identifier(&owner);
        if owner_key.len() < 3 || seen_owners.contains(&owner_key) {
            continue;
        }
        let position = prompt_term_keys
            .iter()
            .filter(|term_key| packet_owner_identity_matches_prompt_term(&owner_key, term_key))
            .filter_map(|term_key| normalized_question.rfind(term_key))
            .max();
        if let Some(position) = position
            && seen_owners.insert(owner_key)
        {
            // A component word can establish that the typed owner is relevant, but it is never a
            // replacement identity. Turning `Logger` into `Log` or `SourceObject` into `Source`
            // fabricates owner/member probes that cannot resolve to the indexed symbol.
            owners.push((position, owner));
        }
    }
    owners.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.len().cmp(&left.1.len()))
            .then_with(|| left.1.cmp(&right.1))
    });
    owners.truncate(2);
    if owners.is_empty() {
        return Vec::new();
    }

    let mut owner_keys = owners
        .iter()
        .map(|(_, owner)| normalize_identifier(owner))
        .collect::<std::collections::HashSet<_>>();
    owner_keys.extend(
        exact_owner_candidates
            .iter()
            .map(|owner| normalize_identifier(owner)),
    );
    let mut candidates = Vec::<(bool, bool, usize, usize, usize, String)>::new();
    for (owner_index, (owner_position, owner)) in owners.iter().enumerate() {
        let owner_key = normalize_identifier(owner);
        for (term_index, term) in prompt_terms.iter().enumerate() {
            if packet_owner_member_term_is_noise(term) {
                continue;
            }
            let term_key = normalize_identifier(term);
            if term_key.is_empty() || term_key == owner_key || owner_keys.contains(&term_key) {
                continue;
            }
            let term_position = normalized_question.rfind(&term_key).unwrap_or_default();
            let after_owner = term_position >= *owner_position;
            let distance = term_position.abs_diff(*owner_position);
            for member in packet_member_term_variants(term) {
                if normalize_identifier(&member) == owner_key {
                    continue;
                }
                candidates.push((
                    !after_owner,
                    member != term.to_ascii_lowercase(),
                    distance,
                    owner_index,
                    term_index,
                    format!("{owner}.{member}"),
                ));
            }
        }
    }
    candidates.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.3.cmp(&right.3))
            .then_with(|| left.4.cmp(&right.4))
            .then_with(|| left.5.cmp(&right.5))
    });
    let mut seen = std::collections::HashSet::<String>::new();
    candidates
        .into_iter()
        .filter_map(|(_, _, _, _, _, query)| {
            let key = normalize_identifier(&query);
            seen.insert(key).then_some(query)
        })
        .take(limit)
        .collect()
}

fn packet_owner_member_anchor_owner(citation: &AgentCitationDto) -> Option<String> {
    if citation.origin != SearchHitOrigin::IndexedSymbol || !citation.resolvable {
        return None;
    }
    if matches!(
        citation.evidence_tier,
        Some(codestory_contracts::api::PacketEvidenceTierDto::ComponentReport)
    ) || citation.evidence_producer.as_deref() == Some("component_report")
    {
        return None;
    }
    let segments = citation
        .display_name
        .split(['.', ':', '#', '/', '\\'])
        .map(|segment| segment.trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')))
        .filter(|segment| segment.len() >= 3)
        .collect::<Vec<_>>();
    let owner = match citation.kind {
        NodeKind::FUNCTION | NodeKind::METHOD => segments.get(segments.len().checked_sub(2)?)?,
        NodeKind::MODULE
        | NodeKind::NAMESPACE
        | NodeKind::PACKAGE
        | NodeKind::STRUCT
        | NodeKind::CLASS
        | NodeKind::INTERFACE
        | NodeKind::UNION
        | NodeKind::ENUM
        | NodeKind::TYPEDEF => segments.last()?,
        _ => return None,
    };
    Some((*owner).to_string())
}

fn packet_owner_identity_matches_prompt_term(owner_key: &str, term_key: &str) -> bool {
    if owner_key == term_key {
        return true;
    }
    owner_key.len().abs_diff(term_key.len()) <= 4
        && (owner_key.starts_with(term_key) || term_key.starts_with(owner_key))
}

fn packet_owner_member_term_is_noise(term: &str) -> bool {
    packet_query_stop_term(term)
        || matches!(
            term,
            "answer"
                | "api"
                | "apis"
                | "cooperate"
                | "cite"
                | "cites"
                | "explain"
                | "expose"
                | "exposes"
                | "file"
                | "files"
                | "behavior"
                | "behaviour"
                | "convenience"
                | "helper"
                | "helpers"
                | "http"
                | "level"
                | "method"
                | "methods"
                | "name"
                | "names"
                | "package"
                | "phase"
                | "phases"
                | "source"
                | "sources"
                | "supporting"
                | "symbol"
                | "symbols"
                | "trace"
                | "top"
        )
}

fn packet_member_term_variants(term: &str) -> Vec<String> {
    let term = term.trim().to_ascii_lowercase();
    let mut variants = Vec::new();
    let mut push = |value: String| {
        if value.len() >= 3 && !variants.iter().any(|variant| variant == &value) {
            variants.push(value);
        }
    };

    if let Some(stem) = term.strip_suffix("ization") {
        push(format!("{stem}ize"));
    } else if let Some(stem) = term.strip_suffix("isation") {
        push(format!("{stem}ise"));
    } else if let Some(stem) = term.strip_suffix("ation") {
        push(format!("{stem}ate"));
    } else if let Some(stem) = term.strip_suffix("ies") {
        push(format!("{stem}y"));
    } else if let Some(stem) = term.strip_suffix("ing") {
        push(stem.to_string());
        push(format!("{stem}e"));
    } else if let Some(stem) = term.strip_suffix("ed") {
        push(stem.to_string());
        push(term.trim_end_matches('d').to_string());
    } else if term.ends_with("sses") {
        push(term.trim_end_matches("es").to_string());
    } else if let Some(stem) = term.strip_suffix('s') {
        push(stem.to_string());
    } else {
        push(term);
    }
    variants
}

fn packet_camel_identifier_words(identifier: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut start = 0usize;
    let chars = identifier.char_indices().collect::<Vec<_>>();
    for index in 1..chars.len() {
        let (_, previous) = chars[index - 1];
        let (offset, current) = chars[index];
        let next_is_lower = chars
            .get(index + 1)
            .is_some_and(|(_, next)| next.is_ascii_lowercase());
        if current.is_ascii_uppercase()
            && (previous.is_ascii_lowercase() || previous.is_ascii_digit() || next_is_lower)
        {
            let word = &identifier[start..offset];
            if word.len() >= 3 {
                words.push(word.to_string());
            }
            start = offset;
        }
    }
    let word = &identifier[start..];
    if word.len() >= 3 {
        words.push(word.to_string());
    }
    words
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

fn task_class_seed_queries(task_class: PacketTaskClassDto) -> &'static [&'static str] {
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
        PacketTaskClassDto::RouteTracing => &["route handler endpoint", "references"],
        PacketTaskClassDto::SymbolOwnership => &["definition references", "callers"],
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

#[cfg(test)]
mod owner_member_probe_tests {
    use super::packet_owner_member_probe_queries;
    use codestory_contracts::api::{
        AgentCitationDto, NodeId, NodeKind, PacketEvidenceTierDto, SearchHitOrigin,
    };

    fn citation(
        display_name: &str,
        kind: NodeKind,
        origin: SearchHitOrigin,
        resolvable: bool,
    ) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(display_name.to_string()),
            display_name: display_name.to_string(),
            kind,
            file_path: Some("src/example.rs".to_string()),
            line: Some(1),
            score: 1.0,
            origin,
            target: None,
            resolvable,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            source_excerpt: None,
        }
    }

    fn symbols(display_names: &[&str]) -> Vec<AgentCitationDto> {
        display_names
            .iter()
            .map(|display_name| {
                citation(
                    display_name,
                    NodeKind::FUNCTION,
                    SearchHitOrigin::IndexedSymbol,
                    true,
                )
            })
            .collect()
    }

    #[test]
    fn exact_owner_members_cover_late_lifecycle_phases_without_task_specific_names() {
        let queries = packet_owner_member_probe_queries(
            "Trace how Jekyll's build command creates a site and runs the read, generate, render, and write phases. Cite the source files and name the supporting symbols.",
            &symbols(&["Build.build", "Site.posts", "Command.process_site"]),
            10,
        );

        for expected in ["Site.read", "Site.generate", "Site.render", "Site.write"] {
            assert!(
                queries.iter().any(|query| query == expected),
                "missing {expected} from {queries:?}"
            );
        }
        assert!(!queries.iter().any(|query| query.ends_with(".cite")));
    }

    #[test]
    fn owner_member_probes_normalize_noun_and_verb_inflections() {
        let queries = packet_owner_member_probe_queries(
            "Explain how package:http exposes top-level helpers, BaseClient convenience methods, BaseRequest finalization, and IOClient send behavior.",
            &[],
            6,
        );

        assert!(queries.iter().any(|query| query == "BaseRequest.finalize"));
        assert!(queries.iter().any(|query| query == "IOClient.send"));
    }

    #[test]
    fn owner_member_probes_reject_untyped_retrieval_anchors() {
        let citations = vec![
            citation(
                "examples/forms/validation.html",
                NodeKind::FILE,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
            citation(
                "validation.html",
                NodeKind::FILE,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
            citation(
                "validation.check",
                NodeKind::FUNCTION,
                SearchHitOrigin::TextMatch,
                true,
            ),
            citation(
                "validation.handle",
                NodeKind::FUNCTION,
                SearchHitOrigin::IndexedSymbol,
                false,
            ),
            citation(
                "validation.unknown",
                NodeKind::UNKNOWN,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
            citation(
                "validation.enabled",
                NodeKind::CONSTANT,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
            {
                let mut citation = citation(
                    "codestory::component_report::dir:validation",
                    NodeKind::MODULE,
                    SearchHitOrigin::IndexedSymbol,
                    true,
                );
                citation.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
                citation.evidence_producer = Some("component_report".to_string());
                citation
            },
        ];
        let queries = packet_owner_member_probe_queries(
            "Explain how validation handles checks.",
            &citations,
            10,
        );

        assert!(
            queries.is_empty(),
            "untyped citation authorized owner/member probes: {queries:?}"
        );
    }

    #[test]
    fn owner_member_probes_use_types_but_not_value_symbols_as_owners() {
        let citations = vec![
            citation(
                "AutoMapper.Mapper",
                NodeKind::CLASS,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
            citation(
                "animate__fast.animate",
                NodeKind::CONSTANT,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
        ];

        let mapper_queries = packet_owner_member_probe_queries(
            "Explain how the mapper maps source objects.",
            &citations,
            8,
        );
        assert!(mapper_queries.iter().any(|query| query == "Mapper.map"));

        let animation_queries = packet_owner_member_probe_queries(
            "Explain how animate constants define shared base behavior.",
            &citations[1..],
            8,
        );
        assert!(
            animation_queries.is_empty(),
            "value symbols must not mint owner/member probes: {animation_queries:?}"
        );
    }

    #[test]
    fn owner_member_probes_preserve_the_typed_owner_instead_of_prompt_fragments() {
        let queries = packet_owner_member_probe_queries(
            "Explain how Monolog turns a log call into a LogRecord and passes it through handlers.",
            &symbols(&[
                "Monolog.Logger.addRecord",
                "SourceObject.map",
                "ReplaceName.map",
                "Monolog\\Handler.HandlerInterface.handle",
            ]),
            8,
        );

        assert!(
            queries.iter().any(|query| query == "Logger.log"),
            "typed Logger owner was not retained: {queries:?}"
        );
        assert!(
            !queries.iter().any(|query| {
                query.starts_with("Log.")
                    || query.starts_with("Source.")
                    || query.starts_with("Name.")
            }),
            "prompt fragments became fabricated owners: {queries:?}"
        );
    }
}
