//! Packet citation scoring helpers for batch retrieval ranking.

use super::eval_probes::eval_citation_rank_adjustment;
use crate::retrieval_file_role_from_path;
use codestory_contracts::api::{
    AgentCitationDto, NodeKind, PacketBudgetLimitsDto, SearchHitOrigin,
};

/// Citations merged from each packet retrieval stage before the final budget cap.
pub(crate) fn packet_stage_citation_carry_limit(limits: &PacketBudgetLimitsDto) -> usize {
    limits.max_anchors.clamp(8, 16) as usize
}

/// Candidate hits fetched per planned subquery or anchor-probe batch query.
pub(crate) fn packet_subquery_hit_limit(limits: &PacketBudgetLimitsDto) -> usize {
    limits.max_anchors.clamp(8, 20) as usize
}

pub(crate) fn packet_citation_key(citation: &AgentCitationDto) -> String {
    format!(
        "{}\t{}\t{}",
        citation.node_id.0,
        citation.file_path.as_deref().unwrap_or_default(),
        citation.line.unwrap_or_default()
    )
}
pub(crate) fn packet_citation_rank(
    citation: &AgentCitationDto,
    terms: &[String],
    prefer_primary_sources: bool,
) -> f32 {
    let display = citation.display_name.to_ascii_lowercase();
    let normalized_display = normalize_identifier(&citation.display_name);
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();

    let mut score = citation.score;
    if citation.origin == SearchHitOrigin::IndexedSymbol {
        score += 1.0;
    }
    if citation.resolvable {
        score += 0.5;
    }
    if display.contains("::") {
        score += 0.25;
    }
    if prefer_primary_sources {
        let role = retrieval_file_role_from_path(&path);
        if role.is_non_primary() {
            score -= 100.0;
        }
    }
    if path.ends_with(".d.ts")
        || path.ends_with(".d.cts")
        || path.ends_with(".d.mts")
        || path.ends_with(".d.tsx")
    {
        score -= 3.0;
    }
    if path.starts_with("extensions/")
        || path.starts_with("vendor/")
        || path.starts_with("deps/")
        || path.contains("/deps/")
    {
        score -= 20.0;
    }
    if packet_path_is_test_segment(&path) {
        score -= 18.0;
    }
    if prefer_primary_sources && packet_display_name_is_test_like(&display) {
        score -= 24.0;
    }
    if packet_display_name_is_import_literal(&display) {
        score -= 30.0;
    }
    if packet_concrete_module_file_citation(citation.kind, &normalized_display, &path) {
        score += 2.0;
    }
    if packet_facade_module_citation(citation.kind, &normalized_display, &path) {
        score -= 3.0;
    }
    if path.contains("/sandbox/")
        || path.contains("/examples/")
        || path.contains("/test/")
        || path.contains("/tests/")
    {
        score -= 14.0;
    }
    if path.contains("/server/") && !packet_terms_contain(terms, "server") {
        score -= 12.0;
    }
    if path.contains("/collections/")
        && terms
            .iter()
            .any(|term| term.contains("collection") || term.contains("payload"))
    {
        score += 4.0;
    }

    score = eval_citation_rank_adjustment(&normalized_display, &path, score);
    if let Some(breakdown) = citation.retrieval_score_breakdown.as_ref() {
        score += breakdown.lexical * 2.0;
        score += breakdown.graph;
    }

    for term in terms {
        if term.len() < 3 {
            continue;
        }
        let normalized_term = normalize_identifier(term);
        if !normalized_term.is_empty() && normalized_display.contains(&normalized_term) {
            score += 1.25;
            if normalized_display == normalized_term
                || normalized_display.ends_with(&normalized_term)
            {
                score += 4.0;
            }
        }
        if path.contains(term) {
            score += 0.5;
        }
    }

    if packet_low_signal_display_name(normalized_display.as_str())
        && !packet_terms_contain(terms, normalized_display.as_str())
    {
        score -= 8.0;
    }

    score
}

fn packet_facade_module_citation(kind: NodeKind, normalized_display: &str, path: &str) -> bool {
    if kind != NodeKind::MODULE {
        return false;
    }
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if file_name != "lib.rs" && file_name != "mod.rs" {
        return false;
    }
    !matches!(normalized_display, "" | "lib" | "mod" | "main")
}

fn packet_concrete_module_file_citation(
    kind: NodeKind,
    normalized_display: &str,
    path: &str,
) -> bool {
    if kind != NodeKind::MODULE || normalized_display.is_empty() {
        return false;
    }
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if matches!(file_name, "lib.rs" | "mod.rs" | "main.rs") {
        return false;
    }
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    normalize_identifier(stem) == normalized_display
}

/// Rank citations for role-backed claim carry: prefer primary-source flow evidence over tests.
pub(crate) fn packet_claim_carry_rank(
    citation: &AgentCitationDto,
    terms: &[String],
    prefer_primary_sources: bool,
) -> f32 {
    let mut score = packet_citation_rank(citation, terms, prefer_primary_sources);
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if packet_path_is_test_segment(&path) {
        score -= 40.0;
    }
    if prefer_primary_sources
        && packet_display_name_is_test_like(&citation.display_name.to_ascii_lowercase())
    {
        score -= 40.0;
    }
    if packet_display_name_is_import_literal(&citation.display_name.to_ascii_lowercase()) {
        score -= 25.0;
    }
    score
}

