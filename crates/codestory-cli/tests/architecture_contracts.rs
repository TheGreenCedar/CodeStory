use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use toml::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate has workspace parent")
        .parent()
        .expect("workspace root exists")
        .to_path_buf()
}

fn read(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).expect("file should be readable")
}

fn manifest(path: &str) -> Value {
    read(path).parse::<Value>().expect("valid Cargo.toml")
}

fn dependency_names(path: &str) -> BTreeSet<String> {
    let manifest = manifest(path);
    let mut names = BTreeSet::new();
    for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = manifest.get(table_name).and_then(Value::as_table) {
            names.extend(table.keys().cloned());
        }
    }
    names
}

fn workspace_members() -> BTreeSet<String> {
    manifest("Cargo.toml")
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(Value::as_array)
        .expect("workspace members")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn collect_rs_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read source dir") {
        let entry = entry.expect("source entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, files);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn source_tree_contains(dir: &str, needle: &str) -> bool {
    let mut files = Vec::new();
    collect_rs_files(&repo_root().join(dir), &mut files);
    files.into_iter().any(|path| {
        fs::read_to_string(path)
            .expect("read source")
            .contains(needle)
    })
}

fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = source.find(start).expect("start marker exists");
    let tail = &source[start_index..];
    let end_index = tail.find(end).expect("end marker exists");
    &tail[..end_index]
}

#[test]
fn workspace_crate_stays_decoupled_from_store_and_runtime() {
    let dependencies = dependency_names("crates/codestory-workspace/Cargo.toml");
    assert!(
        !dependencies.contains("codestory-store")
            && !dependencies.contains("codestory-runtime")
            && !dependencies.contains("codestory-cli"),
        "workspace crate should only own discovery and planning inputs"
    );
}

#[test]
fn indexer_crate_stays_decoupled_from_runtime_and_cli() {
    let dependencies = dependency_names("crates/codestory-indexer/Cargo.toml");
    assert!(
        !dependencies.contains("codestory-runtime") && !dependencies.contains("codestory-cli"),
        "indexer crate should not depend on runtime or cli"
    );
}

#[test]
fn runtime_crate_depends_on_v2_surfaces_only() {
    let dependencies = dependency_names("crates/codestory-runtime/Cargo.toml");
    for required in [
        "codestory-contracts",
        "codestory-indexer",
        "codestory-store",
    ] {
        assert!(
            dependencies.contains(required),
            "runtime should depend on {required}"
        );
    }
    for legacy in [
        "codestory-app",
        "codestory-search",
        "codestory-storage",
        "codestory-api",
        "codestory-events",
        "codestory-core",
        "codestory-index",
    ] {
        assert!(
            !dependencies.contains(legacy),
            "runtime should not depend on removed legacy crate {legacy}"
        );
    }
}

#[test]
fn store_crate_owns_persistence_without_legacy_escape_hatches() {
    let dependencies = dependency_names("crates/codestory-store/Cargo.toml");
    assert!(
        !dependencies.contains("codestory-workspace"),
        "store should not depend on workspace discovery or refresh planning"
    );

    for legacy in [
        "codestory-storage",
        "codestory-core",
        "codestory-api",
        "codestory-events",
    ] {
        assert!(
            !dependencies.contains(legacy),
            "store should not depend on removed legacy crate {legacy}"
        );
    }

    let store_src = read("crates/codestory-store/src/lib.rs");
    assert!(
        !store_src.contains("from_storage(")
            && !store_src.contains("into_inner(")
            && !store_src.contains("storage_mut(")
            && !store_src.contains("as_inner(")
            && !store_src.contains("Deref for Store")
            && !store_src.contains("DerefMut for Store"),
        "store facade should not expose raw storage escape hatches"
    );
}

#[test]
fn cli_stays_thin() {
    let dependencies = dependency_names("crates/codestory-cli/Cargo.toml");
    assert!(
        dependencies.contains("codestory-runtime"),
        "cli should depend on runtime surface"
    );
    assert!(
        !dependencies.contains("codestory-store") && !dependencies.contains("codestory-indexer"),
        "cli should not reach directly into store or indexer"
    );
    for forbidden in ["codestory_store::", "codestory_indexer::"] {
        assert!(
            !source_tree_contains("crates/codestory-cli/src", forbidden),
            "CLI source tree should not reference {forbidden} directly"
        );
    }
}

