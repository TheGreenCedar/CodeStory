use codestory_contracts::api::{NodeKind, SearchHit, SearchHitOrigin};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SymbolNameMatchRank {
    pub exact_display: u8,
    pub exact_terminal: u8,
    pub exact_leading: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SearchMatchRank {
    full_definition: u8,
    definition_quality: u8,
    qualified_prefix_path: u8,
    exact_display: u8,
    exact_terminal: u8,
    exact_leading: u8,
    source_bucket: u8,
    camel_case_match: u8,
    compound_term_match: u8,
    path_term_match: u8,
    architecture_role_intent: u8,
    architecture_source_bucket: u8,
    query_kind_intent: u8,
    query_entrypoint_intent: u8,
    kind_bucket: u8,
    kind_tiebreak: u8,
    indexed_symbol: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalFileRole {
    Source,
    Test,
    Docs,
    Benchmark,
    Generated,
    Vendor,
}

impl RetrievalFileRole {
    pub fn is_non_primary(self) -> bool {
        !matches!(self, Self::Source)
    }
}

pub fn normalize_symbol_query(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

pub fn terminal_symbol_segment(value: &str) -> String {
    value
        .rsplit([':', '.', '/', '\\'])
        .next()
        .map(normalize_symbol_query)
        .unwrap_or_default()
}

pub fn leading_symbol_segment(value: &str) -> String {
    value
        .split("::")
        .next()
        .map(normalize_symbol_query)
        .unwrap_or_default()
}

pub fn symbol_query_tokens(value: &str) -> Vec<String> {
    let normalized = value.replace("::", " ").replace("->", " ").replace(
        [
            '.', '#', '/', '\\', '_', '-', ':', '<', '>', '(', ')', '[', ']', '{', '}',
        ],
        " ",
    );
    normalized
        .split_whitespace()
        .flat_map(split_identifier_segment)
        .filter(|token| !token.is_empty())
        .collect()
}

fn split_identifier_segment(segment: &str) -> Vec<String> {
    let chars = segment.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut current = String::new();

    for (idx, ch) in chars.iter().copied().enumerate() {
        if !ch.is_ascii_alphanumeric() {
            if !current.is_empty() {
                tokens.push(current.to_ascii_lowercase());
                current.clear();
            }
            continue;
        }

        let prev = idx.checked_sub(1).and_then(|prev| chars.get(prev)).copied();
        let next = chars.get(idx + 1).copied();
        let starts_new_token = !current.is_empty()
            && prev.is_some_and(|prev| {
                (prev.is_ascii_lowercase() && ch.is_ascii_uppercase())
                    || (prev.is_ascii_digit() && ch.is_ascii_alphabetic())
                    || (prev.is_ascii_alphabetic() && ch.is_ascii_digit())
                    || (prev.is_ascii_uppercase()
                        && ch.is_ascii_uppercase()
                        && next.is_some_and(|next| next.is_ascii_lowercase()))
            });
        if starts_new_token {
            tokens.push(current.to_ascii_lowercase());
            current.clear();
        }
        current.push(ch);
    }

    if !current.is_empty() {
        tokens.push(current.to_ascii_lowercase());
    }

    tokens
}

fn symbol_name_match_rank_single(query: &str, display_name: &str) -> SymbolNameMatchRank {
    let query = normalize_symbol_query(query);
    let display = normalize_symbol_query(display_name);
    let terminal = terminal_symbol_segment(display_name);
    let leading = leading_symbol_segment(display_name);

    SymbolNameMatchRank {
        exact_display: u8::from(display == query),
        exact_terminal: u8::from(terminal == query),
        exact_leading: u8::from(leading == query),
    }
}

fn best_symbol_name_match(query: &str, display_name: &str) -> (SymbolNameMatchRank, String) {
    let trimmed = trim_symbol_candidate(query);
    let mut best_query = trimmed.to_string();
    let mut best_rank = symbol_name_match_rank_single(trimmed, display_name);

    for term in exact_symbol_query_terms(query) {
        let rank = symbol_name_match_rank_single(&term, display_name);
        if rank > best_rank {
            best_rank = rank;
            best_query = term;
        }
    }

    (best_rank, best_query)
}

pub fn symbol_name_match_rank(query: &str, display_name: &str) -> SymbolNameMatchRank {
    best_symbol_name_match(query, display_name).0
}

pub(crate) fn exact_symbol_query_terms(query: &str) -> Vec<String> {
    let trimmed = trim_symbol_candidate(query);
    if looks_like_standalone_symbol_query(trimmed) {
        let mut terms = Vec::new();
        let mut seen = HashSet::new();
        push_exact_symbol_query_term(trimmed, &mut terms, &mut seen);
        if let Some((_, terminal)) = qualified_symbol_query_parts(trimmed) {
            push_exact_symbol_query_term(terminal, &mut terms, &mut seen);
        }
        return terms;
    }

    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    let mut candidate = String::new();
    for ch in query.chars() {
        if is_symbol_query_char(ch) {
            candidate.push(ch);
            continue;
        }
        push_embedded_symbol_candidate(&candidate, &mut terms, &mut seen);
        candidate.clear();
    }
    push_embedded_symbol_candidate(&candidate, &mut terms, &mut seen);
    terms
}

fn push_exact_symbol_query_term(raw: &str, terms: &mut Vec<String>, seen: &mut HashSet<String>) {
    let candidate = trim_symbol_candidate(raw);
    if !looks_like_standalone_symbol_query(candidate) {
        return;
    }
    let normalized = normalize_symbol_query(candidate);
    if seen.insert(normalized) {
        terms.push(candidate.to_string());
    }
}

pub(crate) fn looks_like_standalone_symbol_query(query: &str) -> bool {
    let trimmed = trim_symbol_candidate(query);
    !trimmed.is_empty()
        && !trimmed.chars().any(char::is_whitespace)
        && trimmed.chars().any(|ch| ch.is_ascii_alphabetic())
        && trimmed.chars().all(is_symbol_query_char)
}

/// Natural-language prompt with embedded symbol-like tokens (not a standalone symbol query).
#[cfg(test)]
pub(crate) fn mixed_natural_language_query(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() || looks_like_standalone_symbol_query(trimmed) {
        return false;
    }
    if !trimmed.contains(char::is_whitespace) {
        return false;
    }
    !exact_symbol_query_terms(query).is_empty()
}

fn push_embedded_symbol_candidate(raw: &str, terms: &mut Vec<String>, seen: &mut HashSet<String>) {
    let candidate = trim_symbol_candidate(raw);
    if terms.len() >= 8
        || !looks_like_standalone_symbol_query(candidate)
        || !has_embedded_exact_symbol_signal(candidate)
    {
        return;
    }

    let normalized = normalize_symbol_query(candidate);
    if seen.insert(normalized) {
        terms.push(candidate.to_string());
    }
}

fn trim_symbol_candidate(value: &str) -> &str {
    value
        .trim()
        .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'))
}

fn is_symbol_query_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '.' | '$')
}

fn has_embedded_exact_symbol_signal(value: &str) -> bool {
    value.contains('_')
        || value.contains("::")
        || value.contains('.')
        || value.contains('$')
        || value.chars().skip(1).any(|ch| ch.is_ascii_uppercase())
}

fn qualified_symbol_query_parts(query: &str) -> Option<(&str, &str)> {
    let trimmed = trim_symbol_candidate(query);
    let index = trimmed.rfind("::")?;
    let prefix = trimmed[..index].trim();
    let terminal = trimmed[index + 2..].trim();
    if prefix.is_empty() || terminal.is_empty() {
        return None;
    }
    Some((prefix, terminal))
}

pub fn retrieval_file_role_from_path(path: &str) -> RetrievalFileRole {
    let normalized_raw = normalize_retrieval_path(path);
    let normalized = strip_materialized_repo_cache_prefix(&normalized_raw).to_string();
    let marked = format!("/{normalized}");
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());

    if path_contains_any(
        &marked,
        &[
            "/node_modules/",
            "/src/external/",
            "/external/",
            "/deps/",
            "/vendor/",
            "/vendors/",
            "/third_party/",
            "/third-party/",
        ],
    ) {
        return RetrievalFileRole::Vendor;
    }

    if path_contains_any(&marked, &["/target/", "/dist/", "/build/", "/generated/"])
        || marked.contains("/schema/typescript/")
        || marked.contains(".generated.")
        || file_name.contains("generated")
        || file_name.contains("payload-types")
        || file_name.ends_with(".g.cs")
    {
        return RetrievalFileRole::Generated;
    }

    if path_contains_any(
        &marked,
        &["/benches/", "/bench/", "/benchmarks/", "/benchmark/"],
    ) || (marked.contains("/scripts/")
        && (marked.contains("bench") || marked.contains("benchmark")))
    {
        return RetrievalFileRole::Benchmark;
    }

    if path_contains_any(
        &marked,
        &[
            "/bin/test/",
            "/test/data/",
            "/tests/",
            "/test/",
            "/spec/",
            "/fixtures/",
            "/fixture/",
            "/examples/",
            "/example/",
            "/__tests__/",
            "/__test__/",
            "-test-client/",
            "_test_client/",
        ],
    ) || file_name.contains(".test.")
        || file_name.contains(".spec.")
        || file_name.ends_with("_test.rs")
        || file_name.ends_with("_tests.rs")
        || file_name.ends_with("_test.py")
        || file_name.ends_with("_tests.py")
        || file_name.ends_with("_test.ts")
        || file_name.ends_with("_tests.ts")
        || file_name.ends_with("_test.tsx")
        || file_name.ends_with("_tests.tsx")
        || file_name.ends_with("test.java")
        || file_name.ends_with("tests.java")
    {
        return RetrievalFileRole::Test;
    }

    if path_contains_any(&marked, &["/docs/", "/doc/"])
        || matches!(file_name, "readme.md" | "changelog.md")
    {
        return RetrievalFileRole::Docs;
    }

    RetrievalFileRole::Source
}

