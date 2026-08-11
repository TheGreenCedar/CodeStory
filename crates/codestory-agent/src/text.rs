//! Pure lexical and path classification used by packet planning.
//!
//! These helpers read nothing: no controller, no store, no publication, no
//! filesystem. Planning needs them on every prompt, so they live alongside the
//! policy that consumes them.

use std::collections::HashSet;

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

pub fn normalize_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

pub fn exact_symbol_query_terms(query: &str) -> Vec<String> {
    let trimmed = trim_symbol_candidate(query);
    if looks_like_standalone_symbol_query(trimmed) && !ambiguous_single_slash_concept(trimmed) {
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
    let mut chars = query.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '!' && chars.peek().is_some_and(|next| *next == '=') {
            // `!=` and `!==` terminate the left operand. Keeping this bang would fabricate a
            // Ruby-style suffix identity (`foo_bar!`) from an ordinary comparison expression.
            push_embedded_symbol_candidate(&candidate, &mut terms, &mut seen);
            candidate.clear();
            continue;
        }
        if is_symbol_query_char(ch) {
            candidate.push(ch);
            continue;
        }
        push_embedded_symbol_candidate(&candidate, &mut terms, &mut seen);
        candidate.clear();
    }
    push_embedded_symbol_candidate(&candidate, &mut terms, &mut seen);
    for candidate in case_distinct_slash_symbol_candidates(query) {
        push_exact_symbol_query_term(&candidate, &mut terms, &mut seen);
    }
    terms
}

fn case_distinct_slash_symbol_candidates(query: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut candidate = String::new();
    for ch in query.chars().chain(std::iter::once(' ')) {
        if is_symbol_query_char(ch) {
            candidate.push(ch);
            continue;
        }
        let trimmed = trim_symbol_candidate(&candidate);
        if trimmed.matches('/').count() == 1 && looks_like_standalone_symbol_query(trimmed) {
            candidates.push(trimmed.to_string());
        }
        candidate.clear();
    }
    candidates
        .iter()
        .filter(|candidate| {
            candidates
                .iter()
                .any(|peer| peer != *candidate && peer.eq_ignore_ascii_case(candidate.as_str()))
        })
        .cloned()
        .collect()
}

fn push_exact_symbol_query_term(raw: &str, terms: &mut Vec<String>, seen: &mut HashSet<String>) {
    let candidate = trim_symbol_candidate(raw);
    if !looks_like_standalone_symbol_query(candidate) {
        return;
    }
    // Exact symbol requests are case-bearing identities. `Foo::run` and `foo::run` may be two
    // distinct symbols in the same project and must survive as separate requested candidates.
    if seen.insert(candidate.to_string()) {
        terms.push(candidate.to_string());
    }
}

pub fn looks_like_standalone_symbol_query(query: &str) -> bool {
    let trimmed = trim_symbol_candidate(query);
    !trimmed.is_empty()
        && !trimmed.chars().any(char::is_whitespace)
        && trimmed.chars().any(|ch| ch.is_ascii_alphabetic())
        && trimmed.chars().all(is_symbol_query_char)
        && symbol_identity_punctuation_is_lawful(trimmed)
}

fn push_embedded_symbol_candidate(raw: &str, terms: &mut Vec<String>, seen: &mut HashSet<String>) {
    let candidate = trim_symbol_candidate(raw);
    if !looks_like_standalone_symbol_query(candidate)
        || !has_embedded_exact_symbol_signal(candidate)
    {
        return;
    }

    // Embedded exact symbols carry the same case-sensitive identity as standalone exact probes.
    // Java-style `Foo.run` and `foo.run`, for example, may resolve to different owners.
    if seen.insert(candidate.to_string()) {
        terms.push(candidate.to_string());
    }
}

fn trim_symbol_candidate(value: &str) -> &str {
    value.trim().trim_matches(|ch: char| {
        !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '?' | '!' | '~'))
    })
}

fn is_symbol_query_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '.' | '/' | '$' | '?' | '!' | '~')
}

fn symbol_identity_punctuation_is_lawful(value: &str) -> bool {
    let ruby_suffix_count = value.chars().filter(|ch| matches!(ch, '?' | '!')).count();
    if ruby_suffix_count > 0 {
        let Some(stem) = value.strip_suffix('?').or_else(|| value.strip_suffix('!')) else {
            return false;
        };
        let terminal = stem.rsplit([':', '.', '/']).next().unwrap_or_default();
        return ruby_suffix_count == 1
            && !value.contains('~')
            && symbol_identity_component_is_lawful(terminal);
    }

    let destructor_count = value.chars().filter(|ch| *ch == '~').count();
    if destructor_count == 0 {
        return true;
    }
    if destructor_count != 1 {
        return false;
    }

    let Some((owner_path, destructor_name)) = value.rsplit_once("::~") else {
        return false;
    };
    let owner_name = owner_path.rsplit("::").next().unwrap_or_default();
    symbol_identity_component_is_lawful(owner_name)
        && owner_name == destructor_name
        && symbol_identity_component_is_lawful(destructor_name)
}

fn symbol_identity_component_is_lawful(value: &str) -> bool {
    !value.is_empty()
        && value.chars().any(|ch| ch.is_ascii_alphabetic())
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'))
}

fn has_embedded_exact_symbol_signal(value: &str) -> bool {
    value.contains('_')
        || value.contains("::")
        || value.contains('.')
        || (value.contains('/') && !ambiguous_single_slash_concept(value))
        || value.contains('$')
        || value.chars().skip(1).any(|ch| ch.is_ascii_uppercase())
}

fn ambiguous_single_slash_concept(value: &str) -> bool {
    value.matches('/').count() == 1
        && !value.starts_with('/')
        && !value.starts_with("./")
        && !value.starts_with("../")
        && !value.contains(['.', '_', '$'])
        && !value
            .chars()
            .any(|character| character.is_ascii_digit() || character.is_ascii_uppercase())
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

pub fn terminal_symbol_segment(value: &str) -> String {
    value
        .rsplit([':', '.', '/', '\\'])
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

pub fn terms_contain_phrase(terms: &[String], phrase: &[&str]) -> bool {
    terms
        .windows(phrase.len())
        .any(|window| window.iter().map(String::as_str).eq(phrase.iter().copied()))
}

pub fn query_mentions_non_primary_source(query: &str) -> bool {
    let terms = query
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .map(|term| term.to_ascii_lowercase())
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();

    terms.iter().enumerate().any(|(index, term)| {
        is_non_primary_source_term(term) && !is_non_primary_source_exclusion_context(&terms, index)
    })
}

pub fn is_non_primary_source_term(term: &str) -> bool {
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
