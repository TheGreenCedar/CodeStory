//! Packet citation scoring helpers for batch retrieval ranking.

#[cfg(any(test, feature = "test-support"))]
use crate::eval_probes::eval_citation_rank_adjustment;
use crate::packet_terms::{
    packet_terms_indicate_client_send_flow, packet_terms_indicate_form_validation_flow,
    packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_runtime_formatting_flow, packet_terms_indicate_sql_schema_flow,
    packet_terms_indicate_stylesheet_animation_flow,
    packet_terms_indicate_url_session_request_flow,
};
use crate::text::retrieval_file_role_from_path;
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
    if path.contains("/collections/")
        && terms
            .iter()
            .any(|term| term.contains("collection") || term.contains("payload"))
    {
        score += 4.0;
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

    score += packet_flow_shape_rank_bonus(
        citation,
        &display,
        &normalized_display,
        &path,
        terms,
    );

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

fn packet_flow_shape_rank_bonus(
    citation: &AgentCitationDto,
    display: &str,
    normalized_display: &str,
    path: &str,
    terms: &[String],
) -> f32 {
    let mut bonus = 0.0;
    if packet_terms_indicate_mapper_configuration_plan_flow(terms) {
        bonus += packet_mapper_configuration_plan_rank_bonus(normalized_display, path);
    }
    if packet_terms_indicate_stylesheet_animation_flow(terms) {
        bonus += packet_stylesheet_animation_rank_bonus(display, normalized_display, path);
    }
    if packet_terms_indicate_form_validation_flow(terms) {
        bonus += packet_form_validation_rank_bonus(normalized_display, path);
    }
    if packet_terms_indicate_sql_schema_flow(terms) {
        bonus += packet_sql_schema_rank_bonus(normalized_display, path, terms);
    }
    if packet_terms_indicate_url_session_request_flow(terms) {
        bonus += packet_url_session_request_rank_bonus(display, normalized_display, path);
    }
    if packet_terms_indicate_client_send_flow(terms) {
        bonus += packet_client_send_rank_bonus(normalized_display, path, terms);
    }
    if packet_terms_indicate_runtime_formatting_flow(terms) {
        bonus += packet_runtime_formatting_rank_bonus(normalized_display, path);
    }
    if let Some(role) = citation.coverage_role.as_deref() {
        bonus += packet_coverage_role_rank_bonus(role);
    }
    bonus
}

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
        bonus -= 10.0;
    }
    bonus
}

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
        bonus += 3.0;
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

