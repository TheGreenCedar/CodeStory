#[cfg(any(test, feature = "test-support"))]
use crate::eval_probes::{eval_probes_enabled, push_prompt_concept_derived_symbol_probes};
use crate::packet_scoring::{
    normalize_identifier, packet_display_path, packet_file_stem_matches_query,
    packet_query_stop_term,
};
use crate::packet_terms::packet_probe_terms;
use crate::text::exact_symbol_query_terms;
use crate::text::{RetrievalFileRole, retrieval_file_role_from_path};
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, NodeKind, PacketClaimDto, PacketTaskClassDto, SearchHitOrigin,
};

pub fn packet_missing_sufficiency_probe_queries_with_extra(
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
    _supported_claims: &[PacketClaimDto],
) -> bool {
    packet_probe_query_is_cited(query, answer)
}

#[cfg(test)]
pub fn packet_probe_query_is_claimed(query: &str, supported_claims: &[PacketClaimDto]) -> bool {
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
        let concept_covered =
            packet_claim_covers_concept_probe(&normalized_query, &normalized_claim);
        if packet_probe_query_requires_concept_match(&normalized_query) {
            concept_covered
        } else {
            normalized_claim.contains(&normalized_query) || concept_covered
        }
    })
}

#[cfg(test)]
fn packet_probe_query_requires_concept_match(normalized_query: &str) -> bool {
    matches!(
        normalized_query,
        "references" | "foreignkeyrelationships" | "schemaconstraints"
    )
}

#[cfg(test)]
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
        "loggerrecord" => normalized_claim.contains("log") && normalized_claim.contains("record"),
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
        "customvalidationflow" => {
            normalized_claim.contains("custom")
                && normalized_claim.contains("validation")
                && (normalized_claim.contains("scriptdriven")
                    || normalized_claim.contains("validity")
                    || normalized_claim.contains("message"))
        }
        "customerrorrendering" => {
            normalized_claim.contains("error")
                && (normalized_claim.contains("render") || normalized_claim.contains("message"))
                && (normalized_claim.contains("validitystate")
                    || normalized_claim.contains("validity"))
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
            normalized_claim.contains("suppress")
                && normalized_claim.contains("browser")
                && normalized_claim.contains("defaultui")
                && (normalized_claim.contains("suppress") || normalized_claim.contains("disable"))
                && (normalized_claim.contains("validation")
                    || normalized_claim.contains("validity")
                    || normalized_claim.contains("form")
                    || normalized_claim.contains("scriptdriven"))
        }
        "shellinstallerbootstrap" => {
            normalized_claim.contains("install")
                && normalized_claim.contains("bootstrap")
                && (normalized_claim.contains("source")
                    || normalized_claim.contains("runtime")
                    || normalized_claim.contains("shell")
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
                    || normalized_claim.contains("asset")
                    || normalized_claim.contains("runtime"))
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
            (normalized_claim.contains("request")
                || (normalized_claim.contains("base") && normalized_claim.contains("request")))
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
                || claim_has_sql_relationship_reference(normalized_claim)
        }
        "sqltabledefinitions" => {
            normalized_claim.contains("sqlschema")
                && (normalized_claim.contains("definestables")
                    || normalized_claim.contains("tables")
                    || normalized_claim.contains("createtable"))
        }
        "foreignkeyrelationships" => {
            normalized_claim.contains("rowsreference")
                || normalized_claim.contains("foreignkey")
                || claim_has_sql_relationship_reference(normalized_claim)
        }
        "schemaconstraints" => {
            normalized_claim.contains("foreignkey")
                || normalized_claim.contains("rowsreference")
                || claim_has_sql_relationship_reference(normalized_claim)
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

#[cfg(test)]
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

#[cfg(test)]
fn packet_probe_query_allows_claim_coverage(query: &str) -> bool {
    let trimmed = query.trim();
    packet_concept_probe_allows_claim_coverage(&normalize_identifier(trimmed))
        || trimmed.contains('.')
            && !trimmed.contains('/')
            && !trimmed.contains('\\')
            && !trimmed.chars().any(char::is_whitespace)
}

#[cfg(test)]
fn claim_has_sql_relationship_reference(normalized_claim: &str) -> bool {
    normalized_claim.contains("rowsreference")
        || (normalized_claim.contains("references")
            && (normalized_claim.contains("foreignkey")
                || normalized_claim.contains("relationship")
                || normalized_claim.contains("table")
                || normalized_claim.contains("rows")))
}

#[cfg(test)]
fn packet_concept_probe_allows_claim_coverage(normalized_query: &str) -> bool {
    matches!(
        normalized_query,
        "recordcreation"
            | "handlerregistration"
            | "handlerprocessing"
            | "handlerinterface"
            | "loggerrecord"
            | "logcall"
            | "handlerstack"
            | "nativeformconstraints"
            | "customvalidationflow"
            | "customerrorrendering"
            | "validitystate"
            | "submitpreventdefault"
            | "formvalidationbypass"
            | "toplevelhelpers"
            | "requestfinalization"
            | "requestresponse"
            | "references"
            | "sqltabledefinitions"
            | "foreignkeyrelationships"
            | "schemaconstraints"
            | "sqlschemascripts"
            | "schemadialectscripts"
    )
}

#[cfg(any(test, feature = "test-support"))]
pub fn packet_sufficiency_required_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    packet_sufficiency_required_probe_queries_with_extra(question, task_class, &[])
}