pub fn retrieval_file_role_for_hit(hit: &SearchHit) -> RetrievalFileRole {
    if hit.display_name.starts_with("tests::") {
        return RetrievalFileRole::Test;
    }
    hit.file_path
        .as_deref()
        .map(retrieval_file_role_from_path)
        .unwrap_or(RetrievalFileRole::Source)
}

fn normalize_retrieval_path(path: &str) -> String {
    path.trim_start_matches("\\\\?\\")
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_ascii_lowercase()
}

fn strip_materialized_repo_cache_prefix(path: &str) -> &str {
    let mut best_match: Option<(usize, &str)> = None;
    for marker in ["/source/repos/", "source/repos/", "/repos/", "repos/"] {
        let Some(index) = path.rfind(marker) else {
            continue;
        };
        let after_marker = &path[index + marker.len()..];
        if let Some((_, repo_relative)) = after_marker.split_once('/')
            && !repo_relative.is_empty()
            && best_match
                .as_ref()
                .is_none_or(|(best_index, _)| index > *best_index)
        {
            best_match = Some((index, repo_relative));
        }
    }
    best_match
        .map(|(_, repo_relative)| repo_relative)
        .unwrap_or(path)
}

fn path_contains_any(path: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| path.contains(marker))
}

pub fn compare_ranked_hits<T: Ord>(
    left: &SearchHit,
    right: &SearchHit,
    left_rank: T,
    right_rank: T,
) -> Ordering {
    right_rank
        .cmp(&left_rank)
        .then_with(|| right.score.total_cmp(&left.score))
        .then_with(|| left.display_name.len().cmp(&right.display_name.len()))
        .then_with(|| left.display_name.cmp(&right.display_name))
}

fn search_kind_bucket(kind: NodeKind, origin: SearchHitOrigin) -> u8 {
    if origin == SearchHitOrigin::TextMatch {
        return 0;
    }

    match kind {
        NodeKind::MODULE
        | NodeKind::NAMESPACE
        | NodeKind::PACKAGE
        | NodeKind::STRUCT
        | NodeKind::CLASS
        | NodeKind::INTERFACE
        | NodeKind::ENUM
        | NodeKind::UNION
        | NodeKind::TYPEDEF => 3,
        NodeKind::FUNCTION
        | NodeKind::METHOD
        | NodeKind::MACRO
        | NodeKind::FIELD
        | NodeKind::VARIABLE
        | NodeKind::GLOBAL_VARIABLE
        | NodeKind::CONSTANT
        | NodeKind::ENUM_CONSTANT => 2,
        NodeKind::UNKNOWN => 0,
        _ => 1,
    }
}

fn search_kind_tiebreak(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::FUNCTION => 4,
        NodeKind::METHOD => 3,
        NodeKind::MACRO => 2,
        NodeKind::FIELD
        | NodeKind::VARIABLE
        | NodeKind::GLOBAL_VARIABLE
        | NodeKind::CONSTANT
        | NodeKind::ENUM_CONSTANT => 1,
        _ => 0,
    }
}

fn inexact_search_kind_bucket(kind: NodeKind, origin: SearchHitOrigin) -> u8 {
    if origin == SearchHitOrigin::TextMatch {
        return 0;
    }

    match kind {
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO => 3,
        NodeKind::FIELD
        | NodeKind::VARIABLE
        | NodeKind::GLOBAL_VARIABLE
        | NodeKind::CONSTANT
        | NodeKind::ENUM_CONSTANT => 2,
        NodeKind::MODULE
        | NodeKind::NAMESPACE
        | NodeKind::PACKAGE
        | NodeKind::STRUCT
        | NodeKind::CLASS
        | NodeKind::INTERFACE
        | NodeKind::ENUM
        | NodeKind::UNION
        | NodeKind::TYPEDEF => 1,
        NodeKind::UNKNOWN => 0,
        _ => 1,
    }
}

fn is_type_like_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::ENUM
            | NodeKind::UNION
            | NodeKind::TYPEDEF
            | NodeKind::TYPE_PARAMETER
    )
}

fn query_mentions_type_role(query: &str) -> bool {
    let mut previous_was_data = false;
    for term in query.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_') {
        let term = term.to_ascii_lowercase();
        if matches!(
            term.as_str(),
            "struct" | "record" | "class" | "interface" | "enum" | "type" | "typedef"
        ) || (previous_was_data && term == "type")
        {
            return true;
        }
        previous_was_data = term == "data";
    }
    false
}

fn query_kind_intent_bucket(query: &str, kind: NodeKind, is_exact_match: bool) -> u8 {
    if is_exact_match {
        return 0;
    }
    u8::from(query_mentions_type_role(query) && is_type_like_kind(kind))
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter_map(|term| {
            let normalized = term.trim().to_ascii_lowercase();
            (!normalized.is_empty()).then_some(normalized)
        })
        .collect()
}

