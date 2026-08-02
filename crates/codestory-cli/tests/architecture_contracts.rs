use std::collections::{BTreeMap, BTreeSet};
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

fn production_source_prefix(source: &str) -> &str {
    source
        .split_once("#[cfg(test)]\nmod tests {")
        .map_or(source, |(production, _)| production)
}

/// Everything in `source` that a release build actually compiles: every
/// top-level `#[cfg(test)]` item is removed, wherever in the file it sits.
///
/// The shape this replaces was `source.split("\n#[cfg(test)]\n").next()`, which
/// keeps only the text before the *first* gate. A single `#[cfg(test)] use ..;`
/// in a file's import block therefore exempted the entire rest of that file
/// from every contract built on the helper — including
/// `crates/codestory-runtime/src/search/engine.rs` (gate on line 2),
/// `crates/codestory-runtime/src/index_commit.rs` (line 3),
/// `crates/codestory-store/src/storage_impl/mod.rs` (line 12), and
/// `crates/codestory-runtime/src/lib.rs` (line 41), which between them hold the
/// biggest lock and owned-artifact call sites in the tree.
///
/// `#[cfg(any(test, feature = "test-support"))]` is deliberately *not* stripped:
/// that gate compiles into a release build whenever the feature is on, so its
/// body is production code.
fn production_source(source: &str) -> String {
    let mut kept = String::with_capacity(source.len());
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        if line.trim_end() == "#[cfg(test)]" {
            skip_gated_item(&mut lines);
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

/// Consume the item a top-level `#[cfg(test)]` gates. Rustfmt guarantees the
/// two shapes that matter: a block item closes with a column-zero `}` or `};`,
/// and a statement item ends at the first line closing with `;`.
fn skip_gated_item<'a>(lines: &mut impl Iterator<Item = &'a str>) {
    let mut opened_block = false;
    for line in lines {
        if line.starts_with('}') {
            return;
        }
        if line.contains('{') {
            opened_block = true;
            continue;
        }
        if !opened_block && line.trim_end().ends_with(';') {
            return;
        }
    }
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
    let runtime_source = read_source_tree("crates/codestory-runtime/src");
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
    assert!(
        !repo_root()
            .join("crates/codestory-runtime/src/system_actions.rs")
            .exists()
            && !runtime_source.contains("CODESTORY_IDE_COMMAND"),
        "the unreachable system-actions shell surface must stay deleted"
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
    let controller_files = read("crates/codestory-runtime/src/controller_files.rs");
    let services = read("crates/codestory-runtime/src/services.rs");

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
    for forbidden in ["open_definition", "open_containing_folder"] {
        assert!(
            !controller_files.contains(forbidden) && !services.contains(forbidden),
            "dormant runtime system action must stay absent: {forbidden}"
        );
    }
}

#[test]
fn graph_family_adapters_answer_with_typed_errors_instead_of_stringified_ones() {
    // ARCH-019: the graph tools used to hand agents `format!("{e}")`, forcing
    // downstream substring re-classification. Every error slot in the graph
    // handlers must now come from a producer that keeps the ApiError code.
    let stdio_transport = read("crates/codestory-cli/src/stdio_transport.rs");
    let graph_handlers = source_between(
        &stdio_transport,
        "fn handle_stdio_symbol(",
        "fn stdio_graph_tool_output(",
    );
    let typed_producers = [
        "stdio_typed_error_value(",
        "stdio_api_error_value(",
        "stdio_graph_argument_error(",
    ];
    let lines = graph_handlers.lines().collect::<Vec<_>>();
    let mut violations = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains("\"error\":") {
            continue;
        }
        let window = lines[index..lines.len().min(index + 3)].join("\n");
        if !typed_producers
            .iter()
            .any(|producer| window.contains(producer))
        {
            violations.push(line.trim().to_string());
        }
    }
    assert!(
        violations.is_empty(),
        "graph-family error slots must carry a typed ApiError:\n{}",
        violations.join("\n")
    );

    assert!(
        stdio_transport.contains("fn stdio_typed_error_value(")
            && source_between(
                &stdio_transport,
                "fn stdio_typed_error_value(",
                "fn read_stdio_resource("
            )
            .contains("crate::runtime::api_error_in_chain(error)"),
        "the stdio graph error seam must recover the runtime's typed classification"
    );

    let http_transport = read("crates/codestory-cli/src/http_transport.rs");
    assert!(
        source_between(
            &http_transport,
            "fn write_http_typed_error(",
            "fn target_selection_from_params("
        )
        .contains("runtime::api_error_in_chain(error)"),
        "the HTTP graph error seam must recover the runtime's typed classification"
    );
    assert!(
        source_between(
            &http_transport,
            "fn write_http_target_error(",
            "fn write_http_typed_error("
        )
        .contains("write_http_typed_error("),
        "HTTP target failures must route through the typed error writer"
    );
}

#[test]
fn mcp_tool_arguments_are_validated_from_the_generated_catalog() {
    // CR-026: the advertised schema is the contract. Validation reads the
    // published declaration so a new catalog constraint cannot ship unenforced.
    let validator = read("crates/codestory-cli/src/stdio_arguments.rs");
    let catalog = read("crates/codestory-cli/src/stdio_catalog.rs");
    let stdio_transport = read("crates/codestory-cli/src/stdio_transport.rs");

    assert!(
        validator.contains("crate::stdio_catalog::tool_input_schema(tool)"),
        "argument validation must read the published catalog schema, not a parallel rule set"
    );
    assert!(
        catalog.contains("pub(crate) fn tool_input_schema(")
            && source_between(
                &catalog,
                "pub(crate) fn tool_input_schema(",
                "/// Build the `tools/list` response."
            )
            .contains(".to_json()"),
        "the validated schema must be the same value tools/list emits"
    );
    assert!(
        source_between(
            &stdio_transport,
            "\"tools/call\" => {",
            "let prepared = match"
        )
        .contains("crate::stdio_arguments::validate_tool_arguments("),
        "tools/call must validate arguments before dispatching a tool"
    );

    // Selectors the runtime resolves to exactly one target must advertise
    // oneOf; anyOf would promise a combination the runtime cannot honour.
    let input_schemas = source_between(&catalog, "static SEARCH_INPUT_SCHEMA", "static TOOLS");
    assert!(
        !input_schemas.contains("with_any_of_required"),
        "tool input selectors must advertise oneOf, not anyOf"
    );
}

#[test]
fn production_source_never_spawns_git() {
    let mut files = Vec::new();
    collect_rs_files(&repo_root().join("crates"), &mut files);
    let mut violations = Vec::new();
    for path in files {
        if !path
            .components()
            .any(|component| component.as_os_str() == "src")
        {
            continue;
        }
        if path == repo_root().join("crates/codestory-runtime/src/test_support.rs") {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read Rust source");
        if production_source_prefix(&source).contains("Command::new(\"git\")") {
            violations.push(path.display().to_string());
        }
    }
    assert!(
        violations.is_empty(),
        "production Git reads must stay behind the non-executing workspace reader:\n{}",
        violations.join("\n")
    );
}

#[test]
fn advisory_file_locks_have_exactly_one_bounded_entry_point() {
    // A blocking `flock` observes neither a deadline nor a cancellation flag,
    // so a stalled sibling could hold an unrelated request, an eviction, or
    // shutdown for as long as it liked. Acquisition lives behind one bounded
    // module; nothing else in production source may reach `fs4` directly.
    let owner = repo_root().join("crates/codestory-contracts/src/bounded_locks.rs");
    let mut files = Vec::new();
    collect_rs_files(&repo_root().join("crates"), &mut files);
    files.sort();
    let mut violations = Vec::new();
    let mut scanned = 0_usize;
    for path in files {
        // Everything a crate ships or builds with is in scope: `src`, build
        // scripts at the crate root, and modules they include. Only integration
        // test targets, which exist to build hostile fixtures, are exempt.
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
            || path == owner
        {
            continue;
        }
        scanned += 1;
        let source = fs::read_to_string(&path).expect("read Rust source");
        // Inline `#[cfg(test)]` items may still drive raw locks to build
        // fixtures; nothing a release build compiles may.
        if production_source(&source).contains("fs4::") {
            violations.push(path.display().to_string());
        }
    }
    assert!(
        violations.is_empty(),
        "advisory locks must be taken through codestory_contracts::bounded_locks:\n{}",
        violations.join("\n")
    );
    assert!(
        scanned > 100,
        "the fs4 gate scanned only {scanned} files; its file filter has stopped matching"
    );

    let bounded = read("crates/codestory-contracts/src/bounded_locks.rs");
    assert!(
        bounded.contains("pub fn acquire_with_deadline(")
            && bounded.contains("deadline: LockDeadline")
            && bounded.contains("cancel: Option<&AtomicBool>"),
        "the bounded entry point must take an absolute deadline and a cancellation flag"
    );
    assert!(
        bounded.contains("let cancel = cancel.or(inherited.as_deref());"),
        "a call site with no flag of its own must inherit the thread's cancellation"
    );
    for blocking in ["FileExt::lock_exclusive", "FileExt::lock_shared"] {
        assert!(
            !bounded.contains(blocking),
            "the bounded entry point must never call the uninterruptible {blocking}"
        );
    }
}

/// The helper above is the whole reason the fs4 and owned-artifact gates see
/// anything at all, so it is proven directly rather than trusted.
#[test]
fn the_production_source_helper_strips_every_test_gate_not_just_the_first() {
    let gate_in_the_import_block = "\
use std::fs::File;
#[cfg(test)]
use std::sync::Arc;

fn production() {
    real_call();
}

#[cfg(test)]
mod tests {
    fn fixture() {
        gated_call();
    }
}
";
    let production = production_source(gate_in_the_import_block);
    assert!(
        production.contains("real_call()"),
        "production code must survive the strip: {production}"
    );
    assert!(
        !production.contains("use std::sync::Arc;"),
        "a gated use statement must be stripped: {production}"
    );
    assert!(
        !production.contains("gated_call()"),
        "a gated module after an earlier gate must still be stripped: {production}"
    );

    let feature_gated = "\
#[cfg(any(test, feature = \"test-support\"))]
pub fn shipped_with_the_feature() {
    real_call();
}
";
    assert!(
        production_source(feature_gated).contains("real_call()"),
        "a feature-reachable gate compiles into a release build and is production code"
    );

    let multiline_gated_use = "\
#[cfg(test)]
use crate::{
    first, second,
};

fn production() {
    real_call();
}
";
    let production = production_source(multiline_gated_use);
    assert!(!production.contains("first, second"));
    assert!(production.contains("real_call()"));
}

/// Every lock a peer holds for a whole publication must be waited on with the
/// publication budget. `DEFAULT_LOCK_WAIT` is ten seconds — shorter than a
/// legitimate commit — so pointing one of these at it converts ordinary
/// contention into a hard failure for a reader that previously waited.
#[test]
fn publication_class_lock_waits_carry_the_publication_budget() {
    let publication_class = [
        // The atomic old-or-new promotion.
        (
            "crates/codestory-store/src/storage_impl/mod.rs",
            "fn acquire(path: &Path) -> Result<Self, StorageError> {",
        ),
        // The persisted search index publication.
        (
            "crates/codestory-runtime/src/search/engine.rs",
            "fn acquire_with_mode(search_dir: &Path, mode: PersistedSearchIndexLockMode)",
        ),
        // The search generation catalog write.
        (
            "crates/codestory-runtime/src/search_publication.rs",
            "acquire_with_deadline(",
        ),
        // Sidecar generation publication and retention, both directions.
        (
            "crates/codestory-retrieval/src/retention.rs",
            "pub fn acquire_with_cancel(",
        ),
        (
            "crates/codestory-retrieval/src/retention.rs",
            "pub fn acquire_shared_with_cancel(",
        ),
    ];
    for (path, marker) in publication_class {
        let source = read(path);
        let body = source_between(&source, marker, "\n    }\n");
        assert!(
            body.contains("PUBLICATION_LOCK_WAIT"),
            "{path} waits on a publication-class lock with a budget other than PUBLICATION_LOCK_WAIT:\n{body}"
        );
    }

    // A long budget is only safe while it stays interruptible, and the two
    // constants must not collapse into each other.
    let bounded = read("crates/codestory-contracts/src/bounded_locks.rs");
    assert!(
        bounded.contains("pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(10);")
            && bounded
                .contains("pub const PUBLICATION_LOCK_WAIT: Duration = Duration::from_secs(120);"),
        "the two wait classes must stay distinct and named"
    );
}

/// Bounded acquisition replaced a blocking `flock`; it must not also have
/// changed what callers report. A refusal keeps the surface each site already
/// had and carries the typed lock code in its message.
#[test]
fn bounded_acquisition_did_not_change_public_error_codes() {
    let publication = read("crates/codestory-runtime/src/search_publication.rs");
    assert!(
        !publication.contains("ApiError::new(\n                error.code(),")
            && !publication.contains("ApiError::new(\n            error.code(),")
            && !publication.contains("ApiError::new(\n        error.code(),"),
        "search publication lock refusals must keep reporting `internal`, not promote the lock code to a public API error code"
    );
    for context in [
        "Failed to acquire search generation catalog lock",
        "Failed to inspect persisted search generation lock",
        "Failed to lock persisted search generation",
    ] {
        assert!(
            publication.contains(context),
            "the refusal for {context:?} must survive"
        );
    }

    let storage = read("crates/codestory-store/src/storage_impl/mod.rs");
    assert!(
        storage.contains("StorageError::Other(format!(\n                \"Failed to acquire promotion lock for {} ({}): {error}\","),
        "the promotion refusal must stay a StorageError::Other carrying the typed lock code, matching the other sites"
    );
}

#[test]
fn evicting_a_context_never_detaches_an_unquiesced_activation_worker() {
    let services = read("crates/codestory-runtime/src/services.rs");
    assert!(
        services.contains("pub fn cancel_and_wait_within(&self, budget: Duration)")
            && services.contains("ActivationQuiescence::FailStopRequired")
            && services.contains("run_activation_fail_stop(ACTIVATION_QUIESCENCE_FAIL_STOP)"),
        "eviction must join with a deadline and fail-stop instead of detaching a worker that may hold a publication or store lock"
    );
    // The budget only bounds the worker if the worker's own lock waits can end
    // on cancellation. Past that budget the CLI hook aborts the process, so a
    // worker merely waiting behind a peer's publication would kill a healthy
    // session and blame quiescence for it. Scoped to the worker's body: another
    // caller installing a scope proves nothing about this thread.
    let worker_body = source_between(
        &services,
        "fn run_activation_worker(",
        "\nimpl ActivationOperation {",
    );
    assert!(
        worker_body.contains("bounded_locks::with_thread_cancellation(")
            && worker_body.contains("Arc::clone(&operation.cancelled)"),
        "run_activation_worker must install the operation's cancellation as the ambient one for every bounded lock wait the worker performs:\n{worker_body}"
    );
    assert!(
        services.contains("ACTIVATION_QUIESCENCE_BUDGET.as_millis()")
            && services.contains(
                "codestory_contracts::bounded_locks::MAX_CANCELLATION_LATENCY.as_millis()"
            ),
        "the quiescence budget must be asserted against the bounded-lock cancellation latency"
    );

    let diagnostics = read("crates/codestory-cli/src/diagnostics.rs");
    assert!(
        diagnostics.contains("set_activation_fail_stop_hook"),
        "the hosting binary must install the activation fail-stop evidence path"
    );
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

/// Production sources of the product crates, using the same test carve-out as
/// the owned-artifact contract: files under a `tests` directory and the
/// trailing inline test module pin identities on purpose.
fn production_sources() -> Vec<(String, String)> {
    let producer_crates = [
        "crates/codestory-workspace/src",
        "crates/codestory-store/src",
        "crates/codestory-indexer/src",
        "crates/codestory-retrieval/src",
        "crates/codestory-runtime/src",
        "crates/codestory-cli/src",
    ];
    let mut sources = Vec::new();
    for dir in producer_crates {
        let mut files = Vec::new();
        collect_rs_files(&repo_root().join(dir), &mut files);
        files.sort();
        for path in files {
            let relative = path
                .strip_prefix(repo_root())
                .expect("producer file lives in the repository")
                .to_string_lossy()
                .replace('\\', "/");
            // The generalization lint classifies this file as eval-only
            // production (its `evalOnlyProductionFiles` set) and bans product
            // paths from depending on it, so it cannot also be a production
            // surface here: the two boundaries must name the same thing.
            if relative == "crates/codestory-runtime/src/agent/eval_probes.rs"
                || relative.split('/').any(|part| part == "tests")
                || relative.ends_with("/tests.rs")
                || relative.ends_with("/test_support.rs")
            {
                continue;
            }
            let source = fs::read_to_string(&path).expect("read producer source");
            let production = match source.rfind("\n#[cfg(test)]\nmod tests {") {
                Some(index) => source[..index].to_string(),
                None => source,
            };
            sources.push((relative, production));
        }
    }
    sources
}

#[test]
fn environment_identities_are_declared_in_the_config_registry_with_one_owner() {
    // ARCH-021: every environment knob must exist in
    // codestory_contracts::config_registry, which is what generates the
    // configuration reference and records the module accountable for the
    // setting. A second file spelling the same identity is the ambient read
    // that let the same variable mean two things in two crates.
    let identity = production_environment_identities();
    let mut undeclared = Vec::new();
    let mut misowned = Vec::new();
    for (name, files) in &identity {
        let Some(setting) = codestory_contracts::config_registry::env_setting(name) else {
            let spelled_in = files.iter().map(String::as_str).collect::<Vec<_>>();
            undeclared.push(format!("{name} in {}", spelled_in.join(", ")));
            continue;
        };
        for file in files {
            if file != setting.owner {
                misowned.push(format!(
                    "{name} is spelled in {file} but the registry owner is {}",
                    setting.owner
                ));
            }
        }
    }
    assert!(
        undeclared.is_empty(),
        "declare these environment identities in codestory_contracts::config_registry:\n{}",
        undeclared.join("\n")
    );
    assert!(
        misowned.is_empty(),
        "import the identity from codestory_contracts::config_registry instead of spelling it:\n{}",
        misowned.join("\n")
    );
}

#[test]
fn config_registry_owners_name_real_production_files() {
    for setting in codestory_contracts::config_registry::ENV_SETTINGS {
        assert!(
            repo_root().join(setting.owner).is_file(),
            "{} names a missing owner {}",
            setting.name,
            setting.owner
        );
    }
}

/// `CODESTORY_*` identities spelled in production, mapped to the files that
/// spell them.
fn production_environment_identities() -> BTreeMap<String, BTreeSet<String>> {
    let mut identities: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (relative, source) in production_sources() {
        for name in quoted_codestory_identities(&source) {
            identities.entry(name).or_default().insert(relative.clone());
        }
    }
    identities
}

fn quoted_codestory_identities(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find("\"CODESTORY_") {
        let after_quote = &rest[start + 1..];
        match after_quote.find('"') {
            Some(end) => {
                let candidate = &after_quote[..end];
                if candidate
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
                {
                    names.push(candidate.to_string());
                }
                rest = &after_quote[end + 1..];
            }
            None => break,
        }
    }
    names
}

#[test]
fn owned_artifact_identities_are_declared_only_in_the_registry() {
    // Every file identity CodeStory writes beside its storage file lives in
    // codestory_contracts::owned_artifacts. A producer or the discovery
    // exclusion spelling one of these names directly recreates the split
    // naming that let owned artifacts leak into source discovery.
    let literals = [
        ".promotion.lock",
        ".promotion.prepared.json",
        ".promotion.committed.json",
        ".promotion.cleanup-blocked",
        "index-writer.lock",
        "sqlite.backup",
        "local-refresh-status.json",
        "local-refresh.lock",
        "local-refresh-state.guard",
        "annotations.sqlite3",
        "annotations.pre-migration.json",
        "embedded-models",
        ".materialize.lock",
    ];
    let producer_crates = [
        "crates/codestory-workspace/src",
        "crates/codestory-store/src",
        "crates/codestory-runtime/src",
        "crates/codestory-cli/src",
        "crates/codestory-retrieval/src",
        "crates/codestory-indexer/src",
        "crates/codestory-llama-sys/src",
    ];
    for dir in producer_crates {
        let mut files = Vec::new();
        collect_rs_files(&repo_root().join(dir), &mut files);
        files.sort();
        for path in files {
            if path.components().any(|part| part.as_os_str() == "tests") {
                continue;
            }
            let source = fs::read_to_string(&path).expect("read producer source");
            // Inline test modules pin these names on purpose; nothing a release
            // build compiles may spell them.
            let production = production_source(&source);
            for literal in literals {
                assert!(
                    !production.contains(literal),
                    "{} spells owned artifact identity {literal:?}; derive it from codestory_contracts::owned_artifacts instead",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn packet_claim_profile_contracts_are_enforced_at_runtime_not_only_in_debug_builds() {
    // ARCH-005 shipped the claim-profile contract behind `debug_assert!`, so a release
    // binary answered from a profile whose calibration no longer held. Validation has to
    // run in the shipped build and skip the profile it rejects.
    let profiles = read("crates/codestory-runtime/src/agent/packet_claim_profiles.rs");
    let production = production_source_prefix(&profiles);
    assert!(
        !production.contains("debug_assert"),
        "claim-profile contract validation must not be compiled out of release builds"
    );
    for required in [
        "-> Result<(), SourceClaimProfileContractViolation>",
        "enum SourceClaimProfileContractViolation",
        "telemetry.record_profile_skipped(profile_id, violation.code());",
        "const PACKET_CLAIM_PROFILE_PENDING_MIGRATION_RATCHET: usize",
    ] {
        assert!(
            production.contains(required),
            "packet claim-profile registry must keep runtime validate-or-skip: missing {required}"
        );
    }

    let telemetry = read("crates/codestory-runtime/src/agent/packet_profile_telemetry.rs");
    assert!(
        production_source_prefix(&telemetry).contains("PACKET_CLAIM_PROFILE_CONTRACT_VERSION"),
        "packet claim-profile telemetry must publish a contract version"
    );
}

#[test]
fn packet_profile_telemetry_travels_on_a_typed_field_not_the_evidence_annotation_channel() {
    // `retrieval_trace.annotations` is an evidence channel: `codestory-cli`'s
    // `is_gap_annotation` substring-matches it for gap markers and downgrades packet
    // confidence when one hits. Always-on telemetry published there matched on "skipped" and
    // moved every packet from high/ready to medium/review. The counters must therefore be
    // structurally separated from evidence text, not merely worded around the heuristic.
    let telemetry = read("crates/codestory-runtime/src/agent/packet_profile_telemetry.rs");
    let telemetry_production = production_source_prefix(&telemetry);
    assert!(
        telemetry_production.contains("-> PacketClaimProfileTelemetryDto"),
        "claim-profile telemetry must be published as a typed DTO"
    );
    assert!(
        !telemetry_production.contains("fn trace_annotations"),
        "claim-profile telemetry must not render itself as trace annotations"
    );

    let orchestrator = read("crates/codestory-runtime/src/agent/orchestrator.rs");
    let orchestrator_production = production_source_prefix(&orchestrator);
    for required in [
        "answer.retrieval_trace.packet_claim_profile_telemetry =",
        "claim_telemetry.to_dto(",
    ] {
        assert!(
            orchestrator_production.contains(required),
            "packet assembly must attach claim-profile telemetry to the typed trace field: missing {required}"
        );
    }
    for forbidden in [
        ".extend(claim_telemetry",
        "claim_telemetry.trace_annotations(",
        "annotations.push(claim_telemetry",
    ] {
        assert!(
            !orchestrator_production.contains(forbidden),
            "claim-profile telemetry must not be appended to the evidence annotation channel: {forbidden}"
        );
    }
}
