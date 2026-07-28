use super::{HashSet, SearchPlanDroppedTermDto, SearchPlanTermsDto};

// "anchor", "answer", "around", "cite", "cited", and "cites" are
// CodeStory's own prompt and skill vocabulary -- they appear throughout the
// shipped grounding skill and back SEARCH_PLAN_EXPLICIT_ANCHOR_MARKER -- not
// benchmark phrasing, so they stay.
pub(super) const SEARCH_PLAN_STOPWORDS: &[&str] = &[
    "a",
    "an",
    "and",
    "anchor",
    "answer",
    "are",
    "around",
    "as",
    "at",
    "be",
    "by",
    "can",
    "cite",
    "cited",
    "cites",
    "code",
    "codestory",
    "does",
    "explain",
    "for",
    "from",
    "how",
    "in",
    "into",
    "is",
    "it",
    "later",
    "of",
    "on",
    "or",
    "repo",
    "repository",
    "show",
    "that",
    "the",
    "then",
    "this",
    "through",
    "to",
    "what",
    "where",
    "which",
    "why",
    "with",
];
pub(super) const SEARCH_PLAN_OPTIONAL_SUBQUERY_LIMIT: usize = 8;
pub(super) const SEARCH_PLAN_MAX_SEED_ANCHORS: usize = 32;
pub(super) const SEARCH_PLAN_SEED_ANCHOR_MARKER: &str = "Seed anchors:";
pub(super) const SEARCH_PLAN_EXPLICIT_ANCHOR_MARKER: &str = "Anchor the answer around";
pub(super) const SEARCH_PLAN_BASE_SOURCE_TRUTH_CHECKS: &[&str] = &[
    "Draft the CodeStory-only answer from selected anchors, bridge status, symbol, trail, and snippet evidence before opening source.",
    "Open the cited source files after the CodeStory-only draft and classify each claim as correct, partial, misleading, or unsupported.",
];
pub(super) const SEARCH_PLAN_REPO_TEXT_SOURCE_TRUTH_CHECK: &str = "Repo-text-only or ambiguous groups require direct source reads before they can support architecture claims.";

pub(super) fn search_plan_terms(query: &str) -> SearchPlanTermsDto {
    let mut extracted = Vec::new();
    let mut dropped = Vec::new();
    let mut seen = HashSet::new();
    let mut dropped_seen = HashSet::new();

    for raw in query.split_whitespace() {
        let token = raw
            .trim_matches(|ch: char| {
                matches!(
                    ch,
                    '"' | '\'' | '`' | ',' | '.' | ';' | ':' | '?' | '!' | '(' | ')' | '[' | ']'
                )
            })
            .trim_end_matches("'s");
        if token.is_empty() {
            continue;
        }
        let fragments = token
            .split('/')
            .flat_map(|part| part.split('.'))
            .flat_map(|part| part.split(':'))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        for fragment in fragments {
            let normalized = fragment
                .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'));
            if normalized.is_empty() {
                continue;
            }
            add_search_plan_term(
                normalized,
                &mut extracted,
                &mut seen,
                &mut dropped,
                &mut dropped_seen,
            );
            if normalized.contains('-') {
                for part in normalized.split('-').filter(|part| !part.is_empty()) {
                    add_search_plan_term(
                        part,
                        &mut extracted,
                        &mut seen,
                        &mut dropped,
                        &mut dropped_seen,
                    );
                }
            }
            for camel_part in split_camel_identifier(normalized) {
                add_search_plan_term(
                    &camel_part,
                    &mut extracted,
                    &mut seen,
                    &mut dropped,
                    &mut dropped_seen,
                );
            }
        }
    }
    SearchPlanTermsDto { extracted, dropped }
}

/// True when a term looks like an identifier a repository would actually
/// declare.
///
/// This replaces the fixed noun list that decided which extracted terms were
/// worth a typed-symbol subquery. Shape, not vocabulary: a term qualifies by
/// carrying a separator, an interior capital, or enough alphabetic length to be
/// a name rather than filler.
pub(super) fn search_plan_identifier_shaped_term(term: &str) -> bool {
    if term.contains('_') || term.contains("::") {
        return true;
    }
    let has_interior_uppercase = term
        .chars()
        .skip(1)
        .any(|character| character.is_ascii_uppercase());
    if has_interior_uppercase {
        return true;
    }
    term.len() >= 5
        && term
            .chars()
            .all(|character| character.is_ascii_alphabetic())
        && !SEARCH_PLAN_STOPWORDS.contains(&term.to_ascii_lowercase().as_str())
}

/// Every token the query itself supplies, including camel and snake splits.
///
/// Anything a generated subquery contains must be a member of this set, so the
/// plan can never inject vocabulary the caller did not write.
pub(super) fn search_plan_query_token_closure(query: &str) -> HashSet<String> {
    search_plan_terms(query)
        .extracted
        .into_iter()
        .map(|term| term.to_ascii_lowercase())
        .collect()
}

pub(super) fn add_search_plan_term(
    raw: &str,
    extracted: &mut Vec<String>,
    seen: &mut HashSet<String>,
    dropped: &mut Vec<SearchPlanDroppedTermDto>,
    dropped_seen: &mut HashSet<String>,
) {
    let clean = raw
        .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        .to_string();
    if clean.is_empty() {
        return;
    }
    let lower = clean.to_ascii_lowercase();
    if lower.len() < 3 {
        if dropped_seen.insert(lower.clone()) {
            dropped.push(SearchPlanDroppedTermDto {
                term: clean,
                reason: "too_short".to_string(),
            });
        }
        return;
    }
    if SEARCH_PLAN_STOPWORDS.contains(&lower.as_str()) {
        if dropped_seen.insert(lower.clone()) {
            dropped.push(SearchPlanDroppedTermDto {
                term: clean,
                reason: "natural_language_filler".to_string(),
            });
        }
        return;
    }
    let value = if clean.chars().any(|ch| ch.is_ascii_uppercase()) && clean.len() > 3 {
        clean
    } else {
        lower.clone()
    };
    if seen.insert(value.to_ascii_lowercase()) {
        extracted.push(value);
    }
}

pub(super) fn split_camel_identifier(value: &str) -> Vec<String> {
    if !value.chars().any(|ch| ch.is_ascii_uppercase()) {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in value.chars() {
        if ch == '_' || ch == '-' {
            if current.len() >= 3 {
                parts.push(current.clone());
            }
            current.clear();
            continue;
        }
        if ch.is_ascii_uppercase() && !current.is_empty() {
            if current.len() >= 3 {
                parts.push(current.clone());
            }
            current.clear();
        }
        current.push(ch.to_ascii_lowercase());
    }
    if current.len() >= 3 {
        parts.push(current);
    }
    parts
}
