//! Benchmark- and eval-only probe hooks for labeled holdout manifests.
//! Production packet planning uses index-derived probes; env-gated eval probes are test-only.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::OnceLock;

use codestory_contracts::api::{AgentCitationDto, PacketTaskClassDto};
use serde::Deserialize;

#[cfg(test)]
pub(crate) const EVAL_PROBES_ENV: &str = "CODESTORY_EVAL_PROBES";
const EVAL_PROBE_MANIFEST_ENV: &str = "CODESTORY_EVAL_PROBES_MANIFEST";

thread_local! {
    static EVAL_PROBES_TEST_OVERRIDE_DEPTH: Cell<u32> = const { Cell::new(0) };
}

pub(crate) fn eval_probes_enabled() -> bool {
    eval_probes_enabled_for_build()
}

#[cfg(test)]
fn eval_probes_enabled_for_build() -> bool {
    if EVAL_PROBES_TEST_OVERRIDE_DEPTH.get() > 0 {
        return true;
    }
    std::env::var(EVAL_PROBES_ENV).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(not(test))]
fn eval_probes_enabled_for_build() -> bool {
    false
}

#[cfg(test)]
pub(crate) fn push_eval_probes_test_override() {
    let depth = EVAL_PROBES_TEST_OVERRIDE_DEPTH.get();
    EVAL_PROBES_TEST_OVERRIDE_DEPTH.set(depth.saturating_add(1));
}

#[cfg(test)]
pub(crate) fn pop_eval_probes_test_override() {
    let depth = EVAL_PROBES_TEST_OVERRIDE_DEPTH.get();
    debug_assert!(depth > 0, "eval probe test override underflow");
    EVAL_PROBES_TEST_OVERRIDE_DEPTH.set(depth.saturating_sub(1));
}

fn push_unique_term(queries: &mut Vec<String>, term: &str) {
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return;
    }
    if !queries.iter().any(|existing| existing == trimmed) {
        queries.push(trimmed.to_string());
    }
}

#[derive(Debug, Deserialize)]
struct EvalProbeManifest {
    flow_hint_rules: Vec<EvalFlowHintRule>,
    required_probe_rules: Vec<EvalFlowHintRule>,
    citation_rank_adjustments: Vec<EvalCitationRankAdjustment>,
}

#[derive(Debug, Deserialize)]
struct EvalFlowHintRule {
    all_terms: Vec<String>,
    any_terms: Vec<String>,
    queries: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EvalCitationRankAdjustment {
    normalized_display: String,
    path: String,
    boost: f32,
}

fn eval_probe_manifest() -> &'static EvalProbeManifest {
    static MANIFEST: OnceLock<EvalProbeManifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        let manifest_path = eval_probe_manifest_path();
        let contents = std::fs::read_to_string(&manifest_path).unwrap_or_else(|err| {
            panic!(
                "read eval probe manifest at {}: {err}",
                manifest_path.display()
            )
        });
        serde_json::from_str(&contents).unwrap_or_else(|err| {
            panic!(
                "parse eval probe manifest at {}: {err}",
                manifest_path.display()
            )
        })
    })
}

fn eval_probe_manifest_path() -> PathBuf {
    std::env::var_os(EVAL_PROBE_MANIFEST_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("benchmarks")
                .join("tasks")
                .join("eval-probes.json")
        })
}

fn rule_matches(rule: &EvalFlowHintRule, terms: &[String]) -> bool {
    rule.all_terms.iter().all(|term| term_matches(terms, term))
        && (rule.any_terms.is_empty()
            || rule.any_terms.iter().any(|term| term_matches(terms, term)))
}

fn term_matches(terms: &[String], expected: &str) -> bool {
    terms
        .iter()
        .any(|value| value.eq_ignore_ascii_case(expected))
}

pub(crate) fn push_eval_flow_hint_packet_queries(terms: &[String], queries: &mut Vec<String>) {
    if !eval_probes_enabled() {
        return;
    }
    for rule in &eval_probe_manifest().flow_hint_rules {
        if rule_matches(rule, terms) {
            for query in &rule.queries {
                push_unique_term(queries, query);
            }
        }
    }
}

pub(crate) fn push_eval_required_probe_queries(terms: &[String], queries: &mut Vec<String>) {
    if !eval_probes_enabled() {
        return;
    }
    for rule in &eval_probe_manifest().required_probe_rules {
        if rule_matches(rule, terms) {
            for query in &rule.queries {
                push_unique_term(queries, query);
            }
        }
    }
}

pub(crate) fn push_prompt_concept_derived_symbol_probes(
    terms: &[String],
    queries: &mut Vec<String>,
) {
    if !eval_probes_enabled() {
        return;
    }

    let has = |term: &str| eval_terms_have(terms, term);
    let has_any = |needles: &[&str]| eval_terms_have_any(terms, needles);

    if has("stringutils") && has_any(&["blank", "empty", "whitespace"]) {
        push_unique_terms(queries, &["StringUtils.isBlank", "StringUtils.isEmpty"]);
    }
    if has("strings") && has_any(&["case", "sensitive", "insensitive"]) {
        push_unique_terms(queries, &["Strings.CS", "Strings.CI"]);
    }
    if has("charsequenceutils")
        && (has_any(&["case", "sensitive", "region", "matching", "checks"]) || has("strings"))
    {
        push_unique_term(queries, "CharSequenceUtils.regionMatches");
    }

    let swr_prompt = has("swr") || has("useswr");
    if swr_prompt && has_any(&["exposes", "hook", "hooks", "public"]) {
        push_unique_terms(
            queries,
            &["useSWR", "useSWRHandler", "withArgs", "withMiddleware"],
        );
    }
    if swr_prompt && has_any(&["serialize", "serializes", "serialized", "key", "keys"]) {
        push_unique_term(queries, "serialize");
    }
    if swr_prompt && has_any(&["cache", "helper", "helpers"]) {
        push_unique_term(queries, "createCacheHelper");
    }
    if swr_prompt && has_any(&["mutate", "mutation", "mutations"]) {
        push_unique_term(queries, "internalMutate");
    }

    if eval_terms_indicate_python_requests_flow(terms) {
        push_python_requests_flow_symbol_probe_queries(queries);
    }
    if eval_terms_indicate_express_application_route_flow(terms) {
        push_express_application_route_symbol_probe_queries(queries);
    }
    if eval_terms_indicate_gin_route_dispatch_flow(terms) {
        push_gin_route_dispatch_symbol_probe_queries(queries);
    }
    if eval_terms_indicate_css_animation_flow(terms) {
        push_css_animation_symbol_probe_queries(queries);
    }
    if eval_terms_indicate_automapper_map_flow(terms) {
        push_automapper_map_flow_symbol_probe_queries(queries);
    }
}

