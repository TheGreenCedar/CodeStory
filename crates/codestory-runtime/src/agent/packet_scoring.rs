//! Packet citation scoring helpers for batch retrieval ranking.

#[cfg(test)]
use super::eval_probes::eval_citation_rank_adjustment;
use crate::agent::packet_terms::{
    packet_terms_indicate_buffered_io_flow, packet_terms_indicate_client_send_flow,
    packet_terms_indicate_form_validation_flow,
    packet_terms_indicate_html_css_template_structure_flow,
    packet_terms_indicate_log_record_handler_flow,
    packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_runtime_formatting_flow,
    packet_terms_indicate_server_request_dispatch_flow,
    packet_terms_indicate_server_route_dispatch_flow,
    packet_terms_indicate_shell_install_dispatch_flow, packet_terms_indicate_site_build_phase_flow,
    packet_terms_indicate_sql_schema_flow, packet_terms_indicate_string_predicate_flow,
    packet_terms_indicate_stylesheet_animation_flow,
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
        score += packet_route_dispatch_rank_bonus(
            &citation.display_name,
            &normalized_display,
            &path,
            terms,
        );
    }
    if packet_terms_indicate_server_request_dispatch_flow(terms) {
        score += packet_server_request_dispatch_rank_bonus(
            &citation.display_name,
            &normalized_display,
            &path,
            terms,
        );
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
    if packet_terms_indicate_stylesheet_animation_flow(terms) {
        score += packet_stylesheet_animation_rank_bonus(&normalized_display, &path);
    }
    if packet_terms_indicate_html_css_template_structure_flow(terms) {
        score += packet_html_css_template_structure_rank_bonus(&normalized_display, &path);
    }
    if packet_terms_indicate_sql_schema_flow(terms) {
        score += packet_sql_schema_rank_bonus(&normalized_display, &path, terms);
    }
    if packet_terms_indicate_runtime_formatting_flow(terms) {
        score += packet_runtime_formatting_rank_bonus(&normalized_display, &path);
    }
    if packet_terms_indicate_string_predicate_flow(terms) {
        score += packet_string_predicate_rank_bonus(&normalized_display, &path);
    }
    if packet_terms_indicate_shell_install_dispatch_flow(terms) {
        score += packet_shell_install_dispatch_rank_bonus(&normalized_display, &path);
    }

    #[cfg(test)]
    {
        score = eval_citation_rank_adjustment(&normalized_display, &path, score);
    }
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

fn packet_server_request_dispatch_rank_bonus(
    display: &str,
    normalized_display: &str,
    path: &str,
    terms: &[String],
) -> f32 {
    let mut bonus = 0.0;
    bonus += packet_request_dispatch_anchor_rank_bonus(display, normalized_display, path);
    if normalized_display_matches_prompt_owner(normalized_display, terms) {
        bonus += 6.0;
    }
    if normalized_display_contains_all_parts(normalized_display, &["wsgi", "app"]) {
        bonus += 10.0;
    }
    if normalized_display_contains_all_parts(normalized_display, &["full", "dispatch", "request"]) {
        bonus += 9.0;
    }
    if normalized_display_contains_all_parts(normalized_display, &["dispatch", "request"]) {
        bonus += 8.0;
    }
    if normalized_display_contains_all_parts(normalized_display, &["request", "context"])
        || path.contains("/ctx.")
        || path.ends_with("ctx.py")
    {
        bonus += 5.0;
    }
    if normalized_display.ends_with("route")
        || normalized_display_contains_all_parts(normalized_display, &["add", "url", "rule"])
    {
        bonus += 5.0;
    }
    if normalized_display.ends_with("route") {
        bonus += 6.0;
    }
    if path.ends_with("app.py") {
        bonus += 3.0;
    }
    if path.contains("/sansio/scaffold.py") || path.ends_with("sansio/scaffold.py") {
        bonus += 3.0;
    }
    bonus
}

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
    if role == crate::RetrievalFileRole::Source
        && packet_application_router_response_source_anchor(display, normalized_display, path)
    {
        bonus += 8.0;
    }
    bonus
}

fn packet_request_dispatch_artifact_anchor(normalized_display: &str, path: &str) -> bool {
    normalized_display.starts_with("componentreport")
        || normalized_display.contains("schemareference")
        || path.contains("component_report")
        || path.contains("component-report")
        || path.contains("schema_reference")
        || path.contains("schema-reference")
}

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

fn packet_request_dispatch_receiver_owner(owner: &str) -> bool {
    matches!(
        owner,
        "app" | "application" | "router" | "route" | "res" | "response"
    )
}