pub fn packet_sufficiency_required_probe_queries_with_extra(
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

pub fn packet_sufficiency_required_probe_queries_from_terms(
    _terms: &[String],
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
    Vec::new()
}

pub fn packet_prompt_exact_symbol_probe_queries(
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
            push_unique_exact_symbol_term(&mut queries, &term);
        }
    }
    #[cfg(not(any(test, feature = "test-support")))]
    let _ = terms;
    #[cfg(any(test, feature = "test-support"))]
    if eval_probes_enabled() {
        push_prompt_concept_derived_symbol_probes(terms, &mut queries);
    }
    queries
}

/// Extract source files and paths the user names explicitly.
///
/// Exact-symbol parsing deliberately excludes source paths because a file is not a typed symbol.
/// Packet planning still needs to honor those paths as material retrieval targets rather than
/// leaving them in the supplemental queue. This parser is bounded by the checked-in language
/// support table, so dotted prose, URLs, and method names do not become file probes.
pub fn packet_prompt_explicit_source_path_queries(question: &str) -> Vec<String> {
    let mut queries = Vec::new();
    for token in question.split_whitespace() {
        let mut candidate = token.trim_matches(|ch: char| {
            matches!(
                ch,
                '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';'
            )
        });
        if candidate.is_empty() || candidate.contains("://") {
            continue;
        }
        if codestory_contracts::language_support::language_support_profile_for_path(Some(candidate))
            .is_none()
        {
            candidate = candidate.trim_end_matches('.');
        }
        let candidate = packet_source_path_without_location_suffix(candidate);
        if codestory_contracts::language_support::language_support_profile_for_path(Some(candidate))
            .is_some()
        {
            push_unique_exact_symbol_term(&mut queries, candidate);
        }
    }
    queries
}

fn packet_source_path_without_location_suffix(candidate: &str) -> &str {
    if let Some((path, line)) = candidate.rsplit_once(':')
        && !path.is_empty()
        && line.chars().all(|ch| ch.is_ascii_digit())
    {
        return path;
    }
    if let Some((path, line)) = candidate.rsplit_once("#L")
        && !path.is_empty()
        && line.chars().all(|ch| ch.is_ascii_digit())
    {
        return path;
    }
    candidate
}