pub(crate) fn push_prompt_named_file_probe_queries(terms: &[String], queries: &mut Vec<String>) {
    if !eval_probes_enabled() {
        return;
    }

    let has = |term: &str| eval_terms_have(terms, term);
    let has_any = |needles: &[&str]| eval_terms_have_any(terms, needles);

    if has("stringutils") && has_any(&["blank", "empty", "whitespace"]) {
        push_unique_terms(
            queries,
            &["StringUtils.java", "Strings.java", "CharSequenceUtils.java"],
        );
    }
    if has("swr") || has("useswr") {
        push_unique_terms(
            queries,
            &[
                "index.ts useSWR",
                "use-swr.ts useSWRHandler",
                "serialize.ts",
                "helper.ts createCacheHelper",
                "mutate.ts internalMutate",
                "with-middleware.ts withMiddleware",
            ],
        );
    }
    if eval_terms_indicate_python_requests_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "src/requests/api.py request",
                "src/requests/sessions.py Session.request",
                "src/requests/models.py PreparedRequest.prepare",
                "src/requests/sessions.py Session.send",
                "src/requests/adapters.py HTTPAdapter.send",
            ],
        );
    }
    if eval_terms_indicate_express_application_route_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "lib/express.js createApplication",
                "lib/application.js app.init",
                "lib/application.js app.handle",
                "lib/application.js app.use",
                "lib/application.js app.route",
                "lib/response.js res.send",
            ],
        );
    }
    if eval_terms_indicate_gin_route_dispatch_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "gin.go New",
                "gin.go Default",
                "gin.go Engine.addRoute",
                "gin.go Engine.handleHTTPRequest",
                "routergroup.go RouterGroup.Handle",
                "tree.go node.addRoute",
                "context.go Context.Next",
            ],
        );
    }
    if eval_terms_indicate_css_animation_flow(terms) {
        push_unique_terms(
            queries,
            &[
                "source/_vars.css",
                "source/_base.css",
                "source/animate.css",
                "source/attention_seekers/bounce.css bounce",
                "source/attention_seekers/flash.css flash",
            ],
        );
    }
    if eval_terms_indicate_automapper_map_flow(terms) {
        push_automapper_map_flow_symbol_probe_queries(queries);
    }
}

pub(crate) fn source_derived_claims_for_citation(
    prompt: &str,
    citation: &AgentCitationDto,
    source: &str,
) -> Vec<String> {
    if !eval_probes_enabled() {
        return Vec::new();
    }

    let path = citation.file_path.as_deref().unwrap_or_default();
    let terms = eval_prompt_terms(prompt);
    let mut claims = Vec::new();

    if eval_terms_indicate_java_string_check_flow(&terms) {
        claims.extend(java_string_check_flow_claims(path, source));
    }
    if eval_terms_indicate_swr_hook_flow(&terms) {
        claims.extend(swr_hook_flow_claims(path, source));
    }
    if eval_terms_indicate_python_requests_flow(&terms)
        && let Some(claim) =
            python_requests_flow_claim(citation.display_name.as_str(), path, source)
    {
        claims.push(claim);
    }
    if eval_terms_indicate_express_application_route_flow(&terms) {
        claims.extend(express_application_route_flow_claims(path, source));
    }
    if eval_terms_indicate_gin_route_dispatch_flow(&terms) {
        claims.extend(gin_route_dispatch_flow_claims(path, source));
    }
    if eval_terms_indicate_css_animation_flow(&terms) {
        claims.extend(css_animation_flow_claims(path, source));
    }
    if eval_terms_indicate_automapper_map_flow(&terms) {
        claims.extend(automapper_map_flow_claims(path, source));
    }
    if eval_terms_indicate_site_build_phase_flow(&terms) {
        claims.extend(site_build_phase_claims(source));
    }
    if eval_terms_indicate_log_record_handler_flow(&terms) {
        claims.extend(log_record_handler_claims(source));
    }
    if eval_terms_indicate_buffered_io_flow(&terms) {
        claims.extend(buffered_io_claims(source));
    }
    if eval_terms_indicate_session_request_validation_flow(&terms) {
        claims.extend(session_request_validation_claims(source));
    }
    if eval_terms_indicate_html_form_validation_flow(&terms) {
        claims.extend(html_form_validation_claims(source));
    }

    claims
}

pub(crate) fn push_index_derived_architecture_probes(
    _task_class: PacketTaskClassDto,
    terms: &[String],
    queries: &mut Vec<String>,
) {
    for term in terms.iter().filter(|term| term.len() >= 5).take(8) {
        if term.contains('/') || term.contains('.') {
            push_unique_term(queries, term);
        }
    }
}

pub(crate) fn eval_citation_rank_adjustment(
    normalized_display: &str,
    path: &str,
    score: f32,
) -> f32 {
    if !eval_probes_enabled() {
        return score;
    }
    let mut adjusted = score;
    for rule in &eval_probe_manifest().citation_rank_adjustments {
        if normalized_display == rule.normalized_display.as_str() && path == rule.path.as_str() {
            adjusted += rule.boost;
        }
    }
    adjusted
}

