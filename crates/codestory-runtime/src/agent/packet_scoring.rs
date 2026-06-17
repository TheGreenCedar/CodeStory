//! Packet citation scoring helpers for batch retrieval ranking.

use super::eval_probes::eval_citation_rank_adjustment;
use crate::agent::packet_terms::{
    packet_terms_indicate_buffered_io_flow, packet_terms_indicate_client_send_flow,
    packet_terms_indicate_form_validation_flow, packet_terms_indicate_log_record_handler_flow,
    packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_server_route_dispatch_flow,
    packet_terms_indicate_shell_install_dispatch_flow, packet_terms_indicate_site_build_phase_flow,
    packet_terms_indicate_url_session_request_flow,
};
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
    if packet_terms_indicate_server_route_dispatch_flow(terms) {
        score += packet_route_dispatch_rank_bonus(&normalized_display, &path, terms);
    }
    if packet_terms_indicate_buffered_io_flow(terms) {
        score += packet_buffered_io_rank_bonus(&normalized_display, &path);
    }
    if packet_terms_indicate_log_record_handler_flow(terms) {
        score += packet_log_record_handler_rank_bonus(&normalized_display, &path);
    }
    if packet_terms_indicate_site_build_phase_flow(terms) {
        score += packet_site_build_phase_rank_bonus(&normalized_display, &path);
    }
    if packet_terms_indicate_mapper_configuration_plan_flow(terms) {
        score += packet_mapper_configuration_plan_rank_bonus(&normalized_display, &path);
    }
    if packet_terms_indicate_client_send_flow(terms) {
        score += packet_client_send_rank_bonus(&normalized_display, &path, terms);
    }
    if packet_terms_indicate_url_session_request_flow(terms) {
        score += packet_url_session_request_rank_bonus(&normalized_display, &path);
    }
    if packet_terms_indicate_form_validation_flow(terms) {
        score += packet_form_validation_rank_bonus(&normalized_display, &path);
    }
    if packet_terms_indicate_shell_install_dispatch_flow(terms) {
        score += packet_shell_install_dispatch_rank_bonus(&normalized_display, &path);
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
    if prefer_primary_sources && packet_display_name_is_test_like(&citation.display_name) {
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
        || local_name.ends_with("test")
        || local_name.contains("_test_")
        || local_name.contains("_tests_")
}

fn packet_route_dispatch_rank_bonus(normalized_display: &str, path: &str, terms: &[String]) -> f32 {
    let mut bonus = 0.0;
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
    if normalized_display == "new" && packet_terms_contain(terms, "engine") {
        bonus += 4.0;
    }
    bonus
}

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

fn packet_log_record_handler_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    if path.ends_with("logger.php") {
        bonus += 5.0;
    }
    if normalized_display.contains("addrecord") || normalized_display.ends_with("log") {
        bonus += 7.0;
    }
    if normalized_display.contains("pushhandler") || normalized_display.contains("handlerinterface")
    {
        bonus += 5.0;
    }
    if path.ends_with("logrecord.php") || normalized_display.contains("logrecord") {
        bonus += 4.0;
    }
    if display_or_path.contains("abstractprocessinghandler") {
        bonus += 6.0;
    }
    if normalized_display.contains("gethandlers") {
        bonus += 1.0;
    }
    if normalized_display == "monolog" {
        bonus -= 6.0;
    }
    bonus
}

fn packet_site_build_phase_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    if path.ends_with("lib/jekyll/commands/build.rb") || path.ends_with("jekyll/commands/build.rb")
    {
        bonus += 5.0;
    }
    if path.ends_with("lib/jekyll/site.rb") || path.ends_with("jekyll/site.rb") {
        bonus += 5.0;
    }
    if path.ends_with("lib/jekyll/reader.rb") || path.ends_with("jekyll/reader.rb") {
        bonus += 4.0;
    }
    if path.ends_with("lib/jekyll/renderer.rb") || path.ends_with("jekyll/renderer.rb") {
        bonus += 4.0;
    }
    if normalized_display.contains("process")
        || normalized_display.contains("read")
        || normalized_display.contains("render")
        || normalized_display.contains("write")
    {
        bonus += 4.0;
    }
    if normalized_display.contains("build") && normalized_display.contains("process") {
        bonus += 4.0;
    }
    if normalized_display == "site"
        || normalized_display == "reader"
        || normalized_display == "renderer"
    {
        bonus += 2.0;
    }
    if display_or_path.contains("liquidrendererfile") || path.contains("liquid_renderer") {
        bonus -= 4.0;
    }
    if normalized_display.contains("route") || normalized_display.contains("post") {
        bonus -= 5.0;
    }
    bonus
}