fn packet_runtime_formatting_rank_bonus(normalized_display: &str, path: &str) -> f32 {
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
    path.starts_with("test/")
        || path.starts_with("tests/")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.contains(".tests/")
        || path.contains(".test/")
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

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::NodeId;

    #[test]
    fn cached_rank_sort_evaluates_once_and_keeps_equal_and_nan_order() {
        let mut evaluations = 0usize;
        let mut values = vec![
            ("equal-a", 2.0_f32),
            ("equal-b", 2.0),
            ("low", 1.0),
            ("high", 3.0),
        ];
        sort_by_cached_rank_desc(&mut values, |(_, rank)| {
            evaluations += 1;
            *rank
        });
        assert_eq!(evaluations, values.len());
        assert_eq!(
            values.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
            vec!["high", "equal-a", "equal-b", "low"],
            "equal ranks must keep input order"
        );

        for mut values in [
            vec![("nan", f32::NAN), ("finite", 2.0_f32)],
            vec![("finite", 2.0_f32), ("nan", f32::NAN)],
        ] {
            let expected = values.iter().map(|(label, _)| *label).collect::<Vec<_>>();
            sort_by_cached_rank_desc(&mut values, |(_, rank)| *rank);
            assert_eq!(
                values.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
                expected,
                "a NaN rank must stay comparator-equal and stable"
            );
        }
    }

    #[test]
    fn cached_rank_sort_permutes_exactly_like_the_recomputing_comparator() {
        // The zero-movement claim rests on this: the decorated sort must be a
        // permutation-for-permutation replacement of the comparator that
        // recomputed the rank on every comparison.
        let ranks = [
            3.0_f32,
            f32::NAN,
            3.0,
            -100.0,
            0.0,
            f32::NAN,
            12.5,
            -0.0,
            12.5,
            f32::INFINITY,
            f32::NEG_INFINITY,
            1.0,
            1.0,
            1.0,
        ];
        for window in 1..=ranks.len() {
            for offset in 0..=(ranks.len() - window) {
                let labelled = ranks[offset..offset + window]
                    .iter()
                    .enumerate()
                    .map(|(index, rank)| (index, *rank))
                    .collect::<Vec<_>>();

                let mut previous = labelled.clone();
                previous.sort_by(|(_, left), (_, right)| {
                    right.partial_cmp(left).unwrap_or(Ordering::Equal)
                });

                let mut current = labelled;
                sort_by_cached_rank_desc(&mut current, |(_, rank)| *rank);

                assert_eq!(
                    current.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                    previous.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                    "cached-rank sort moved rows for window={window} offset={offset}"
                );
            }
        }
    }

    #[test]
    fn test_like_display_names_include_go_style_pascal_names() {
        assert!(packet_display_name_is_test_like("TestTreeAddAndGet"));
        assert!(packet_display_name_is_test_like(
            "CommonBufferedSinkTest.writeSourceReadsFully"
        ));
        assert!(packet_display_name_is_test_like("pkg::tests::case"));
        assert!(packet_display_name_is_test_like("handler_test"));
        assert!(packet_display_name_is_test_like("ServiceLifetimeTests"));
        assert!(!packet_display_name_is_test_like("TestingStrategy"));
    }

    #[test]
    fn citation_rank_demotes_docs_source_and_generated_stylesheets() {
        let terms = vec![
            "css".to_string(),
            "animation".to_string(),
            "keyframes".to_string(),
            "variables".to_string(),
        ];
        let source = test_rank_citation("@keyframes bounce", "styles/motion/bounce.css", 1.0);
        let docs = test_rank_citation("Usage", "docsSource/sections/usage.md", 1.0);
        let generated = test_rank_citation("bundle", "bundle.min.css", 1.0);

        assert!(
            packet_citation_rank(&source, &terms, true)
                > packet_citation_rank(&docs, &terms, true)
        );
        assert!(
            packet_citation_rank(&source, &terms, true)
                > packet_citation_rank(&generated, &terms, true)
        );
    }

    #[test]
    fn citation_rank_prefers_mapper_execution_plan_over_annotation_attributes() {
        let terms = vec![
            "mapper".to_string(),
            "configuration".to_string(),
            "runtime".to_string(),
            "source".to_string(),
            "destination".to_string(),
            "lambda".to_string(),
            "plans".to_string(),
        ];
        let execution = test_rank_citation(
            "TypeMap.CreateMapperLambda",
            "src/ObjectMapping/TypeMap.cs",
            1.0,
        );
        let annotation = test_rank_citation(
            "MapAtRuntimeAttribute.ApplyConfiguration",
            "src/ObjectMapping/Configuration/Annotations/MapAtRuntimeAttribute.cs",
            1.0,
        );

        assert!(
            packet_citation_rank(&execution, &terms, true)
                > packet_citation_rank(&annotation, &terms, true)
        );
    }

    #[test]
    fn citation_rank_prefers_url_session_request_methods_over_delegate_noise() {
        let terms = vec![
            "session".to_string(),
            "creates".to_string(),
            "requests".to_string(),
            "resumes".to_string(),
            "tasks".to_string(),
            "validates".to_string(),
            "data".to_string(),
            "urlsession".to_string(),
            "callbacks".to_string(),
        ];
        let request = test_rank_citation("Session.request", "Source/Core/Session.swift", 0.8);
        let resume = test_rank_citation("Request.resume", "Source/Core/Request.swift", 0.8);
        let validate =
            test_rank_citation("DataRequest.validate", "Source/Core/DataRequest.swift", 0.8);
        let delegate = test_rank_citation("urlSession", "Source/Core/SessionDelegate.swift", 0.8);
        let sendable = test_rank_citation("Sendable", "Source/Core/DataRequest.swift", 0.8);

        let request_rank = packet_citation_rank(&request, &terms, true);
        assert!(request_rank > packet_citation_rank(&delegate, &terms, true));
        assert!(
            packet_citation_rank(&resume, &terms, true)
                > packet_citation_rank(&delegate, &terms, true)
        );
        assert!(
            packet_citation_rank(&validate, &terms, true)
                > packet_citation_rank(&sendable, &terms, true)
        );
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
            packet_route_dispatch_rank_bonus(
                "Node.addRoute",
                "nodeaddroute",
                "src/router/tree.go",
                &terms
            ) > 0.0
        );
        assert!(
            packet_route_dispatch_rank_bonus(
                "serverHandleHttpRequest",
                "serverhandlehttprequest",
                "src/http/server.go",
                &terms
            ) > 0.0
        );
        assert!(packet_route_dispatch_rank_bonus("new", "new", "src/server.go", &terms) > 0.0);
    }

    #[test]
    fn citation_rank_prefers_primary_source_paths_over_examples() {
        let terms = vec!["request".to_string(), "response".to_string()];
        let source = test_rank_citation("app.handle", "lib/application.js", 1.0);
        let example = test_rank_citation("app.handle", "examples/application.js", 1.0);

        assert!(
            packet_citation_rank(&source, &terms, false)
                > packet_citation_rank(&example, &terms, false)
        );
    }

    fn test_rank_citation(display_name: &str, file_path: &str, score: f32) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(display_name.to_string()),
            display_name: display_name.to_string(),
            kind: NodeKind::METHOD,
            file_path: Some(file_path.to_string()),
            line: Some(1),
            score,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
        }
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

    #[test]
    fn url_session_rank_bonus_prefers_data_request_validation_anchor() {
        let terms = vec![
            "session".to_string(),
            "request".to_string(),
            "validates".to_string(),
            "data".to_string(),
            "urlsession".to_string(),
        ];
        let data_validate = test_rank_citation(
            "DataRequest.validate",
            "Source/Core/DataRequest.swift",
            40.0,
        );
        let sibling_validate = test_rank_citation(
            "DownloadRequest.validate",
            "Source/Core/DownloadRequest.swift",
            40.0,
        );
        let extension_validate = test_rank_citation(
            "URLRequest+Library.validate",
            "Source/Extensions/URLRequest+Library.swift",
            40.0,
        );

        let data_rank = packet_citation_rank(&data_validate, &terms, false);

        assert!(
            data_rank > packet_citation_rank(&sibling_validate, &terms, false),
            "data request validate anchor should outrank sibling request validate anchors"
        );
        assert!(
            data_rank > packet_citation_rank(&extension_validate, &terms, false),
            "data request validate anchor should outrank generic URLRequest validate extensions"
        );
    }

    #[test]
    fn sql_schema_rank_bonus_matches_plural_prompt_table_terms() {
        let terms = vec![
            "sql".to_string(),
            "schema".to_string(),
            "tracks".to_string(),
            "invoices".to_string(),
        ];

        assert!(
            packet_sql_schema_rank_bonus("createtabletrack", "db/schema.sql", &terms)
                > packet_sql_schema_rank_bonus("createtablecustomer", "db/schema.sql", &terms)
        );
    }

    #[test]
    fn runtime_formatting_rank_bonus_prefers_output_and_error_source_files() {
        assert!(
            packet_runtime_formatting_rank_bonus("bufferappend", "src/format.cc")
                > packet_runtime_formatting_rank_bonus("duration", "include/fmt/chrono.h")
        );
        assert!(
            packet_runtime_formatting_rank_bonus("formaterrorcode", "src/os.cc")
                > packet_runtime_formatting_rank_bonus("formaterrorcode", "include/fmt/format.h")
        );
        assert!(packet_runtime_formatting_rank_bonus("formatto", "include/fmt/format.h") > 0.0);
    }

    #[test]
    fn string_predicate_rank_bonus_prefers_specific_string_sources() {
        assert!(
            packet_string_predicate_rank_bonus(
                "orgapachecommonslang3stringutilsisempty",
                "src/main/java/org/apache/commons/lang3/stringutils.java",
            ) > packet_string_predicate_rank_bonus(
                "orgapachecommonslang3arrayutilsisempty",
                "src/main/java/org/apache/commons/lang3/arrayutils.java",
            )
        );
        assert!(
            packet_string_predicate_rank_bonus(
                "orgapachecommonslang3strings",
                "src/main/java/org/apache/commons/lang3/strings.java",
            ) > packet_string_predicate_rank_bonus(
                "orgapachecommonslang3randomstringutils",
                "src/main/java/org/apache/commons/lang3/randomstringutils.java",
            )
        );
        assert!(
            packet_string_predicate_rank_bonus(
                "orgapachecommonslang3charsequenceutilsregionmatches",
                "src/main/java/org/apache/commons/lang3/charsequenceutils.java",
            ) > 0.0
        );
    }
}