#[test]
fn runtime_exposes_read_only_browser_service_boundary() {
    let runtime_lib = read("crates/codestory-runtime/src/lib.rs");
    let browser = read("crates/codestory-runtime/src/browser.rs");
    let cli_runtime = read("crates/codestory-cli/src/runtime.rs");
    let cli_main = read("crates/codestory-cli/src/main.rs");
    let http_transport = read("crates/codestory-cli/src/http_transport.rs");
    let stdio_transport = read("crates/codestory-cli/src/stdio_transport.rs");
    let explore = read("crates/codestory-cli/src/explore.rs");
    let cli_browser_surfaces = [
        cli_main.as_str(),
        http_transport.as_str(),
        stdio_transport.as_str(),
        explore.as_str(),
    ]
    .join("\n");

    assert!(
        runtime_lib.contains("pub use browser::{BrowserQueryItem, ReadOnlyBrowserService}")
            && runtime_lib.contains("pub fn browser_service(&self) -> ReadOnlyBrowserService"),
        "runtime should export a read-only browser service accessor"
    );
    assert!(
        browser.contains("pub struct ReadOnlyBrowserService")
            && browser.contains("pub fn search_results")
            && browser.contains("pub fn symbol_context")
            && browser.contains("pub fn definition_context")
            && browser.contains("pub fn trail_context")
            && browser.contains("pub fn references_context")
            && browser.contains("pub fn snippet_context")
            && browser.contains("pub fn list_root_symbols")
            && browser.contains("pub fn list_children_symbols")
            && browser.contains("pub fn query")
            && browser.contains("pub fn ask"),
        "read-only browser service should own the browser-facing read methods"
    );
    assert!(
        !browser.contains("run_local_agent"),
        "read-only browser context retrieval should not carry local-agent execution controls"
    );

    for forbidden in [
        "open_definition",
        "write_file",
        "WriteFile",
        "OpenContainingFolder",
        "SystemActionResponse",
        "launch_definition",
        "TcpListener",
        "run_stdio_server",
        "handle_http_request",
    ] {
        assert!(
            !browser.contains(forbidden),
            "read-only browser service should not mention forbidden write/system/transport API {forbidden}"
        );
    }

    assert!(
        cli_runtime.contains("pub(crate) browser: ReadOnlyBrowserService")
            && cli_runtime.contains("browser: runtime.browser_service()"),
        "CLI runtime context should carry the runtime-owned browser boundary"
    );
    assert!(
        cli_browser_surfaces.contains(".search_results(SearchRequest")
            && cli_browser_surfaces.contains(".symbol_context(")
            && cli_browser_surfaces.contains(".definition_context(")
            && cli_browser_surfaces.contains(".references_context(")
            && cli_browser_surfaces.contains(".list_root_symbols(")
            && cli_browser_surfaces.contains(".list_children_symbols(")
            && cli_browser_surfaces.contains(".trail_context(")
            && cli_browser_surfaces.contains(".snippet_context(")
            && cli_browser_surfaces.contains(".query(&ast)")
            && cli_main.contains("runtime.browser.ask(request)")
            && !cli_main.contains("runtime.agent.ask(request)"),
        "CLI read-only browser operations should route through RuntimeContext.browser"
    );
}

#[test]
fn stdio_tool_catalog_stays_aligned_with_read_only_browser_service_operations() {
    let browser = read("crates/codestory-runtime/src/browser.rs");
    let stdio_transport = read("crates/codestory-cli/src/stdio_transport.rs");
    let stdio_catalog = read("crates/codestory-cli/src/stdio_catalog.rs");
    let stdio_tool_catalog = source_between(&stdio_catalog, "static TOOLS", "static RESOURCES");

    let expected_tools = [
        ("search", ".search_results(", "pub fn search_results"),
        ("symbol", ".symbol_context(", "pub fn symbol_context"),
        (
            "definition",
            ".definition_context(",
            "pub fn definition_context",
        ),
        (
            "references",
            ".references_context(",
            "pub fn references_context",
        ),
        ("symbols", ".list_root_symbols(", "pub fn list_root_symbols"),
        (
            "symbols",
            ".list_children_symbols(",
            "pub fn list_children_symbols",
        ),
        ("trail", ".trail_context(", "pub fn trail_context"),
        ("snippet", ".snippet_context(", "pub fn snippet_context"),
        ("context", ".ask(", "pub fn ask"),
    ];

    for (tool_name, cli_call, browser_method) in expected_tools {
        assert!(
            stdio_tool_catalog.contains(&format!("\"{tool_name}\"")),
            "stdio catalog/router should include read-only browser tool {tool_name}"
        );
        assert!(
            stdio_transport.contains(cli_call),
            "stdio tool {tool_name} should route through RuntimeContext.browser operation {cli_call}"
        );
        assert!(
            browser.contains(browser_method),
            "ReadOnlyBrowserService should expose operation for stdio tool {tool_name}: {browser_method}"
        );
    }

    for forbidden in [
        "\"write",
        "\"edit",
        "\"delete",
        "\"patch",
        "\"shell",
        "\"exec",
        "\"launch",
        "\"open_folder",
    ] {
        assert!(
            !stdio_tool_catalog.contains(forbidden),
            "stdio read-only tool catalog should not expose write/system tool prefix {forbidden}"
        );
    }
}