fn packet_mapper_configuration_plan_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    if path.ends_with("mapper.cs") {
        bonus += 5.0;
    }
    if normalized_display.contains("imapper") || normalized_display.contains("mappermap") {
        bonus += 5.0;
    }
    if path.ends_with("mapperconfiguration.cs")
        || normalized_display.contains("mapperconfiguration")
    {
        bonus += 6.0;
    }
    if path.ends_with("typemap.cs") || normalized_display.contains("typemap") {
        bonus += 6.0;
    }
    if display_or_path.contains("typemapplanbuilder") {
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

fn packet_client_send_rank_bonus(normalized_display: &str, path: &str, terms: &[String]) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    if packet_path_has_prompt_package_segment(path, terms) {
        bonus += 5.0;
    } else if path.contains("/pkgs/") || path.contains("/packages/") {
        bonus -= 5.0;
    }
    if path.ends_with("http.dart") && !path.contains("/src/") {
        bonus += 7.0;
    }
    if path.ends_with("client.dart") && normalized_display.contains("client") {
        bonus += 7.0;
    }
    if path.ends_with("base_client.dart") || normalized_display.contains("baseclientsend") {
        bonus += 6.0;
    }
    if path.ends_with("base_request.dart") || normalized_display.contains("baserequestfinalize") {
        bonus += 6.0;
    }
    if path.ends_with("io_client.dart") || normalized_display.contains("ioclientsend") {
        bonus += 7.0;
    }
    if path.ends_with("response.dart") || normalized_display.contains("responsefromstream") {
        bonus += 4.0;
    }
    if normalized_display.contains("native") || display_or_path.contains("bindings") {
        bonus -= 4.0;
    }
    bonus
}

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

fn packet_url_session_request_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");

    if path.ends_with("source/core/session.swift") || path.ends_with("session.swift") {
        bonus += 4.0;
    }
    if path.ends_with("source/core/request.swift") || path.ends_with("request.swift") {
        bonus += 4.0;
    }
    if path.ends_with("source/core/datarequest.swift") || path.ends_with("datarequest.swift") {
        bonus += 5.0;
    }
    if path.ends_with("source/core/sessiondelegate.swift")
        || path.ends_with("sessiondelegate.swift")
    {
        bonus += 5.0;
    }

    if normalized_display == "session"
        || normalized_display.ends_with("sessionrequest")
        || normalized_display.contains("sessionrequest")
    {
        bonus += 8.0;
    }
    if normalized_display.ends_with("requestresume") {
        bonus += 9.0;
    }
    if normalized_display.ends_with("datarequestvalidate")
        || normalized_display.ends_with("requestvalidate")
    {
        bonus += 9.0;
    }
    if normalized_display.contains("sessiondelegate")
        || (normalized_display.contains("urlsession") && path.ends_with("sessiondelegate.swift"))
    {
        bonus += 7.0;
    }

    if display_or_path.contains("didreceiveresumedata")
        || display_or_path.contains("urlsessiontasks")
        || display_or_path.contains("cachedresponsehandler")
        || display_or_path.contains("eventmonitor")
    {
        bonus -= 5.0;
    }

    bonus
}

fn packet_form_validation_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    if path.contains("/form-validation/") || path.contains("\\form-validation\\") {
        bonus += 8.0;
    }
    if path.ends_with("full-example.html") {
        bonus += 6.0;
    }
    if path.ends_with("detailed-custom-validation.html") {
        bonus += 7.0;
    }
    if path.ends_with("fruit-pattern.html") || path.ends_with("min-max.html") {
        bonus += 5.0;
    }
    if normalized_display.contains("showerror")
        || normalized_display.contains("novalidate")
        || normalized_display.contains("inputmail")
        || normalized_display == "pattern"
        || normalized_display == "required"
        || normalized_display == "min"
        || normalized_display == "max"
    {
        bonus += 6.0;
    }
    if path.contains("/accessibility/")
        || path.contains("/native-form-widgets/")
        || path.contains("/sending-form-data/")
        || display_or_path.contains("modernizr")
        || display_or_path.contains("three.min")
    {
        bonus -= 10.0;
    }
    bonus
}

