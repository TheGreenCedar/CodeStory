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

fn read_source_tree(dir: &str) -> String {
    let mut files = Vec::new();
    collect_rs_files(&repo_root().join(dir), &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| fs::read_to_string(path).expect("read source"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = source.find(start).expect("start marker exists");
    let tail = &source[start_index..];
    let end_index = tail.find(end).expect("end marker exists");
    &tail[..end_index]
}

#[test]
fn cli_sidecar_construction_stays_behind_test_safe_gateway() {
    let source_root = repo_root().join("crates/codestory-cli/src");
    let gateway_path = source_root.join("sidecar_runtime.rs");
    let gateway = fs::read_to_string(&gateway_path).expect("read sidecar runtime gateway");
    let activation = gateway
        .find("enable_automatic_test_cache_root_for_process")
        .expect("gateway enables automatic unit-test cache isolation");
    let first_cache_lookup = gateway
        .find("codestory_retrieval::user_cache_root()")
        .expect("gateway owns the platform cache lookup");
    assert!(
        activation < first_cache_lookup,
        "test cache isolation must be enabled before the first platform cache lookup"
    );

    let config = read("crates/codestory-cli/src/config.rs");
    let startup = source_between(
        &config,
        "pub(crate) fn from_process_env()",
        "#[derive(Debug, Clone, Default, Deserialize)]",
    );
    assert!(
        startup.contains("crate::sidecar_runtime::prepare_cache_access();"),
        "startup configuration must activate cache isolation before resolving its cache root"
    );

    let mut files = Vec::new();
    collect_rs_files(&source_root, &mut files);
    let forbidden = [
        "SidecarRuntimeConfig::local(",
        "SidecarRuntimeConfig::for_project_",
        "sidecar_runtime_for_project(",
        "sidecar_runtime_for_project_with_run_id(",
        "strict_sidecar_status_for_profile(",
        "codestory_retrieval::embedding_runtime_id()",
        "codestory_retrieval::user_cache_root(",
        "enable_automatic_test_cache_root_for_process",
    ];
    let mut violations = Vec::new();
    for path in files {
        if path == gateway_path {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read CLI source");
        for needle in forbidden {
            if source.contains(needle) {
                violations.push(format!("{}: {needle}", path.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "CLI sidecar constructors must remain behind sidecar_runtime.rs:\n{}",
        violations.join("\n")
    );
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
fn cli_binaries_preserve_the_library_module_graph() {
    let cli_main = read("crates/codestory-cli/src/main.rs");
    let runtime_main = read("crates/codestory-cli/src/runtime_main.rs");
    let cli_lib = read("crates/codestory-cli/src/lib.rs");
    let launcher_modules = cli_main
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            line.strip_prefix("mod ")
                .and_then(|module| module.strip_suffix(';'))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        launcher_modules,
        ["native_launcher", "native_runtime_layout"],
        "the public CLI binary may own only its static native launcher"
    );
    assert!(
        runtime_main.contains("codestory_cli::run()"),
        "the internal runtime binary should delegate to the library entrypoint"
    );
    for module in ["embedding_server_transport", "sidecar_runtime"] {
        let declaration = format!("mod {module};");
        assert_eq!(
            cli_lib.matches(&declaration).count(),
            1,
            "{module} should have one library-owned module declaration"
        );
        assert!(
            !cli_main.contains(&declaration) && !runtime_main.contains(&declaration),
            "{module} must not be compiled again by either binary"
        );
    }
}

#[test]
fn runtime_exposes_read_only_browser_service_boundary() {
    let runtime_lib = read("crates/codestory-runtime/src/lib.rs");
    let browser = read("crates/codestory-runtime/src/browser.rs");
    let cli_runtime = read("crates/codestory-cli/src/runtime.rs");
    let cli_app = read_source_tree("crates/codestory-cli/src/app");
    let http_transport = read("crates/codestory-cli/src/http_transport.rs");
    let stdio_transport = read("crates/codestory-cli/src/stdio_transport.rs");
    let explore = read("crates/codestory-cli/src/explore.rs");
    let cli_browser_surfaces = [
        cli_app.as_str(),
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
            && cli_app.contains("runtime.browser.ask(request)")
            && !cli_app.contains("runtime.agent.ask(request)"),
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
        ("callers", ".trail_context(", "pub fn trail_context"),
        ("callees", ".trail_context(", "pub fn trail_context"),
        ("trace", ".trail_context(", "pub fn trail_context"),
        ("symbols", ".list_root_symbols(", "pub fn list_root_symbols"),
        (
            "symbols",
            ".list_children_symbols(",
            "pub fn list_children_symbols",
        ),
        ("trail", ".trail_context(", "pub fn trail_context"),
        ("snippet", ".snippet_context(", "pub fn snippet_context"),
        (
            "affected",
            ".affected_analysis(",
            "pub fn affected_analysis",
        ),
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

    assert!(
        command_enum.contains("Explore(ExploreCommand)")
            && command_enum.contains("Serve(ServeCommand)"),
        "explore and serve should remain the current browser surfaces"
    );
    for forbidden in ["Browse(", "BrowseCommand", "WebCockpit", "CockpitCommand"] {
        assert!(
            !command_enum.contains(forbidden),
            "web UI/browse surface is deferred; unexpected CLI command {forbidden}"
        );
    }
    for forbidden_route in ["\"/browse\"", "\"/cockpit\"", "\"/ui\"", "\"/web\""] {
        assert!(
            !http_routes.contains(forbidden_route),
            "web UI/browse route is deferred until the browser surface gate opens: {forbidden_route}"
        );
    }
}

#[test]
fn runtime_snapshot_lifecycle_flows_through_store_snapshot_surface() {
    let full_refresh = read("crates/codestory-runtime/src/index_full.rs");
    let incremental_refresh = read("crates/codestory-runtime/src/index_incremental.rs");
    let commit = read("crates/codestory-runtime/src/index_commit.rs");
    assert!(
        full_refresh.contains("SnapshotStore::open_disposable_full_refresh(storage_path)")
            && full_refresh.contains("staged.snapshots().finalize_staged()")
            && full_refresh.contains("staged.snapshots().refresh_detail()")
            && commit.contains(".publish_with_stats(&self.storage_path)"),
        "full refresh should stage, finalize, and publish snapshots through the store snapshot surface"
    );
    assert!(
        incremental_refresh.contains("SnapshotStore::clone_live_to_staged(storage_path)")
            && incremental_refresh.contains(".snapshots()\n        .finalize_staged()")
            && incremental_refresh.contains(".snapshots()\n        .refresh_detail()")
            && commit.contains(".publish_with_stats(&self.storage_path)"),
        "incremental refresh should clone, finalize both snapshot tiers, and publish through the staged snapshot surface"
    );
    for forbidden in [
        "create_deferred_secondary_indexes()",
        "refresh_grounding_summary_snapshots()",
        "hydrate_grounding_detail_snapshots()",
    ] {
        assert!(
            !source_tree_contains("crates/codestory-runtime/src", forbidden),
            "snapshot lifecycle should not be orchestrated directly outside the store snapshot surface: {forbidden}"
        );
    }
}

#[test]
fn staged_publication_identity_and_fence_are_complete_before_publication() {
    let full_refresh = read("crates/codestory-runtime/src/index_full.rs");
    let incremental_refresh = read("crates/codestory-runtime/src/index_incremental.rs");
    let commit = read("crates/codestory-runtime/src/index_commit.rs");
    let store = read("crates/codestory-store/src/storage_impl/mod.rs");
    let schema = read("crates/codestory-store/src/storage_impl/schema.rs");

    assert!(
        store.contains("pub struct IndexPublicationRecord")
            && store.contains("pub fn database_index_publication")
            && store.contains("pub fn put_index_publication"),
        "publication identity should be a typed store contract with read-only and staged-write surfaces"
    );
    assert!(
        schema.contains("CREATE TABLE IF NOT EXISTS index_publication"),
        "publication identity should survive process restarts in the SQLite schema"
    );
    assert!(
        commit.contains("pub(super) fn next_index_publication(")
            && commit.contains(".put_index_publication(publication)")
            && commit.contains(".finish_incremental_run()")
            && commit.contains(".publish_with_stats(&self.storage_path)")
            && full_refresh.contains("next_index_publication(")
            && full_refresh.contains("stage_core_publication_identity(")
            && full_refresh.contains("CoreCommitMode::Full")
            && incremental_refresh.contains("next_index_publication(")
            && incremental_refresh.contains("stage_core_publication_identity(")
            && incremental_refresh.contains("CoreCommitMode::Incremental"),
        "full and incremental staging should persist publication identity and clear compatibility fences before publishing"
    );
}

#[test]
fn product_search_builds_stream_canonical_nodes_without_legacy_projection_rebuilds() {
    let runtime = read("crates/codestory-runtime/src/search_state_cache.rs");
    let persisted_builder = source_between(
        &runtime,
        "pub(super) fn build_persisted_search_state_from_canonical_symbols(",
        "#[cfg(test)]\npub(super) fn rebuild_search_state_from_storage(",
    );
    let runtime_rebuild = source_between(
        &runtime,
        "pub(super) fn rebuild_search_state_from_storage_for_runtime(",
        "pub(super) fn refresh_caches(",
    );
    let retrieval = read("crates/codestory-retrieval/src/index.rs");
    let retrieval_scip = read("crates/codestory-retrieval/src/scip_index.rs");
    let scip_emit = source_between(
        &retrieval_scip,
        "pub fn emit_scip_artifacts_from_store(",
        "fn scip_revision_for_symbols(",
    );

    assert!(
        persisted_builder.contains("get_canonical_search_symbol_count()")
            && persisted_builder.contains("get_canonical_search_symbol_batch_after(")
            && persisted_builder.contains("engine.begin_symbol_index()")
            && runtime_rebuild.contains("build_persisted_search_state_from_canonical_symbols("),
        "persisted product search should stream canonical node pages through one symbol writer"
    );
    for forbidden in [
        ".get_nodes()",
        "rebuild_search_symbol_projection",
        "get_search_symbol_projection_batch_after",
    ] {
        assert!(
            !persisted_builder.contains(forbidden) && !runtime_rebuild.contains(forbidden),
            "runtime product search build must not use legacy materialization path {forbidden}"
        );
    }
    assert!(
        !retrieval.contains("rebuild_search_symbol_projection")
            && scip_emit.contains("get_canonical_search_symbol_detail_batch_after(")
            && !scip_emit.contains("get_search_symbol_projection"),
        "retrieval preparation and SCIP emission should not rebuild or read the legacy search projection"
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
