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
    limits.max_anchors.clamp(8, 16) as usize
}

/// Path-local heuristic: SQL dialect variant copies are weaker schema evidence.
pub fn packet_sql_schema_file_is_variant_copy(path: &str) -> bool {
    let lower = packet_display_path(path).to_ascii_lowercase();
    if !lower.ends_with(".sql") {
        return false;
    }
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    file_name.contains("autoincrement")
        || file_name.contains("serialpks")
        || file_name.contains("serial_pks")
        || file_name.contains("db2")
        || file_name.contains("oracle")
        || file_name.contains("sqlserver")
}

/// Candidate hits fetched per planned subquery or anchor-probe batch query.
pub fn packet_subquery_hit_limit(limits: &PacketBudgetLimitsDto) -> usize {
    limits.max_anchors.clamp(8, 20) as usize
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
    let concrete_module_file =
        packet_concrete_module_file_citation(citation.kind, &normalized_display, &path);
    let facade_module_file =
        packet_facade_module_citation(citation.kind, &normalized_display, &path);
    if concrete_module_file {
        score += 2.0;
    }
    if facade_module_file {
        score -= 3.0;
    }
    if citation.kind == NodeKind::MODULE && !concrete_module_file && !facade_module_file {
        score -= 12.0;
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
    if path.contains("/server/") && !packet_terms_contain(terms, "server") {
        score -= 12.0;
    }
    #[cfg(any(test, feature = "test-support"))]
    {
        score = eval_citation_rank_adjustment(&normalized_display, &path, score);
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
    score += packet_shared_source_set_rank_adjustment(&path, terms);

    {
        if normalized_display.chars().count() <= 1 {
            score -= 16.0;
        }
    }
    

    score
}

fn packet_citation_is_formatting_argument_store(citation: &AgentCitationDto) -> bool {
    let normalized = normalize_identifier(&citation.display_name);
    normalized.contains("format")
        && (normalized.contains("arg") || normalized.contains("argument"))
        && normalized.contains("store")
}

fn packet_citation_is_formatter_specialization(citation: &AgentCitationDto) -> bool {
    let display = citation.display_name.trim();
    let normalized = normalize_identifier(display);
    normalized == "formatter" || (display.contains('<') && normalized.starts_with("formatter"))
}

fn packet_animation_base_parent_dirs(citations: &[AgentCitationDto]) -> Vec<String> {
    citations
        .iter()
        .filter(|citation| packet_citation_is_animation_base_source(citation))
        .filter_map(|citation| {
            citation
                .file_path
                .as_deref()
                .map(packet_display_path)
                .map(|path| packet_path_parent_dir(&path))
        })
        .filter(|parent| !parent.is_empty())
        .collect()
}

fn packet_path_is_animation_entry_parent(path: &str, entry_parents: &[String]) -> bool {
    let parent = packet_path_parent_dir(path);
    !parent.is_empty() && entry_parents.iter().any(|kept| kept == &parent)
}

fn packet_stylesheet_path_is_nested(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.contains('/')
}

fn packet_citation_is_animation_file_alias(citation: &AgentCitationDto) -> bool {
    citation
        .coverage_role
        .as_deref()
        .is_some_and(|role| role == "css animation source file")
}

fn packet_citation_is_animation_custom_property(citation: &AgentCitationDto) -> bool {
    citation.display_name.trim_start().starts_with("--")
        || citation
            .coverage_role
            .as_deref()
            .is_some_and(|role| role == "css animation variables")
}

fn packet_citation_is_primary_stylesheet(citation: &AgentCitationDto) -> bool {
    let path = packet_citation_display_path(citation);
    path.ends_with(".css") && !packet_citation_is_non_primary_source(citation)
}

fn packet_citation_is_non_primary_source(citation: &AgentCitationDto) -> bool {
    citation
        .file_path
        .as_deref()
        .is_some_and(|path| retrieval_file_role_from_path(path).is_non_primary())
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

fn packet_mpp_source_set_key(citation: &AgentCitationDto) -> Option<(PacketSourceSetKind, String)> {
    let path = packet_citation_display_path(citation).replace('\\', "/");
    if path.is_empty() {
        return None;
    }
    let mut kind = None;
    let mut parts = Vec::new();
    for segment in path.split('/') {
        if let Some(segment_kind) = packet_source_set_segment_kind(segment) {
            kind = Some(segment_kind);
            parts.push("{srcset}");
        } else {
            parts.push(segment);
        }
    }
    kind.map(|kind| (kind, parts.join("/")))
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

fn packet_stem_tokens_are_display_subset(
    citation: &AgentCitationDto,
    display_tokens: &[String],
) -> bool {
    let stem = packet_path_file_stem(&packet_citation_display_path(citation));
    let stem_tokens = crate::text::symbol_query_tokens(&stem);
    !stem_tokens.is_empty()
        && stem_tokens
            .iter()
            .all(|token| display_tokens.iter().any(|display| display == token))
}

fn packet_citation_is_export_macro_display(citation: &AgentCitationDto) -> bool {
    let name = citation.display_name.trim();
    !name.is_empty()
        && name.contains('_')
        && name
            .chars()
            .any(|character| character.is_ascii_alphabetic())
        && name.chars().all(|character| {
            character.is_ascii_uppercase() || character == '_' || character.is_ascii_digit()
        })
}

fn packet_citation_is_system_format_failure(citation: &AgentCitationDto) -> bool {
    let normalized = normalize_identifier(&citation.display_name);
    normalized.contains("format")
        && normalized.contains("system")
        && (normalized.contains("error")
            || normalized.contains("failure")
            || normalized.contains("exception"))
}

fn packet_citation_is_animation_class_selector(citation: &AgentCitationDto) -> bool {
    if citation
        .coverage_role
        .as_deref()
        .is_some_and(|role| role == "css animation selector")
    {
        return true;
    }
    citation.display_name.trim_start().starts_with('.')
}

fn packet_citation_is_animation_base_source(citation: &AgentCitationDto) -> bool {
    let stem = packet_path_file_stem(&packet_citation_display_path(citation));
    stem == "base" || stem.ends_with("base") || stem == "vars" || stem.ends_with("vars")
}

fn packet_animation_class_stem(citation: &AgentCitationDto) -> String {
    let trimmed = citation.display_name.trim_start().trim_start_matches('.');
    let token = trimmed.rsplit(['-', '_']).next().unwrap_or(trimmed);
    normalize_identifier(token)
}

fn packet_citation_is_wide_char_sibling(citation: &AgentCitationDto) -> bool {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let normalized_display = normalize_identifier(&citation.display_name);
    path.contains("xchar") || path.contains("wchar") || normalized_display.contains("wchar")
}

fn packet_citation_is_python_source(citation: &AgentCitationDto) -> bool {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    path.ends_with(".py") || path.ends_with(".pyi") || path.ends_with(".pyx")
}

fn packet_question_wants_python(terms: &[String]) -> bool {
    terms.iter().any(|term| {
        let normalized = normalize_identifier(term);
        normalized == "py" || normalized == "python" || normalized.contains("python")
    })
}

fn packet_citation_display_path(citation: &AgentCitationDto) -> String {
    citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn packet_question_wants_windows(terms: &[String]) -> bool {
    terms.iter().any(|term| {
        let normalized = normalize_identifier(term);
        normalized.contains("windows") || normalized.contains("win32") || normalized == "win"
    })
}

fn packet_citation_is_windows_formatting_sibling(citation: &AgentCitationDto) -> bool {
    let path = packet_citation_display_path(citation);
    let normalized_display = normalize_identifier(&citation.display_name);
    let normalized_path = normalize_identifier(&path);
    normalized_display.contains("windows")
        || normalized_display.contains("win32")
        || normalized_path.contains("windows")
        || normalized_path.contains("win32")
}

fn packet_citation_is_non_windows_formatter_failure(citation: &AgentCitationDto) -> bool {
    let normalized_display = normalize_identifier(&citation.display_name);
    normalized_display.starts_with("format")
        && normalized_display.ends_with("error")
        && !normalized_display.contains("windows")
        && !normalized_display.contains("system")
        && !normalized_display.contains("duration")
}

fn packet_citation_is_runtime_formatting_core_source(citation: &AgentCitationDto) -> bool {
    let path = packet_citation_display_path(citation);
    let stem = packet_path_file_stem(&path);
    stem.contains("format") || stem == "args" || stem == "base" || stem == "os" || stem == "fmt"
}

fn packet_citation_is_single_letter_display(citation: &AgentCitationDto) -> bool {
    citation
        .display_name
        .rsplit([':', '.', '#'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(citation.display_name.as_str())
        .chars()
        .count()
        <= 1
}

fn packet_named_client_adapter_prefix(path: &str) -> Option<String> {
    let stem = packet_path_file_stem(path);
    stem.strip_suffix("_client")
        .or_else(|| stem.strip_suffix("client"))
        .filter(|prefix| !prefix.is_empty() && prefix.chars().count() >= 2)
        .map(normalize_identifier)
}

fn packet_question_names_client_prefix(terms: &[String], prefix: &str) -> bool {
    terms.iter().any(|term| {
        let normalized = normalize_identifier(term);
        normalized.contains(prefix) && normalized.contains("client")
    })
}

fn packet_question_names_any_client_adapter(
    terms: &[String],
    citations: &[AgentCitationDto],
) -> bool {
    citations.iter().any(|citation| {
        let path = packet_citation_display_path(citation);
        packet_named_client_adapter_prefix(&path)
            .is_some_and(|prefix| packet_question_names_client_prefix(terms, &prefix))
    })
}

fn packet_citation_is_unrequested_client_adapter(
    citation: &AgentCitationDto,
    terms: &[String],
) -> bool {
    let path = packet_citation_display_path(citation);
    packet_named_client_adapter_prefix(&path)
        .is_some_and(|prefix| !packet_question_names_client_prefix(terms, &prefix))
}

fn packet_citation_is_example_or_generated_binding(citation: &AgentCitationDto) -> bool {
    let path = packet_citation_display_path(citation);
    let stem = packet_path_file_stem(&path);
    path.contains("/example")
        || path.contains("\\example")
        || path.contains("_example")
        || path.contains("-example")
        || path.contains("/jni/")
        || path.contains("\\jni\\")
        || stem == "bindings"
        || stem == "binding"
}

fn packet_question_wants_annotations(terms: &[String]) -> bool {
    terms.iter().any(|term| {
        let normalized = normalize_identifier(term);
        normalized.contains("annotation") || normalized.contains("attribute")
    })
}

fn packet_citation_is_mapper_annotation_sibling(citation: &AgentCitationDto) -> bool {
    let path = packet_citation_display_path(citation);
    let normalized_display = normalize_identifier(&citation.display_name);
    path.contains("/annotations/")
        || path.contains("\\annotations\\")
        || path.contains("/annotation/")
        || normalized_display.contains("attribute")
}

fn packet_question_wants_tests(terms: &[String]) -> bool {
    terms.iter().any(|term| {
        let normalized = normalize_identifier(term);
        normalized == "test"
            || normalized == "tests"
            || normalized.starts_with("test")
            || normalized.contains("unittest")
    })
}

fn packet_citation_is_test_source(citation: &AgentCitationDto) -> bool {
    citation
        .file_path
        .as_deref()
        .is_some_and(|path| retrieval_file_role_from_path(path) == RetrievalFileRole::Test)
        || packet_path_is_test_segment(&packet_citation_display_path(citation))
        || packet_display_name_is_test_like(&citation.display_name)
}

fn packet_citation_is_keyframe_rule(citation: &AgentCitationDto) -> bool {
    citation
        .display_name
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("@keyframes")
}

fn packet_keyframe_rule_stem(citation: &AgentCitationDto) -> String {
    citation
        .display_name
        .trim_start_matches(|ch: char| ch.is_whitespace() || ch == '@')
        .split_whitespace()
        .nth(1)
        .map(normalize_identifier)
        .unwrap_or_default()
}

fn packet_keyframe_stems_named_in_question(terms: &[String]) -> Vec<String> {
    terms
        .iter()
        .map(|term| normalize_identifier(term))
        .filter(|term| term.len() >= 3 && !packet_query_stop_term(term))
        .collect()
}

fn packet_keyframe_stem_is_named(citation: &AgentCitationDto, named_stems: &[String]) -> bool {
    let stem = packet_keyframe_rule_stem(citation);
    !stem.is_empty() && named_stems.iter().any(|named| named == &stem)
}

fn packet_citation_is_markdown_source(citation: &AgentCitationDto) -> bool {
    let path = packet_citation_display_path(citation);
    path.ends_with(".md") || path.ends_with(".markdown") || path.ends_with(".mdx")
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

#[cfg(test)]
fn packet_route_dispatch_rank_bonus(
    display: &str,
    normalized_display: &str,
    path: &str,
    terms: &[String],
) -> f32 {
    let mut bonus = 0.0;
    bonus += packet_request_dispatch_anchor_rank_bonus(display, normalized_display, path);
    if path.contains("/examples/") || path.starts_with("examples/") {
        bonus -= 14.0;
    }
    if normalized_display.contains("create") && normalized_display.contains("application") {
        bonus += 8.0;
    }
    if normalized_display.contains("add") && normalized_display.contains("route") {
        bonus += 7.0;
    }
    if normalized_display.contains("handle")
        && (normalized_display.contains("request") || normalized_display.contains("http"))
    {
        bonus += 7.0;
    }
    if normalized_display.ends_with("next") && (path.contains("context") || path.contains("ctx")) {
        bonus += 5.0;
    }
    if normalized_display.contains("combine") && normalized_display.contains("handler") {
        bonus += 3.0;
    }
    let path_stem = packet_path_file_stem(path);
    if path.contains("/lib/")
        && (path_stem.contains("application") || path_stem.contains("response"))
        && packet_request_dispatch_method_tail(normalized_display)
    {
        bonus += 8.0;
    }
    if normalized_display == "new" && packet_terms_contain(terms, "engine") {
        bonus += 4.0;
    }
    bonus
}

#[cfg(test)]
fn packet_request_dispatch_anchor_rank_bonus(
    display: &str,
    normalized_display: &str,
    path: &str,
) -> f32 {
    let mut bonus = 0.0;
    let role = retrieval_file_role_from_path(path);
    if packet_request_dispatch_artifact_anchor(normalized_display, path) {
        bonus -= 18.0;
    } else if role.is_non_primary() {
        bonus -= 10.0;
    }
    if role == crate::text::RetrievalFileRole::Source
        && packet_application_router_response_source_anchor(display, normalized_display, path)
    {
        bonus += 8.0;
    }
    bonus
}

#[cfg(test)]
fn packet_request_dispatch_artifact_anchor(normalized_display: &str, path: &str) -> bool {
    normalized_display.starts_with("componentreport")
        || normalized_display.contains("schemareference")
        || path.contains("component_report")
        || path.contains("component-report")
        || path.contains("schema_reference")
        || path.contains("schema-reference")
}

#[cfg(test)]
fn packet_application_router_response_source_anchor(
    display: &str,
    normalized_display: &str,
    path: &str,
) -> bool {
    if normalized_display.contains("create") && normalized_display.contains("application") {
        return true;
    }
    if let Some((owner, method)) = packet_display_owner_and_method(display)
        && packet_request_dispatch_receiver_owner(&owner)
        && packet_request_dispatch_method_tail(&method)
    {
        return true;
    }
    let path_stem = packet_path_file_stem(path);
    packet_request_dispatch_owner_path_stem(&path_stem)
        && packet_request_dispatch_method_tail(normalized_display)
}

#[cfg(test)]
fn packet_display_owner_and_method(display: &str) -> Option<(String, String)> {
    let trimmed = display.trim();
    for separator in ['.', '#', ':'] {
        if let Some(index) = trimmed.rfind(separator) {
            let owner = normalize_identifier(&trimmed[..index]);
            let method = normalize_identifier(&trimmed[index + separator.len_utf8()..]);
            if !owner.is_empty() && !method.is_empty() {
                return Some((owner, method));
            }
        }
    }
    None
}

#[cfg(test)]
fn packet_request_dispatch_receiver_owner(owner: &str) -> bool {
    matches!(
        owner,
        "app" | "application" | "router" | "route" | "res" | "response"
    )
}

#[cfg(test)]
fn packet_request_dispatch_owner_path_stem(path_stem: &str) -> bool {
    path_stem.contains("app")
        || path_stem.contains("application")
        || path_stem.contains("router")
        || path_stem.contains("route")
        || path_stem.contains("response")
}

#[cfg(test)]
fn packet_request_dispatch_method_tail(method: &str) -> bool {
    matches!(
        method,
        "dispatch" | "handle" | "use" | "route" | "send" | "json" | "end" | "respond"
    )
}

#[cfg(test)]
fn packet_buffered_io_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    let has_buffer = display_or_path.contains("buffer");
    let has_source = display_or_path.contains("source");
    let has_sink = display_or_path.contains("sink");
    if has_buffer && (has_source || has_sink) {
        bonus += 6.0;
    }
    if normalized_display.contains("read") && has_source && has_buffer {
        bonus += 3.0;
    }
    if normalized_display.contains("write") && has_sink && has_buffer {
        bonus += 3.0;
    }
    if normalized_display == "buffer" && path.contains("buffer") {
        bonus += 2.0;
    }
    if path.contains("commonmain") && has_buffer && (has_source || has_sink) {
        bonus += 2.0;
    }
    bonus
}

#[cfg(test)]
fn packet_flow_shape_rank_bonus(
    citation: &AgentCitationDto,
    display: &str,
    normalized_display: &str,
    path: &str,
    terms: &[String],
) -> f32 {
    let mut bonus = 0.0;
    bonus += packet_mapper_configuration_plan_rank_bonus(normalized_display, path);
    bonus += packet_stylesheet_animation_rank_bonus(display, normalized_display, path);
    bonus += packet_form_validation_rank_bonus(normalized_display, path);
    bonus += packet_sql_schema_rank_bonus(normalized_display, path, terms);
    bonus += packet_url_session_request_rank_bonus(display, normalized_display, path);
    bonus += packet_client_send_rank_bonus(normalized_display, path, terms);
    bonus += packet_runtime_formatting_rank_bonus(normalized_display, path, terms);
    if let Some(role) = citation.coverage_role.as_deref() {
        bonus += packet_coverage_role_rank_bonus(role);
    }
    bonus
}

#[cfg(test)]
fn packet_coverage_role_rank_bonus(role: &str) -> f32 {
    match role {
        "mapping execution plan" | "css keyframes" | "css animation selector" => 18.0,
        "css animation variables" | "css animation import" | "sql schema anchor" => 14.0,
        "form_native_constraints"
        | "form_pattern_constraint"
        | "request_resume_dispatch"
        | "request_validation_pipeline" => 12.0,
        "material schema entity" => 16.0,
        _ => 0.0,
    }
}

#[cfg(test)]
fn packet_stylesheet_animation_rank_bonus(
    display: &str,
    normalized_display: &str,
    path: &str,
) -> f32 {
    let mut bonus = 0.0;
    if path.ends_with(".css") {
        if path.contains('/') {
            bonus += 4.0;
        } else {
            bonus -= 8.0;
        }
    }
    if display.contains("@keyframes")
        || normalized_display.contains("keyframes")
        || path.contains("/keyframes")
    {
        bonus += 10.0;
    }
    if display.starts_with("--")
        && (normalized_display.contains("animat")
            || normalized_display.contains("duration")
            || normalized_display.contains("delay")
            || normalized_display.contains("repeat"))
    {
        bonus += 8.0;
    }
    if display.starts_with('.') && normalized_display.contains("animat") {
        bonus += 3.0;
    }
    bonus
}

#[cfg(test)]
fn packet_url_session_request_rank_bonus(
    display: &str,
    normalized_display: &str,
    path: &str,
) -> f32 {
    let mut bonus = 0.0;
    let path_stem = packet_path_file_stem(path);
    if let Some((owner, method)) = packet_display_owner_and_method(display) {
        if matches!(owner.as_str(), "session" | "request" | "datarequest")
            && matches!(method.as_str(), "request" | "resume" | "validate")
        {
            bonus += 10.0;
        }
        if owner == "session" && method == "request" {
            bonus += 4.0;
        }
        if owner == "datarequest" && method == "validate" {
            bonus += 4.0;
        }
    }
    if path_stem == "session" || path_stem == "request" || path_stem == "datarequest" {
        bonus += 3.0;
    }
    if normalized_display == "sendable"
        || normalized_display.contains("eventmonitor")
        || path.contains("/features/")
        || path.contains("httpheaders")
    {
        bonus -= 8.0;
    }
    bonus
}

#[cfg(test)]
fn packet_mapper_configuration_plan_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    let path_stem = packet_path_file_stem(path);
    if path_stem.contains("mapper") && path.ends_with(".cs") {
        bonus += 5.0;
    }
    if normalized_display.contains("imapper") || normalized_display.contains("mappermap") {
        bonus += 5.0;
    }
    if display_or_path.contains("mapper") && display_or_path.contains("configuration") {
        bonus += 6.0;
    }
    if path_stem.contains("typemap") || normalized_display.contains("typemap") {
        bonus += 6.0;
    }
    if (path_stem.contains("plan") && path_stem.ends_with("builder"))
        || normalized_display.contains("planbuilder")
    {
        bonus += 8.0;
    }
    if normalized_display.contains("createmapperlambda")
        || normalized_display.contains("buildexecutionplan")
    {
        bonus += 7.0;
    }
    if path.contains("/configuration/annotations/")
        || display_or_path.contains("attribute")
        || display_or_path.contains("exception")
    {
        bonus -= 8.0;
    }
    if path.contains("/mappers/") && !display_or_path.contains("typemap") {
        bonus -= 4.0;
    }
    bonus
}

#[cfg(test)]
fn packet_client_send_rank_bonus(normalized_display: &str, path: &str, terms: &[String]) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    if packet_path_has_prompt_package_segment(path, terms) {
        bonus += 5.0;
    } else if path.contains("/pkgs/") || path.contains("/packages/") {
        bonus -= 5.0;
    }
    let path_stem = packet_path_file_stem(path);
    if path_stem == "http" && !path.contains("/src/") {
        bonus += 7.0;
    }
    if path_stem == "client" && normalized_display.contains("client") {
        bonus += 7.0;
    }
    if path_stem == "base_client"
        || (normalized_display.contains("base")
            && normalized_display.contains("client")
            && normalized_display.contains("send"))
    {
        bonus += 6.0;
    }
    if path_stem == "base_request"
        || (normalized_display.contains("base")
            && normalized_display.contains("request")
            && normalized_display.contains("finalize"))
    {
        bonus += 6.0;
    }
    if (path_stem.contains("io") && path_stem.contains("client"))
        || normalized_display.contains("ioclientsend")
    {
        bonus += 7.0;
    }
    if path_stem == "response" || normalized_display.contains("responsefromstream") {
        bonus += 4.0;
    }
    if normalized_display.contains("native") || display_or_path.contains("bindings") {
        bonus -= 4.0;
    }
    bonus
}

#[cfg(test)]
fn packet_path_has_prompt_package_segment(path: &str, terms: &[String]) -> bool {
    let segments = path
        .split(['/', '\\'])
        .map(normalize_identifier)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    terms.iter().any(|term| {
        let normalized_term = normalize_identifier(term);
        normalized_term.len() >= 3 && segments.iter().any(|segment| segment == &normalized_term)
    })
}

fn packet_path_file_stem(path: &str) -> String {
    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path).trim();
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    normalize_identifier(stem)
}

fn packet_path_parent_dir(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let trimmed = normalized.trim_end_matches('/');
    trimmed
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
fn packet_form_validation_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let normalized_path = normalize_identifier(path);
    let is_html = path.ends_with(".html");
    if normalized_path.contains("form") && normalized_path.contains("validation") {
        bonus += 8.0;
    }
    if is_html && normalized_path.contains("form") && normalized_path.contains("example") {
        bonus += 6.0;
    }
    if is_html && normalized_path.contains("custom") && normalized_path.contains("validation") {
        bonus += 7.0;
    }
    if is_html
        && (normalized_path.contains("pattern")
            || normalized_path.contains("constraint")
            || (normalized_path.contains("min") && normalized_path.contains("max")))
    {
        bonus += 5.0;
    }
    if normalized_display.contains("error")
        || normalized_display.contains("validity")
        || normalized_display.contains("validate")
        || normalized_display.contains("input")
        || normalized_display == "pattern"
        || normalized_display == "required"
        || normalized_display == "min"
        || normalized_display == "max"
    {
        bonus += 6.0;
    }
    if path.contains("/accessibility/") {
        bonus -= 10.0;
    }
    if !path.ends_with(".html") {
        bonus -= 24.0;
    }
    bonus
}

#[cfg(test)]
fn packet_sql_schema_rank_bonus(normalized_display: &str, path: &str, terms: &[String]) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");

    if path.ends_with(".sql") {
        bonus += 5.0;
    }
    if normalized_display.contains("createtable") || normalized_display.contains("create_table") {
        bonus += 8.0;
    }
    if normalized_display.contains("foreignkey")
        || normalized_display.contains("foreign_key")
        || normalized_display.contains("references")
    {
        bonus += 8.0;
    }
    if display_or_path.contains("sqlite")
        || display_or_path.contains("mysql")
        || display_or_path.contains("postgres")
        || display_or_path.contains("postgresql")
        || display_or_path.contains("sqlserver")
    {
        bonus += 8.0;
    }
    if path.contains("/test/")
        || path.contains("/tests/")
        || path.contains(".test/")
        || path.contains("fixture")
    {
        bonus -= 14.0;
    }
    if normalized_display.contains("createtable") {
        for term in terms {
            let normalized_term = normalize_identifier(term);
            if normalized_term.len() < 4 || packet_query_stop_term(&normalized_term) {
                continue;
            }
            let singular = if let Some(prefix) = normalized_term.strip_suffix("ies") {
                format!("{prefix}y")
            } else if let Some(prefix) = normalized_term.strip_suffix('s') {
                prefix.to_string()
            } else {
                normalized_term.clone()
            };
            if singular.len() >= 4 && normalized_display.contains(&singular) {
                bonus += 6.0;
            }
        }
    }

    bonus
}

fn packet_runtime_formatting_question_wants_wide_char(terms: &[String]) -> bool {
    terms.iter().any(|term| {
        let normalized = normalize_identifier(term);
        normalized.contains("wchar")
            || normalized.contains("wide")
            || normalized == "xchar"
            || normalized.contains("wchar_t")
    })
}

fn packet_runtime_formatting_wide_char_sibling_rank_bonus(
    normalized_display: &str,
    path: &str,
    terms: &[String],
) -> f32 {
    if packet_runtime_formatting_question_wants_wide_char(terms) {
        return 0.0;
    }
    let normalized_path = normalize_identifier(path);
    if normalized_path.contains("xchar")
        || normalized_display.contains("wchar")
        || normalized_path.contains("wchar")
    {
        -8.0
    } else {
        0.0
    }
}

#[cfg(test)]
fn packet_runtime_formatting_rank_bonus(
    normalized_display: &str,
    path: &str,
    terms: &[String],
) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    let path_stem = packet_path_file_stem(path);
    let is_compiled_source =
        path.ends_with(".cc") || path.ends_with(".cpp") || path.ends_with(".cxx");

    if path_stem == "format" && (path.ends_with(".h") || path.ends_with(".hpp")) {
        bonus += 4.0;
    }
    if path_stem == "format" && is_compiled_source {
        bonus += 8.0;
    }
    if (path_stem == "os" || path_stem.contains("system")) && is_compiled_source {
        bonus += 7.0;
    }
    if normalized_display.contains("formatargstore")
        || normalized_display.contains("basicformatargs")
        || normalized_display.contains("dynamicformatargstore")
    {
        bonus += 7.0;
    }
    if normalized_display.contains("vformat")
        || normalized_display.contains("vformatto")
        || normalized_display.contains("formatto")
    {
        bonus += 8.0;
    }
    if normalized_display.contains("formaterror")
        || normalized_display.contains("formaterrorcode")
        || normalized_display.contains("formatwindowserror")
    {
        bonus += 8.0;
    }
    if display_or_path.contains("buffer") && display_or_path.contains("append") {
        bonus += 7.0;
    }
    if display_or_path.contains("chrono")
        || display_or_path.contains("ranges")
        || display_or_path.contains("compile")
        || display_or_path.contains("support")
    {
        bonus -= 5.0;
    }
    bonus +=
        packet_runtime_formatting_wide_char_sibling_rank_bonus(normalized_display, path, terms);

    bonus
}

#[cfg(test)]
fn packet_string_predicate_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    let path_stem = packet_path_file_stem(path);

    if path_stem.starts_with("string") && path_stem.ends_with("utils") {
        bonus += 7.0;
    }
    if path_stem == "strings" {
        bonus += 8.0;
    }
    if path_stem.contains("charsequence") && path_stem.ends_with("utils") {
        bonus += 7.0;
    }
    if normalized_display.contains("string")
        && normalized_display.contains("utils")
        && (normalized_display.contains("isblank") || normalized_display.contains("isempty"))
    {
        bonus += 8.0;
    }
    if normalized_display.contains("strings")
        || normalized_display.ends_with("cs")
        || normalized_display.ends_with("ci")
    {
        bonus += 6.0;
    }
    if normalized_display.contains("regionmatches") {
        bonus += 8.0;
    }
    if display_or_path.contains("arrayutils")
        || display_or_path.contains("annotationutils")
        || display_or_path.contains("circuitbreaker")
        || (display_or_path.contains("random") && display_or_path.contains("string"))
    {
        bonus -= 10.0;
    }

    bonus
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
    "requests",
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

pub fn packet_file_stem_matches_query(query: &str, path: Option<&str>) -> bool {
    let Some(path) = path else {
        return false;
    };
    let query_path = query.replace('\\', "/");
    let query_file_name = query_path.rsplit('/').next().unwrap_or(query).trim();
    let query_stem = query_file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(query_file_name);
    let normalized_query = normalize_identifier(query_stem);
    if normalized_query.is_empty() {
        return false;
    }
    let normalized_path = path.replace('\\', "/");
    let file_name = normalized_path
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim();
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    normalize_identifier(stem) == normalized_query
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