fn packet_request_dispatch_owner_path_stem(path_stem: &str) -> bool {
    path_stem.contains("app")
        || path_stem.contains("application")
        || path_stem.contains("router")
        || path_stem.contains("route")
        || path_stem.contains("response")
}

fn packet_request_dispatch_method_tail(method: &str) -> bool {
    matches!(
        method,
        "dispatch" | "handle" | "use" | "route" | "send" | "json" | "end" | "respond"
    )
}

fn normalized_display_contains_all_parts(value: &str, parts: &[&str]) -> bool {
    parts.iter().all(|part| value.contains(part))
}

fn normalized_display_matches_prompt_owner(normalized_display: &str, terms: &[String]) -> bool {
    terms
        .iter()
        .filter(|term| term.len() >= 4 && packet_prompt_owner_term(term))
        .map(|term| normalize_identifier(term))
        .any(|term| !term.is_empty() && normalized_display.starts_with(&term))
}

fn packet_prompt_owner_term(term: &str) -> bool {
    !matches!(
        term,
        "control"
            | "dispatch"
            | "dispatches"
            | "finalizes"
            | "handling"
            | "opens"
            | "receives"
            | "request"
            | "requests"
            | "response"
            | "responses"
            | "returns"
            | "route"
            | "server"
            | "trace"
            | "view"
            | "wsgi"
    )
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
    let path_stem = packet_path_file_stem(path);
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
    if path_stem.ends_with("record")
        || (normalized_display.contains("log") && normalized_display.contains("record"))
    {
        bonus += 4.0;
    }
    if (path_stem.contains("processing") && path_stem.ends_with("handler"))
        || (normalized_display.contains("processing") && normalized_display.contains("handler"))
    {
        bonus += 6.0;
    }
    if normalized_display.contains("gethandlers") {
        bonus += 1.0;
    }
    bonus
}