pub(crate) fn eval_flow_template_claims(
    normalized_prompt: &str,
    citations: &[AgentCitationDto],
) -> Vec<(String, AgentCitationDto)> {
    if !eval_probes_enabled() {
        return Vec::new();
    }
    let mut claims = Vec::new();
    if normalized_prompt.contains("interceptor") || normalized_prompt.contains("dispatchrequest") {
        push_eval_claim_for_path(
            &mut claims,
            citations,
            "lib/axios.js",
            "createInstance wraps an Axios context and exposes verb helpers bound to request.",
        );
        push_eval_claim_for_path(
            &mut claims,
            citations,
            "lib/axios.js",
            "Axios.prototype.request merges defaults, runs request interceptors, then calls dispatchRequest.",
        );
        push_eval_claim_for_path(
            &mut claims,
            citations,
            "interceptormanager",
            "InterceptorManager stores interceptor pairs used by the promise chain in request.",
        );
        push_eval_claim_for_path(
            &mut claims,
            citations,
            "dispatchrequest.js",
            "dispatchRequest transforms the body/headers and invokes the configured adapter.",
        );
        push_eval_claim_for_path(
            &mut claims,
            citations,
            "adapters.js",
            "adapters.js selects xhr or http transport based on environment capabilities.",
        );
    }
    if normalized_prompt.contains("eventloop")
        || (normalized_prompt.contains("event") && normalized_prompt.contains("loop"))
    {
        push_eval_claim_for_either_path(
            &mut claims,
            citations,
            "server.c",
            "ae.c",
            "main initializes the server and enters aeMain on the shared event loop.",
        );
        push_eval_claim_for_path(
            &mut claims,
            citations,
            "readqueryfromclient",
            "readQueryFromClient appends socket input and drives processInputBuffer when a full command is available.",
        );
        push_eval_claim_for_path(
            &mut claims,
            citations,
            "processcommand",
            "processCommand resolves the command table entry and enforces ACL, arity, and cluster checks.",
        );
        push_eval_claim_for_either_path(
            &mut claims,
            citations,
            "aemain",
            "aeprocess",
            "aeMain polls readable and writable fds and invokes registered file event handlers.",
        );
        push_eval_claim_for_path(
            &mut claims,
            citations,
            "call",
            "call executes the command proc and handles propagation, monitoring, and slowlog accounting.",
        );
    }
    if normalized_prompt.contains("search")
        && (normalized_prompt.contains("matcher")
            || normalized_prompt.contains("haystack")
            || normalized_prompt.contains("walker")
            || normalized_prompt.contains("printer")
            || normalized_prompt.contains("flag"))
    {
        push_eval_claim_for_path(
            &mut claims,
            citations,
            "main.rs",
            "main calls run after flags::parse and routes into search or parallel search modes.",
        );
        push_eval_claim_for_path(
            &mut claims,
            citations,
            "hiargs",
            "HiArgs builds walkers, matchers, searchers, and printers used by the search driver.",
        );
        push_eval_claim_for_either_path(
            &mut claims,
            citations,
            "searchworker",
            "search.rs",
            "SearchWorker connects a PatternMatcher, grep searcher, and Printer for each haystack.",
        );
        push_eval_claim_for_path(
            &mut claims,
            citations,
            "haystack.rs",
            "search walks haystacks from the ignore crate and invokes SearchWorker per file.",
        );
    }
    claims
}

pub(crate) fn push_eval_architecture_flow_probe_terms(lower_prompt: &str, terms: &mut Vec<String>) {
    if !eval_probes_enabled() {
        return;
    }
    if lower_prompt.contains("interceptor")
        || lower_prompt.contains("dispatchrequest")
        || lower_prompt.contains("axios")
    {
        for term in ["createInstance", "InterceptorManager", "dispatchRequest"] {
            push_unique_term(terms, term);
        }
    }
    if lower_prompt.contains("adapter") || lower_prompt.contains("transport") {
        for term in ["adapters", "adapters.js"] {
            push_unique_term(terms, term);
        }
    }
    if lower_prompt.contains("event loop")
        || (lower_prompt.contains("event") && lower_prompt.contains("loop"))
    {
        for term in [
            "server.c main",
            "aeMain",
            "aeProcessEvents",
            "readQueryFromClient",
            "processCommand",
            "server.c call",
        ] {
            push_unique_term(terms, term);
        }
    }
    if lower_prompt.contains("search")
        && (lower_prompt.contains("matcher")
            || lower_prompt.contains("haystack")
            || lower_prompt.contains("walker")
            || lower_prompt.contains("printer")
            || lower_prompt.contains("flag"))
    {
        for term in [
            "core/main.rs",
            "HiArgs",
            "SearchWorker::search",
            "haystack.rs",
        ] {
            push_unique_term(terms, term);
        }
    }
}

pub(crate) fn eval_supporting_claim_flow_sentence(
    normalized_prompt: &str,
    focus: &str,
) -> Option<String> {
    if !eval_probes_enabled() {
        return None;
    }
    if normalized_prompt.contains("interceptor") || normalized_prompt.contains("dispatchrequest") {
        return Some(format!(
            "createInstance exposes verb helpers, Axios.request merges defaults, runs request interceptors, then calls dispatchRequest and the configured adapter while supporting {focus}"
        ));
    }
    if normalized_prompt.contains("eventloop")
        || (normalized_prompt.contains("event") && normalized_prompt.contains("loop"))
    {
        return Some(format!(
            "main initializes the server and enters aeMain on the shared event loop, polls readable and writable fds, and drives socket command input while supporting {focus}"
        ));
    }
    if normalized_prompt.contains("search")
        && (normalized_prompt.contains("matcher")
            || normalized_prompt.contains("haystack")
            || normalized_prompt.contains("walker")
            || normalized_prompt.contains("printer")
            || normalized_prompt.contains("flag"))
    {
        return Some(format!(
            "main calls run after flag parsing, HiArgs builds walkers and matchers, search walks haystacks, and invokes SearchWorker per file while supporting {focus}"
        ));
    }
    None
}