pub(crate) fn packet_low_signal_display_name(normalized_display: &str) -> bool {
    matches!(normalized_display, "current" | "actual" | "existing")
}

pub(crate) fn packet_display_name_is_import_literal(display: &str) -> bool {
    let trimmed = display.trim();
    (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        || (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || trimmed.ends_with(" (import)")
        || (trimmed.contains('/') && trimmed.contains('.') && !trimmed.contains("::"))
        || trimmed.starts_with("\\\\?\\")
}

pub(crate) fn packet_display_name_is_test_like(display: &str) -> bool {
    let display = display.trim().to_ascii_lowercase();
    let local_name = display.rsplit("::").next().unwrap_or(display.as_str());
    display.starts_with("tests::")
        || display.contains("::tests::")
        || local_name.starts_with("test_")
        || local_name.ends_with("_test")
        || local_name.ends_with("_tests")
        || local_name.contains("_test_")
        || local_name.contains("_tests_")
}

fn packet_path_is_test_segment(path: &str) -> bool {
    path.starts_with("test/")
        || path.starts_with("tests/")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("-test-")
        || path.contains("_test.")
        || path.starts_with("test\\")
        || path.starts_with("tests\\")
        || path.contains("\\test\\")
        || path.contains("\\tests\\")
}

const PACKET_QUERY_STOP_TERMS: &[&str] = &[
    "about",
    "actual",
    "already",
    "also",
    "and",
    "are",
    "area",
    "areas",
    "across",
    "boundaries",
    "boundary",
    "can",
    "code",
    "current",
    "does",
    "explain",
    "existing",
    "file",
    "files",
    "find",
    "for",
    "from",
    "full",
    "how",
    "implementation",
    "implemented",
    "in",
    "into",
    "is",
    "it",
    "its",
    "like",
    "module",
    "modules",
    "move",
    "moves",
    "of",
    "on",
    "or",
    "risk",
    "risks",
    "show",
    "source",
    "study",
    "surface",
    "surfaces",
    "that",
    "the",
    "this",
    "through",
    "turns",
    "what",
    "when",
    "where",
    "with",
    "flows",
    "level",
    "requests",
    "support",
];

pub(crate) fn packet_query_stop_term(term: &str) -> bool {
    let lower = term.to_ascii_lowercase();
    PACKET_QUERY_STOP_TERMS.contains(&lower.as_str())
}

pub(crate) fn packet_adjacent_query_stop_term(term: &str) -> bool {
    matches!(
        term.to_ascii_lowercase().as_str(),
        "actual"
            | "already"
            | "area"
            | "areas"
            | "across"
            | "boundaries"
            | "boundary"
            | "current"
            | "existing"
            | "full"
            | "implementation"
            | "implemented"
            | "move"
            | "moves"
            | "risk"
            | "risks"
            | "study"
            | "surface"
            | "surfaces"
    )
}

pub(crate) fn packet_terms_contain(terms: &[String], needle: &str) -> bool {
    terms
        .iter()
        .any(|term| term.eq_ignore_ascii_case(needle) || normalize_identifier(term) == needle)
}

pub(crate) fn normalize_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

pub(crate) fn packet_display_path(path: &str) -> String {
    let normalized = path.trim_start_matches("\\\\?\\").replace('\\', "/");
    if let Some(path) = path_after_named_repo_root(&normalized) {
        return path;
    }
    if !normalized.contains(':') && !normalized.starts_with('/') {
        return normalized;
    }
    for prefix in [
        "crates/",
        "src/",
        "packages/",
        "apps/",
        "lib/",
        "tests/",
        "benches/",
    ] {
        if normalized.starts_with(prefix) {
            return normalized;
        }
    }
    for marker in [
        "/crates/",
        "/src/",
        "/packages/",
        "/apps/",
        "/lib/",
        "/tests/",
        "/benches/",
    ] {
        if let Some(index) = normalized.find(marker) {
            return normalized[index + 1..].to_string();
        }
    }
    normalized
}

fn path_after_named_repo_root(normalized: &str) -> Option<String> {
    let mut best_match: Option<(usize, String)> = None;
    for marker in ["/source/repos/", "source/repos/", "/repos/", "repos/"] {
        let Some(index) = normalized.rfind(marker) else {
            continue;
        };
        let suffix = &normalized[index + marker.len()..];
        let Some(repo_name_end) = suffix.find('/') else {
            continue;
        };
        let path = &suffix[repo_name_end + 1..];
        if !path.is_empty() {
            let candidate = path.to_string();
            if best_match
                .as_ref()
                .is_none_or(|(best_index, _)| index > *best_index)
            {
                best_match = Some((index, candidate));
            }
        }
    }
    best_match.map(|(_, path)| path)
}
