use codestory_contracts::api::{
    AgentAskRequest, AgentCitationDto, AgentCustomRetrievalConfigDto, AgentResponseBlockDto,
    AgentResponseModeDto, AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto,
    AgentRetrievalProfileSelectionDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto,
    ApiError, IndexMode, LayoutDirection, NodeDetailsRequest, NodeId, SearchHit,
    SearchRepoTextMode, SearchRequest, TrailCallerScope, TrailConfigDto, TrailDirection, TrailMode,
};
use codestory_runtime::AppController;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use tempfile::{TempDir, tempdir};

static BROWSER_CONTRACT_ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

struct BrowserContractEnv {
    guards: Option<Vec<EnvGuard>>,
    _lock: MutexGuard<'static, ()>,
}

impl Drop for BrowserContractEnv {
    fn drop(&mut self) {
        let _ = self.guards.take();
    }
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(value) = self.previous.as_deref() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

fn browser_contract_env() -> BrowserContractEnv {
    let lock = BROWSER_CONTRACT_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let guards = vec![
        EnvGuard::set("CODESTORY_HYBRID_RETRIEVAL_ENABLED", "true"),
        EnvGuard::set("CODESTORY_EMBED_RUNTIME_MODE", "hash"),
    ];
    BrowserContractEnv {
        guards: Some(guards),
        _lock: lock,
    }
}

fn browser_contract_symbolic_env() -> BrowserContractEnv {
    let lock = BROWSER_CONTRACT_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let guards = vec![
        EnvGuard::set("CODESTORY_HYBRID_RETRIEVAL_ENABLED", "false"),
        EnvGuard::set("CODESTORY_EMBED_RUNTIME_MODE", "hash"),
    ];
    BrowserContractEnv {
        guards: Some(guards),
        _lock: lock,
    }
}

fn write_browser_fixture(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src dir");
    fs::write(
        src.join("lib.rs"),
        r#"
pub mod browser;
pub mod ingest;
pub mod routing;

pub use browser::{build_snapshot_digest, exact_symbol_anchor, expand_browser_context};
pub use ingest::{BrowserEvent, parse_event as parse_ingest_event};
pub use routing::{RouteDecision, parse_event as parse_route_event, route_browser_request};

pub fn orchestrate_browser_session(payload: &str) -> RouteDecision {
    let event = ingest::parse_event(payload);
    let route = routing::parse_event(&event);
    browser::expand_browser_context();
    route
}
"#,
    )
    .expect("write lib fixture");
    fs::write(
        src.join("browser.rs"),
        r#"
use crate::routing;

/// Exact anchor for symbol lookup in the browser golden fixture.
pub fn exact_symbol_anchor() -> &'static str {
    "exact-symbol-anchor"
}

/// Build the deterministic digest used by natural-language browser questions.
pub fn build_snapshot_digest() -> &'static str {
    "browser retrieval integrates ingest parsing with route decisions"
}

pub fn expand_browser_context() -> String {
    let anchor = exact_symbol_anchor();
    let digest = build_snapshot_digest();
    let plan = routing::route_browser_request();
    format!("{anchor}:{digest}:{plan}")
}
"#,
    )
    .expect("write browser fixture");
    fs::write(
        src.join("ingest.rs"),
        r#"
pub const ROUTE_LITERAL: &str = "CODESTORY_BROWSER_LITERAL";

#[derive(Clone, Debug)]
pub struct BrowserEvent {
    pub raw: String,
    pub marker: &'static str,
}

pub fn parse_event(input: &str) -> BrowserEvent {
    BrowserEvent {
        raw: input.to_string(),
        marker: ROUTE_LITERAL,
    }
}
"#,
    )
    .expect("write ingest fixture");
    fs::write(
        src.join("routing.rs"),
        r#"
use crate::ingest::BrowserEvent;

#[derive(Clone, Debug)]
pub struct RouteDecision {
    pub target: &'static str,
}

pub fn parse_event(event: &BrowserEvent) -> RouteDecision {
    let _literal = event.marker;
    RouteDecision { target: "browser-route" }
}

pub fn route_browser_request() -> &'static str {
    "route browser requests through ingest parse_event and build_snapshot_digest"
}
"#,
    )
    .expect("write routing fixture");
}

