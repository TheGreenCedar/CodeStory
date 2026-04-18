use crate::search_runtime::HybridSearchConfig;
use codestory_contracts::api::{AgentHybridWeightsDto, NodeDetailsDto, SearchHit};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Write as _;

pub(crate) const HYBRID_RETRIEVAL_ENABLED_ENV: &str = "CODESTORY_HYBRID_RETRIEVAL_ENABLED";

pub(crate) fn hybrid_retrieval_enabled() -> bool {
    env_flag_enabled(HYBRID_RETRIEVAL_ENABLED_ENV, true)
}

fn env_flag_enabled(var_name: &str, default: bool) -> bool {
    match std::env::var(var_name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

pub(crate) fn normalized_hybrid_weights(
    request_weights: Option<AgentHybridWeightsDto>,
    fallback: &HybridSearchConfig,
) -> (f32, f32, f32) {
    let lexical = request_weights
        .as_ref()
        .and_then(|weights| weights.lexical)
        .unwrap_or(fallback.lexical_weight)
        .clamp(0.0, 1.0);
    let semantic = request_weights
        .as_ref()
        .and_then(|weights| weights.semantic)
        .unwrap_or(fallback.semantic_weight)
        .clamp(0.0, 1.0);
    let graph = request_weights
        .and_then(|weights| weights.graph)
        .unwrap_or(fallback.graph_weight)
        .clamp(0.0, 1.0);

    let sum = lexical + semantic + graph;
    if sum <= f32::EPSILON {
        return (
            fallback.lexical_weight,
            fallback.semantic_weight,
            fallback.graph_weight,
        );
    }

    (lexical / sum, semantic / sum, graph / sum)
}

pub(crate) fn node_display_name(node: &codestory_contracts::graph::Node) -> String {
    node.qualified_name
        .clone()
        .unwrap_or_else(|| node.serialized_name.clone())
}

pub(crate) fn clamp_i64_to_u32(v: i64) -> u32 {
    if v <= 0 {
        0
    } else if v > u32::MAX as i64 {
        u32::MAX
    } else {
        v as u32
    }
}

pub(crate) fn clamp_u64_to_u32(v: u64) -> u32 {
    v.min(u32::MAX as u64) as u32
}

pub(crate) fn clamp_u128_to_u32(v: u128) -> u32 {
    v.min(u32::MAX as u128) as u32
}

pub(crate) fn clamp_usize_to_u32(v: usize) -> u32 {
    v.min(u32::MAX as usize) as u32
}

const NL_STOPWORDS: &[&str] = &[
    "a",
    "an",
    "and",
    "are",
    "as",
    "at",
    "be",
    "by",
    "can",
    "do",
    "does",
    "for",
    "from",
    "how",
    "in",
    "is",
    "it",
    "of",
    "on",
    "or",
    "repo",
    "repository",
    "show",
    "tell",
    "that",
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

pub(crate) fn extract_symbol_search_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut seen = HashSet::new();

    for ch in query.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch.to_ascii_lowercase());
            continue;
        }

        if current.len() >= 3
            && !NL_STOPWORDS.contains(&current.as_str())
            && seen.insert(current.clone())
        {
            terms.push(current.clone());
        }
        current.clear();
    }

    if current.len() >= 3
        && !NL_STOPWORDS.contains(&current.as_str())
        && seen.insert(current.clone())
    {
        terms.push(current);
    }

    terms.truncate(8);
    terms
}

pub(crate) fn should_expand_symbol_query(query: &str, direct_hit_count: usize) -> bool {
    let word_count = query.split_whitespace().count();
    let has_text_punctuation = query
        .chars()
        .any(|ch| matches!(ch, '.' | ',' | ':' | ';' | '!' | '?' | '"' | '\''));
    if word_count > 1 && has_text_punctuation {
        return true;
    }
    if direct_hit_count >= 3 {
        return false;
    }

    word_count > 2 || query.len() > 28
}

pub(crate) fn looks_like_repo_text_query(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return false;
    }

    let word_count = trimmed.split_whitespace().count();
    let has_text_punctuation = query
        .chars()
        .any(|ch| matches!(ch, '.' | ',' | ':' | ';' | '!' | '?' | '"' | '\''));
    if (word_count > 1 && has_text_punctuation) || trimmed.len() > 28 || word_count >= 4 {
        return true;
    }

    if word_count < 2 {
        return false;
    }

    trimmed.split_whitespace().any(|term| {
        matches!(
            term.to_ascii_lowercase().as_str(),
            "how"
                | "what"
                | "why"
                | "where"
                | "when"
                | "which"
                | "who"
                | "does"
                | "do"
                | "is"
                | "are"
                | "should"
                | "can"
        )
    })
}