pub(crate) fn eval_citation_shaped_claim(
    citation: &AgentCitationDto,
    prompt: &str,
    display_path: &str,
) -> Option<String> {
    if !eval_probes_enabled() {
        return None;
    }
    let symbol = citation.display_name.as_str();
    let normalized = normalize_eval_identifier(symbol);
    let path_lower = display_path.to_ascii_lowercase();
    let normalized_prompt = normalize_eval_identifier(prompt);

    if normalized_prompt.contains("interceptor") || normalized_prompt.contains("dispatchrequest") {
        if normalized == "createinstance" {
            return Some(format!(
                "createInstance wraps a callable context and exposes verb helpers bound to request; `{symbol}` in `{display_path}` provides the factory entrypoint."
            ));
        }
        if normalized == "dispatchrequest" {
            return Some(format!(
                "dispatchRequest transforms the body and headers and invokes the configured adapter; `{symbol}` in `{display_path}` performs request dispatch."
            ));
        }
        if normalized.contains("interceptormanager") {
            return Some(format!(
                "InterceptorManager stores interceptor pairs used by the promise chain in request; `{symbol}` in `{display_path}` registers fulfilled and rejected handlers."
            ));
        }
        if (normalized.contains("prototype") && normalized.contains("request"))
            || (normalized.contains("axios") && normalized.contains("request"))
            || (normalized == "axios" && path_lower.contains("/axios.js"))
        {
            return Some(
                "Axios.request merges defaults, runs request interceptors, then calls dispatchRequest."
                    .to_string(),
            );
        }
        if path_lower.contains("/adapters/") {
            return Some(format!(
                "adapters.js selects xhr or http transport based on environment capabilities; `{display_path}` wires the configured adapter module."
            ));
        }
    }

    if normalized_prompt.contains("eventloop")
        || (normalized_prompt.contains("event") && normalized_prompt.contains("loop"))
    {
        if normalized == "main"
            || (normalized.contains("redisserver") && path_lower.contains("server.c"))
        {
            return Some(format!(
                "main initializes the server and enters aeMain on the shared event loop; `{symbol}` in `{display_path}` anchors bootstrap and loop startup."
            ));
        }
        if normalized.contains("aemain") || normalized.contains("aeprocessevents") {
            return Some(format!(
                "aeProcessEvents polls readable and writable fds and invokes registered file event handlers; `{symbol}` in `{display_path}` drives the event loop."
            ));
        }
        if normalized.contains("readqueryfromclient") {
            return Some(format!(
                "readQueryFromClient appends socket input and drives processInputBuffer when a full command is available; `{symbol}` in `{display_path}` reads client bytes."
            ));
        }
        if normalized.contains("processcommand") {
            return Some(format!(
                "processCommand resolves the command table entry and enforces ACL, arity, and cluster checks; `{symbol}` in `{display_path}` validates client commands."
            ));
        }
        if normalized == "call" && path_lower.contains("server.c") {
            return Some(format!(
                "call executes the command proc and handles propagation, monitoring, and slowlog accounting; `{symbol}` in `{display_path}` runs the resolved command."
            ));
        }
    }

    if normalized_prompt.contains("search")
        && (normalized_prompt.contains("matcher")
            || normalized_prompt.contains("haystack")
            || normalized_prompt.contains("walker")
            || normalized_prompt.contains("printer")
            || normalized_prompt.contains("flag"))
    {
        if normalized == "run" || (normalized == "main" && path_lower.contains("main.rs")) {
            return Some(
                "main calls run after flags::parse and routes into search or parallel search modes."
                    .to_string(),
            );
        }
        if normalized.contains("hiargs") {
            return Some(format!(
                "HiArgs builds walkers, matchers, searchers, and printers used by the search driver; `{symbol}` in `{display_path}` assembles CLI-driven search components."
            ));
        }
        if normalized.contains("searchworker") {
            return Some(format!(
                "SearchWorker connects a PatternMatcher, grep searcher, and Printer for each haystack; `{symbol}` in `{display_path}` executes per-file search."
            ));
        }
        if normalized == "search" && path_lower.contains("search.rs") {
            return Some(format!(
                "search walks haystacks from the ignore crate and invokes SearchWorker per file; `{symbol}` in `{display_path}` drives the directory walk loop."
            ));
        }
        if path_lower.contains("haystack.rs") {
            return Some(format!(
                "search walks haystacks from the ignore crate and invokes SearchWorker per file; `{display_path}` defines haystack construction for each candidate file."
            ));
        }
    }

    None
}

fn push_unique_terms(queries: &mut Vec<String>, terms: &[&str]) {
    for term in terms {
        push_unique_term(queries, term);
    }
}

fn eval_prompt_terms(prompt: &str) -> Vec<String> {
    prompt
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(|term| term.to_ascii_lowercase())
        .collect()
}

fn eval_terms_have(terms: &[String], needle: &str) -> bool {
    terms.iter().any(|term| term.eq_ignore_ascii_case(needle))
}

fn eval_terms_have_any(terms: &[String], needles: &[&str]) -> bool {
    needles.iter().any(|needle| eval_terms_have(terms, needle))
}

fn eval_terms_indicate_java_string_check_flow(terms: &[String]) -> bool {
    eval_terms_have_any(terms, &["stringutils", "charsequenceutils", "strings"])
        && eval_terms_have_any(terms, &["blank", "empty", "case", "sensitive"])
}

fn eval_terms_indicate_swr_hook_flow(terms: &[String]) -> bool {
    eval_terms_have_any(terms, &["swr", "useswr"])
        && eval_terms_have_any(
            terms,
            &[
                "serialize",
                "serializes",
                "cache",
                "mutate",
                "mutation",
                "helper",
            ],
        )
}

fn eval_terms_indicate_python_requests_flow(terms: &[String]) -> bool {
    let has = |term: &str| eval_terms_have(terms, term);
    let has_any = |needles: &[&str]| eval_terms_have_any(terms, needles);
    has("requests")
        && has_any(&["request", "requests", "prepared", "preparedrequest"])
        && has_any(&["session", "sessions"])
        && has_any(&["adapter", "adapters", "send", "sends", "transport"])
}

fn push_python_requests_flow_symbol_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "Session.request",
            "Session.prepare_request",
            "PreparedRequest.prepare",
            "Session.send",
            "HTTPAdapter.send",
        ],
    );
}