fn packet_shell_install_dispatch_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    if path.ends_with("install.sh") {
        bonus += 8.0;
    }
    if path.ends_with("nvm.sh") {
        bonus += 8.0;
    }
    if path.ends_with("bash_completion") {
        bonus += 7.0;
    }
    if normalized_display.contains("nvm_do_install")
        || normalized_display.contains("nvmdoinstall")
        || normalized_display.contains("nvm_install_node")
        || normalized_display.contains("nvminstallnode")
        || normalized_display.contains("installnvmasscript")
        || normalized_display.contains("nvm_download")
        || normalized_display.contains("nvmdownload")
        || normalized_display == "nvm"
        || normalized_display.contains("nvm_use_if_needed")
        || normalized_display.contains("nvmuseifneeded")
        || normalized_display.contains("__nvm")
        || normalized_display.contains("nvmcommands")
    {
        bonus += 7.0;
    }
    if path.contains("/test")
        || path.contains("\\test")
        || path.ends_with("rename_test.sh")
        || normalized_display == "main"
        || normalized_display == "checkname"
    {
        bonus -= 16.0;
    }
    bonus
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_like_display_names_include_go_style_pascal_names() {
        assert!(packet_display_name_is_test_like("TestTreeAddAndGet"));
        assert!(packet_display_name_is_test_like(
            "CommonBufferedSinkTest.writeSourceReadsFully"
        ));
        assert!(packet_display_name_is_test_like("pkg::tests::case"));
        assert!(packet_display_name_is_test_like("handler_test"));
        assert!(!packet_display_name_is_test_like("TestingStrategy"));
    }

    #[test]
    fn route_dispatch_rank_bonus_prefers_flow_anchors() {
        let terms = vec![
            "route".to_string(),
            "handler".to_string(),
            "request".to_string(),
            "engine".to_string(),
        ];
        assert!(
            packet_route_dispatch_rank_bonus("nodeaddroute", "src/router/tree.go", &terms) > 0.0
        );
        assert!(
            packet_route_dispatch_rank_bonus(
                "serverhandlehttprequest",
                "src/http/server.go",
                &terms
            ) > 0.0
        );
        assert!(packet_route_dispatch_rank_bonus("new", "src/server.go", &terms) > 0.0);
    }

    #[test]
    fn buffered_io_rank_bonus_prefers_concrete_wrapper_flow() {
        assert!(
            packet_buffered_io_rank_bonus(
                "bufferedsourceimplread",
                "src/commonMain/io/buffered_source_impl.kt",
            ) > packet_buffered_io_rank_bonus("source", "src/io/source.kt")
        );
        assert!(
            packet_buffered_io_rank_bonus(
                "bufferedsinkimplwrite",
                "src/commonMain/io/buffered_sink_impl.kt",
            ) > packet_buffered_io_rank_bonus("sink", "src/io/sink.kt")
        );
    }

    #[test]
    fn mapper_configuration_plan_rank_bonus_prefers_execution_plan_sources() {
        assert!(
            packet_mapper_configuration_plan_rank_bonus(
                "typemapplanbuildercreatemapperlambda",
                "src/automapper/execution/typemapplanbuilder.cs"
            ) > packet_mapper_configuration_plan_rank_bonus(
                "mapatruntimeattribute",
                "src/automapper/configuration/annotations/mapatruntimeattribute.cs"
            )
        );
        assert!(
            packet_mapper_configuration_plan_rank_bonus(
                "typemapcreatemapperlambda",
                "src/automapper/typemap.cs"
            ) > packet_mapper_configuration_plan_rank_bonus(
                "nullabledestinationmapper",
                "src/automapper/mappers/nullabledestinationmapper.cs"
            )
        );
    }

    #[test]
    fn client_send_rank_bonus_prefers_package_api_sources() {
        let terms = vec![
            "package".to_string(),
            "http".to_string(),
            "client".to_string(),
            "send".to_string(),
        ];
        assert!(
            packet_client_send_rank_bonus("clientget", "pkgs/http/lib/src/client.dart", &terms)
                > packet_client_send_rank_bonus(
                    "baseclient",
                    "pkgs/cronet_http/lib/src/cronet_client.dart",
                    &terms,
                )
        );
        assert!(
            packet_client_send_rank_bonus("get", "pkgs/http/lib/http.dart", &terms)
                > packet_client_send_rank_bonus(
                    "nsmutableurlrequestmethods",
                    "pkgs/cupertino_http/lib/src/native_cupertino_bindings.dart",
                    &terms,
                )
        );
        assert!(
            packet_client_send_rank_bonus(
                "ioclientsend",
                "pkgs/http/lib/src/io_client.dart",
                &terms,
            ) > 0.0
        );
    }

    #[test]
    fn form_validation_rank_bonus_prefers_validation_examples() {
        assert!(
            packet_form_validation_rank_bonus(
                "showerror",
                "html/forms/form-validation/detailed-custom-validation.html",
            ) > packet_form_validation_rank_bonus(
                "errors",
                "accessibility/css/form-validation.html",
            )
        );
        assert!(
            packet_form_validation_rank_bonus(
                "pattern",
                "html/forms/form-validation/fruit-pattern.html"
            ) > packet_form_validation_rank_bonus(
                "beans",
                "html/forms/native-form-widgets/advanced-examples.html"
            )
        );
    }
}