fn indexed_controller() -> (AppController, TempDir, TempDir) {
    let workspace = tempdir().expect("workspace dir");
    write_browser_fixture(workspace.path());

    let storage = tempdir().expect("storage dir");
    let controller = AppController::new();
    controller
        .open_project_with_storage_path(
            workspace.path().to_path_buf(),
            storage.path().join("codestory.db"),
        )
        .expect("open project");
    controller
        .run_indexing_blocking(IndexMode::Full)
        .expect("index workspace");

    (controller, workspace, storage)
}

fn search_symbols(controller: &AppController, query: &str, max_results: u32) -> Vec<SearchHit> {
    controller
        .search_hybrid(
            SearchRequest {
                query: query.to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: max_results,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            },
            None,
            Some(max_results),
            None,
        )
        .expect("search symbols")
}

fn ask_browser(
    controller: &AppController,
    prompt: &str,
    focus_node_id: Option<NodeId>,
) -> codestory_contracts::api::AgentAnswerDto {
    ask_browser_with_profile(
        controller,
        prompt,
        focus_node_id,
        AgentRetrievalProfileSelectionDto::Custom {
            config: AgentCustomRetrievalConfigDto {
                depth: 2,
                direction: TrailDirection::Both,
                max_nodes: 40,
                include_edge_occurrences: true,
                enable_source_reads: true,
                ..AgentCustomRetrievalConfigDto::default()
            },
        },
    )
}

fn ask_investigate_browser(
    controller: &AppController,
    prompt: &str,
    focus_node_id: Option<NodeId>,
) -> codestory_contracts::api::AgentAnswerDto {
    ask_browser_with_profile(
        controller,
        prompt,
        focus_node_id,
        AgentRetrievalProfileSelectionDto::Preset {
            preset: AgentRetrievalPresetDto::Investigate,
        },
    )
}

fn ask_browser_with_profile(
    controller: &AppController,
    prompt: &str,
    focus_node_id: Option<NodeId>,
    retrieval_profile: AgentRetrievalProfileSelectionDto,
) -> codestory_contracts::api::AgentAnswerDto {
    try_ask_browser_with_profile(controller, prompt, focus_node_id, retrieval_profile)
        .expect("ask browser")
}

fn try_ask_browser_with_profile(
    controller: &AppController,
    prompt: &str,
    focus_node_id: Option<NodeId>,
    retrieval_profile: AgentRetrievalProfileSelectionDto,
) -> Result<codestory_contracts::api::AgentAnswerDto, ApiError> {
    controller.browser_service().ask(AgentAskRequest {
        prompt: prompt.to_string(),
        retrieval_profile,
        focus_node_id,
        max_results: Some(8),
        response_mode: AgentResponseModeDto::Structured,
        latency_budget_ms: Some(30_000),
        include_evidence: true,
        hybrid_weights: None,
    })
}

fn assert_mandatory_sidecar_unavailable(error: &ApiError) {
    assert_eq!(error.code, "retrieval_unavailable");
    assert!(
        error
            .message
            .contains("sidecar retrieval primary is unavailable or degraded"),
        "error should name mandatory sidecar unavailability: {error:?}"
    );
    assert!(
        error.message.contains("expected mode=full"),
        "error should name the full-mode requirement: {error:?}"
    );
    let details = error.details.as_ref().expect("retrieval error details");
    assert_eq!(details.failed_layer.as_deref(), Some("retrieval_sidecar"));
    assert!(
        details
            .next_commands
            .iter()
            .all(|command| !command.contains("codestory-cli index")),
        "sidecar retrieval errors should not repeat core index repair commands: {error:?}"
    );
    assert!(
        details
            .next_commands
            .iter()
            .next()
            .is_some_and(|command| command.contains("codestory-cli ready --goal agent --repair")),
        "retrieval error should start with the canonical agent repair command: {error:?}"
    );
    assert!(
        details
            .next_commands
            .iter()
            .all(|command| !command.contains("codestory-cli retrieval index")),
        "retrieval error should not expose a separate sidecar index command before ready repair: {error:?}"
    );
    assert!(
        details
            .next_commands
            .iter()
            .any(|command| command.contains("codestory-cli retrieval status")
                && command.contains("--format json")),
        "retrieval error should include sidecar status proof command: {error:?}"
    );
}

fn citation_named<'a>(citations: &'a [AgentCitationDto], name: &str) -> &'a AgentCitationDto {
    citations
        .iter()
        .find(|citation| citation.display_name == name)
        .unwrap_or_else(|| panic!("expected citation for {name}"))
}