#[test]
fn web_cockpit_stays_deferred_until_browser_surface_gate_opens() {
    let gate = read("docs/architecture/browser-surface-gate.md");
    let warm_stats = read("docs/testing/codestory-stdio-warm-loop-stats.md");
    let runtime_path = read("docs/architecture/runtime-execution-path.md");
    let cli_args = read("crates/codestory-cli/src/args.rs");
    let http_transport = read("crates/codestory-cli/src/http_transport.rs");
    let command_enum = source_between(
        &cli_args,
        "pub(crate) enum Command",
        "#[derive(Args, Debug, Clone)]",
    );
    let http_routes = source_between(
        &http_transport,
        "match path {",
        "fn browser_references_config",
    );

    for required in [
        "Status: deferred.",
        "Do not add a new `browse` command, web cockpit route, or browser-specific web UI",
        "Tool, resource, and prompt manifests",
        "Warm stdio/browser-loop p50, p95, and p99",
        "Current Promotion Budget",
        "docs/testing/codestory-stdio-warm-loop-stats.md",
        "Browser stress lanes",
        "`explore` must demonstrate the cockpit workflow",
        "Screenshot-visible review",
    ] {
        assert!(
            gate.contains(required),
            "browser surface gate should document required gate evidence: missing {required}"
        );
    }

    assert!(
        runtime_path.contains("browser surface gate")
            && runtime_path.contains("Do not add a")
            && runtime_path.contains(
                "separate `browse` command, web cockpit route, or browser-specific web UI"
            ),
        "runtime execution path should point future UI work at the browser surface gate"
    );
    assert!(
        warm_stats.contains("## Current Promotion Budget")
            && warm_stats.contains("### Smoke Budget")
            && warm_stats.contains("| search | 75 ms |")
            && warm_stats.contains("| symbol | 50 ms |")
            && warm_stats.contains("| trail | 75 ms |")
            && warm_stats.contains("| snippet | 50 ms |")
            && warm_stats.contains("| resources/read:status | 50 ms |")
            && warm_stats.contains("| full `search -> symbol -> trail -> snippet` loop | 250 ms |")
            && warm_stats.contains("### Web Cockpit Promotion Budget")
            && warm_stats.contains("| each default read operation | 500 ms |")
            && warm_stats.contains("| full `search -> symbol -> trail -> snippet` loop | 1.5 s |")
            && warm_stats.contains("browser surface gate stays closed"),
        "warm-loop stats doc should own the active p95 budget state"
    );

    assert!(
        command_enum.contains("Explore(ExploreCommand)")
            && command_enum.contains("Serve(ServeCommand)"),
        "explore and serve should remain the current browser surfaces"
    );
    for forbidden in ["Browse(", "BrowseCommand", "WebCockpit", "CockpitCommand"] {
        assert!(
            !command_enum.contains(forbidden),
            "web cockpit/browse surface is deferred; unexpected CLI command {forbidden}"
        );
    }
    for forbidden_route in ["\"/browse\"", "\"/cockpit\"", "\"/ui\"", "\"/web\""] {
        assert!(
            !http_routes.contains(forbidden_route),
            "web cockpit/browse route is deferred until the browser surface gate opens: {forbidden_route}"
        );
    }
}

#[test]
fn runtime_snapshot_lifecycle_flows_through_store_snapshot_surface() {
    let runtime = read("crates/codestory-runtime/src/lib.rs");
    assert!(
        runtime.contains("SnapshotStore::open_staged(storage_path)")
            && runtime.contains("finalize_staged()")
            && runtime.contains("staged.publish(storage_path)"),
        "full refresh should stage, finalize, and publish snapshots through the store snapshot surface"
    );
    assert!(
        runtime.contains("store.snapshots().refresh_all_with_stats()"),
        "incremental refresh should use the snapshot surface for summary/detail refresh"
    );
    assert!(
        !runtime.contains("create_deferred_secondary_indexes()")
            && !runtime.contains("refresh_grounding_summary_snapshots()")
            && !runtime.contains("hydrate_grounding_detail_snapshots()"),
        "snapshot lifecycle should not be orchestrated directly outside the store snapshot surface"
    );
}

#[test]
fn legacy_crates_are_removed_from_the_workspace() {
    let members = workspace_members();
    for legacy in [
        "crates/codestory-app",
        "crates/codestory-project",
        "crates/codestory-search",
        "crates/codestory-core",
        "crates/codestory-api",
        "crates/codestory-events",
        "crates/codestory-storage",
        "crates/codestory-index",
    ] {
        assert!(
            !members.contains(legacy),
            "workspace should not register removed crate {legacy}"
        );
    }
}