fn camel_case_initials(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn compact_alphanumeric(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn camel_case_match_bucket(query: &str, display_name: &str, is_exact_match: bool) -> u8 {
    if is_exact_match {
        return 0;
    }
    let compact_query = compact_alphanumeric(query);
    if compact_query.len() < 2 {
        return 0;
    }
    let terminal = display_name
        .rsplit([':', '.', '/', '\\'])
        .next()
        .unwrap_or(display_name);
    let initials = camel_case_initials(terminal);
    u8::from(!initials.is_empty() && initials == compact_query)
}

fn compound_term_match_bucket(query: &str, display_name: &str, is_exact_match: bool) -> u8 {
    if is_exact_match {
        return 0;
    }
    let terms = query_terms(query);
    if terms.len() < 2 {
        return 0;
    }
    let compact_query = terms.join("");
    let compact_display = compact_alphanumeric(display_name);
    u8::from(!compact_query.is_empty() && compact_display.contains(&compact_query))
}

fn path_term_match_bucket(query: &str, hit: &SearchHit, is_exact_match: bool) -> u8 {
    if is_exact_match {
        return 0;
    }
    let Some(path) = hit.file_path.as_deref() else {
        return 0;
    };
    let terms = query_terms(query)
        .into_iter()
        .filter(|term| term.len() > 2)
        .collect::<Vec<_>>();
    if terms.is_empty() {
        return 0;
    }
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    u8::from(terms.iter().any(|term| normalized_path.contains(term)))
}

fn terms_contain_phrase(terms: &[String], phrase: &[&str]) -> bool {
    terms
        .windows(phrase.len())
        .any(|window| window.iter().map(String::as_str).eq(phrase.iter().copied()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchitectureQueryIntent {
    Orchestration,
    Pipeline,
    Entrypoint,
    Handoff,
    CliRuntime,
    PageRender,
    DataLoader,
    Auth,
    Feed,
    Persistence,
}

impl ArchitectureQueryIntent {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Orchestration => "orchestration",
            Self::Pipeline => "pipeline",
            Self::Entrypoint => "entrypoint",
            Self::Handoff => "handoff",
            Self::CliRuntime => "cli-runtime",
            Self::PageRender => "page-render",
            Self::DataLoader => "data-loader",
            Self::Auth => "auth",
            Self::Feed => "feed",
            Self::Persistence => "persistence",
        }
    }
}

pub(crate) fn architecture_query_intents(query: &str) -> Vec<ArchitectureQueryIntent> {
    let terms = query_terms(query);
    let mut intents = Vec::new();
    if terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "architecture"
                | "architectural"
                | "orchestration"
                | "orchestrator"
                | "orchestrate"
                | "coordinates"
                | "coordinate"
                | "workflow"
                | "flow"
                | "flows"
                | "connect"
                | "connects"
                | "connected"
        )
    }) || terms_contain_phrase(&terms, &["explain", "how"])
    {
        intents.push(ArchitectureQueryIntent::Orchestration);
    }
    if terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "pipeline" | "pipelines" | "stage" | "staged" | "staging"
        )
    }) {
        intents.push(ArchitectureQueryIntent::Pipeline);
    }
    if terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "entrypoint" | "entrypoints" | "entry" | "main" | "start" | "starts"
        )
    }) || terms_contain_phrase(&terms, &["entry", "point"])
    {
        intents.push(ArchitectureQueryIntent::Entrypoint);
    }
    if terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "handoff" | "handoffs" | "dispatch" | "dispatches" | "bridge" | "bridges"
        )
    }) || terms_contain_phrase(&terms, &["hands", "off"])
        || terms_contain_phrase(&terms, &["hand", "off"])
    {
        intents.push(ArchitectureQueryIntent::Handoff);
    }
    if terms
        .iter()
        .any(|term| matches!(term.as_str(), "cli" | "runtime" | "command" | "commands"))
        || terms_contain_phrase(&terms, &["cli", "runtime"])
    {
        intents.push(ArchitectureQueryIntent::CliRuntime);
    }
    if terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "page" | "render" | "renders" | "rendering" | "route" | "routes"
        )
    }) {
        intents.push(ArchitectureQueryIntent::PageRender);
    }
    if terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "loader" | "loaders" | "load" | "loads" | "fetch" | "fetches"
        )
    }) || terms_contain_phrase(&terms, &["data", "loader"])
    {
        intents.push(ArchitectureQueryIntent::DataLoader);
    }
    if terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "auth" | "authentication" | "authorization" | "session" | "login"
        )
    }) {
        intents.push(ArchitectureQueryIntent::Auth);
    }
    if terms
        .iter()
        .any(|term| matches!(term.as_str(), "feed" | "feeds" | "social" | "timeline"))
    {
        intents.push(ArchitectureQueryIntent::Feed);
    }
    if terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "persistence"
                | "persist"
                | "persists"
                | "storage"
                | "store"
                | "stored"
                | "snapshot"
                | "publish"
                | "publishes"
                | "save"
                | "saves"
                | "write"
                | "writes"
        )
    }) {
        intents.push(ArchitectureQueryIntent::Persistence);
    }
    intents
}

fn architecture_query_has_intent(query: &str) -> bool {
    !architecture_query_intents(query).is_empty()
}

fn architecture_candidate_source_bucket(query: &str, hit: &SearchHit, is_exact_match: bool) -> u8 {
    if is_exact_match || !architecture_query_has_intent(query) {
        return 1;
    }
    if is_non_primary_source_hit(hit) {
        return 0;
    }
    if architecture_helper_like_hit(hit) {
        return 1;
    }
    2
}

fn architecture_helper_like_hit(hit: &SearchHit) -> bool {
    let text = format!(
        "{} {}",
        hit.display_name.to_ascii_lowercase(),
        hit.file_path
            .as_deref()
            .unwrap_or_default()
            .replace('\\', "/")
            .to_ascii_lowercase()
    );
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|term| {
            matches!(
                term,
                "helper" | "helpers" | "mock" | "mocks" | "fake" | "fixture" | "fixtures"
            )
        })
}

fn architecture_role_intent_bucket(query: &str, hit: &SearchHit, is_exact_match: bool) -> u8 {
    if is_exact_match {
        return 0;
    }
    let intents = architecture_query_intents(query);
    if intents.is_empty() || hit.origin == SearchHitOrigin::TextMatch {
        return 0;
    }
    let candidate_terms = architecture_candidate_terms(hit);
    let mut bucket = 0;
    for intent in intents {
        bucket = bucket.max(architecture_intent_candidate_bucket(
            intent,
            &candidate_terms,
        ));
    }
    bucket
}

fn architecture_candidate_terms(hit: &SearchHit) -> Vec<String> {
    let display = hit.display_name.replace("::", " ").replace('_', " ");
    let mut terms = query_terms(&display);
    if let Some(path) = hit.file_path.as_deref() {
        let path = path.replace('\\', "/").replace(['/', '.', '-', '_'], " ");
        terms.extend(query_terms(&path));
    }
    terms
}

fn architecture_intent_candidate_bucket(intent: ArchitectureQueryIntent, terms: &[String]) -> u8 {
    let any = |needles: &[&str]| {
        terms
            .iter()
            .any(|term| needles.iter().any(|needle| term == needle))
    };
    let phrase = |needles: &[&str]| terms_contain_phrase(terms, needles);
    match intent {
        ArchitectureQueryIntent::Orchestration => u8::from(any(&[
            "orchestrate",
            "orchestrator",
            "run",
            "execute",
            "dispatch",
            "handle",
            "workflow",
            "controller",
            "manager",
            "service",
        ])),
        ArchitectureQueryIntent::Pipeline => u8::from(any(&[
            "pipeline", "stage", "staged", "prepare", "build", "finalize", "index", "publish",
            "run",
        ])),
        ArchitectureQueryIntent::Entrypoint => u8::from(
            any(&["main", "run", "start", "open", "handle", "index_full"])
                || phrase(&["index", "full"]),
        ),
        ArchitectureQueryIntent::Handoff => u8::from(any(&[
            "handoff", "dispatch", "route", "bridge", "ensure", "open", "call", "handle", "execute",
        ])),
        ArchitectureQueryIntent::CliRuntime => u8::from(any(&[
            "cli", "runtime", "command", "run", "ensure", "open", "handler", "stdio", "snippet",
            "trail", "search", "index",
        ])),
        ArchitectureQueryIntent::PageRender => u8::from(any(&[
            "page",
            "render",
            "route",
            "layout",
            "component",
            "home",
            "view",
        ])),
        ArchitectureQueryIntent::DataLoader => u8::from(any(&[
            "load", "loader", "get", "fetch", "query", "select", "find", "read",
        ])),
        ArchitectureQueryIntent::Auth => u8::from(any(&[
            "auth",
            "authentication",
            "authorization",
            "session",
            "login",
            "user",
            "verify",
        ])),
        ArchitectureQueryIntent::Feed => u8::from(any(&[
            "feed", "feeds", "social", "sync", "entry", "entries",
        ])),
        ArchitectureQueryIntent::Persistence => u8::from(any(&[
            "persist",
            "persistence",
            "store",
            "storage",
            "snapshot",
            "publish",
            "save",
            "write",
            "flush",
            "commit",
            "upsert",
            "insert",
            "finalize",
            "staged",
        ])),
    }
}

fn query_entrypoint_intent_bucket(query: &str, display_name: &str, is_exact_match: bool) -> u8 {
    if is_exact_match {
        return 0;
    }
    let terms = query_terms(query);
    let terminal = terminal_symbol_segment(display_name);

    u8::from(
        (terminal == "node_details" && terms_contain_phrase(&terms, &["node", "details"]))
            || (terminal == "source_files" && terms_contain_phrase(&terms, &["source", "files"]))
            || (terminal == "compare_resolution_hits"
                && terms_contain_phrase(&terms, &["compare", "resolution", "hits"]))
            || (terminal == "file_text_match_line"
                && terms_contain_phrase(&terms, &["file", "text", "match", "line"]))
            || (terminal == "parse"
                && terms_contain_phrase(&terms, &["endpoint"])
                && terms
                    .iter()
                    .any(|term| matches!(term.as_str(), "url" | "env" | "environment"))),
    )
}