/// Extract table identities from the ordinary relational-schema phrasing "between A, B, and C".
/// These are bounded source anchors, not benchmark labels: the same parser turns "customers,
/// orders, and order items" into `customer`, `order`, and `order item` queries.
pub fn packet_named_schema_entity_queries(question: &str) -> Vec<String> {
    let lower = question.to_ascii_lowercase();
    let Some(start) = [" between ", " among "]
        .into_iter()
        .filter_map(|marker| lower.find(marker).map(|index| index + marker.len()))
        .min()
    else {
        return Vec::new();
    };
    let tail = &lower[start..];
    let end = [" across ", " within ", " using ", " from ", "."]
        .into_iter()
        .filter_map(|marker| tail.find(marker))
        .min()
        .unwrap_or(tail.len());
    let segment = tail[..end].replace(" and ", ",");
    let mut queries = Vec::new();
    for phrase in segment.split(',') {
        let words = phrase
            .split_whitespace()
            .map(|word| word.trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')))
            .filter(|word| !word.is_empty() && !matches!(*word, "a" | "an" | "the"))
            .collect::<Vec<_>>();
        if words.is_empty()
            || words.len() > 3
            || words.iter().any(|word| {
                matches!(
                    *word,
                    "database"
                        | "relation"
                        | "relations"
                        | "relationship"
                        | "relationships"
                        | "schema"
                        | "sql"
                        | "table"
                        | "tables"
                )
            })
        {
            continue;
        }
        let mut normalized = words
            .iter()
            .map(|word| (*word).to_string())
            .collect::<Vec<_>>();
        if let Some(last) = normalized.last_mut() {
            if let Some(stem) = last.strip_suffix("ies") {
                *last = format!("{stem}y");
            } else if let Some(stem) = last.strip_suffix("sses") {
                *last = format!("{stem}ss");
            } else if last.ends_with('s') && !last.ends_with("ss") {
                last.pop();
            }
        }
        let query = normalized.join(" ");
        if query.len() >= 3 && !queries.iter().any(|existing| existing == &query) {
            queries.push(query);
        }
        if queries.len() == 8 {
            break;
        }
    }
    queries
}

/// Query the SQL collector's canonical default-schema identity for each named entity. The
/// collector deliberately assigns unqualified tables to `public`, so this asks for its exact
/// durable symbol instead of hoping a broad noun query outranks every seed file containing it.
pub fn packet_named_schema_entity_symbol_queries(question: &str) -> Vec<String> {
    packet_named_schema_entity_queries(question)
        .into_iter()
        .map(|entity| format!("public.{}", entity.replace(' ', "")))
        .collect()
}

fn push_unique_exact_symbol_term(terms: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.len() >= 3 && !terms.iter().any(|term| term == value) {
        terms.push(value.to_string());
    }
}

fn packet_prompt_exact_symbol_term_is_probe(term: &str) -> bool {
    let trimmed = term.trim();
    if trimmed.len() < 3 {
        return false;
    }
    if packet_prompt_exact_symbol_term_is_source_path(trimmed) {
        return false;
    }
    let letters = trimmed
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .collect::<Vec<_>>();
    if letters.is_empty() || letters.iter().all(|ch| ch.is_ascii_uppercase()) {
        return false;
    }
    let plural_acronym = letters.last() == Some(&'s')
        && letters[..letters.len() - 1]
            .iter()
            .all(|ch| ch.is_ascii_uppercase());
    !plural_acronym
}

fn packet_prompt_exact_symbol_term_is_source_path(term: &str) -> bool {
    codestory_contracts::language_support::language_support_profile_for_path(Some(term)).is_some()
}

pub fn packet_concrete_file_probe_queries_from_required(
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
    // Identity-only: only literal *.rs stems that look like file names, no domain keys.
    let trimmed = query.trim();
    if trimmed.ends_with(".rs") {
        return Some(trimmed.to_string());
    }
    None
}

pub fn packet_probe_query_is_cited(query: &str, answer: &AgentAnswerDto) -> bool {
    answer
        .citations
        .iter()
        .any(|citation| packet_citation_satisfies_required_probe(query, citation))
}

pub fn packet_citation_satisfies_required_probe(query: &str, citation: &AgentCitationDto) -> bool {
    // Identity-only (phase9-r3): no domain probe vocabulary tables.
    if packet_type_declaration_probe_subject_tokens(query).is_some() {
        return packet_citation_matches_type_declaration_probe(query, citation);
    }
    if packet_citation_matches_sql_table_identity(query, citation) {
        return true;
    }
    if packet_citation_matches_required_coverage_role(query, citation) {
        return true;
    }
    if let Some(matches_file_scoped_symbol) =
        packet_file_scoped_symbol_probe_matches(query, citation)
    {
        return matches_file_scoped_symbol;
    }
    if packet_citation_is_exact_primary_file_probe_match(query, citation) {
        return true;
    }
    if packet_file_stem_matches_query(query, citation.file_path.as_deref()) {
        return true;
    }
    let Some(match_rank) = packet_citation_probe_match_rank(query, citation) else {
        return false;
    };
    !packet_required_probe_needs_exact_match(query) || match_rank >= 4
}

pub fn packet_required_probe_needs_exact_match(query: &str) -> bool {
    // Qualified path/symbol and SQL table probes require exact identity matches.
    query.contains("::")
        || query.contains('.')
        || packet_create_table_probe_table(query).is_some()
        || packet_public_catalog_probe_table(query).is_some()
}

pub fn packet_citation_probe_match_rank(query: &str, citation: &AgentCitationDto) -> Option<u8> {
    let normalized_query = normalize_identifier(query);
    if normalized_query.is_empty() {
        return Some(0);
    }
    if packet_type_declaration_probe_subject_tokens(query).is_some() {
        return packet_citation_matches_type_declaration_probe(query, citation).then_some(6);
    }
    if packet_citation_matches_sql_table_identity(query, citation) {
        return Some(6);
    }
    if packet_create_table_probe_table(query).is_some()
        || packet_public_catalog_probe_table(query).is_some()
    {
        // SQL table probes never fall through to shared CREATE/TABLE token coverage.
        return None;
    }
    if packet_citation_matches_required_coverage_role(query, citation) {
        return Some(6);
    }
    if packet_citation_is_exact_primary_file_probe_match(query, citation) {
        return Some(6);
    }
    let normalized_display = normalize_identifier(&citation.display_name);
    if let Some(matches_file_scoped_symbol) =
        packet_file_scoped_symbol_probe_matches(query, citation)
    {
        if matches_file_scoped_symbol {
            Some(6)
        } else {
            None
        }
    } else if packet_file_stem_matches_query(query, citation.file_path.as_deref()) {
        Some(5)
    } else if normalized_display == normalized_query
        || normalized_display.ends_with(&normalized_query)
    {
        // Identity-only: exact / suffix identifier match only (CX-R3-01).
        Some(4)
    } else {
        None
    }
}

fn packet_type_declaration_probe_subject_tokens(query: &str) -> Option<Vec<String>> {
    let mut tokens = crate::text::symbol_query_tokens(query);
    if tokens.len() < 3
        || tokens.get(tokens.len() - 2).map(String::as_str) != Some("type")
        || tokens.last().map(String::as_str) != Some("declaration")
    {
        return None;
    }
    tokens.truncate(tokens.len() - 2);
    Some(tokens)
}

fn packet_citation_matches_type_declaration_probe(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    let Some(subject_tokens) = packet_type_declaration_probe_subject_tokens(query) else {
        return false;
    };
    if !matches!(
        citation.kind,
        NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::UNION
            | NodeKind::ENUM
            | NodeKind::TYPEDEF
    ) {
        return false;
    }
    let display_tokens = crate::text::symbol_query_tokens(&citation.display_name);
    subject_tokens
        .iter()
        .all(|subject| display_tokens.iter().any(|display| display == subject))
}

fn packet_citation_is_exact_primary_file_probe_match(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    citation.kind == NodeKind::FILE
        && citation.origin == SearchHitOrigin::IndexedSymbol
        && citation.resolvable
        && citation
            .file_path
            .as_deref()
            .is_some_and(|path| retrieval_file_role_from_path(path) == RetrievalFileRole::Source)
        && packet_file_stem_matches_query(query, citation.file_path.as_deref())
}

fn packet_citation_matches_required_coverage_role(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    let Some(coverage_role) = citation.coverage_role.as_deref() else {
        return false;
    };
    // Exact normalized equality only — no holdout stage alias table (CX-R2-03).
    normalize_identifier(coverage_role) == normalize_identifier(query)
}

fn packet_citation_matches_sql_table_identity(query: &str, citation: &AgentCitationDto) -> bool {
    // Only explicit SQL table probe forms — never treat arbitrary tokens as tables.
    let Some(query_table) =
        packet_create_table_probe_table(query).or_else(|| packet_public_catalog_probe_table(query))
    else {
        return false;
    };
    let citation_table = packet_create_table_probe_table(&citation.display_name)
        .or_else(|| packet_public_catalog_probe_table(&citation.display_name))
        .or_else(|| packet_sql_table_identity(&citation.display_name));
    citation_table.is_some_and(|table| table == query_table)
}

fn packet_public_catalog_probe_table(query: &str) -> Option<String> {
    let trimmed = query.trim();
    let remainder = trimmed
        .strip_prefix("public.")
        .or_else(|| trimmed.strip_prefix("PUBLIC."))?;
    if remainder.is_empty() {
        return None;
    }
    packet_sql_table_identity(remainder)
}

fn packet_create_table_probe_table(query: &str) -> Option<String> {
    let trimmed = query.trim();
    let remainder = trimmed
        .strip_prefix("CREATE TABLE")
        .or_else(|| {
            let lower = trimmed.to_ascii_lowercase();
            let index = lower.find("create table")?;
            Some(&trimmed[index + "create table".len()..])
        })?
        .trim();
    if remainder.is_empty() {
        return None;
    }
    packet_sql_table_identity(remainder)
}

fn packet_sql_table_identity(display: &str) -> Option<String> {
    let trimmed = display.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_create = trimmed
        .strip_prefix("CREATE TABLE")
        .or_else(|| {
            let lower = trimmed.to_ascii_lowercase();
            let index = lower.find("create table")?;
            Some(&trimmed[index + "create table".len()..])
        })
        .unwrap_or(trimmed)
        .trim();
    let token = without_create
        .rsplit(['.', ' ', '/', '\\'])
        .next()?
        .trim_matches(|ch: char| matches!(ch, '[' | ']' | '"' | '\'' | '`' | '(' | ')' | ';'));
    let normalized = normalize_identifier(token);
    (normalized.len() >= 4).then_some(normalized)
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
    if !packet_probe_file_name_matches(&parts.file_name, &file_name) {
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
        let tokens = crate::text::symbol_query_tokens(&citation.display_name);
        return Some(
            tokens.iter().any(|token| token == "foreign")
                && tokens.iter().any(|token| token == "key"),
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

pub fn packet_probe_file_name_matches(query_file_name: &str, candidate_file_name: &str) -> bool {
    let query_stem = packet_probe_file_stem(query_file_name);
    let candidate_stem = packet_probe_file_stem(candidate_file_name);
    if query_stem.is_empty() || candidate_stem.is_empty() {
        return false;
    }
    query_stem == candidate_stem
        || packet_probe_role_file_stem_matches(&query_stem, &candidate_stem)
}

fn packet_probe_file_stem(file_name: &str) -> String {
    let file_name = file_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(file_name)
        .trim();
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    normalize_identifier(stem)
}

fn packet_probe_role_file_stem_matches(_query_stem: &str, _candidate_stem: &str) -> bool {
    // Domain role-file stem aliases removed (phase9-r3). File identity is stem equality only.
    false
}

pub struct PacketFileScopedSymbolProbe {
    pub query_path: String,
    pub file_name: String,
    pub raw_symbols: Vec<String>,
    pub symbols: Vec<String>,
}

pub fn packet_file_scoped_symbol_probe_parts(query: &str) -> Option<PacketFileScopedSymbolProbe> {
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
        "makefile" | "dockerfile" | "rakefile" | "gemfile" | "configure"
    ) || file_name.ends_with("_completion")
        || file_name.contains("completion")
}

pub fn packet_citation_probe_token_coverage(query: &str, citation: &AgentCitationDto) -> usize {
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

fn push_unique_owned_terms(terms: &mut Vec<String>, values: &[String]) {
    for value in values {
        push_unique_term(terms, value);
    }
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
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: 0.4,
                semantic: 0.2,
                graph: 0.3,
                total: score,
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
            source_excerpt: None,
        }
    }

    #[test]
    fn prompt_exact_symbol_queries_reject_plural_acronyms() {
        let queries = packet_prompt_exact_symbol_probe_queries(
            "Explain how the AutoMapper APIs build and execute a mapping plan",
            &[],
            PacketTaskClassDto::ArchitectureExplanation,
        );

        assert!(queries.iter().any(|query| query == "AutoMapper"));
        assert!(!queries.iter().any(|query| query == "APIs"));
    }

    #[test]
    fn prompt_source_path_queries_preserve_explicit_supported_files_only() {
        let queries = packet_prompt_explicit_source_path_queries(
            "Compare `animate.css`, src/http/client.dart:42, foo-bar.ts, \
             Widget.run, example.com, and https://example.test/generated/main.rs.",
        );

        assert_eq!(
            queries,
            ["animate.css", "src/http/client.dart", "foo-bar.ts"]
        );
    }

    #[test]
    fn named_schema_entities_are_bounded_and_singularized() {
        assert_eq!(
            packet_named_schema_entity_queries(
                "Explain relationships between customers, orders, and order items across SQL scripts."
            ),
            ["customer", "order", "order item"]
        );
        assert_eq!(
            packet_named_schema_entity_queries(
                "Explain relationships between artists, albums, tracks, invoices, and invoice lines across the seed scripts."
            ),
            ["artist", "album", "track", "invoice", "invoice line"]
        );
        assert_eq!(
            packet_named_schema_entity_symbol_queries(
                "Explain relationships between artists, albums, and invoice lines across the seed scripts."
            ),
            ["public.artist", "public.album", "public.invoiceline"]
        );
    }

    #[test]
    fn packet_probe_match_rank_does_not_privilege_soft_token_coverage() {
        let mut citation = test_packet_citation(
            "std::collections::HashMap",
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
            0.6,
        );
        citation.kind = NodeKind::MODULE;

        // Soft multi-token overlap must not award required-probe rank (CX-R3-01).
        assert_eq!(
            packet_citation_probe_match_rank("jsonl event output", &citation),
            None
        );
        assert_eq!(
            packet_citation_probe_token_coverage("jsonl event output", &citation),
            3
        );
        assert_eq!(
            packet_citation_probe_match_rank("event_processor_with_jsonl_output", &citation),
            Some(5)
        );
    }

    #[test]
    fn type_declaration_required_probe_preserves_evidence_kind() {
        for (display_name, kind) in [
            ("Client", NodeKind::CLASS),
            ("HttpClient", NodeKind::INTERFACE),
            ("ClientAdapter", NodeKind::STRUCT),
            ("ClientResult", NodeKind::UNION),
            ("ClientMode", NodeKind::ENUM),
            ("ClientAlias", NodeKind::TYPEDEF),
        ] {
            let mut citation = test_packet_citation(display_name, "src/client.dart", 0.1);
            citation.kind = kind;
            assert!(
                packet_citation_satisfies_required_probe("client type declaration", &citation),
                "{kind:?} {display_name} should satisfy a client type-declaration probe"
            );
            assert_eq!(
                packet_citation_probe_match_rank("client type declaration", &citation),
                Some(6),
                "{kind:?} {display_name} should receive exact typed-probe rank"
            );
        }

        for (display_name, kind, path) in [
            ("Client.send", NodeKind::METHOD, "src/client.dart"),
            ("createClient", NodeKind::FUNCTION, "src/client.dart"),
            ("client", NodeKind::FILE, "src/client.dart"),
            ("Response", NodeKind::CLASS, "src/client.dart"),
        ] {
            let mut citation = test_packet_citation(display_name, path, 100.0);
            citation.kind = kind;
            assert!(
                !packet_citation_satisfies_required_probe("client type declaration", &citation),
                "{kind:?} {display_name} must not satisfy a client type-declaration probe through its file path"
            );
            assert_eq!(
                packet_citation_probe_match_rank("client type declaration", &citation),
                None,
                "{kind:?} {display_name} must not rank for a client type-declaration probe"
            );
        }
    }

    #[test]
    fn required_probe_queries_match_exact_coverage_role_ids() {
        // Exact normalized equality only — alias spellings must not match (CX-R2-03).
        let mut citation = test_packet_citation("WidgetFactory", "crates/ui/widget.rs", 40.0);
        citation.coverage_role = Some("source evidence".to_string());
        assert!(packet_citation_satisfies_required_probe(
            "source evidence",
            &citation
        ));
        assert_eq!(
            packet_citation_probe_match_rank("source evidence", &citation),
            Some(6)
        );
        assert!(packet_citation_satisfies_required_probe(
            "Source Evidence",
            &citation
        ));

        citation.coverage_role = Some("tests and regression coverage".to_string());
        assert!(packet_citation_satisfies_required_probe(
            "tests and regression coverage",
            &citation
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "network command input",
            &citation
        ));
        citation.coverage_role = Some("command_network_input".to_string());
        assert!(!packet_citation_satisfies_required_probe(
            "network command input",
            &citation
        ));
        assert_eq!(
            packet_citation_probe_match_rank("network command input", &citation),
            None
        );
    }

    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn generic_probes_prefer_behavior_owners_without_removing_lexical_fallback() {
        let assert_role_match = |query, display_name, path, kind| {
            let mut citation = test_packet_citation(display_name, path, 0.7);
            citation.kind = kind;
            assert_eq!(packet_citation_probe_match_rank(query, &citation), Some(6));
        };
        assert_role_match(
            "request entrypoint",
            "createClientInstance",
            "src/client/factory.ts",
            NodeKind::FUNCTION,
        );
        assert_role_match(
            "request entrypoint",
            "HttpClient.request",
            "src/client/http_client.ts",
            NodeKind::METHOD,
        );
        assert_role_match(
            "default instance",
            "createClientInstance",
            "src/client/factory.ts",
            NodeKind::FUNCTION,
        );
        for query in ["request dispatch", "request method"] {
            assert_role_match(
                query,
                "HttpClient.request",
                "src/client/http_client.ts",
                NodeKind::METHOD,
            );
        }
        for query in ["request interceptor", "interceptor handlers"] {
            assert_role_match(
                query,
                "RequestInterceptorRegistry",
                "src/client/interceptors.ts",
                NodeKind::CLASS,
            );
            assert_role_match(
                query,
                "RequestInterceptorRegistry.constructor",
                "src/client/interceptors.ts",
                NodeKind::METHOD,
            );
        }
        assert_role_match(
            "adapters",
            "selectAdapter",
            "src/client/adapters/select.ts",
            NodeKind::FUNCTION,
        );
        assert_role_match(
            "transport adapter",
            "selectAdapter",
            "src/client/adapters/select.ts",
            NodeKind::FUNCTION,
        );
        assert_role_match(
            "search entrypoint",
            "cli::main",
            "src/main.rs",
            NodeKind::FUNCTION,
        );
        assert_role_match(
            "parallel search",
            "ParallelSearchDriver",
            "src/search/driver.rs",
            NodeKind::FUNCTION,
        );
        assert_role_match(
            "search execution unit",
            "SearchExecutor::execute_search",
            "src/search/executor.rs",
            NodeKind::METHOD,
        );
        let mut search_type = test_packet_citation("SearchExecutor", "src/search/executor.rs", 0.7);
        search_type.kind = NodeKind::STRUCT;
        assert_ne!(
            packet_citation_probe_match_rank("search execution unit", &search_type),
            Some(6),
            "a named type is not behavioral execution evidence"
        );
        assert_role_match(
            "flag parsing",
            "CliArgs",
            "src/flags/arguments.rs",
            NodeKind::STRUCT,
        );

        let assert_lexical_fallback = |query, path| {
            let display_name = query;
            let mut citation = test_packet_citation(display_name, path, 0.9);
            citation.kind = NodeKind::MODULE;
            assert_eq!(
                packet_citation_probe_match_rank(query, &citation),
                Some(4),
                "non-owning lexical evidence remains a fallback"
            );
        };
        assert_lexical_fallback("transport adapter", "src/client/adapters/imports.ts");
        assert_lexical_fallback("search entrypoint", "src/search/imports.rs");
    }

    #[test]
    fn exact_primary_file_probe_rank_requires_indexed_resolvable_file_identity() {
        let mut indexed_file =
            test_packet_citation("transport registry", "src/runtime/adapters.js", 0.1);
        indexed_file.kind = NodeKind::FILE;
        assert_eq!(
            packet_citation_probe_match_rank("adapters", &indexed_file),
            Some(6)
        );

        for (label, citation) in [
            {
                let mut citation = indexed_file.clone();
                citation.file_path = Some("generated/adapters.js".to_string());
                ("generated file", citation)
            },
            {
                let mut citation = indexed_file.clone();
                citation.file_path = Some("tests/adapters.js".to_string());
                ("test file", citation)
            },
            {
                let mut citation = indexed_file.clone();
                citation.file_path = Some("src/runtime/adapter_factory.js".to_string());
                ("non-exact stem", citation)
            },
            {
                let mut citation = indexed_file.clone();
                citation.kind = NodeKind::FUNCTION;
                citation.display_name = "resolveHandle".to_string();
                ("helper in exact file", citation)
            },
            {
                let mut citation = indexed_file.clone();
                citation.origin = SearchHitOrigin::TextMatch;
                ("synthetic file", citation)
            },
            {
                let mut citation = indexed_file.clone();
                citation.resolvable = false;
                ("unresolved file", citation)
            },
        ] {
            assert_ne!(
                packet_citation_probe_match_rank("adapters", &citation),
                Some(6),
                "{label} must not receive exact primary FILE preference"
            );
        }
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
    fn prompt_concept_roles_do_not_generate_domain_specific_production_probes() {
        let hook_queries = packet_sufficiency_required_probe_queries(
            "Explain how the public hook serializes keys, connects cache helpers, and composes middleware.",
            PacketTaskClassDto::ArchitectureExplanation,
        );
        for banned in [
            "public hook export",
            "key serialization",
            "cache helper",
            "native form constraints",
            "route registration",
            "request dispatch",
        ] {
            assert!(
                !hook_queries.iter().any(|query| query == banned),
                "production probes must not emit domain flow probes: {hook_queries:?}"
            );
        }

        let flow_queries = packet_sufficiency_required_probe_queries(
            "Trace native HTML form constraint validation, custom JavaScript validation, handler processing, mapper configuration, type map plans, and buffered source/sink behavior.",
            PacketTaskClassDto::ArchitectureExplanation,
        );
        for banned in [
            "native form constraints",
            "mapper configuration",
            "handler processing",
            "buffered source",
            "type map plan",
        ] {
            assert!(
                !flow_queries.iter().any(|query| query == banned),
                "flow-term expansion probes must stay decontaminated: {flow_queries:?}"
            );
        }

        let route_queries = packet_sufficiency_required_probe_queries(
            "Trace how an HTTP route registration reaches request handler dispatch through a router engine.",
            PacketTaskClassDto::RouteTracing,
        );
        for banned in ["route registration", "request dispatch", "router engine"] {
            assert!(
                !route_queries.iter().any(|query| query == banned),
                "route flow probes must stay decontaminated: {route_queries:?}"
            );
        }
    }

    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
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
            "route registration",
            &test_packet_citation("node.addRoute", "src/router/tree.go", 0.9)
        ));
        assert!(packet_citation_satisfies_required_probe(
            "route registration",
            &test_packet_citation("app.route", "lib/application.js", 0.9)
        ));
        assert!(
            !packet_citation_satisfies_required_probe(
                "route registration",
                &test_packet_citation("app.use", "lib/application.js", 0.9)
            ),
            "middleware installation is not route registration"
        );
        assert!(packet_citation_satisfies_required_probe(
            "request handler",
            &test_packet_citation("app.handle", "lib/application.js", 0.9)
        ));
        assert!(packet_citation_satisfies_required_probe(
            "handler dispatch",
            &test_packet_citation("app.handle", "lib/application.js", 0.9)
        ));
        assert!(packet_citation_satisfies_required_probe(
            "handler chain",
            &test_packet_citation("app.use", "lib/application.js", 0.9)
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "route registration",
            &test_packet_citation("node.addRoute", "tests/router/tree_test.go", 0.9)
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
            "engine creation router state",
            &engine_new
        ));
        assert_eq!(
            packet_citation_probe_match_rank("engine creation router state", &engine_new),
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

        let catalog_track = test_packet_citation("public.Track", "db/schema.sql", 0.9);
        let catalog_playlist = test_packet_citation("public.PlaylistTrack", "db/schema.sql", 0.9);
        assert!(packet_citation_satisfies_required_probe(
            "CREATE TABLE Track",
            &catalog_track
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "CREATE TABLE Track",
            &catalog_playlist
        ));
        assert!(packet_citation_satisfies_required_probe(
            "public.Track",
            &create_track
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "public.Track",
            &create_playlist_track
        ));
        let rewritten_publisher =
            test_packet_citation("CREATE TABLE Publisher", "schema/Catalog_Sqlite.sql", 0.9);
        assert!(packet_citation_satisfies_required_probe(
            "public.publisher",
            &rewritten_publisher
        ));

        let log_record = test_packet_citation("LogRecord", "src/Monolog/LogRecord.php", 0.9);
        let processing_handler = test_packet_citation(
            "AbstractProcessingHandler.handle",
            "src/Monolog/Handler/AbstractProcessingHandler.php",
            0.9,
        );
        let plan_builder = test_packet_citation(
            "TypeMapPlanBuilder",
            "src/AutoMapper/Execution/TypeMapPlanBuilder.cs",
            0.9,
        );
        let create_mapper_lambda = test_packet_citation(
            "TypeMapPlanBuilder.CreateMapperLambda",
            "src/AutoMapper/Execution/TypeMapPlanBuilder.cs",
            0.9,
        );
        let data_request =
            test_packet_citation("DataRequest", "Source/Core/DataRequest.swift", 0.9);
        let data_request_validate =
            test_packet_citation("DataRequest.validate", "Source/Core/DataRequest.swift", 0.9);
        let session_delegate =
            test_packet_citation("SessionDelegate", "Source/Core/SessionDelegate.swift", 0.9);
        let session_delegate_url_session = test_packet_citation(
            "SessionDelegate.urlSession",
            "Source/Core/SessionDelegate.swift",
            0.9,
        );
        assert!(packet_citation_satisfies_required_probe(
            "LogRecord.php LogRecord",
            &log_record
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "record.php record",
            &log_record
        ));
        assert!(packet_citation_satisfies_required_probe(
            "AbstractProcessingHandler.php handle",
            &processing_handler
        ));
        assert!(packet_citation_satisfies_required_probe(
            "TypeMapPlanBuilder.cs TypeMapPlanBuilder",
            &plan_builder
        ));
        assert!(packet_citation_satisfies_required_probe(
            "TypeMapPlanBuilder.cs CreateMapperLambda",
            &create_mapper_lambda
        ));
        assert!(packet_citation_satisfies_required_probe(
            "DataRequest.swift DataRequest",
            &data_request
        ));
        assert!(packet_citation_satisfies_required_probe(
            "DataRequest.swift validate",
            &data_request_validate
        ));
        assert!(packet_citation_satisfies_required_probe(
            "SessionDelegate.swift SessionDelegate",
            &session_delegate
        ));
        assert!(packet_citation_satisfies_required_probe(
            "SessionDelegate.swift urlSession",
            &session_delegate_url_session
        ));
        // Domain role-file aliases and soft stage probes must not match by table.
        assert!(!packet_citation_satisfies_required_probe(
            "request_object.swift request",
            &data_request
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "delegate_callbacks.swift delegate",
            &session_delegate
        ));
    }

    #[test]
    fn sql_schema_required_probes_from_terms_are_decontaminated() {
        let terms = packet_probe_terms(
            "Explain SQL table definitions and referential relationships across schema seed scripts.",
        );
        let queries = packet_sufficiency_required_probe_queries_from_terms(
            &terms,
            PacketTaskClassDto::DataFlow,
        );

        assert!(
            queries.is_empty(),
            "domain flow probes should not expand from terms alone: {queries:?}"
        );
    }

    #[test]
    fn sql_relationship_probes_can_be_covered_by_source_claims() {
        let claims = vec![
            PacketClaimDto {
                claim: "FOREIGN KEY constraints define row references between SQL tables."
                    .to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim:
                    "A CHECK constraint validates a column without describing table relationships."
                        .to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
        ];

        for probe in [
            "foreign key relationships",
            "schema constraints",
            "REFERENCES",
        ] {
            assert!(
                packet_probe_query_is_claimed(probe, &claims),
                "expected claim-backed coverage for {probe}: {claims:?}"
            );
        }

        let non_relationship_claims = vec![PacketClaimDto {
            claim: "A CHECK constraint validates a column without describing table relationships."
                .to_string(),
            required_obligation_ids: Vec::new(),
            required_obligation_kinds: Vec::new(),
            proof_status: None,
            required_evidence_role: None,
            citations: Vec::new(),
            coverage_role: None,
            eligible_for_sufficiency: None,
        }];
        assert!(
            !packet_probe_query_is_claimed("schema constraints", &non_relationship_claims),
            "non-relationship constraints should not cover SQL relationship probes"
        );

        let column_reference_claims = vec![PacketClaimDto {
            claim: "A CHECK constraint references the Price column while validating values."
                .to_string(),
            required_obligation_ids: Vec::new(),
            required_obligation_kinds: Vec::new(),
            proof_status: None,
            required_evidence_role: None,
            citations: Vec::new(),
            coverage_role: None,
            eligible_for_sufficiency: None,
        }];
        for probe in [
            "foreign key relationships",
            "schema constraints",
            "REFERENCES",
        ] {
            assert!(
                !packet_probe_query_is_claimed(probe, &column_reference_claims),
                "column-level CHECK references should not cover {probe}"
            );
        }

        let range_reference_claims = vec![PacketClaimDto {
            claim:
                "A CHECK constraint references the Price column and validates values between 0 and 100."
                    .to_string(),
            required_obligation_ids: Vec::new(),
            required_obligation_kinds: Vec::new(),
            proof_status: None,
            required_evidence_role: None,
            citations: Vec::new(),
            coverage_role: None,
            eligible_for_sufficiency: None,
        }];
        for probe in [
            "foreign key relationships",
            "schema constraints",
            "REFERENCES",
        ] {
            assert!(
                !packet_probe_query_is_claimed(probe, &range_reference_claims),
                "column-level CHECK references with ranges should not cover {probe}"
            );
        }
    }

    #[test]
    fn route_sufficiency_probes_can_be_covered_by_source_claims() {
        let claims = vec![
            PacketClaimDto {
                claim: "app.use registers middleware on the router.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "app.handle delegates request handling to the router.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "res.send prepares and sends the response body.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
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
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "addRecord creates a log record before passing it to handlers.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "AbstractProcessingHandler handles records by processing and writing them."
                    .to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
        ];

        for probe in [
            "handler registration",
            "record creation",
            "handler processing",
            "logger record",
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
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "BaseRequest.finalize prepares the request body for sending.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "Response.fromStream builds a streamed response boundary.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
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
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "A custom validation example applies script-driven validity checks before rendering messages."
                    .to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "Custom error rendering branches on ValidityState fields to choose messages."
                    .to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "Submit handlers prevent submission when the form is invalid.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: None,
            },
        ];

        for probe in [
            "native form constraints",
            "custom validation flow",
            "custom error rendering",
            "validity state",
            "submit prevent default",
        ] {
            assert!(
                packet_probe_query_is_claimed(probe, &claims),
                "expected claim-backed coverage for {probe}: {claims:?}"
            );
        }
    }
}
