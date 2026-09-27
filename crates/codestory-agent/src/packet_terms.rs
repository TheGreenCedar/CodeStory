use crate::text::{
    is_non_primary_source_term, normalize_identifier, query_mentions_non_primary_source,
};
use std::collections::HashSet;

pub fn prompt_search_terms(prompt: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "a",
        "actual",
        "already",
        "an",
        "and",
        "are",
        "area",
        "areas",
        "across",
        "as",
        "at",
        "be",
        "boundaries",
        "boundary",
        "by",
        "can",
        "current",
        "does",
        "existing",
        "for",
        "from",
        "how",
        "implementation",
        "implemented",
        "in",
        "is",
        "it",
        "of",
        "on",
        "or",
        "repo",
        "repository",
        "risk",
        "risks",
        "study",
        "the",
        "this",
        "to",
        "what",
        "where",
        "which",
        "why",
        "with",
        "work",
        "works",
    ];

    let mut terms = Vec::new();
    let mut current = String::new();
    let mut seen = HashSet::new();

    for ch in prompt.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch.to_ascii_lowercase());
            continue;
        }

        if current.len() >= 3
            && !STOPWORDS.contains(&current.as_str())
            && seen.insert(current.clone())
        {
            terms.push(current.clone());
        }
        current.clear();
    }

    if current.len() >= 3 && !STOPWORDS.contains(&current.as_str()) && seen.insert(current.clone())
    {
        terms.push(current);
    }

    terms
}

/// Generic probe terms from the prompt. Domain nouns remain ordinary search
/// terms; they do not select taxonomies or strip brand tokens.
pub fn packet_probe_terms(question: &str) -> Vec<String> {
    let include_non_primary_terms = query_mentions_non_primary_source(question);
    prompt_search_terms(question)
        .into_iter()
        .filter(|term| include_non_primary_terms || !is_non_primary_source_term(term))
        .collect()
}

pub fn packet_terms_have(terms: &[String], needle: &str) -> bool {
    let normalized_needle = normalize_identifier(needle);
    terms.iter().any(|value| {
        value.eq_ignore_ascii_case(needle) || normalize_identifier(value) == normalized_needle
    })
}

pub fn packet_terms_have_any(terms: &[String], needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| packet_terms_have(terms, needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_terms_keep_ordinary_domain_nouns_as_search_tokens() {
        let terms = packet_probe_terms(
            "Explain how the client cache formatter mapper request animation works.",
        );
        for expected in [
            "client",
            "cache",
            "formatter",
            "mapper",
            "request",
            "animation",
        ] {
            assert!(
                terms.iter().any(|term| term == expected),
                "expected {expected:?} in {terms:?}"
            );
        }
    }
}