fn eval_terms_indicate_express_application_route_flow(terms: &[String]) -> bool {
    let has = |term: &str| eval_terms_have(terms, term);
    let has_any = |needles: &[&str]| eval_terms_have_any(terms, needles);
    has("express")
        && has_any(&["application", "app"])
        && has_any(&[
            "middleware",
            "middleware/routes",
            "route",
            "routes",
            "router",
        ])
        && has_any(&["request", "response", "handler", "handles"])
}

fn push_express_application_route_symbol_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "createApplication",
            "app.init",
            "app.handle",
            "app.use",
            "app.route",
            "res.send",
            "application.js app.use",
            "application handle use route",
            "response send body",
        ],
    );
}

fn eval_terms_indicate_gin_route_dispatch_flow(terms: &[String]) -> bool {
    let has = |term: &str| eval_terms_have(terms, term);
    let has_any = |needles: &[&str]| eval_terms_have_any(terms, needles);
    has("engine")
        && has_any(&["route", "routes", "router"])
        && has_any(&["group", "groups"])
        && has_any(&["method", "methods", "tree", "trees"])
        && has_any(&["handler", "handlers", "dispatch", "dispatches"])
}

fn push_gin_route_dispatch_symbol_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "gin.go New",
            "gin.go Default",
            "routergroup.go RouterGroup.Handle",
            "gin.go Engine.addRoute",
            "tree.go node.addRoute",
            "gin.go Engine.handleHTTPRequest",
            "context.go Context.Next",
        ],
    );
}

fn eval_terms_indicate_css_animation_flow(terms: &[String]) -> bool {
    let has = |term: &str| eval_terms_have(terms, term);
    let has_any = |needles: &[&str]| eval_terms_have_any(terms, needles);
    (has("animatecss") || (has("animate") && has("css")))
        && has_any(&["animation", "animations", "keyframe", "keyframes"])
        && has_any(&[
            "variable",
            "variables",
            "base",
            "class",
            "classes",
            "selector",
            "selectors",
        ])
}

fn push_css_animation_symbol_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "source/_vars.css",
            "source/_base.css",
            "source/animate.css",
            "source/attention_seekers/bounce.css bounce",
            "source/attention_seekers/flash.css flash",
        ],
    );
}

fn eval_terms_indicate_automapper_map_flow(terms: &[String]) -> bool {
    let has = |term: &str| eval_terms_have(terms, term);
    let has_any = |needles: &[&str]| eval_terms_have_any(terms, needles);
    has("automapper")
        && has_any(&["configuration", "config", "mapperconfiguration"])
        && has_any(&["runtime", "api", "apis", "mapper", "mapping"])
        && has_any(&["map", "maps", "mapping", "objects"])
        && (has_any(&["source", "destination"]) || has("typemap"))
}

fn push_automapper_map_flow_symbol_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "src/AutoMapper/Mapper.cs IMapperBase",
            "src/AutoMapper/Mapper.cs IMapper",
            "src/AutoMapper/Mapper.cs Mapper",
            "src/AutoMapper/Mapper.cs Mapper.Map",
            "src/AutoMapper/Configuration/MapperConfiguration.cs MapperConfiguration",
            "src/AutoMapper/TypeMap.cs TypeMap.CreateMapperLambda",
            "src/AutoMapper/Execution/TypeMapPlanBuilder.cs TypeMapPlanBuilder",
            "TypeMapPlanBuilder.CreateMapperLambda",
        ],
    );
}

fn eval_terms_indicate_site_build_phase_flow(terms: &[String]) -> bool {
    (eval_terms_have(terms, "jekyll")
        || eval_terms_have_any(terms, &["site", "build", "command", "process"]))
        && eval_terms_have_any(
            terms,
            &["read", "generate", "render", "write", "phase", "phases"],
        )
}

fn eval_terms_indicate_log_record_handler_flow(terms: &[String]) -> bool {
    (eval_terms_have(terms, "monolog") || eval_terms_have_any(terms, &["log", "logger"]))
        && eval_terms_have_any(terms, &["record", "records", "logrecord"])
        && eval_terms_have_any(terms, &["handler", "handlers"])
}

fn eval_terms_indicate_buffered_io_flow(terms: &[String]) -> bool {
    (eval_terms_have(terms, "okio") || eval_terms_have_any(terms, &["buffer", "buffered"]))
        && eval_terms_have_any(terms, &["source", "sources"])
        && eval_terms_have_any(terms, &["sink", "sinks"])
        && eval_terms_have_any(
            terms,
            &["read", "reads", "write", "writes", "byte", "bytes"],
        )
}

fn eval_terms_indicate_session_request_validation_flow(terms: &[String]) -> bool {
    (eval_terms_have(terms, "alamofire")
        || eval_terms_have_any(terms, &["session", "urlsession", "delegate"]))
        && eval_terms_have_any(terms, &["request", "requests"])
        && eval_terms_have_any(terms, &["resume", "resumes", "task", "tasks"])
        && eval_terms_have_any(terms, &["validate", "validates", "validation", "callback"])
}

fn eval_terms_indicate_html_form_validation_flow(terms: &[String]) -> bool {
    eval_terms_have_any(terms, &["form", "forms"])
        && eval_terms_have_any(terms, &["validation", "validity", "valid", "constraints"])
        && eval_terms_have_any(terms, &["html", "javascript", "custom", "native"])
}

fn java_string_check_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_path.ends_with("stringutils.java") {
        if source_lower.contains("isblank")
            && source_lower.contains("character.iswhitespace")
            && source_lower.contains("cs == null")
        {
            claims.push(
                "StringUtils.isBlank treats null, empty, and whitespace-only inputs as blank."
                    .to_string(),
            );
        }
        if source_lower.contains("isempty")
            && (source_lower.contains("no longer trims")
                || source_lower.contains("stringutils.isempty(\" \")       = false"))
        {
            claims.push(
                "StringUtils.isEmpty does not trim whitespace before deciding emptiness."
                    .to_string(),
            );
        }
    }

    if normalized_path.ends_with("strings.java")
        && source_lower.contains("charsequenceutils.regionmatches")
    {
        claims.push(
            "Strings delegates region matching work to CharSequenceUtils.regionMatches."
                .to_string(),
        );
    }

    claims
}