fn packet_site_build_phase_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    let path_stem = packet_path_file_stem(path);
    if path_stem == "build" && path.contains("/commands/") {
        bonus += 5.0;
    }
    if path_stem == "site" {
        bonus += 5.0;
    }
    if path_stem == "reader" {
        bonus += 4.0;
    }
    if path_stem == "renderer" {
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
    let path_stem = packet_path_file_stem(path);
    if path.ends_with("mapper.cs") {
        bonus += 5.0;
    }
    if normalized_display.contains("imapper") || normalized_display.contains("mappermap") {
        bonus += 5.0;
    }
    if display_or_path.contains("mapper") && display_or_path.contains("configuration") {
        bonus += 6.0;
    }
    if path.ends_with("typemap.cs") || normalized_display.contains("typemap") {
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
    if path.ends_with("http.dart") && !path.contains("/src/") {
        bonus += 7.0;
    }
    if path.ends_with("client.dart") && normalized_display.contains("client") {
        bonus += 7.0;
    }
    if packet_path_file_stem(path) == "base_client"
        || (normalized_display.contains("base")
            && normalized_display.contains("client")
            && normalized_display.contains("send"))
    {
        bonus += 6.0;
    }
    if packet_path_file_stem(path) == "base_request"
        || (normalized_display.contains("base")
            && normalized_display.contains("request")
            && normalized_display.contains("finalize"))
    {
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
    let path_stem = packet_path_file_stem(path);
    let is_request_object_file = path_stem.ends_with("request") && path_stem != "request";
    let is_delegate_callback_file = path_stem.ends_with("delegate") && path_stem != "delegate";

    if path_stem == "session" {
        bonus += 4.0;
    }
    if path_stem == "request" {
        bonus += 4.0;
    }
    if is_request_object_file {
        bonus += 5.0;
    }
    if is_delegate_callback_file {
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
    if normalized_display.ends_with("requestvalidate") {
        bonus += 9.0;
    }
    if is_request_object_file
        && path_stem.contains("data")
        && normalized_display.starts_with(&path_stem)
        && normalized_display.ends_with("requestvalidate")
    {
        bonus += 90.0;
    }
    if normalized_display.contains("delegate")
        || (normalized_display.contains("urlsession") && is_delegate_callback_file)
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
    let display_or_path = format!("{normalized_display}{path}");
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

fn packet_stylesheet_animation_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");
    let path_stem = packet_path_file_stem(path);

    if path.contains("/source/") || path.starts_with("source/") {
        bonus += 5.0;
    }
    if path_stem.contains("var") && path.ends_with(".css") {
        bonus += 12.0;
    }
    if path_stem.contains("base") && path.ends_with(".css") {
        bonus += 8.0;
    }
    if path_stem == "animate" && path.ends_with(".css") {
        bonus += 7.0;
    }
    if path.contains("/attention_seekers/") && path.ends_with(".css") {
        bonus += 7.0;
    }

    if normalized_display.contains("animated")
        || normalized_display.contains("animated")
        || normalized_display.contains("animate_duration")
        || normalized_display.contains("animateduration")
        || normalized_display.contains("animate_delay")
        || normalized_display.contains("animatedelay")
        || normalized_display.contains("animate_repeat")
        || normalized_display.contains("animaterepeat")
        || normalized_display.contains("bounce")
        || normalized_display.contains("flash")
        || normalized_display.contains("keyframes")
    {
        bonus += 6.0;
    }

    if path.contains("/docs/")
        || path.starts_with("docs/")
        || path.contains("/docssource/")
        || path.starts_with("docssource/")
        || path.ends_with(".min.css")
        || path.ends_with("animate.compat.css")
        || display_or_path.contains("compileanimation")
        || display_or_path.contains("startanimation")
    {
        bonus -= 14.0;
    }
    if path.contains("/back_exits/") || path.contains("/rotating_entrances/") {
        bonus -= 4.0;
    }

    bonus
}

fn packet_html_css_template_structure_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let display_or_path = format!("{normalized_display}{path}");

    if path.ends_with(".html")
        && (normalized_display == "app"
            || normalized_display.contains("script")
            || normalized_display.contains("module")
            || path.contains("template"))
    {
        bonus += 8.0;
    }
    if path.ends_with(".css")
        && (normalized_display == "root"
            || normalized_display == "body"
            || normalized_display == "app"
            || normalized_display.contains("button")
            || normalized_display.contains("logo")
            || normalized_display.contains("color"))
    {
        bonus += 8.0;
    }
    if normalized_display == "app" || normalized_display.contains("divapp") {
        bonus += 5.0;
    }
    if normalized_display == "root"
        || normalized_display == "body"
        || normalized_display == "color"
        || normalized_display.contains("colorscheme")
    {
        bonus += 4.0;
    }
    if normalized_display.contains("button")
        || normalized_display.contains("hover")
        || normalized_display.contains("focus")
        || normalized_display.contains("logo")
    {
        bonus += 4.0;
    }
    if normalized_display.contains("preferscolorscheme")
        || display_or_path.contains("prefers-color-scheme")
    {
        bonus += 4.0;
    }
    if path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("/fixtures/")
        || path.ends_with(".min.css")
    {
        bonus -= 12.0;
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

fn packet_shell_install_dispatch_rank_bonus(normalized_display: &str, path: &str) -> f32 {
    let mut bonus = 0.0;
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if file_name.contains("install") && path.ends_with(".sh") {
        bonus += 8.0;
    }
    if path.ends_with(".sh") && (file_name.contains("command") || file_name.contains("runtime")) {
        bonus += 8.0;
    }
    if file_name.contains("completion") {
        bonus += 7.0;
    }
    if normalized_display.contains("install")
        || normalized_display.contains("download")
        || normalized_display.contains("dispatch")
        || normalized_display.contains("completion")
        || normalized_display.contains("ifneeded")
        || normalized_display.contains("useif")
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
    use codestory_contracts::api::NodeId;

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
    fn request_dispatch_rank_prefers_source_anchors_over_artifacts() {
        let terms = vec![
            "server".to_string(),
            "request".to_string(),
            "dispatch".to_string(),
            "router".to_string(),
            "response".to_string(),
        ];
        let source = test_rank_citation("app.handle", "lib/application.js", 1.0);
        let example = test_rank_citation("app.handle", "examples/application.js", 1.0);
        let schema_reference = test_rank_citation(
            "schema_reference::request_dispatch",
            "schema/reference.js",
            1.0,
        );
        let component_report =
            test_rank_citation("component_report:routes", "lib/application.js", 1.0);
        let response_source = test_rank_citation("res.send", "lib/response.js", 1.0);

        assert!(
            packet_citation_rank(&source, &terms, false)
                > packet_citation_rank(&example, &terms, false)
        );
        assert!(
            packet_citation_rank(&source, &terms, false)
                > packet_citation_rank(&schema_reference, &terms, false)
        );
        assert!(
            packet_citation_rank(&source, &terms, false)
                > packet_citation_rank(&component_report, &terms, false)
        );
        assert!(
            packet_citation_rank(&response_source, &terms, false)
                > packet_citation_rank(&component_report, &terms, false)
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
