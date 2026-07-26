//! Read-only browser surface contracts: graph reads serve from the complete
//! core publication while product search and ask fail closed without full
//! retrieval.

use codestory_contracts::api::{
    AgentAskRequest, AgentResponseModeDto, AgentRetrievalPresetDto,
    AgentRetrievalProfileSelectionDto, ApiError, IndexMode, LayoutDirection,
    ListRootSymbolsRequest, NodeDetailsRequest, NodeId, SearchRepoTextMode, SearchRequest,
    TrailCallerScope, TrailConfigDto, TrailDirection, TrailMode,
};
use codestory_contracts::workspace::SourceIndexPolicy;
use codestory_retrieval::{
    SidecarProcessDefaults, SidecarProfile, SidecarRuntimeConfig, SidecarRuntimeDefaults,
    SidecarRuntimeOverrides,
};
use codestory_runtime::{ReadOnlyBrowserService, Runtime, RuntimeProcessConfig};
use std::fs;
use std::path::Path;
use tempfile::{TempDir, tempdir};

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

fn indexed_runtime() -> (Runtime, TempDir, TempDir, TempDir) {
    let workspace = tempdir().expect("workspace dir");
    write_browser_fixture(workspace.path());

    let storage = tempdir().expect("storage dir");
    let cache = tempdir().expect("cache dir");
    let sidecar = SidecarRuntimeConfig::for_project_profile_with_process_defaults(
        Some(workspace.path()),
        SidecarProfile::Local,
        None,
        &SidecarProcessDefaults::new(
            cache.path().to_path_buf(),
            SidecarRuntimeDefaults::default(),
        ),
        &SidecarRuntimeOverrides::default(),
    );
    let runtime = Runtime::new_with_process_config(RuntimeProcessConfig::new(
        sidecar,
        SourceIndexPolicy::default(),
    ));
    let project = runtime.project_service();
    project
        .open_project_with_storage_path(
            workspace.path().to_path_buf(),
            storage.path().join("codestory.db"),
        )
        .expect("open project");
    // Core-only indexing keeps the fixture without retrieval sidecars so the
    // fail-closed contracts observe exactly the missing-full-retrieval state.
    project
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index workspace");

    (runtime, workspace, storage, cache)
}

fn assert_mandatory_retrieval_unavailable(error: &ApiError) {
    assert_eq!(error.code, "retrieval_unavailable");
    assert!(
        error
            .message
            .contains("retrieval is unavailable or degraded"),
        "error should name mandatory retrieval unavailability: {error:?}"
    );
    assert!(
        error.message.contains("expected profile=agent mode=full"),
        "error should name the agent full-mode requirement: {error:?}"
    );
    let details = error.details.as_ref().expect("retrieval error details");
    assert_eq!(details.failed_layer.as_deref(), Some("retrieval_engine"));
    assert!(
        details.next_commands.first().is_some_and(|command| {
            command.contains("codestory-cli retrieval index --profile agent")
                && command.contains("--format json")
        }),
        "retrieval error should start with the canonical retrieval repair command: {error:?}"
    );
    assert!(
        details
            .next_commands
            .iter()
            .any(|command| command.contains("codestory-cli retrieval status")
                && command.contains("--format json")),
        "retrieval error should include the retrieval status proof command: {error:?}"
    );
    assert!(
        details
            .next_commands
            .iter()
            .all(|command| !command.contains("codestory-cli index")),
        "retrieval errors should not repeat core index repair commands: {error:?}"
    );
}

fn root_symbol_id(browser: &ReadOnlyBrowserService, needle: &str, file: &str) -> NodeId {
    // The fixture re-exports the browser symbols from lib.rs, so the defining
    // file disambiguates the definition node from its alias.
    browser
        .list_root_symbols(ListRootSymbolsRequest { limit: Some(200) })
        .expect("list root symbols")
        .into_iter()
        .find(|symbol| {
            symbol.label.contains(needle)
                && symbol
                    .file_path
                    .as_deref()
                    .is_some_and(|path| path.ends_with(file))
        })
        .unwrap_or_else(|| panic!("expected root symbol {needle} in {file}"))
        .id
}

#[test]
fn exact_literal_product_search_fails_closed_without_full_retrieval() {
    let (runtime, _workspace, _storage, _cache) = indexed_runtime();

    let error = runtime
        .browser_service()
        .search_results(SearchRequest {
            query: "CODESTORY_BROWSER_LITERAL".to_string(),
            repo_text: SearchRepoTextMode::On,
            limit_per_source: 8,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("mandatory product search should fail closed without full retrieval");

    assert_mandatory_retrieval_unavailable(&error);
}

#[test]
fn exact_file_literal_investigate_ask_fails_closed_without_full_retrieval() {
    let (runtime, _workspace, _storage, _cache) = indexed_runtime();

    let error = runtime
        .browser_service()
        .ask(AgentAskRequest {
            prompt: "Where is CODESTORY_BROWSER_LITERAL defined?".to_string(),
            retrieval_profile: AgentRetrievalProfileSelectionDto::Preset {
                preset: AgentRetrievalPresetDto::Investigate,
            },
            focus_node_id: None,
            max_results: Some(8),
            response_mode: AgentResponseModeDto::Structured,
            latency_budget_ms: Some(30_000),
            include_evidence: true,
            hybrid_weights: None,
        })
        .expect_err("investigate ask should fail closed instead of citing repo-text fallback");

    assert_mandatory_retrieval_unavailable(&error);
}

#[test]
fn graph_and_snippet_expansion_preserve_neighbor_and_source_evidence() {
    let (runtime, _workspace, _storage, _cache) = indexed_runtime();
    let browser = runtime.browser_service();

    let focus = root_symbol_id(&browser, "expand_browser_context", "browser.rs");
    let trail = browser
        .trail_context(TrailConfigDto {
            root_id: focus.clone(),
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
            .any(|label| label.contains("expand_browser_context")),
        "trail should keep the focus node: {labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|label| label.contains("exact_symbol_anchor")),
        "trail should preserve callee neighbor evidence: {labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|label| label.contains("build_snapshot_digest")),
        "trail should preserve callee neighbor evidence: {labels:?}"
    );
    assert!(!trail.trail.truncated);

    let details = browser
        .node_details(NodeDetailsRequest { id: focus.clone() })
        .expect("node details");
    assert_eq!(details.display_name, "expand_browser_context");
    let snippet = browser.snippet_context(focus, 4).expect("snippet context");
    assert!(snippet.snippet.contains("routing::route_browser_request"));
}