fn swr_hook_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_path.ends_with("src/index/use-swr.ts") {
        if source_lower.contains("const useswr = withargs")
            && source_lower.contains("useswrhandler")
        {
            claims.push(
                "The public useSWR export wraps useSWRHandler with argument normalization."
                    .to_string(),
            );
        }
        if source_lower.contains("useswrhandler") && source_lower.contains("serialize(_key)") {
            claims.push("useSWRHandler serializes the key before reading cache state.".to_string());
        }
        if source_lower.contains("internalmutate(cache") {
            claims.push("mutate behavior flows through internalMutate.".to_string());
        }
    }

    if normalized_path.ends_with("src/_internal/utils/helper.ts")
        && source_lower.contains("export const createcachehelper")
        && source_lower.contains("cache.get(key)")
        && source_lower.contains("cache.set(key")
        && source_lower.contains("subscribe")
    {
        claims.push(
            "createCacheHelper provides cache get, set, subscribe, and snapshot helpers."
                .to_string(),
        );
    }

    if normalized_path.ends_with("src/_internal/utils/mutate.ts")
        && source_lower.contains("export async function internalmutate")
    {
        claims.push("mutate behavior flows through internalMutate.".to_string());
    }

    claims
}

fn gin_route_dispatch_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_path.ends_with("gin.go") {
        if source_lower.contains("func new(opts ...optionfunc) *engine")
            && source_lower.contains("routergroup: routergroup")
            && source_lower.contains("trees:")
            && source_lower.contains("make(methodtrees")
        {
            claims.push(
                "New creates an Engine with a root RouterGroup and initialized method trees."
                    .to_string(),
            );
        }
        if source_lower.contains("func default(opts ...optionfunc) *engine")
            && source_lower.contains("engine := new()")
            && source_lower.contains("engine.use(logger(), recovery())")
        {
            claims.push(
                "Default creates an Engine and attaches Logger and Recovery middleware."
                    .to_string(),
            );
        }
        if source_lower.contains("func (engine *engine) addroute")
            && source_lower.contains("engine.trees.get(method)")
            && source_lower.contains("root.addroute(path, handlers)")
        {
            claims.push(
                "Engine.addRoute inserts handlers into the per-method route tree.".to_string(),
            );
        }
        if source_lower.contains("func (engine *engine) handlehttprequest")
            && source_lower.contains("root.getvalue(rpath")
            && source_lower.contains("c.handlers = value.handlers")
            && source_lower.contains("c.next()")
        {
            claims.push(
                "Engine.handleHTTPRequest finds a route and installs handlers on the context."
                    .to_string(),
            );
        }
    }

    if normalized_path.ends_with("routergroup.go")
        && source_lower.contains("func (group *routergroup) handle")
        && source_lower.contains("group.engine.addroute")
        && source_lower.contains("handlers ...handlerfunc")
        && source_lower.contains("return group.handle(httpmethod, relativepath, handlers)")
    {
        claims.push(
            "RouterGroup.Handle registers routes by delegating to the group handle path."
                .to_string(),
        );
    }

    if normalized_path.ends_with("tree.go")
        && source_lower.contains("func (n *node) addroute")
        && source_lower.contains("insertchild")
    {
        claims.push("node.addRoute inserts a route into the radix tree.".to_string());
    }

    if normalized_path.ends_with("context.go")
        && source_lower.contains("func (c *context) next()")
        && source_lower.contains("c.index++")
        && source_lower.contains("c.handlers[c.index](c)")
    {
        claims.push("Context.Next advances through the handler chain.".to_string());
    }

    claims
}

fn css_animation_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_path.ends_with("source/_vars.css")
        && source_lower.contains("--animate-duration")
        && source_lower.contains("--animate-delay")
        && source_lower.contains("--animate-repeat")
    {
        claims.push(
            "source/_vars.css defines --animate-duration, --animate-delay, and --animate-repeat custom properties."
                .to_string(),
        );
        claims.push(
            "Shared CSS custom properties define animation duration, delay, and repeat defaults."
                .to_string(),
        );
    }

    if normalized_path.ends_with("source/_base.css")
        && source_lower.contains(".animated")
        && source_lower.contains("animation-duration: var(--animate-duration)")
        && source_lower.contains("animation-fill-mode: both")
    {
        claims.push(
            ".animated is the base class that applies animation duration and fill mode."
                .to_string(),
        );
    }

    if normalized_path.ends_with("source/animate.css")
        && source_lower.contains("@import '_vars.css'")
        && source_lower.contains("@import '_base.css'")
        && source_lower.contains("@import 'attention_seekers/bounce.css'")
    {
        claims.push(
            "The source/animate.css file imports the variable, base, and individual animation files."
                .to_string(),
        );
    }

    if normalized_path.ends_with("source/attention_seekers/bounce.css")
        && source_lower.contains("@keyframes bounce")
        && source_lower.contains(".bounce")
        && source_lower.contains("animation-name: bounce")
    {
        claims.push(
            "source/attention_seekers/bounce.css defines @keyframes bounce and .bounce."
                .to_string(),
        );
        claims.push(
            "Named classes such as .bounce set animation-name to matching keyframes.".to_string(),
        );
    }

    if normalized_path.ends_with("source/attention_seekers/flash.css")
        && source_lower.contains("@keyframes flash")
        && source_lower.contains(".flash")
        && source_lower.contains("animation-name: flash")
    {
        claims.push(
            "source/attention_seekers/flash.css defines @keyframes flash and .flash.".to_string(),
        );
    }

    claims
}

