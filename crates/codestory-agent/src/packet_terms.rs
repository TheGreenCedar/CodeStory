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
        .filter(|term| {
            include_non_primary_terms
                || !is_non_primary_source_term(term)
                || packet_retains_non_primary_probe_term(question, term)
        })
        .collect()
}

fn packet_retains_non_primary_probe_term(question: &str, term: &str) -> bool {
    if matches!(term, "source" | "sources") {
        // Retain "source" when the prompt discusses buffered I/O wrappers by
        // ordinary wording, not via a domain-flow classifier.
        let lowered = question.to_ascii_lowercase();
        return lowered.contains("buffer")
            || lowered.contains("sink")
            || lowered.contains("read")
            || lowered.contains("write");
    }

    if matches!(term, "bench" | "benchmark" | "benchmarks") {
        let lowered = question.to_ascii_lowercase();
        return lowered.contains("architecture")
            && (lowered.contains("boundary")
                || lowered.contains("boundaries")
                || lowered.contains("across"));
    }

    false
}

pub fn packet_terms_have(terms: &[String], needle: &str) -> bool {
    let normalized_needle = normalize_identifier(needle);
    terms.iter().any(|value| {
        let normalized_value = normalize_identifier(value);
        value.eq_ignore_ascii_case(needle)
            || normalized_value == normalized_needle
            || bounded_action_lemma(&normalized_value) == bounded_action_lemma(&normalized_needle)
    })
}

/// Collapse only the finite action forms packet planning explicitly understands.
fn bounded_action_lemma(term: &str) -> &str {
    match term {
        "send" | "sends" | "sending" | "sent" => "send",
        _ => term,
    }
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
        for expected in ["client", "cache", "formatter", "mapper", "request", "animation"] {
            assert!(
                terms.iter().any(|term| term == expected),
                "expected {expected:?} in {terms:?}"
            );
        }
    }

    #[test]
    fn bounded_send_morphology_is_exact_and_shared() {
        for action in ["send", "sends", "sending", "sent"] {
            let terms = packet_probe_terms(&format!(
                "Explain how a session {action} a request through an adapter."
            ));
            assert!(packet_terms_have(&terms, "send"), "{action}: {terms:?}");
        }

        for unrelated in ["sender", "sending_hook", "dispatch_hook"] {
            let terms =
                packet_probe_terms(&format!("Explain the session request adapter {unrelated}."));
            assert!(!packet_terms_have(&terms, "send"), "{unrelated}: {terms:?}");
        }
    }

    #[test]
    fn buffered_io_prompts_retain_source_as_api_concept() {
        let terms = packet_probe_terms(
            "Explain how buffered Source and Sink wrappers use Buffer state during reads and writes.",
        );
        for expected in ["source", "sink", "buffered", "buffer"] {
            assert!(
                terms.iter().any(|term| term == expected),
                "expected {expected:?} in {terms:?}"
            );
        }
    }
}