fn retrieval_markdown(answer: &codestory_contracts::api::AgentAnswerDto) -> String {
    answer
        .sections
        .iter()
        .flat_map(|section| section.blocks.iter())
        .filter_map(|block| match block {
            AgentResponseBlockDto::Markdown { markdown } => Some(markdown.as_str()),
            AgentResponseBlockDto::Mermaid { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn trace_step_status(
    answer: &codestory_contracts::api::AgentAnswerDto,
    kind: AgentRetrievalStepKindDto,
) -> AgentRetrievalStepStatusDto {
    answer
        .retrieval_trace
        .steps
        .iter()
        .find(|step| step.kind == kind)
        .unwrap_or_else(|| panic!("missing trace step {kind:?}"))
        .status
}

fn trace_step(
    answer: &codestory_contracts::api::AgentAnswerDto,
    kind: AgentRetrievalStepKindDto,
) -> &codestory_contracts::api::AgentRetrievalStepDto {
    answer
        .retrieval_trace
        .steps
        .iter()
        .find(|step| step.kind == kind)
        .unwrap_or_else(|| panic!("missing trace step {kind:?}"))
}

fn trace_has_step(
    answer: &codestory_contracts::api::AgentAnswerDto,
    kind: AgentRetrievalStepKindDto,
) -> bool {
    answer
        .retrieval_trace
        .steps
        .iter()
        .any(|step| step.kind == kind)
}

#[test]
#[ignore = "live full-sidecar browser success contract; requires finalized sidecar fixture evidence"]
fn exact_symbol_query_returns_cited_focus_and_trace() {
    let _env = browser_contract_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let focus = search_symbols(&controller, "exact_symbol_anchor", 5)
        .into_iter()
        .next()
        .expect("exact symbol hit");
    assert_eq!(focus.display_name, "exact_symbol_anchor");
    assert!(focus.resolvable);

    let answer = ask_browser(
        &controller,
        "Show the exact_symbol_anchor implementation source snippet",
        Some(focus.node_id.clone()),
    );
    let citation = citation_named(&answer.citations, "exact_symbol_anchor");
    assert_eq!(citation.node_id, focus.node_id);
    assert!(
        citation
            .file_path
            .as_deref()
            .is_some_and(|path| path.ends_with("browser.rs"))
    );
    assert!(citation.retrieval_score_breakdown.is_some());
    assert_eq!(
        answer.retrieval_trace.policy_mode,
        AgentRetrievalPolicyModeDto::CompletenessFirst
    );
    assert_eq!(
        trace_step_status(&answer, AgentRetrievalStepKindDto::Search),
        AgentRetrievalStepStatusDto::Ok
    );
    assert_eq!(
        trace_step_status(&answer, AgentRetrievalStepKindDto::SourceRead),
        AgentRetrievalStepStatusDto::Ok
    );
    assert!(
        retrieval_markdown(&answer).contains("Focused symbol: **exact_symbol_anchor**"),
        "ask output should expose the selected focus"
    );
}

#[test]
fn exact_literal_product_search_fails_closed_without_full_sidecars() {
    let _env = browser_contract_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let error = controller
        .search_results(SearchRequest {
            query: "CODESTORY_BROWSER_LITERAL".to_string(),
            repo_text: SearchRepoTextMode::On,
            limit_per_source: 8,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("mandatory product search should fail closed without full sidecars");

    assert_mandatory_sidecar_unavailable(&error);
}

#[test]
#[ignore = "live full-sidecar browser success contract; requires finalized sidecar fixture evidence"]
fn no_hit_query_records_current_zero_citation_limitation() {
    let _env = browser_contract_symbolic_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let answer = ask_browser(
        &controller,
        "Where is the nonexistent OAuth billing conveyor implemented?",
        None,
    );
    let markdown = retrieval_markdown(&answer);

    assert!(
        answer.citations.is_empty(),
        "symbolic no-hit ask should not invent citations: {:?}",
        answer
            .citations
            .iter()
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        markdown.contains("No indexed symbol matches found"),
        "current no-hit limitation should be visible in retrieval markdown: {markdown}"
    );
    assert_eq!(
        trace_step_status(&answer, AgentRetrievalStepKindDto::Search),
        AgentRetrievalStepStatusDto::Ok
    );
    assert_eq!(
        trace_step_status(&answer, AgentRetrievalStepKindDto::Neighborhood),
        AgentRetrievalStepStatusDto::Skipped
    );
    assert_eq!(
        trace_step_status(&answer, AgentRetrievalStepKindDto::Trail),
        AgentRetrievalStepStatusDto::Skipped
    );
}

#[test]
#[ignore = "live full-sidecar browser success contract; requires finalized sidecar fixture evidence"]
fn natural_language_integration_question_keeps_citations_and_trace_steps() {
    let _env = browser_contract_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let focus = search_symbols(&controller, "orchestrate_browser_session", 5)
        .into_iter()
        .next()
        .expect("integration focus");
    let answer = ask_browser(
        &controller,
        "How does orchestrate_browser_session integrate parse_event with route_browser_request and build_snapshot_digest?",
        Some(focus.node_id.clone()),
    );

    citation_named(&answer.citations, "orchestrate_browser_session");
    assert!(
        answer
            .citations
            .iter()
            .any(|citation| citation.display_name == "parse_event"
                || citation.display_name == "route_browser_request"
                || citation.display_name == "build_snapshot_digest"),
        "integration question should cite at least one collaborating symbol"
    );
    assert_eq!(
        trace_step_status(&answer, AgentRetrievalStepKindDto::Neighborhood),
        AgentRetrievalStepStatusDto::Ok
    );
    assert_eq!(
        trace_step_status(&answer, AgentRetrievalStepKindDto::Trail),
        AgentRetrievalStepStatusDto::Ok
    );
    assert!(
        !answer.graphs.is_empty(),
        "integration ask should carry graph evidence"
    );
}

#[test]
#[ignore = "live full-sidecar browser success contract; requires finalized sidecar fixture evidence"]
fn investigate_strong_symbol_query_uses_initial_hits_without_fallback() {
    let _env = browser_contract_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let answer = ask_investigate_browser(&controller, "exact_symbol_anchor", None);

    citation_named(&answer.citations, "exact_symbol_anchor");
    assert!(
        !trace_has_step(&answer, AgentRetrievalStepKindDto::QueryExpansion),
        "strong initial symbol hits should not pay the query-expansion cost"
    );
    assert!(
        !trace_has_step(&answer, AgentRetrievalStepKindDto::RepoTextFallback),
        "strong initial symbol hits should not pay the repo-text fallback cost"
    );
    let search_step = trace_step(&answer, AgentRetrievalStepKindDto::Search);
    assert!(
        search_step
            .output
            .iter()
            .any(|field| field.key == "accepted_hits" && field.value != "0"),
        "investigation trace should keep strong initial indexed hits accepted"
    );
}

#[test]
#[ignore = "live full-sidecar browser success contract; requires finalized sidecar fixture evidence"]
fn custom_completeness_profile_does_not_run_investigation_fallback() {
    let _env = browser_contract_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let answer = ask_browser(
        &controller,
        "Where is CODESTORY_BROWSER_LITERAL defined?",
        None,
    );

    assert!(
        !trace_has_step(&answer, AgentRetrievalStepKindDto::QueryExpansion),
        "custom completeness profiles should not silently enter investigation mode"
    );
    assert!(
        !trace_has_step(&answer, AgentRetrievalStepKindDto::RepoTextFallback),
        "repo-text fallback should stay behind the explicit Investigate preset"
    );
}

#[test]
#[ignore = "live full-sidecar browser success contract; requires finalized sidecar fixture evidence"]
fn ambiguous_symbol_search_exposes_ranked_alternatives() {
    let _env = browser_contract_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let hits = search_symbols(&controller, "parse_event", 10);
    let alternatives = hits
        .iter()
        .filter(|hit| hit.display_name == "parse_event")
        .collect::<Vec<_>>();

    assert!(
        alternatives.len() >= 2,
        "ambiguous parse_event query should expose alternatives"
    );
    assert!(alternatives.iter().any(|hit| {
        hit.file_path
            .as_deref()
            .is_some_and(|path| path.ends_with("ingest.rs"))
    }));
    assert!(alternatives.iter().any(|hit| {
        hit.file_path
            .as_deref()
            .is_some_and(|path| path.ends_with("routing.rs"))
    }));
    assert!(
        alternatives.iter().all(|hit| hit.resolvable),
        "alternatives should be stable node-id targets for the next browser call"
    );
}

#[test]
#[ignore = "live full-sidecar browser success contract; requires finalized sidecar fixture evidence"]
fn graph_and_snippet_expansion_preserve_neighbor_and_source_evidence() {
    let _env = browser_contract_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let focus = search_symbols(&controller, "expand_browser_context", 5)
        .into_iter()
        .next()
        .expect("expand_browser_context hit");
    let trail = controller
        .trail_context(TrailConfigDto {
            root_id: focus.node_id.clone(),
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: 1,
            direction: TrailDirection::Outgoing,
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: Vec::new(),
            show_utility_calls: true,
            hide_speculative: false,
            story: false,
            node_filter: Vec::new(),
            max_nodes: 20,
            layout_direction: LayoutDirection::Horizontal,
        })
        .expect("trail context");
    let labels = trail
        .trail
        .nodes
        .iter()
        .map(|node| node.label.as_str())
        .collect::<Vec<_>>();

    assert!(
        labels
            .iter()
            .any(|label| label.contains("expand_browser_context"))
    );
    assert!(
        labels
            .iter()
            .any(|label| label.contains("exact_symbol_anchor"))
    );
    assert!(
        labels
            .iter()
            .any(|label| label.contains("build_snapshot_digest"))
    );
    assert!(!trail.trail.truncated);

    let details = controller
        .node_details(NodeDetailsRequest {
            id: focus.node_id.clone(),
        })
        .expect("node details");
    assert_eq!(details.display_name, "expand_browser_context");
    let snippet = controller
        .snippet_context(focus.node_id, 4)
        .expect("snippet context");
    assert!(snippet.snippet.contains("routing::route_browser_request"));
}

#[test]
fn exact_file_literal_investigate_ask_fails_closed_without_full_sidecars() {
    let _env = browser_contract_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let error = try_ask_browser_with_profile(
        &controller,
        "Where is CODESTORY_BROWSER_LITERAL defined?",
        None,
        AgentRetrievalProfileSelectionDto::Preset {
            preset: AgentRetrievalPresetDto::Investigate,
        },
    )
    .expect_err("investigate ask should fail closed instead of citing repo-text fallback");

    assert_mandatory_sidecar_unavailable(&error);
}

#[test]
#[ignore = "live full-sidecar browser success contract; requires finalized sidecar fixture evidence"]
fn stale_index_warning_reports_changed_files_without_refreshing() {
    let _env = browser_contract_env();
    let (controller, workspace, _storage) = indexed_controller();

    fs::write(
        workspace.path().join("src").join("stale_after_index.rs"),
        "pub fn stale_after_index() {}\n",
    )
    .expect("write file after indexing");

    let answer = ask_browser(
        &controller,
        "What changed after indexing and is the browser cache stale?",
        None,
    );

    assert!(
        answer
            .retrieval_trace
            .annotations
            .iter()
            .any(|annotation| annotation.contains("stale_after_index")),
        "future browser output should warn about stale index gaps"
    );
}

#[test]
#[ignore = "live full-sidecar browser success contract; requires finalized sidecar fixture evidence"]
fn no_hit_query_reports_suggestions_and_explicit_gaps() {
    let _env = browser_contract_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let answer = ask_investigate_browser(
        &controller,
        "Where is the nonexistent OAuth billing conveyor implemented?",
        None,
    );
    let markdown = retrieval_markdown(&answer);

    assert!(answer.citations.is_empty());
    assert!(markdown.contains("No indexed symbol matches found"));
    assert!(
        markdown.contains("Gaps:") || markdown.contains("What is missing:"),
        "future ask output should label no-hit gaps explicitly"
    );
    assert!(
        markdown.contains("Try:") || markdown.contains("next useful"),
        "future ask output should include query suggestions or next calls"
    );
    let search_step = trace_step(&answer, AgentRetrievalStepKindDto::Search);
    assert!(
        search_step
            .output
            .iter()
            .any(|field| field.key.contains("hit") && field.value == "0"),
        "no-hit investigation trace should record the weak initial hit count"
    );
    assert!(
        answer.retrieval_trace.annotations.iter().any(|annotation| {
            annotation.contains("gap")
                || annotation.contains("low confidence")
                || annotation.contains("no relevant")
        }),
        "low-confidence investigation should annotate explicit gaps"
    );
}
