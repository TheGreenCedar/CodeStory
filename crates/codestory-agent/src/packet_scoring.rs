//! Packet citation scoring helpers for batch retrieval ranking.

#[cfg(any(test, feature = "test-support"))]
use crate::eval_probes::eval_citation_rank_adjustment;
use crate::text::{RetrievalFileRole, retrieval_file_role_from_path};
use codestory_contracts::api::{
    AgentCitationDto, NodeKind, PacketBudgetLimitsDto, SearchHitOrigin,
};
use std::cmp::Ordering;

// `normalize_identifier` was hoisted to the leaf `text` module to dissolve the
// packet_terms <-> packet_scoring release-code import cycle
// (`agent_planning_import_graph_stays_acyclic`); the re-export keeps every
// existing `packet_scoring::normalize_identifier` call site valid.
pub use crate::text::normalize_identifier;

/// Sort descending on a rank that is evaluated exactly once per element.
///
/// `packet_citation_rank` and `packet_claim_carry_rank` allocate and rescan every
/// ranking term, so they are decorated before the sort rather than recomputed
/// inside the comparator. The comparator itself is unchanged — a NaN rank still
/// compares equal, and the stable sort keeps input order for equal ranks.
pub fn sort_by_cached_rank_desc<T>(values: &mut Vec<T>, mut rank: impl FnMut(&T) -> f32) {
    let mut decorated = std::mem::take(values)
        .into_iter()
        .map(|value| {
            let rank = rank(&value);
            (value, rank)
        })
        .collect::<Vec<_>>();
    decorated.sort_by(|(_, left), (_, right)| right.partial_cmp(left).unwrap_or(Ordering::Equal));
    *values = decorated.into_iter().map(|(value, _)| value).collect();
}

/// Citations merged from each packet retrieval stage before the final budget cap.
pub fn packet_stage_citation_carry_limit(limits: &PacketBudgetLimitsDto) -> usize {
    (limits.max_anchors as usize).clamp(
        1,
        codestory_contracts::compilation::INTERIM_MAX_ADMITTED_CANDIDATES,
    )
}

/// Candidate hits fetched per planned subquery or anchor-probe batch query.
pub fn packet_subquery_hit_limit(limits: &PacketBudgetLimitsDto) -> usize {
    (limits.max_anchors as usize).clamp(
        1,
        codestory_contracts::compilation::INTERIM_MAX_ADMITTED_CANDIDATES,
    )
}

pub fn packet_citation_key(citation: &AgentCitationDto) -> String {
    format!(
        "{}\t{}\t{}",
        citation.node_id.0,
        citation.file_path.as_deref().unwrap_or_default(),
        citation.line.unwrap_or_default()
    )
}
pub fn packet_citation_rank(
    citation: &AgentCitationDto,
    _terms: &[String],
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
        let role = citation
            .file_path
            .as_deref()
            .map(retrieval_file_role_from_path)
            .unwrap_or(RetrievalFileRole::Source);
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
        if citation.kind == NodeKind::MODULE {
            score -= 20.0;
        }
    }
    if prefer_primary_sources && packet_display_name_is_test_like(&citation.display_name) {
        score -= 24.0;
    }
    if packet_display_name_is_import_literal(&display) {
        score -= 30.0;
    }
    if path.contains("/sandbox/")
        || path.starts_with("sandbox/")
        || path.contains("/examples/")
        || path.starts_with("examples/")
        || path.contains("/test/")
        || path.starts_with("test/")
        || path.contains("/tests/")
        || path.starts_with("tests/")
    {
        score -= 14.0;
    }
    #[cfg(any(test, feature = "test-support"))]
    {
        score = eval_citation_rank_adjustment(&normalized_display, &path, score);
    }
    if packet_low_signal_display_name(normalized_display.as_str()) {
        score -= 8.0;
    }
    score += packet_shared_source_set_rank_adjustment(&path, &[]);

    {
        if normalized_display.chars().count() <= 1 {
            score -= 16.0;
        }
    }

    score
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PacketSourceSetKind {
    Shared,
    Platform,
}

fn packet_shared_source_set_rank_adjustment(path: &str, terms: &[String]) -> f32 {
    if packet_terms_mention_platform_source_set(terms) {
        return 0.0;
    }
    match packet_path_source_set_kind(path) {
        Some(PacketSourceSetKind::Shared) => 2.5,
        Some(PacketSourceSetKind::Platform) => -2.5,
        None => 0.0,
    }
}

fn packet_terms_mention_platform_source_set(terms: &[String]) -> bool {
    terms.iter().any(|term| {
        let normalized = normalize_identifier(term);
        [
            "jvm", "nonjvm", "android", "ios", "native", "linux", "windows", "darwin", "apple",
            "wasm", "nodejs", "browser", "mingw", "macos",
        ]
        .iter()
        .any(|marker| normalized == *marker)
    })
}

fn packet_path_source_set_kind(path: &str) -> Option<PacketSourceSetKind> {
    let lower = packet_display_path(path)
        .replace('\\', "/")
        .to_ascii_lowercase();
    lower.split('/').find_map(packet_source_set_segment_kind)
}