pub(crate) fn file_text_match_line(contents: &str, query: &str, terms: &[String]) -> Option<u32> {
    let normalized_query = query.trim().to_ascii_lowercase();
    for (index, line) in contents.lines().enumerate() {
        let normalized_line = line.to_ascii_lowercase();
        if !normalized_query.is_empty() && normalized_line.contains(&normalized_query) {
            return Some((index + 1).min(u32::MAX as usize) as u32);
        }
        if !terms.is_empty() && terms.iter().all(|term| normalized_line.contains(term)) {
            return Some((index + 1).min(u32::MAX as usize) as u32);
        }
    }
    None
}

pub(crate) fn read_searchable_file_contents(path: &str) -> Option<String> {
    if let Ok(contents) = std::fs::read_to_string(path) {
        return Some(contents);
    }

    #[cfg(windows)]
    {
        if let Some(stripped) = path.strip_prefix(r"\\?\")
            && let Ok(contents) = std::fs::read_to_string(stripped)
        {
            return Some(contents);
        }
    }

    None
}

pub(crate) fn aggregate_symbol_matches(
    primary: Vec<(codestory_contracts::graph::NodeId, f32)>,
    expanded: Vec<(codestory_contracts::graph::NodeId, f32)>,
) -> Vec<(codestory_contracts::graph::NodeId, f32)> {
    let mut scores = HashMap::<codestory_contracts::graph::NodeId, f32>::new();

    for (id, score) in expanded {
        scores.insert(id, score);
    }

    for (id, score) in primary {
        let preferred = score + 100.0;
        scores
            .entry(id)
            .and_modify(|existing| *existing = existing.max(preferred))
            .or_insert(preferred);
    }

    let mut merged = scores.into_iter().collect::<Vec<_>>();
    merged.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal));
    merged.truncate(20);
    merged
}

pub(crate) fn preferred_occurrence(
    occurrences: &[codestory_contracts::graph::Occurrence],
) -> Option<&codestory_contracts::graph::Occurrence> {
    fn occurrence_rank(kind: codestory_contracts::graph::OccurrenceKind) -> u8 {
        match kind {
            codestory_contracts::graph::OccurrenceKind::DECLARATION
            | codestory_contracts::graph::OccurrenceKind::DEFINITION
            | codestory_contracts::graph::OccurrenceKind::MACRO_DEFINITION => 3,
            codestory_contracts::graph::OccurrenceKind::REFERENCE
            | codestory_contracts::graph::OccurrenceKind::MACRO_REFERENCE => 2,
            codestory_contracts::graph::OccurrenceKind::UNKNOWN => 1,
        }
    }

    occurrences.iter().max_by(|left, right| {
        occurrence_rank(left.kind)
            .cmp(&occurrence_rank(right.kind))
            .then_with(|| right.location.start_line.cmp(&left.location.start_line))
            .then_with(|| right.location.start_col.cmp(&left.location.start_col))
    })
}

#[derive(Debug, Clone)]
pub(crate) struct FocusedSourceContext {
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) snippet: String,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalAgentResponse {
    pub(crate) backend_label: &'static str,
    pub(crate) command: String,
    pub(crate) markdown: String,
}

pub(crate) fn truncate_for_diagnostic(raw: &str, max_chars: usize) -> String {
    let mut compact = raw.trim().replace('\r', "");
    if compact.len() > max_chars {
        compact.truncate(max_chars);
        compact.push_str("...");
    }
    compact
}

pub(crate) fn build_local_agent_prompt(
    user_prompt: &str,
    hits: &[SearchHit],
    focused_node: Option<&NodeDetailsDto>,
    focused_source: Option<&FocusedSourceContext>,
) -> String {
    let mut out = String::new();
    out.push_str("You are a codebase assistant. Use only the provided indexed context.\n");
    out.push_str("Do not run tools or execute commands. If context is insufficient, say so.\n\n");
    let _ = writeln!(out, "User request:\n{}\n", user_prompt.trim());

    out.push_str("Indexed symbol hits:\n");
    if hits.is_empty() {
        out.push_str("- none\n");
    } else {
        for hit in hits.iter().take(8) {
            let location = match (&hit.file_path, hit.line) {
                (Some(path), Some(line)) => format!(" ({path}:{line})"),
                (Some(path), None) => format!(" ({path})"),
                _ => String::new(),
            };
            let _ = writeln!(
                out,
                "- {} [{:?}] score {:.3}{}",
                hit.display_name, hit.kind, hit.score, location
            );
        }
    }

    if let Some(node) = focused_node {
        let _ = writeln!(
            out,
            "\nFocused symbol:\n- {} [{:?}]",
            node.display_name, node.kind
        );
        if let Some(path) = node.file_path.as_deref() {
            let _ = writeln!(out, "- file: {}", path);
        }
        if let Some(line) = node.start_line {
            let _ = writeln!(out, "- start line: {}", line);
        }
    }

    if let Some(source) = focused_source {
        let _ = writeln!(
            out,
            "\nSource snippet from {}:{}:\n{}",
            source.path, source.line, source.snippet
        );
    }

    out.push_str(
        "\nRespond in markdown with:\n1. Summary\n2. Key findings\n3. Recommended next steps\n",
    );

    out
}
