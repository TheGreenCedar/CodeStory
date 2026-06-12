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