fn packet_source_set_segment_kind(segment: &str) -> Option<PacketSourceSetKind> {
    if matches!(segment, "commonmain" | "common" | "shared") {
        return Some(PacketSourceSetKind::Shared);
    }
    if matches!(
        segment,
        "jvmmain"
            | "nonjvmmain"
            | "androidmain"
            | "iosmain"
            | "nativemain"
            | "linuxmain"
            | "windowsmain"
            | "darwinmain"
            | "applemain"
            | "wasmmain"
            | "wasmwasimain"
            | "nodejsmain"
            | "jsmain"
            | "browsermain"
            | "mingwmain"
            | "macosmain"
    ) || (segment.len() > 4
        && segment.ends_with("main")
        && [
            "jvm", "nonjvm", "android", "ios", "native", "linux", "mingw", "macos", "wasm", "js",
            "browser", "apple", "darwin", "windows",
        ]
        .iter()
        .any(|prefix| segment.starts_with(prefix)))
    {
        return Some(PacketSourceSetKind::Platform);
    }
    None
}

/// Rank citations for role-backed claim carry: prefer primary-source flow evidence over tests.
pub fn packet_claim_carry_rank(
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
    if prefer_primary_sources && packet_display_name_is_test_like(&citation.display_name) {
        score -= 40.0;
    }
    if packet_display_name_is_import_literal(&citation.display_name.to_ascii_lowercase()) {
        score -= 25.0;
    }
    score
}

pub fn packet_low_signal_display_name(normalized_display: &str) -> bool {
    matches!(normalized_display, "current" | "actual" | "existing")
}

pub fn packet_display_name_is_import_literal(display: &str) -> bool {
    let trimmed = display.trim();
    (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        || (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || trimmed.ends_with(" (import)")
        || (trimmed.contains('/') && trimmed.contains('.') && !trimmed.contains("::"))
        || trimmed.starts_with("\\\\?\\")
}

pub fn packet_display_name_is_test_like(display: &str) -> bool {
    let trimmed = display.trim();
    let display = trimmed.to_ascii_lowercase();
    let local_name = display.rsplit("::").next().unwrap_or(display.as_str());
    let local_original = trimmed.rsplit("::").next().unwrap_or(trimmed);
    let pascal_test_name = local_original.starts_with("Test")
        && local_original
            .chars()
            .nth(4)
            .is_some_and(|ch| ch == '_' || ch.is_ascii_digit() || ch.is_ascii_uppercase());
    display.starts_with("tests::")
        || display.contains("::tests::")
        || local_name.starts_with("test_")
        || pascal_test_name
        || local_name.contains("test.")
        || local_name.ends_with("_test")
        || local_name.ends_with("_tests")
        || local_name.ends_with("tests")
        || local_name.ends_with("test")
        || local_name.contains("_test_")
        || local_name.contains("_tests_")
}

fn packet_path_is_test_segment(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    let segment_is_test = path.split(['/', '\\']).any(|segment| {
        matches!(
            segment,
            "test"
                | "tests"
                | "unittest"
                | "unittests"
                | "__tests__"
                | "samples"
                | "commontest"
                | "jvmtest"
                | "androidtest"
                | "nativetest"
                | "androidunittest"
        ) || segment.ends_with("_test")
            || segment.ends_with("_tests")
            || segment.ends_with("-test")
            || segment.ends_with("-tests")
    });
    path.starts_with("test/")
        || path.starts_with("tests/")
        || path.starts_with("samples/")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("/samples/")
        || path.contains("/unittest/")
        || path.contains("/unittests/")
        || path.contains(".tests/")
        || path.contains(".test/")
        || path.contains("-test-")
        || path.contains("_test.")
        || path.starts_with("test\\")
        || path.starts_with("tests\\")
        || path.starts_with("samples\\")
        || path.contains("\\test\\")
        || path.contains("\\tests\\")
        || path.contains("\\samples\\")
        || path.contains("\\unittest\\")
        || path.contains("\\unittests\\")
        || segment_is_test
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
    "support",
];

pub fn packet_query_stop_term(term: &str) -> bool {
    let lower = term.to_ascii_lowercase();
    PACKET_QUERY_STOP_TERMS.contains(&lower.as_str())
}

pub fn packet_adjacent_query_stop_term(term: &str) -> bool {
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

pub fn packet_terms_contain(terms: &[String], needle: &str) -> bool {
    terms
        .iter()
        .any(|term| term.eq_ignore_ascii_case(needle) || normalize_identifier(term) == needle)
}

pub fn packet_path_matches_query(query: &str, path: Option<&str>) -> bool {
    let Some(path) = path else {
        return false;
    };
    let query_path = packet_display_path(query.trim().trim_start_matches("./"));
    if query_path.is_empty() {
        return false;
    }
    packet_display_path(path) == query_path
}

pub fn packet_display_path(path: &str) -> String {
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