pub(crate) fn query_mentions_non_primary_source(query: &str) -> bool {
    let terms = query
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .map(|term| term.to_ascii_lowercase())
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();

    terms.iter().enumerate().any(|(index, term)| {
        is_non_primary_source_term(term) && !is_non_primary_source_exclusion_context(&terms, index)
    })
}

pub(crate) fn is_non_primary_source_term(term: &str) -> bool {
    matches!(
        term,
        "test"
            | "tests"
            | "testing"
            | "doc"
            | "docs"
            | "documentation"
            | "example"
            | "examples"
            | "sample"
            | "samples"
            | "script"
            | "scripts"
            | "bench"
            | "benchmark"
            | "benchmarks"
            | "fixture"
            | "fixtures"
            | "external"
            | "vendor"
            | "vendors"
            | "vendored"
            | "generated"
            | "thirdparty"
            | "third_party"
            | "third-party"
    )
}

fn is_non_primary_source_exclusion_context(terms: &[String], index: usize) -> bool {
    let start = index.saturating_sub(8);
    let end = (index + 9).min(terms.len());
    terms[start..end].iter().any(|term| {
        matches!(
            term.as_str(),
            "avoid"
                | "demote"
                | "demotes"
                | "demoted"
                | "downrank"
                | "downranking"
                | "exclude"
                | "excluding"
                | "hide"
                | "ignore"
                | "omit"
                | "pollute"
                | "pollution"
                | "precision"
                | "primary"
                | "prod"
                | "production"
                | "role"
                | "roles"
                | "skip"
                | "without"
        )
    })
}

pub(crate) fn is_non_primary_source_hit(hit: &SearchHit) -> bool {
    retrieval_file_role_for_hit(hit).is_non_primary()
}

fn search_match_rank(project_root: Option<&Path>, query: &str, hit: &SearchHit) -> SearchMatchRank {
    let (rank, matched_symbol_query) = best_symbol_name_match(query, &hit.display_name);
    let is_exact_match =
        rank.exact_display != 0 || rank.exact_terminal != 0 || rank.exact_leading != 0;
    let full_definition = full_definition_bucket(project_root, query, hit);
    let definition_quality =
        exact_definition_quality_bucket(project_root, &matched_symbol_query, hit, is_exact_match);
    let qualified_prefix_path = qualified_prefix_path_bucket(query, hit);
    let source_bucket = u8::from(
        is_exact_match
            || query_mentions_non_primary_source(query)
            || !is_non_primary_source_hit(hit),
    );
    let kind_bucket = if is_exact_match {
        search_kind_bucket(hit.kind, hit.origin)
    } else {
        inexact_search_kind_bucket(hit.kind, hit.origin)
    };
    let query_kind_intent = query_kind_intent_bucket(query, hit.kind, is_exact_match);
    let query_entrypoint_intent =
        query_entrypoint_intent_bucket(query, &hit.display_name, is_exact_match);
    let architecture_role_intent = architecture_role_intent_bucket(query, hit, is_exact_match);
    let architecture_source_bucket =
        architecture_candidate_source_bucket(query, hit, is_exact_match);
    let kind_tiebreak = if is_exact_match {
        search_kind_tiebreak(hit.kind)
    } else {
        0
    };

    SearchMatchRank {
        full_definition,
        definition_quality,
        qualified_prefix_path,
        exact_display: rank.exact_display,
        exact_terminal: rank.exact_terminal,
        exact_leading: rank.exact_leading,
        source_bucket,
        camel_case_match: camel_case_match_bucket(query, &hit.display_name, is_exact_match),
        compound_term_match: compound_term_match_bucket(query, &hit.display_name, is_exact_match),
        path_term_match: path_term_match_bucket(query, hit, is_exact_match),
        architecture_role_intent,
        architecture_source_bucket,
        query_kind_intent,
        query_entrypoint_intent,
        kind_bucket,
        kind_tiebreak,
        indexed_symbol: u8::from(hit.origin == SearchHitOrigin::IndexedSymbol),
    }
}

fn full_definition_bucket(project_root: Option<&Path>, query: &str, hit: &SearchHit) -> u8 {
    if normalize_symbol_query(&hit.display_name) != normalize_symbol_query(query) {
        return 0;
    }
    if is_type_like_kind(hit.kind) {
        return type_hit_line_quality(project_root, query, hit);
    }
    let Some(trimmed) = hit_source_line_without_comment(project_root, hit) else {
        return 1;
    };
    if is_import_or_reexport_line(&trimmed) {
        return 0;
    }
    2
}

fn qualified_prefix_path_bucket(query: &str, hit: &SearchHit) -> u8 {
    let Some((prefix, terminal)) = qualified_symbol_query_parts(query) else {
        return 0;
    };
    if terminal_symbol_segment(&hit.display_name) != normalize_symbol_query(terminal) {
        return 0;
    }
    let Some(path) = hit.file_path.as_deref() else {
        return 0;
    };
    let path = path.replace('\\', "/").to_ascii_lowercase();
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return 0;
    }

    let normalized_prefix = normalize_symbol_query(prefix);
    let hyphenated_prefix = normalized_prefix.replace('_', "-");
    if !hyphenated_prefix.is_empty() && segments.iter().any(|segment| *segment == hyphenated_prefix)
    {
        return 3;
    }

    let trailing_prefix_segment = normalized_prefix
        .rsplit('_')
        .next()
        .unwrap_or(normalized_prefix.as_str())
        .replace('_', "-");
    if trailing_prefix_segment.len() >= 3
        && segments
            .iter()
            .any(|segment| *segment == trailing_prefix_segment)
    {
        return 2;
    }

    let compact_prefix = compact_alphanumeric(prefix);
    if compact_prefix.len() >= 3
        && segments
            .iter()
            .any(|segment| compact_alphanumeric(segment) == compact_prefix)
    {
        return 1;
    }

    0
}

fn exact_definition_quality_bucket(
    project_root: Option<&Path>,
    query: &str,
    hit: &SearchHit,
    is_exact_match: bool,
) -> u8 {
    if !is_exact_match || hit.origin == SearchHitOrigin::TextMatch || hit.kind == NodeKind::UNKNOWN
    {
        return 0;
    }
    if is_type_like_kind(hit.kind) {
        return type_hit_line_quality(project_root, query, hit);
    }
    if is_callable_like_kind(hit.kind) {
        return callable_hit_line_quality(project_root, query, hit);
    }
    if matches!(
        hit.kind,
        NodeKind::MODULE | NodeKind::NAMESPACE | NodeKind::PACKAGE
    ) {
        return module_hit_line_quality(project_root, query, hit);
    }
    1
}

fn is_callable_like_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    )
}