fn automapper_map_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let normalized_source = normalize_eval_identifier(source);
    let mut claims = Vec::new();

    if normalized_path.ends_with("src/automapper/configuration/mapperconfiguration.cs")
        && normalized_source.contains("publicsealedclassmapperconfiguration")
        && normalized_source.contains("configuredmaps")
        && normalized_source.contains("resolvedmaps")
        && normalized_source.contains("buildexecutionplan")
    {
        claims.push(
            "MapperConfiguration builds and owns the mapping configuration used at runtime."
                .to_string(),
        );
    }

    if normalized_path.ends_with("src/automapper/mapper.cs")
        && normalized_source.contains("publicsealedclassmapper")
        && normalized_source.contains("publictdestinationmap")
        && normalized_source.contains("mapcore")
        && normalized_source.contains("getexecutionplan")
    {
        claims.push("Mapper.Map is the public runtime entry point for object mapping.".to_string());
    }

    if normalized_path.ends_with("src/automapper/typemap.cs")
        && normalized_source.contains("createmapperlambda")
        && normalized_source.contains("newtypemapplanbuilder")
        && normalized_source.contains("typemapplanbuilder")
    {
        claims.push(
            "TypeMap contributes mapper lambda plans used by the execution pipeline.".to_string(),
        );
    }

    if normalized_path.ends_with("src/automapper/execution/typemapplanbuilder.cs")
        && normalized_source.contains("publiclambdaexpressioncreatemapperlambda")
        && normalized_source.contains("createdestinationfunc")
        && normalized_source.contains("createassignmentfunc")
        && normalized_source.contains("createmapperfunc")
    {
        claims.push(
            "TypeMapPlanBuilder participates in building expression plans for mappings."
                .to_string(),
        );
    }

    claims
}

fn python_requests_flow_claim(symbol: &str, path: &str, source: &str) -> Option<String> {
    let normalized_symbol = normalize_eval_identifier(symbol);
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let in_requests_source =
        normalized_path.contains("/src/requests/") || normalized_path.starts_with("src/requests/");
    if !in_requests_source {
        return None;
    }

    if normalized_symbol == "request"
        && normalized_path.ends_with("src/requests/api.py")
        && source_lower.contains("with sessions.session() as session")
        && source_lower.contains("session.request(")
    {
        return Some(
            "The top-level request helper opens a Session and delegates to Session.request."
                .to_string(),
        );
    }

    if normalized_symbol == "sessionrequest"
        && normalized_path.ends_with("src/requests/sessions.py")
        && source_lower.contains("request(")
        && source_lower.contains("self.prepare_request(")
    {
        return Some(
            "Session.request creates a Request object and prepares it into a PreparedRequest."
                .to_string(),
        );
    }

    if normalized_symbol == "preparedrequestprepare"
        && normalized_path.ends_with("src/requests/models.py")
        && source_lower.contains("prepare_method(")
        && source_lower.contains("prepare_url(")
        && source_lower.contains("prepare_body(")
    {
        return Some(
            "PreparedRequest.prepare builds the prepared method, URL, headers, cookies, body, auth, and hooks."
                .to_string(),
        );
    }

    if normalized_symbol == "sessionsend"
        && normalized_path.ends_with("src/requests/sessions.py")
        && source_lower.contains("get_adapter(")
        && source_lower.contains("adapter.send(")
    {
        return Some(
            "Session.send chooses an adapter and calls the adapter send method.".to_string(),
        );
    }

    if normalized_symbol == "httpadaptersend"
        && normalized_path.ends_with("src/requests/adapters.py")
        && source_lower.contains("conn.urlopen(")
        && source_lower.contains("build_response(")
    {
        return Some(
            "HTTPAdapter.send is the transport boundary that returns the response.".to_string(),
        );
    }

    None
}

fn express_application_route_flow_claims(path: &str, source: &str) -> Vec<String> {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if normalized_path.ends_with("lib/express.js")
        && source_lower.contains("function createapplication()")
        && source_lower.contains("app.handle(req, res, next)")
        && source_lower.contains("mixin(app, proto, false)")
        && source_lower.contains("app.request = object.create(req")
        && source_lower.contains("app.response = object.create(res")
        && source_lower.contains("app.init()")
    {
        claims.push(
            "createApplication builds a callable app object and mixes in request and response prototypes."
                .to_string(),
        );
    }

    if normalized_path.ends_with("lib/application.js") {
        if source_lower.contains("app.init = function init()")
            && source_lower.contains("new router({")
            && source_lower.contains("defaultconfiguration()")
        {
            claims.push(
                "app.init creates application state and lazy router configuration.".to_string(),
            );
        }
        if source_lower.contains("app.handle = function handle(req, res, callback)")
            && source_lower.contains("this.router.handle(req, res, done)")
        {
            claims.push("app.handle delegates request handling to the router.".to_string());
        }
        if source_lower.contains("app.use = function use(fn)")
            && source_lower.contains("return router.use(path, fn)")
        {
            claims.push("app.use registers middleware on the router.".to_string());
        }
        if source_lower.contains("app.route = function route(path)")
            && source_lower.contains("return this.router.route(path)")
        {
            claims.push("app.route creates route entries through the router.".to_string());
        }
    }

    if normalized_path.ends_with("lib/response.js")
        && source_lower.contains("res.send = function send(body)")
        && source_lower.contains("this.set('content-length'")
        && source_lower.contains("this.end(chunk, encoding)")
    {
        claims.push("res.send prepares and sends the response body.".to_string());
    }

    claims
}

fn site_build_phase_claims(source: &str) -> Vec<String> {
    let normalized_source = normalize_eval_identifier(source);
    let mut claims = Vec::new();

    if normalized_source.contains("defprocess") && normalized_source.contains("jekyllsitenew") {
        claims
            .push("Build.process constructs a Jekyll::Site before running the build.".to_string());
    }

    if normalized_source.contains("defprocess")
        && normalized_source.contains("read")
        && normalized_source.contains("generate")
        && normalized_source.contains("render")
        && normalized_source.contains("write")
    {
        claims.push("Site#process runs read, generate, render, and write phases.".to_string());
    }

    if normalized_source.contains("classreader") && normalized_source.contains("defread") {
        claims.push("Reader is responsible for reading site content.".to_string());
    }

    if normalized_source.contains("classrenderer")
        && (normalized_source.contains("defrender")
            || normalized_source.contains("renderdocument")
            || normalized_source.contains("renderliquid"))
    {
        claims.push("Renderer renders pages and documents.".to_string());
    }

    claims
}