fn type_hit_line_quality(project_root: Option<&Path>, query: &str, hit: &SearchHit) -> u8 {
    let Some(path) = hit.file_path.as_deref() else {
        return 1;
    };
    let Some(line) = hit.line else {
        return 1;
    };
    let Some(source_line) = read_source_line(project_root, path, line) else {
        return 1;
    };
    let trimmed = source_line
        .split("//")
        .next()
        .unwrap_or(source_line.as_str())
        .trim();
    let expected_name = terminal_symbol_segment(query);
    if expected_name.is_empty() {
        return 1;
    }
    let tokens = trimmed
        .split(|ch: char| ch.is_whitespace() || ch == ':' || ch == ';' || ch == '{')
        .map(|token| token.trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let Some(type_keyword_index) = tokens
        .iter()
        .position(|token| matches!(*token, "class" | "struct" | "interface" | "enum" | "union"))
    else {
        return 0;
    };
    let Some(type_name) = tokens.get(type_keyword_index + 1).copied() else {
        return 0;
    };
    let direct_type_line = normalize_symbol_query(type_name) == expected_name;
    if !direct_type_line {
        return 0;
    }
    if trimmed.contains('{') || !trimmed.ends_with(';') {
        2
    } else {
        0
    }
}

fn callable_hit_line_quality(project_root: Option<&Path>, query: &str, hit: &SearchHit) -> u8 {
    let Some(trimmed) = hit_source_line_without_comment(project_root, hit) else {
        return 1;
    };
    let expected_name = terminal_symbol_segment(query);
    if expected_name.is_empty() {
        return 1;
    }
    if is_import_or_reexport_line(&trimmed) {
        return 0;
    }
    if !line_contains_symbol_name(&trimmed, &expected_name) {
        return 1;
    }
    if looks_like_callable_declaration(&trimmed) {
        return 1;
    }
    if looks_like_callable_definition(&trimmed, &expected_name) {
        return 2;
    }
    1
}

fn module_hit_line_quality(project_root: Option<&Path>, query: &str, hit: &SearchHit) -> u8 {
    let Some(trimmed) = hit_source_line_without_comment(project_root, hit) else {
        return 1;
    };
    let expected_name = terminal_symbol_segment(query);
    if expected_name.is_empty() {
        return 1;
    }
    if is_import_or_reexport_line(&trimmed) {
        return 0;
    }
    if declares_named_module(&trimmed, &expected_name) {
        return 1;
    }
    u8::from(line_contains_symbol_name(&trimmed, &expected_name))
}

fn hit_source_line_without_comment(project_root: Option<&Path>, hit: &SearchHit) -> Option<String> {
    let path = hit.file_path.as_deref()?;
    let line = hit.line?;
    let source_line = read_source_line(project_root, path, line)?;
    Some(
        source_line
            .split("//")
            .next()
            .unwrap_or(source_line.as_str())
            .trim()
            .to_string(),
    )
}

fn is_import_or_reexport_line(trimmed: &str) -> bool {
    let lower = trimmed.trim_start().to_ascii_lowercase();
    lower.starts_with("use ")
        || lower.starts_with("pub use ")
        || lower.starts_with("import ")
        || lower.starts_with("export {")
        || lower.starts_with("export *")
        || lower.starts_with("export from ")
        || lower.starts_with("from ")
        || lower.contains(" import ")
        || lower.contains(" from ")
}

fn line_contains_symbol_name(trimmed: &str, expected_name: &str) -> bool {
    trimmed
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .any(|token| normalize_symbol_query(token) == expected_name)
}

fn looks_like_callable_declaration(trimmed: &str) -> bool {
    let without_attrs = trimmed.trim_start_matches(|ch: char| ch == '@' || ch.is_whitespace());
    without_attrs.ends_with(';') || without_attrs.ends_with("= 0;")
}

fn looks_like_callable_definition(trimmed: &str, expected_name: &str) -> bool {
    let normalized = normalize_symbol_query(trimmed);
    normalized.contains(&format!("fn {expected_name}"))
        || normalized.contains(&format!("function {expected_name}"))
        || normalized.contains(&format!("def {expected_name}"))
        || normalized.contains(&format!("{expected_name}("))
        || normalized.contains(&format!("{expected_name} ("))
}

fn declares_named_module(trimmed: &str, expected_name: &str) -> bool {
    let normalized = normalize_symbol_query(trimmed);
    normalized.contains(&format!("mod {expected_name}"))
        || normalized.contains(&format!("module {expected_name}"))
        || normalized.contains(&format!("namespace {expected_name}"))
}

fn read_source_line(project_root: Option<&Path>, path: &str, line: u32) -> Option<String> {
    let raw_path = Path::new(path);
    let joined_path;
    let candidate = if raw_path.is_absolute() {
        raw_path
    } else if let Some(root) = project_root {
        joined_path = root.join(raw_path);
        joined_path.as_path()
    } else {
        raw_path
    };

    let contents = fs::read_to_string(candidate)
        .or_else(|_| {
            #[cfg(windows)]
            {
                path.strip_prefix(r"\\?\")
                    .map(fs::read_to_string)
                    .unwrap_or_else(|| fs::read_to_string(path))
            }
            #[cfg(not(windows))]
            {
                fs::read_to_string(path)
            }
        })
        .ok()?;
    contents
        .lines()
        .nth(line.saturating_sub(1) as usize)
        .map(str::to_string)
}

#[cfg(test)]
pub(crate) fn compare_search_hits(query: &str, left: &SearchHit, right: &SearchHit) -> Ordering {
    compare_search_hits_with_project_root(None, query, left, right)
}

pub(crate) fn compare_search_hits_with_project_root(
    project_root: Option<&Path>,
    query: &str,
    left: &SearchHit,
    right: &SearchHit,
) -> Ordering {
    compare_ranked_hits(
        left,
        right,
        search_match_rank(project_root, query, left),
        search_match_rank(project_root, query, right),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::NodeId;
    use tempfile::tempdir;

    fn hit(id: &str, display_name: &str, kind: NodeKind, score: f32) -> SearchHit {
        SearchHit {
            node_id: NodeId(id.to_string()),
            display_name: display_name.to_string(),
            kind,
            file_path: None,
            line: None,
            score,
            origin: SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        }
    }

    fn hit_at_path(
        id: &str,
        display_name: &str,
        kind: NodeKind,
        score: f32,
        path: &str,
    ) -> SearchHit {
        let mut hit = hit(id, display_name, kind, score);
        hit.file_path = Some(path.to_string());
        hit
    }

    fn hit_at_path_line(
        id: &str,
        display_name: &str,
        kind: NodeKind,
        score: f32,
        path: &str,
        line: u32,
    ) -> SearchHit {
        let mut hit = hit_at_path(id, display_name, kind, score, path);
        hit.line = Some(line);
        hit
    }

    #[test]
    fn inexact_queries_use_score_between_callables() {
        let lower_scored_function = hit("lower", "plain_function", NodeKind::FUNCTION, 0.40);
        let higher_scored_method = hit("higher", "Owner::strong_method", NodeKind::METHOD, 0.80);

        let mut hits = [lower_scored_function, higher_scored_method.clone()];
        hits.sort_by(|left, right| compare_search_hits("describe strong behavior", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&higher_scored_method.node_id)
        );
    }

    #[test]
    fn inexact_queries_prefer_callables_over_data_members() {
        let callable = hit("callable", "plain_function", NodeKind::FUNCTION, 0.40);
        let field = hit("field", "Owner::strong_field", NodeKind::FIELD, 0.95);

        let mut hits = [field, callable.clone()];
        hits.sort_by(|left, right| compare_search_hits("describe strong behavior", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&callable.node_id)
        );
    }

    #[test]
    fn inexact_type_role_queries_prefer_type_symbols_over_callables() {
        let refresh_plan = hit("type", "RefreshPlan", NodeKind::STRUCT, 0.40);
        let helper = hit(
            "helper",
            "WorkspaceDiscovery::build_refresh_plan",
            NodeKind::METHOD,
            0.95,
        );

        let mut hits = [helper, refresh_plan.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "struct record data type refresh plan workspace indexing",
                left,
                right,
            )
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&refresh_plan.node_id)
        );
    }

    #[test]
    fn inexact_queries_prefer_named_node_details_entrypoint() {
        let node_details = hit(
            "node_details",
            "GroundingService::node_details",
            NodeKind::METHOD,
            0.40,
        );
        let edge_digest = hit(
            "edge_digest",
            "edge_digest_for_node",
            NodeKind::FUNCTION,
            0.95,
        );

        let mut hits = [edge_digest, node_details.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits("node details source occurrence edge digest", left, right)
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&node_details.node_id)
        );
    }

    #[test]
    fn inexact_queries_prefer_named_source_files_entrypoint() {
        let source_files = hit(
            "source_files",
            "WorkspaceDiscovery::source_files",
            NodeKind::METHOD,
            0.40,
        );
        let language_filter = hit(
            "filter",
            "WorkspaceManifest::should_filter_source_group_language",
            NodeKind::METHOD,
            0.95,
        );

        let mut hits = [language_filter, source_files.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "workspace source files apply language filters and excludes",
                left,
                right,
            )
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&source_files.node_id)
        );
    }

    #[test]
    fn inexact_queries_prefer_compare_resolution_hits_entrypoint() {
        let resolution_hits = hit(
            "resolution_hits",
            "compare_resolution_hits",
            NodeKind::FUNCTION,
            0.40,
        );
        let candidates = hit(
            "candidates",
            "compare_resolution_candidates",
            NodeKind::FUNCTION,
            0.95,
        );

        let mut hits = [candidates, resolution_hits.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "compare resolution hits exact symbol before ambiguous candidates",
                left,
                right,
            )
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&resolution_hits.node_id)
        );
    }

    #[test]
    fn inexact_queries_prefer_file_text_match_line_entrypoint() {
        let file_text_match_line = hit(
            "file_text_match_line",
            "file_text_match_line",
            NodeKind::FUNCTION,
            0.40,
        );
        let excerpt = hit("excerpt", "repo_text_excerpt", NodeKind::FUNCTION, 0.95);

        let mut hits = [excerpt, file_text_match_line.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "file text match line for repo text search terms",
                left,
                right,
            )
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&file_text_match_line.node_id)
        );
    }

    #[test]
    fn detects_architecture_query_intent_buckets() {
        let labels = architecture_query_intents(
            "How does the CLI runtime hand off to the page render data loader auth feed persistence pipeline entrypoint?",
        )
        .into_iter()
        .map(|intent| intent.label())
        .collect::<Vec<_>>();

        for expected in [
            "pipeline",
            "entrypoint",
            "handoff",
            "cli-runtime",
            "page-render",
            "data-loader",
            "auth",
            "feed",
            "persistence",
        ] {
            assert!(
                labels.contains(&expected),
                "missing {expected} in {labels:?}"
            );
        }
    }

    #[test]
    fn architecture_queries_prefer_production_entrypoints_over_helpers() {
        let production = hit_at_path(
            "run_index_once",
            "run_index_once",
            NodeKind::FUNCTION,
            0.40,
            "crates/codestory-cli/src/main.rs",
        );
        let helper = hit_at_path(
            "helper",
            "run_index_once_helper",
            NodeKind::FUNCTION,
            0.98,
            "crates/codestory-cli/src/helpers.rs",
        );
        let test = hit_at_path(
            "test",
            "tests::run_index_once",
            NodeKind::FUNCTION,
            0.99,
            "crates/codestory-cli/tests/index_flow.rs",
        );

        let mut hits = [test, helper, production.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits("CLI runtime full index entrypoint handoff", left, right)
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&production.node_id)
        );
    }

    #[test]
    fn architecture_queries_surface_cross_repo_decisive_candidates() {
        let sourcetrail = hit_at_path(
            "sourcetrail",
            "Project::buildIndex",
            NodeKind::METHOD,
            0.40,
            "src/lib/project/Project.cpp",
        );
        let sourcetrail_helper = hit_at_path(
            "sourcetrail_helper",
            "buildIndexFixture",
            NodeKind::FUNCTION,
            0.99,
            "tests/project/buildIndexFixture.cpp",
        );
        let mut sourcetrail_hits = [sourcetrail_helper, sourcetrail.clone()];
        sourcetrail_hits.sort_by(|left, right| {
            compare_search_hits(
                "project indexing pipeline entrypoint build index",
                left,
                right,
            )
        });
        assert_eq!(
            sourcetrail_hits.first().map(|hit| &hit.node_id),
            Some(&sourcetrail.node_id)
        );

        let rootandruntime = hit_at_path(
            "rootandruntime",
            "getElsewhereFeed",
            NodeKind::FUNCTION,
            0.40,
            "src/lib/social-feed.ts",
        );
        let rootandruntime_helper = hit_at_path(
            "rootandruntime_helper",
            "mockElsewhereFeed",
            NodeKind::FUNCTION,
            0.99,
            "src/lib/__fixtures__/social-feed.ts",
        );
        let mut rootandruntime_hits = [rootandruntime_helper, rootandruntime.clone()];
        rootandruntime_hits.sort_by(|left, right| {
            compare_search_hits("how does the public feed data loader work", left, right)
        });
        assert_eq!(
            rootandruntime_hits.first().map(|hit| &hit.node_id),
            Some(&rootandruntime.node_id)
        );

        let batcave = hit_at_path(
            "batcave",
            "RuntimeStore::snapshot",
            NodeKind::METHOD,
            0.40,
            "src/BatCave.App/src-tauri/src/runtime_store.rs",
        );
        let batcave_test = hit_at_path(
            "batcave_test",
            "RuntimeStore::snapshot_fixture",
            NodeKind::METHOD,
            0.99,
            "src/BatCave.App/src-tauri/tests/runtime_store.rs",
        );
        let mut batcave_hits = [batcave_test, batcave.clone()];
        batcave_hits.sort_by(|left, right| {
            compare_search_hits(
                "runtime persistence snapshot handoff architecture",
                left,
                right,
            )
        });
        assert_eq!(
            batcave_hits.first().map(|hit| &hit.node_id),
            Some(&batcave.node_id)
        );
    }

    #[test]
    fn exact_architecture_named_symbol_is_not_demoted() {
        let exact_test_helper = hit_at_path(
            "exact",
            "entrypoint",
            NodeKind::FUNCTION,
            0.40,
            "crates/codestory-runtime/tests/fixtures.rs",
        );
        let production = hit_at_path(
            "production",
            "runtime_entrypoint",
            NodeKind::FUNCTION,
            0.99,
            "crates/codestory-runtime/src/lib.rs",
        );

        let mut hits = [production, exact_test_helper.clone()];
        hits.sort_by(|left, right| compare_search_hits("entrypoint", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&exact_test_helper.node_id)
        );
    }

    #[test]
    fn qualified_symbol_terms_include_terminal_for_definition_resolution() {
        assert_eq!(
            exact_symbol_query_terms("codex_exec::Cli"),
            vec!["codex_exec::Cli".to_string(), "Cli".to_string()]
        );
        assert_eq!(
            exact_symbol_query_terms("Subcommand::Exec"),
            vec!["Subcommand::Exec".to_string(), "Exec".to_string()]
        );
    }

    #[test]
    fn qualified_symbol_query_prefers_matching_crate_definition_over_import_alias() {
        let temp = tempdir().expect("create temp dir");
        let project_root = temp.path();
        fs::create_dir_all(project_root.join("codex-rs/exec/src")).expect("create exec dirs");
        fs::create_dir_all(project_root.join("codex-rs/file-search/src"))
            .expect("create file-search dirs");
        fs::write(
            project_root.join("codex-rs/exec/src/main.rs"),
            "use codex_exec::Cli;\n",
        )
        .expect("write import");
        fs::write(
            project_root.join("codex-rs/exec/src/cli.rs"),
            "pub struct Cli {\n}\n",
        )
        .expect("write target struct");
        fs::write(
            project_root.join("codex-rs/file-search/src/cli.rs"),
            "pub struct Cli {\n}\n",
        )
        .expect("write distractor struct");

        let import_alias = hit_at_path_line(
            "import",
            "codex_exec::Cli",
            NodeKind::MODULE,
            0.85,
            "codex-rs/exec/src/main.rs",
            1,
        );
        let target_definition = hit_at_path_line(
            "target",
            "Cli",
            NodeKind::STRUCT,
            0.80,
            "codex-rs/exec/src/cli.rs",
            1,
        );
        let higher_scored_distractor = hit_at_path_line(
            "distractor",
            "Cli",
            NodeKind::STRUCT,
            0.99,
            "codex-rs/file-search/src/cli.rs",
            1,
        );

        let mut hits = [
            import_alias,
            higher_scored_distractor,
            target_definition.clone(),
        ];
        hits.sort_by(|left, right| {
            compare_search_hits_with_project_root(
                Some(project_root),
                "codex_exec::Cli",
                left,
                right,
            )
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&target_definition.node_id)
        );
    }

    #[test]
    fn qualified_symbol_query_keeps_full_exact_non_import_definition_first() {
        let temp = tempdir().expect("create temp dir");
        let project_root = temp.path();
        fs::create_dir_all(project_root.join("codex-rs/cli/src")).expect("create cli dirs");
        fs::create_dir_all(project_root.join("codex-rs/core/src")).expect("create core dirs");
        fs::write(
            project_root.join("codex-rs/cli/src/main.rs"),
            "enum Subcommand {\n    Exec(ExecCli),\n}\n",
        )
        .expect("write enum");
        fs::write(
            project_root.join("codex-rs/core/src/exec.rs"),
            "pub fn exec() {}\n",
        )
        .expect("write function");

        let exact_variant = hit_at_path_line(
            "variant",
            "Subcommand::Exec",
            NodeKind::ENUM_CONSTANT,
            0.80,
            "codex-rs/cli/src/main.rs",
            2,
        );
        let terminal_callable = hit_at_path_line(
            "callable",
            "exec",
            NodeKind::FUNCTION,
            0.99,
            "codex-rs/core/src/exec.rs",
            1,
        );

        let mut hits = [terminal_callable, exact_variant.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits_with_project_root(
                Some(project_root),
                "Subcommand::Exec",
                left,
                right,
            )
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&exact_variant.node_id)
        );
    }

    #[test]
    fn inexact_queries_boost_camel_case_symbol_matches() {
        let camel = hit("camel", "SearchQueryAssessmentDto", NodeKind::STRUCT, 0.40);
        let noisy = hit(
            "noisy",
            "search_query_assessment_details",
            NodeKind::FUNCTION,
            0.95,
        );

        let mut hits = [noisy, camel.clone()];
        hits.sort_by(|left, right| compare_search_hits("SQAD", left, right));

        assert_eq!(hits.first().map(|hit| &hit.node_id), Some(&camel.node_id));
    }

    #[test]
    fn inexact_queries_boost_compound_and_path_terms() {
        let compound = hit(
            "compound",
            "collectFrameworkRoutes",
            NodeKind::FUNCTION,
            0.40,
        );
        let unrelated = hit("unrelated", "collect_routes", NodeKind::FUNCTION, 0.95);

        let mut hits = [unrelated, compound.clone()];
        hits.sort_by(|left, right| compare_search_hits("framework routes", left, right));
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&compound.node_id)
        );

        let routed_file = hit_at_path(
            "path",
            "handler",
            NodeKind::FUNCTION,
            0.40,
            "src/framework/routes.rs",
        );
        let high_score = hit_at_path(
            "high",
            "handler",
            NodeKind::FUNCTION,
            0.95,
            "src/service/mod.rs",
        );
        let mut hits = [high_score, routed_file.clone()];
        hits.sort_by(|left, right| compare_search_hits("framework route handler", left, right));
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&routed_file.node_id)
        );
    }

    #[test]
    fn inexact_queries_downrank_tests_unless_requested() {
        let production = hit("production", "plain_function", NodeKind::FUNCTION, 0.40);
        let mut test_hit = hit("test", "tests::strong_case", NodeKind::FUNCTION, 0.95);
        test_hit.file_path = Some("src/module.rs".to_string());

        let mut hits = [test_hit.clone(), production.clone()];
        hits.sort_by(|left, right| compare_search_hits("describe strong behavior", left, right));
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&production.node_id)
        );

        hits.sort_by(|left, right| compare_search_hits("test strong behavior", left, right));
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&test_hit.node_id)
        );

        hits.sort_by(|left, right| {
            compare_search_hits("describe behavior that should hide tests", left, right)
        });
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&production.node_id)
        );
    }

    #[test]
    fn file_role_ranking_queries_do_not_opt_into_non_primary_roles() {
        assert!(!query_mentions_non_primary_source(
            "central file role ranking tests docs bench generated vendor search packet context semantic"
        ));
        assert!(query_mentions_non_primary_source("docs search ranking"));
    }

    #[test]
    fn file_role_classification_catches_colocated_and_helper_tests() {
        assert_eq!(
            retrieval_file_role_from_path(
                "codex-rs/exec/src/event_processor_with_jsonl_output_tests.rs"
            ),
            RetrievalFileRole::Test
        );
        assert_eq!(
            retrieval_file_role_from_path("codex-rs/app-server-test-client/src/lib.rs"),
            RetrievalFileRole::Test
        );
        assert_eq!(
            retrieval_file_role_from_path(
                "codex-rs/app-server-protocol/schema/typescript/index.ts"
            ),
            RetrievalFileRole::Generated
        );
        assert_eq!(
            retrieval_file_role_from_path(
                r"\\?\C:\repo\codestory\target\repo-cache\repos\expressjs-express\lib\response.js"
            ),
            RetrievalFileRole::Source
        );
        assert_eq!(
            retrieval_file_role_from_path("target/generated/client.ts"),
            RetrievalFileRole::Generated
        );
        assert_eq!(
            retrieval_file_role_from_path("redis/deps/hiredis/examples/example-ae.c"),
            RetrievalFileRole::Vendor
        );
    }

    #[test]
    fn inexact_queries_downrank_external_sources_unless_requested() {
        let production = hit_at_path(
            "production",
            "SqliteIndexStorage::addNode",
            NodeKind::FUNCTION,
            0.40,
            "src/lib/data/storage/sqlite/SqliteIndexStorage.cpp",
        );
        let external = hit_at_path(
            "external",
            "sqlite3SrcListIndexedBy",
            NodeKind::FUNCTION,
            0.95,
            "src/external/sqlite/sqlite3.c",
        );

        let mut hits = [external.clone(), production.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "index storage should find project storage code",
                left,
                right,
            )
        });
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&production.node_id)
        );

        hits.sort_by(|left, right| {
            compare_search_hits("external sqlite API indexed source list", left, right)
        });
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&external.node_id)
        );
    }

    #[test]
    fn inexact_queries_downrank_script_benchmarks_unless_requested() {
        let production = hit_at_path(
            "production",
            "handle_http_request",
            NodeKind::FUNCTION,
            0.40,
            "crates/codestory-cli/src/main.rs",
        );
        let benchmark_script = hit_at_path(
            "benchmark",
            "waitForHttpHealth",
            NodeKind::FUNCTION,
            0.95,
            "scripts/codestory-agent-ab-benchmark.mjs",
        );
        let application_script = hit_at_path(
            "script",
            "sendWalletBatch",
            NodeKind::FUNCTION,
            0.95,
            "scripts/hunter.js",
        );

        let mut hits = [benchmark_script.clone(), production.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits("route small http server requests", left, right)
        });
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&production.node_id)
        );

        hits.sort_by(|left, right| {
            compare_search_hits("benchmark waits for http server health", left, right)
        });
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&benchmark_script.node_id)
        );

        let mut hits = [application_script.clone(), production.clone()];
        hits.sort_by(|left, right| compare_search_hits("send wallet batch calls", left, right));
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&application_script.node_id)
        );
    }

    #[test]
    fn inexact_queries_downrank_non_primary_roles_unless_requested() {
        let production = hit_at_path(
            "production",
            "rank_search_hits",
            NodeKind::FUNCTION,
            0.40,
            "crates/codestory-runtime/src/symbol_query.rs",
        );
        let docs = hit_at_path(
            "docs",
            "search_ranking_notes",
            NodeKind::FUNCTION,
            0.95,
            "docs/testing/search-ranking.md",
        );
        let bench = hit_at_path(
            "bench",
            "search_ranking_bench",
            NodeKind::FUNCTION,
            0.95,
            "crates/codestory-bench/benches/search_ranking.rs",
        );
        let generated = hit_at_path(
            "generated",
            "generated_search_ranking",
            NodeKind::FUNCTION,
            0.95,
            "target/generated/search_ranking.rs",
        );
        let vendor = hit_at_path(
            "vendor",
            "vendor_search_ranking",
            NodeKind::FUNCTION,
            0.95,
            "vendor/search/ranking.rs",
        );

        for non_primary in [
            docs.clone(),
            bench.clone(),
            generated.clone(),
            vendor.clone(),
        ] {
            let mut hits = [non_primary, production.clone()];
            hits.sort_by(|left, right| {
                compare_search_hits("production search ranking behavior", left, right)
            });

            assert_eq!(
                hits.first().map(|hit| &hit.node_id),
                Some(&production.node_id)
            );
        }

        let mut doc_hits = [production.clone(), docs.clone()];
        doc_hits.sort_by(|left, right| compare_search_hits("docs search ranking", left, right));
        assert_eq!(
            doc_hits.first().map(|hit| &hit.node_id),
            Some(&docs.node_id)
        );

        let mut bench_hits = [production.clone(), bench.clone()];
        bench_hits
            .sort_by(|left, right| compare_search_hits("benchmark search ranking", left, right));
        assert_eq!(
            bench_hits.first().map(|hit| &hit.node_id),
            Some(&bench.node_id)
        );

        let mut generated_hits = [production.clone(), generated.clone()];
        generated_hits
            .sort_by(|left, right| compare_search_hits("generated search ranking", left, right));
        assert_eq!(
            generated_hits.first().map(|hit| &hit.node_id),
            Some(&generated.node_id)
        );

        let mut vendor_hits = [production, vendor.clone()];
        vendor_hits
            .sort_by(|left, right| compare_search_hits("vendor search ranking", left, right));
        assert_eq!(
            vendor_hits.first().map(|hit| &hit.node_id),
            Some(&vendor.node_id)
        );
    }

    #[test]
    fn exact_non_primary_symbol_matches_are_not_downranked() {
        let production = hit_at_path(
            "production",
            "Project::Parse",
            NodeKind::FUNCTION,
            0.95,
            "src/lib/project/Project.cpp",
        );
        let external = hit_at_path(
            "external",
            "TiXmlDocument::Parse",
            NodeKind::FUNCTION,
            0.40,
            "src/external/tinyxml/tinyxml.cpp",
        );

        let mut hits = [production, external.clone()];
        hits.sort_by(|left, right| compare_search_hits("TiXmlDocument::Parse", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&external.node_id)
        );
    }

    #[test]
    fn exact_type_queries_downrank_forward_declarations() {
        let temp = tempdir().expect("create temp dir");
        let forward_path = temp.path().join("ViewFactory.h");
        let definition_path = temp.path().join("StorageAccess.h");
        std::fs::write(&forward_path, "class StorageAccess;\n").expect("write forward decl");
        std::fs::write(&definition_path, "class StorageAccess\n{\n};\n").expect("write definition");

        let mut forward = hit_at_path(
            "forward",
            "StorageAccess",
            NodeKind::CLASS,
            0.95,
            &forward_path.to_string_lossy(),
        );
        forward.line = Some(1);
        let mut definition = hit_at_path(
            "definition",
            "StorageAccess",
            NodeKind::CLASS,
            0.80,
            &definition_path.to_string_lossy(),
        );
        definition.line = Some(1);

        let mut hits = [forward, definition.clone()];
        hits.sort_by(|left, right| compare_search_hits("StorageAccess", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&definition.node_id)
        );
    }

    #[test]
    fn exact_callable_queries_prefer_implementation_over_reexports() {
        let temp = tempdir().expect("create temp dir");
        let reexport_path = temp.path().join("lib.rs");
        let implementation_path = temp.path().join("browser.rs");
        std::fs::write(
            &reexport_path,
            "pub use browser::{exact_symbol_anchor, expand_browser_context};\n",
        )
        .expect("write reexport");
        std::fs::write(
            &implementation_path,
            "pub fn exact_symbol_anchor() -> &'static str {\n    \"anchor\"\n}\n",
        )
        .expect("write implementation");

        let mut reexport = hit_at_path(
            "reexport",
            "exact_symbol_anchor",
            NodeKind::MODULE,
            0.95,
            &reexport_path.to_string_lossy(),
        );
        reexport.line = Some(1);
        let mut implementation = hit_at_path(
            "implementation",
            "exact_symbol_anchor",
            NodeKind::FUNCTION,
            0.80,
            &implementation_path.to_string_lossy(),
        );
        implementation.line = Some(1);

        let mut hits = [reexport, implementation.clone()];
        hits.sort_by(|left, right| compare_search_hits("exact_symbol_anchor", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&implementation.node_id)
        );
    }

    #[test]
    fn embedded_exact_symbol_terms_sort_ahead_of_semantic_distractors() {
        let exact = hit(
            "exact",
            "SearchEngine::search_hybrid_with_scores",
            NodeKind::METHOD,
            0.30,
        );
        let semantic = hit("semantic", "search_match_rank", NodeKind::FUNCTION, 0.95);

        let mut hits = [semantic, exact.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "exact symbol first semantic ranking search_hybrid_with_scores",
                left,
                right,
            )
        });

        assert_eq!(hits.first().map(|hit| &hit.node_id), Some(&exact.node_id));
    }

    #[test]
    fn mixed_natural_language_query_requires_whitespace_and_embedded_symbol() {
        assert!(mixed_natural_language_query(
            "how ExtensionHostManager starts"
        ));
        assert!(!mixed_natural_language_query("ExtensionHostManager"));
        assert!(!mixed_natural_language_query("explain the architecture"));
    }

    #[test]
    fn embedded_generic_terms_do_not_create_exact_symbol_matches() {
        let exact_generic = hit("generic", "current", NodeKind::VARIABLE, 0.30);
        let semantic = hit("semantic", "architecture_summary", NodeKind::FUNCTION, 0.95);

        let mut hits = [exact_generic, semantic.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits("study current architecture boundaries", left, right)
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&semantic.node_id)
        );
    }

    #[test]
    fn exact_callable_queries_prefer_function_bodies_over_declarations() {
        let temp = tempdir().expect("create temp dir");
        let declaration_path = temp.path().join("SourceGroupCxxCdb.h");
        let implementation_path = temp.path().join("SourceGroupCxxCdb.cpp");
        std::fs::write(
            &declaration_path,
            "std::vector<IndexerCommand> getIndexerCommands() const override;\n",
        )
        .expect("write declaration");
        std::fs::write(
            &implementation_path,
            "std::vector<IndexerCommand> SourceGroupCxxCdb::getIndexerCommands() const\n{\n    return {};\n}\n",
        )
        .expect("write implementation");

        let mut declaration = hit_at_path(
            "declaration",
            "SourceGroupCxxCdb::getIndexerCommands",
            NodeKind::METHOD,
            0.95,
            &declaration_path.to_string_lossy(),
        );
        declaration.line = Some(1);
        let mut implementation = hit_at_path(
            "implementation",
            "SourceGroupCxxCdb::getIndexerCommands",
            NodeKind::METHOD,
            0.80,
            &implementation_path.to_string_lossy(),
        );
        implementation.line = Some(1);

        let mut hits = [declaration, implementation.clone()];
        hits.sort_by(|left, right| compare_search_hits("getIndexerCommands", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&implementation.node_id)
        );
    }

    #[test]
    fn exact_type_queries_use_project_root_for_relative_paths() {
        let temp = tempdir().expect("create temp dir");
        let src = temp.path().join("src");
        std::fs::create_dir_all(&src).expect("create src dir");
        std::fs::write(src.join("ViewFactory.h"), "class StorageAccess;\n")
            .expect("write forward decl");
        std::fs::write(src.join("StorageAccess.h"), "class StorageAccess\n{\n};\n")
            .expect("write definition");

        let mut forward = hit_at_path(
            "forward",
            "StorageAccess",
            NodeKind::CLASS,
            0.95,
            "src/ViewFactory.h",
        );
        forward.line = Some(1);
        let mut definition = hit_at_path(
            "definition",
            "StorageAccess",
            NodeKind::CLASS,
            0.80,
            "src/StorageAccess.h",
        );
        definition.line = Some(1);

        let mut hits = [forward, definition.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits_with_project_root(Some(temp.path()), "StorageAccess", left, right)
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&definition.node_id)
        );
    }

    #[test]
    fn exact_type_queries_downrank_inheritance_mentions_below_exact_members() {
        let temp = tempdir().expect("create temp dir");
        let inherited_path = temp.path().join("PersistentStorage.h");
        let member_path = temp.path().join("StorageAccess.h");
        std::fs::write(
            &inherited_path,
            "class PersistentStorage\n\t: public StorageAccess\n{\n};\n",
        )
        .expect("write inherited type");
        std::fs::write(&member_path, "virtual ~StorageAccess() = default;\n")
            .expect("write member");

        let mut inherited = hit_at_path(
            "inherited",
            "StorageAccess",
            NodeKind::CLASS,
            0.95,
            &inherited_path.to_string_lossy(),
        );
        inherited.line = Some(2);
        let mut member = hit_at_path(
            "member",
            "StorageAccess::~StorageAccess",
            NodeKind::FUNCTION,
            0.80,
            &member_path.to_string_lossy(),
        );
        member.line = Some(1);

        let mut hits = [inherited, member.clone()];
        hits.sort_by(|left, right| compare_search_hits("StorageAccess", left, right));

        assert_eq!(hits.first().map(|hit| &hit.node_id), Some(&member.node_id));
    }
}