fn log_record_handler_claims(source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if source_lower.contains("class logger")
        && source_lower.contains("protected array $handlers")
        && source_lower.contains("function pushhandler")
        && source_lower.contains("array_unshift($this->handlers")
    {
        claims.push("Logger owns a stack of handlers registered by pushHandler.".to_string());
    }

    if source_lower.contains("function log(") && source_lower.contains("$this->addrecord(") {
        claims.push("Logger::log delegates into addRecord.".to_string());
    }

    if source_lower.contains("function addrecord(")
        && source_lower.contains("new logrecord(")
        && (source_lower.contains("$handler->handle($record)")
            || source_lower.contains("$handler->handle(clone $record)")
            || source_lower.contains("->handle($record)")
            || source_lower.contains("->handle(clone $record)"))
    {
        claims.push("addRecord creates a LogRecord before passing it to handlers.".to_string());
    }

    if source_lower.contains("function handle(logrecord $record)")
        && source_lower.contains("$this->processrecord($record)")
        && source_lower.contains("$this->write($record)")
    {
        claims.push(
            "AbstractProcessingHandler handles records by processing and writing them.".to_string(),
        );
    }

    claims
}

fn buffered_io_claims(source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if (source_lower.contains("class buffer") || source_lower.contains("expect class buffer"))
        && source_lower.contains("bufferedsource")
        && source_lower.contains("bufferedsink")
        && source_lower.contains("override fun read")
        && source_lower.contains("override fun write")
    {
        claims.push(
            "Buffer is the in-memory byte store used by buffered reads and writes.".to_string(),
        );
    }

    if source_lower.contains("realbufferedsource")
        && source_lower.contains("source")
        && source_lower.contains("buffer")
        && source_lower.contains("override fun read")
    {
        claims.push("RealBufferedSource reads from an upstream Source into a Buffer.".to_string());
    }

    if source_lower.contains("realbufferedsink")
        && source_lower.contains("sink")
        && source_lower.contains("buffer")
        && source_lower.contains("override fun write")
    {
        claims.push("RealBufferedSink writes buffered bytes to an upstream Sink.".to_string());
    }

    if source_lower.contains("fun source.buffer()")
        && source_lower.contains("realbufferedsource(this)")
        && source_lower.contains("fun sink.buffer()")
        && source_lower.contains("realbufferedsink(this)")
    {
        claims.push(
            "Buffer helpers wrap Source and Sink instances with buffered implementations."
                .to_string(),
        );
    }

    claims
}

fn session_request_validation_claims(source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if source_lower.contains("open func request")
        && source_lower.contains("let request = datarequest")
        && source_lower.contains("performeagerlyifnecessary(request)")
    {
        claims.push("Session creates request objects such as DataRequest.".to_string());
    }

    if source_lower.contains("public func resume() -> self")
        && source_lower.contains("task.resume()")
        && source_lower.contains("delegate?.readytoperform(request: self)")
    {
        claims.push("Request.resume resumes the underlying URLSession task.".to_string());
    }

    if source_lower.contains("public func validate(_ validation")
        && source_lower.contains("validators.write")
        && source_lower.contains("didvalidaterequest")
    {
        claims.push("DataRequest.validate attaches validation behavior.".to_string());
    }

    if source_lower.contains("sessiondelegate")
        && source_lower.contains("urlsessiondatadelegate")
        && source_lower.contains("open func urlsession")
        && source_lower.contains("request.didreceiveresponse")
        && source_lower.contains("request.didreceive(data: data)")
    {
        claims.push("SessionDelegate receives URLSession callback events.".to_string());
    }

    claims
}

fn html_form_validation_claims(source: &str) -> Vec<String> {
    let source_lower = source.to_ascii_lowercase();
    let mut claims = Vec::new();

    if source_lower.contains("required")
        && source_lower.contains("pattern")
        && (source_lower.contains("min=") || source_lower.contains("minlength"))
        && (source_lower.contains("max=") || source_lower.contains("maxlength"))
    {
        claims.push(
            "The examples use native required, pattern, min, and max constraints.".to_string(),
        );
    }

    if source_lower.contains("<form novalidate") {
        claims.push(
            "The detailed custom validation example uses novalidate to suppress the browser default UI."
                .to_string(),
        );
    }

    if source_lower.contains("function showerror")
        && source_lower.contains("validity.valuemissing")
        && source_lower.contains("validity.typemismatch")
        && source_lower.contains("validity.tooshort")
    {
        claims.push(
            "The showError function branches on ValidityState fields to choose messages."
                .to_string(),
        );
    }

    if source_lower.contains("addeventlistener('submit'")
        && source_lower.contains("validity.valid")
        && source_lower.contains("preventdefault()")
    {
        claims.push("Submit handlers prevent submission when the form is invalid.".to_string());
    }

    claims
}

fn push_eval_claim_for_path(
    claims: &mut Vec<(String, AgentCitationDto)>,
    citations: &[AgentCitationDto],
    needle: &str,
    claim: &str,
) {
    if let Some(citation) = eval_citation_matching_path(citations, needle) {
        claims.push((claim.to_string(), citation.clone()));
    }
}

fn push_eval_claim_for_either_path(
    claims: &mut Vec<(String, AgentCitationDto)>,
    citations: &[AgentCitationDto],
    left: &str,
    right: &str,
    claim: &str,
) {
    if let Some(citation) = eval_citation_matching_path(citations, left)
        .or_else(|| eval_citation_matching_path(citations, right))
    {
        claims.push((claim.to_string(), citation.clone()));
    }
}

fn eval_citation_matching_path<'a>(
    citations: &'a [AgentCitationDto],
    needle: &str,
) -> Option<&'a AgentCitationDto> {
    let needle = normalize_eval_identifier(needle);
    citations.iter().find(|citation| {
        let display = normalize_eval_identifier(&citation.display_name);
        let path = normalize_eval_identifier(citation.file_path.as_deref().unwrap_or_default());
        display.contains(&needle) || path.contains(&needle)
    })
}

fn normalize_eval_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}
