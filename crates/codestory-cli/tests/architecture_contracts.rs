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

/// Returns source files reached from a column-zero `#[cfg(test)]` module
/// declaration, including ungated declarations nested below that test-only
/// module. A directory module brings along all of its Rust files: they are
/// reachable only through that module boundary.
fn test_gated_module_files(source_root: &Path) -> BTreeSet<PathBuf> {
    let mut files = Vec::new();
    collect_rs_files(source_root, &mut files);
    files.sort();

    let mut test_gated = BTreeSet::new();
    let mut pending = Vec::new();
    for path in &files {
        let source = fs::read_to_string(path).expect("read Rust source");
        for name in test_gated_module_declarations(&source) {
            add_module_files(source_root, path, &name, &mut test_gated, &mut pending);
        }
    }

    while let Some(path) = pending.pop() {
        let source = fs::read_to_string(&path).expect("read test-gated Rust source");
        for name in module_declarations(&source) {
            add_module_files(source_root, &path, &name, &mut test_gated, &mut pending);
        }
    }

    test_gated
}

fn test_gated_module_declarations(source: &str) -> Vec<String> {
    let lines = source.lines().collect::<Vec<_>>();
    lines
        .windows(2)
        .filter_map(|lines| {
            (lines[0] == "#[cfg(test)]")
                .then(|| module_declaration(lines[1]))
                .flatten()
        })
        .collect()
}

fn module_declarations(source: &str) -> Vec<String> {
    source.lines().filter_map(module_declaration).collect()
}

fn module_declaration(line: &str) -> Option<String> {
    let name = line.strip_prefix("mod ")?.strip_suffix(';')?;
    (!name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_'))
    .then(|| name.to_owned())
}

fn add_module_files(
    source_root: &Path,
    parent_module: &Path,
    name: &str,
    test_gated: &mut BTreeSet<PathBuf>,
    pending: &mut Vec<PathBuf>,
) {
    let module_dir = module_directory(parent_module);
    let file_module = module_dir.join(format!("{name}.rs"));
    if file_module.is_file() {
        add_test_gated_file(source_root, file_module, test_gated, pending);
    }

    let directory = module_dir.join(name);
    let directory_module = directory.join("mod.rs");
    if directory_module.is_file() {
        let mut files = Vec::new();
        collect_rs_files(&directory, &mut files);
        for file in files {
            add_test_gated_file(source_root, file, test_gated, pending);
        }
    }
}

fn module_directory(module_file: &Path) -> PathBuf {
    let parent = module_file.parent().expect("module file has a parent");
    match module_file.file_name().and_then(|name| name.to_str()) {
        Some("lib.rs" | "main.rs" | "mod.rs") => parent.to_path_buf(),
        Some(_) => parent.join(module_file.file_stem().expect("module file has a stem")),
        None => panic!("module file has a name"),
    }
}

fn add_test_gated_file(
    source_root: &Path,
    path: PathBuf,
    test_gated: &mut BTreeSet<PathBuf>,
    pending: &mut Vec<PathBuf>,
) {
    let relative = path
        .strip_prefix(source_root)
        .expect("module file lives below source root")
        .to_path_buf();
    if test_gated.insert(relative) {
        pending.push(path);
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

fn read_source_tree_excluding_many(dir: &str, excluded_suffixes: &[&str]) -> String {
    let root = repo_root().join(dir);
    let mut files = Vec::new();
    collect_rs_files(&root, &mut files);
    files.sort();
    files
        .into_iter()
        .filter(|path| {
            let relative = path
                .strip_prefix(&root)
                .expect("source file stays below source root")
                .to_string_lossy();
            !excluded_suffixes
                .iter()
                .any(|suffix| relative.ends_with(suffix))
        })
        .map(|path| fs::read_to_string(path).expect("read source"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn production_source_prefix(source: &str) -> &str {
    source
        .split_once("#[cfg(test)]\nmod tests {")
        .map_or(source, |(production, _)| production)
}

fn production_source_contains_git_spawn(source: &str) -> bool {
    production_source(source).contains("Command::new(\"git\")")
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
            let trimmed = line.trim_end();
            if !opened_block && (trimmed.ends_with('}') || trimmed.ends_with("};")) {
                return;
            }
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

fn redact_braced_rust_item(source: &mut String, header: &str) {
    let masked = mask_comments_and_strings(source);
    let start = masked.find(header).expect("item header exists");
    let open = start + masked[start..].find('{').expect("item body starts");
    let mut depth = 0_usize;
    let mut end = None;
    for (offset, byte) in masked.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + offset + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.expect("item body closes");
    source.replace_range(start..end, &" ".repeat(end - start));
}

#[test]
fn cli_sidecar_construction_stays_behind_test_safe_gateway() {
    let source_root = repo_root().join("crates/codestory-cli/src");
    let gateway_path = source_root.join("sidecar_runtime.rs");
    let qualification_provider = source_root.join("embedding_qualification/worker.rs");
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
            if path == qualification_provider && needle == "SidecarRuntimeConfig::for_project_" {
                continue;
            }
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
fn ordinary_cli_retrieval_operations_route_through_runtime() {
    let source_root = repo_root().join("crates/codestory-cli/src");
    let mut files = Vec::new();
    collect_rs_files(&source_root, &mut files);
    let forbidden = [
        "codestory_retrieval::activate_retained_rollback_generation(",
        "codestory_retrieval::observe_retained_rollback_generation(",
        "codestory_retrieval::sidecar_inventory_with_storage(",
        "codestory_retrieval::sidecar_gc_apply_with_storage(",
        "codestory_retrieval::cache_inventory(",
        "codestory_retrieval::execute_retrieval_query_with_cache_for_runtime(",
        "codestory_retrieval::finalize_index_for_runtime_with_cancel(",
    ];
    let mut violations = Vec::new();
    let mut scanned = 0_usize;
    for path in files {
        let source = fs::read_to_string(&path).expect("read CLI source");
        let source = production_source(&source);
        scanned += 1;
        for needle in forbidden {
            if source.contains(needle) {
                violations.push(format!("{}: {needle}", path.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "ordinary CLI retrieval operations must route through codestory_runtime:\n{}",
        violations.join("\n")
    );
    assert!(
        scanned > 20,
        "CLI retrieval-operation gate scanned only {scanned} files"
    );
}

/// The CLI's MCP and HTTP transports are adapters (ARCH-017).
///
/// Transport status, activation, and engine-health answers belong to
/// `codestory_runtime::ActivationService`; a transport that probes
/// `codestory_retrieval` itself has taken ownership of retrieval policy.
///
/// The scan matches the crate name rather than the `codestory_retrieval::`
/// path prefix, so importing the crate and calling it unqualified is caught
/// too. `#[cfg(test)]` items are stripped by `production_source`: only what a
/// release build compiles can violate the transport boundary, and the
/// non-vacuity block below proves the stripping did not empty the scan.
#[test]
fn cli_transport_adapters_do_not_probe_retrieval_directly() {
    let source_root = repo_root().join("crates/codestory-cli/src");
    let mut files = Vec::new();
    collect_rs_files(&source_root, &mut files);
    files.sort();
    // Membership is derived, not listed, so a new transport module or a
    // `stdio_transport/` submodule directory is covered the day it lands.
    let adapters: Vec<PathBuf> = files
        .into_iter()
        .filter(|path| {
            path.components().any(|component| {
                component.as_os_str().to_str().is_some_and(|name| {
                    name.starts_with("stdio_") || name.starts_with("http_transport")
                })
            })
        })
        .collect();
    let adapter_names: BTreeSet<String> = adapters
        .iter()
        .filter_map(|path| path.strip_prefix(&source_root).ok())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect();
    for required in [
        "stdio_transport.rs",
        "stdio_arguments.rs",
        "stdio_catalog.rs",
        "http_transport.rs",
    ] {
        assert!(
            adapter_names.contains(required),
            "the transport-boundary scan lost `{required}`; it found {adapter_names:?}"
        );
    }

    let mut violations = Vec::new();
    for path in &adapters {
        let source = fs::read_to_string(path).expect("read CLI transport source");
        for line in production_source(&source).lines() {
            if line.contains("codestory_retrieval") {
                violations.push(format!("{}: {}", path.display(), line.trim()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "CLI transports must reach retrieval through codestory_runtime::ActivationService:\n{}",
        violations.join("\n")
    );

    // Non-vacuity plus the positive direction: the stdio transport still holds
    // the four moved call sites, so an empty violation list means the transport
    // asks the service rather than that the scan found nothing to read.
    let stdio_production = production_source(&read("crates/codestory-cli/src/stdio_transport.rs"));
    for required in [
        "retrieval_engine_diagnostics(",
        "active_embedding_backend_id()",
        "retrieval_contract_version()",
        "host_process_start_identity()",
    ] {
        assert!(
            stdio_production.contains(required),
            "stdio transport must still ask the activation service for `{required}`"
        );
    }
}

/// The CLI routes retrieval work through `codestory_runtime`; only these
/// providers implement retrieval-owned traits or supply qualification-proof
/// operations. Routing providers through `ActivationService` would invert that
/// dependency.
#[test]
fn cli_owns_no_direct_retrieval_consumers_outside_providers() {
    let source_root = repo_root().join("crates/codestory-cli/src");
    let provider_allowlist = BTreeSet::from([
        "embedding_server_transport.rs",
        "embedding_qualification/worker.rs",
        "embedding_qualification/worker/gate.rs",
        "embedding_qualification/worker/protocol.rs",
        "embedding_qualification/worker/operations/absence.rs",
        "embedding_qualification/worker/operations/activation.rs",
        "embedding_qualification/worker/operations/dead_client.rs",
        "embedding_qualification/worker/operations/measure.rs",
        "embedding_qualification/worker/operations/owner_exit.rs",
        "embedding_qualification/worker/operations/queue.rs",
        "sidecar_runtime.rs",
    ]);
    let test_gated_modules = test_gated_module_files(&source_root);

    let transport = production_source(&read(
        "crates/codestory-cli/src/embedding_server_transport.rs",
    ));
    for provider_trait in [
        "impl codestory_retrieval::AwakeMonotonicClock for",
        "impl codestory_retrieval::EmbeddingServerStream for",
        "impl codestory_retrieval::EmbeddingClientTransport for",
        "impl codestory_retrieval::EmbeddingServerTransport for",
        "impl codestory_retrieval::EmbeddingServerListener for",
    ] {
        assert!(
            transport.contains(provider_trait),
            "embedding_server_transport.rs must retain its retrieval provider trait impl: {provider_trait}"
        );
    }
    let qualification_worker = production_source(&read(
        "crates/codestory-cli/src/embedding_qualification/worker.rs",
    ));
    assert!(
        qualification_worker.contains("codestory_retrieval::run_per_user_embedding_qualification("),
        "embedding qualification provider must retain run_per_user_embedding_qualification"
    );

    let mut files = Vec::new();
    collect_rs_files(&source_root, &mut files);
    files.sort();
    let mut scanned = 0_usize;
    let mut violations = Vec::new();
    let mut provider_references = BTreeSet::new();
    for path in files {
        let relative = path
            .strip_prefix(&source_root)
            .expect("CLI source lives below source root")
            .to_string_lossy()
            .replace('\\', "/");
        if test_gated_modules.contains(Path::new(&relative)) {
            continue;
        }

        scanned += 1;
        let source = production_source(&fs::read_to_string(&path).expect("read CLI source"));
        let has_retrieval_reference = source.contains("codestory_retrieval");
        if provider_allowlist.contains(relative.as_str()) {
            if has_retrieval_reference {
                provider_references.insert(relative);
            }
            continue;
        }
        for line in source
            .lines()
            .filter(|line| line.contains("codestory_retrieval"))
        {
            violations.push(format!("{}: {}", path.display(), line.trim()));
        }
    }

    assert!(
        scanned > 20,
        "CLI retrieval-consumer scan must cover more than 20 release-compiled files; scanned {scanned}"
    );
    assert!(
        violations.is_empty(),
        "CLI retrieval consumers must route through codestory_runtime outside enumerated providers:\n{}",
        violations.join("\n")
    );
    for provider in &provider_allowlist {
        assert!(
            provider_references.contains(*provider),
            "provider allowlist entry {provider} has no release-compiled codestory_retrieval reference; remove the stale allowlist entry"
        );
    }
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

/// The moved planning modules `codestory-agent` owns: the allowlist the
/// location, count, and import-DAG contracts below all read.
///
/// Everything under `crates/codestory-agent/src` must appear here except the
/// named exclusions in [`AGENT_MODULE_ALLOWLIST_EXCLUSIONS`];
/// `agent_module_allowlist_stays_in_sync_with_the_agent_source_tree` enforces
/// that, so adding a module to the crate without extending this list fails
/// loudly instead of silently escaping every contract built on it.
const AGENT_PLANNING_MODULES: [&str; 17] = [
    "citation.rs",
    "packet_citations.rs",
    "packet_command.rs",
    "packet_coverage.rs",
    "packet_degradation.rs",
    "packet_evidence.rs",
    "packet_execution_graphs.rs",
    "packet_freshness.rs",
    "packet_plan.rs",
    "packet_probes.rs",
    "packet_scoring.rs",
    "packet_terms.rs",
    "pinned_reader.rs",
    "planning.rs",
    "profiles.rs",
    "text.rs",
    "trail.rs",
];

/// Agent-crate source files that are deliberately not planning modules.
///
/// - `lib.rs` is the crate root: module wiring and re-exports, not a planning
///   surface of its own.
/// - `eval_probes.rs` is the eval-only production file: `lib.rs` gates it
///   behind `cfg(any(test, feature = "test-support"))`, the generalization
///   lint classifies it as eval-only, and
///   `agent_eval_hooks_stay_on_for_runtime_tests_and_off_for_product_builds`
///   pins how it compiles. Listing it as a planning module would hand the
///   import-DAG guard a file no product build links.
const AGENT_MODULE_ALLOWLIST_EXCLUSIONS: [&str; 2] = ["lib.rs", "eval_probes.rs"];

#[test]
fn horizon_a_exposes_only_prompt_blind_seed_planning_and_admission() {
    let agent_lib = read("crates/codestory-agent/src/lib.rs");
    assert!(
        !agent_lib.contains("pub mod evidence_compiler;"),
        "the Horizon B compiler must not enter the Horizon A product graph"
    );
    let contracts = read("crates/codestory-contracts/src/compilation.rs");
    for required in [
        "PacketCandidateDescriptorV1",
        "PacketAdmissionReceiptV1",
        "PacketContinuationSelectorV1",
    ] {
        assert!(
            contracts.contains(required),
            "the typed Horizon A admission boundary lost {required}"
        );
    }
    for deferred in ["RetrievalSeedPlanV1", "PacketCompilationInputV1"] {
        assert!(
            !contracts.contains(deferred),
            "Horizon B contract {deferred} entered Horizon A"
        );
    }
    let current_dto = read("crates/codestory-contracts/src/api/dto.rs");
    let packet_request = source_between(
        &current_dto,
        "pub struct AgentPacketRequestDto",
        "pub struct PacketBudgetLimitsDto",
    );
    assert!(!packet_request.contains("include_evidence"));
    assert!(!packet_request.contains("task_class"));
}

#[test]
fn packet_v3_record_projection_and_public_facade_are_product_wired() {
    let modules = read("crates/codestory-runtime/src/agent/mod.rs");
    assert!(modules.contains("pub(crate) mod packet_execution_record_v3;"));
    assert!(modules.contains("pub(crate) mod packet_projection_v3;"));
    let runtime = read("crates/codestory-runtime/src/lib.rs");
    assert!(runtime.contains("mod evidence_projection_v3;"));
    assert!(runtime.contains("pub use evidence_projection_v3::"));
    for surface in [
        "crates/codestory-cli/src/stdio_transport.rs",
        "crates/codestory-cli/src/http_transport.rs",
        "crates/codestory-cli/src/app/search_command.rs",
        "crates/codestory-cli/src/app/agent_context/packet.rs",
        "crates/codestory-cli/src/app/agent_context/context.rs",
    ] {
        let source = read(surface);
        assert!(
            source.contains("project_") && source.contains("_v3"),
            "{surface} must project through the public evidence-only v3 facade"
        );
    }
}

#[test]
fn public_exact_verifier_uses_the_revision_native_transport_once() {
    let cli_lib = read("crates/codestory-cli/src/lib.rs");
    assert!(
        cli_lib.contains("mod stdio_v3;"),
        "the revision-native evidence transport must compile in the public product graph"
    );

    let facade = read_source_tree("crates/codestory-cli/src/stdio_v3");
    for required in [
        "measure_revision_native_proof_result_v3",
        "RevisionNativeToolResultMeasurementV3",
        "StdioV3InternalError",
    ] {
        assert!(
            facade.contains(required),
            "the stdio v3 facade lost its verifier transport seam via {required}"
        );
    }

    let production_cli = read_source_tree_excluding_many(
        "crates/codestory-cli/src",
        &[
            "lib.rs",
            "stdio_v3/catalog.rs",
            "stdio_v3/mod.rs",
            "stdio_v3/profile.rs",
            "stdio_v3/transport.rs",
            "stdio_v3/diagnostics.rs",
            "stdio_v3/discovery.rs",
        ],
    );
    for forbidden in [
        "measure_revision_native_proof_result_v3",
        "RevisionNativeToolResultMeasurementV3",
    ] {
        assert!(
            !production_cli.contains(forbidden),
            "a production CLI module references the transport measurement seam via {forbidden}"
        );
    }

    let args = read("crates/codestory-cli/src/args.rs");
    assert_eq!(
        args.matches("VerifyIndexedDirectCalls(VerifyIndexedDirectCallsCommand)")
            .count(),
        1,
        "the public CLI verifier command must be registered exactly once"
    );
    let catalog = read("crates/codestory-cli/src/stdio_v3/catalog.rs");
    assert_eq!(
        catalog
            .matches("sources.push(proof_tool_source_v3());")
            .count(),
        1,
        "the exact verifier must enter every revision catalog through one owning registration"
    );
    let stdio_catalog = read("crates/codestory-cli/src/stdio_catalog.rs");
    let legacy_catalog = source_between(&stdio_catalog, "static TOOLS:", "static RESOURCES:");
    assert!(
        !legacy_catalog.contains("prove_call_path"),
        "the legacy Supported catalog must not reach the v3 verifier response"
    );

    let launcher = read("plugins/codestory/scripts/codestory-mcp.cjs");
    let live_launcher = source_between(
        &launcher,
        "async function main()",
        "function runLauncherError",
    );
    assert!(
        !live_launcher.contains("darkV3"),
        "the live launcher route must not select the dark v3 handoff machinery"
    );

    let diagnostics = production_source(&read("crates/codestory-cli/src/stdio_v3/diagnostics.rs"));
    for forbidden in [
        "std::fs::",
        "codestory_runtime::",
        "ActivationService",
        "active_publication(",
        "status(",
        "source_text",
        "render(",
    ] {
        assert!(
            !diagnostics.contains(forbidden),
            "diagnostic capability reads must serve immutable registry bytes without live work via {forbidden}"
        );
    }
}

#[test]
fn public_exact_verifier_compiles_through_the_sealed_proof_facades() {
    const SUPPORT_FEATURE: &str = "proof-qualification-support";
    let agent_manifest = manifest("crates/codestory-agent/Cargo.toml");
    let agent_features = agent_manifest
        .get("features")
        .and_then(Value::as_table)
        .expect("agent features");
    assert!(
        !agent_features
            .get("default")
            .is_some_and(|default| default.to_string().contains(SUPPORT_FEATURE)),
        "the proof kernel must not compile in the default agent crate"
    );
    let runtime_manifest = manifest("crates/codestory-runtime/Cargo.toml");
    let runtime_features = runtime_manifest
        .get("features")
        .and_then(Value::as_table)
        .expect("runtime features");
    assert!(
        runtime_features
            .get("default")
            .is_some_and(|default| default.to_string().contains(SUPPORT_FEATURE)),
        "crates/codestory-runtime/Cargo.toml must compile the sealed proof facade for the public verifier"
    );
    let runtime_lib = read("crates/codestory-runtime/src/lib.rs");
    assert!(runtime_lib.contains("pub mod proof_qualification_support;"));
    assert!(runtime_lib.contains("mod call_path_kernel;"));
    let cli = read_source_tree("crates/codestory-cli/src");
    assert!(cli.contains("run_observed_call_path_public_operation"));
    assert!(cli.contains("run_translation_unknown_public_operation"));
    assert!(!read("crates/codestory-cli/src/http_transport.rs").contains("prove_call_path"));

    let launcher = read("plugins/codestory/scripts/codestory-mcp.cjs");
    let launcher_revisions = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
    for revision in launcher_revisions {
        assert!(
            launcher.contains(revision),
            "the inert launcher discovery session lost Rust's {revision} revision"
        );
    }
    assert!(
        launcher.contains("publicationSchemaVersion: 3"),
        "the inert launcher discovery session must preserve the Rust discovery schema"
    );
}

#[cfg(feature = "proof-qualification-support")]
#[test]
fn sealed_discovery_contracts_drive_the_inert_launcher_session() {
    use std::process::Command;

    let contracts = codestory_cli::proof_qualification_support::discovery_contracts();
    let launcher = repo_root().join("plugins/codestory/scripts/codestory-mcp.cjs");
    let script = r#"
const launcher = require(process.argv[1]);
const contracts = JSON.parse(process.argv[2]);
process.stdout.write(JSON.stringify(
  launcher._test.v3LauncherSession('2025-06-18', contracts),
));
"#;
    let output = Command::new("node")
        .args([
            "-e",
            script,
            &launcher.display().to_string(),
            &serde_json::to_string(&contracts).expect("serialize Rust discovery contracts"),
        ])
        .output()
        .expect("run the inert launcher session");
    assert!(
        output.status.success(),
        "launcher session failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let session: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("launcher session JSON");
    assert_eq!(session["negotiated"], "2025-06-18");
    assert_eq!(
        session["discoveryContractSha256"], contracts["2025-06-18"],
        "the launcher must retain Rust's discovery digest without substituting one"
    );
    assert_eq!(session["publicationSchemaVersion"], 3);
}

#[test]
fn proof_qualification_facades_seal_the_kernel_and_preserve_transport_errors() {
    let runtime_lib = read("crates/codestory-runtime/src/lib.rs");
    assert!(
        runtime_lib.contains("mod call_path_kernel;")
            && !runtime_lib.contains("pub mod call_path_kernel;"),
        "proof qualification must not make the dark runtime kernel directly reachable"
    );
    let facade = read("crates/codestory-runtime/src/proof_qualification_support.rs");
    for required in [
        "AdmittedRawCallEdge",
        "BuiltCallPathFacts",
        "ValidatedCallPathContract",
        "check_built_call_path_integration",
        "project_internal_call_path_result",
    ] {
        assert!(
            facade.contains(required),
            "the sealed runtime facade must name the required {required} API explicitly"
        );
    }

    let cli_lib = read("crates/codestory-cli/src/lib.rs");
    assert!(
        !cli_lib.contains("Result<Vec<RevisionNativeToolResultMeasurement>, String>"),
        "the CLI qualification facade must not erase transport failures into String"
    );
    for required in [
        "pub enum ProofQualificationTransportError",
        "Serialization(String)",
        "InvalidProjection(String)",
        "OutputSchemaViolation",
        "ResultExceedsBudget {",
        "maximum_bytes: usize",
        "actual_bytes: usize",
        "impl From<crate::stdio_v3::StdioV3InternalError>",
    ] {
        assert!(
            cli_lib.contains(required),
            "the CLI qualification facade must preserve {required}"
        );
    }
}

#[test]
fn runtime_test_support_never_reaches_the_private_agent_kernel() {
    for surface in [
        "crates/codestory-runtime/src/indexed_source_call_path_v1.rs",
        "crates/codestory-runtime/src/services.rs",
        "crates/codestory-runtime/src/proof_qualification_support.rs",
    ] {
        let source = read(surface);
        assert!(
            !source.contains("codestory_agent::indexed_source_call_path_v1")
                && !source.contains("codestory_agent::proof_qualification"),
            "{surface} reaches the removed agent proof kernel"
        );
    }
    let agent_lib = read("crates/codestory-agent/src/lib.rs");
    assert!(
        !agent_lib.contains("mod indexed_source_call_path_v1;")
            && !agent_lib.contains("proof_qualification_support"),
        "the default agent crate must not host the proof kernel"
    );
}

#[test]
fn dark_call_path_kernel_stays_on_the_test_support_side_of_the_crate_root() {
    let runtime_lib = read("crates/codestory-runtime/src/lib.rs");
    assert!(
        runtime_lib.contains(
            "#[cfg(any(\n    test,\n    feature = \"test-support\",\n    feature = \"proof-qualification-support\"\n))]\nmod call_path_kernel;"
        ),
        "the dark call-path kernel must remain private behind test or sealed qualification support"
    );
    assert!(
        !runtime_lib.contains("pub mod call_path_kernel;"),
        "the dark call-path kernel must never become a qualification-visible module"
    );

    let runtime_lib = read("crates/codestory-runtime/src/lib.rs");
    assert!(
        runtime_lib.contains(
            "#[cfg(any(\n    test,\n    feature = \"test-support\",\n    feature = \"proof-qualification-support\"\n))]\nmod indexed_source_call_path_v1;"
        ),
        "the dark Store/source adapter must remain behind test or sealed qualification support"
    );
    let adapter = production_source(&read(
        "crates/codestory-runtime/src/indexed_source_call_path_v1.rs",
    ));
    for forbidden in [
        "GraphEdgeDto",
        "with_effective_endpoints",
        "Occurrence",
        "source_text",
        "codestory_retrieval",
        "nucleo",
        "ToolSpec",
        "pub fn ",
        "pub use ",
    ] {
        assert!(
            !adapter.contains(forbidden),
            "dark raw-edge adapter crossed a forbidden boundary via {forbidden}"
        );
    }
}

#[test]
fn call_path_shared_types_live_in_contracts() {
    let contracts = read("crates/codestory-contracts/src/call_path.rs");
    for item in [
        "pub struct UnvalidatedCallPathContract",
        "pub struct ClauseAnchor",
        "pub enum ClauseClassification",
        "pub enum ProofContractField",
        "pub struct UnvalidatedCallPathSpec",
        "pub struct ValidatedCallPathContract",
        "pub struct ProofHashes",
        "pub enum InternalProjection",
    ] {
        assert!(
            contracts.contains(item),
            "call-path shared type {item} must live in codestory-contracts"
        );
    }

    let kernel = read("crates/codestory-runtime/src/call_path_kernel.rs");
    assert!(
        kernel.contains("pub use codestory_contracts::call_path::{"),
        "the kernel must re-export shared call-path types from contracts"
    );
    for item in [
        "pub struct UnvalidatedCallPathContract",
        "pub struct ValidatedCallPathContract",
        "pub struct ProofHashes",
        "pub enum InternalProjection",
    ] {
        assert!(
            !kernel.contains(item),
            "shared call-path type {item} must not be redefined in runtime"
        );
    }
}

#[test]
fn dark_call_path_raw_source_text_stays_out_of_the_proof_boundary() {
    let module = production_source(&read("crates/codestory-runtime/src/call_path_kernel.rs"));
    let mut outside_allowed_regions = module.clone();
    for item in [
        "fn validate_contract_with_domain",
        "fn validate_and_normalize_clauses",
        "fn classify_translation_gaps",
        "fn compute_hashes",
    ] {
        redact_braced_rust_item(&mut outside_allowed_regions, item);
    }
    assert!(
        !contains_word(&outside_allowed_regions, "source_text"),
        "raw source_text must stay confined to unvalidated input, clause validation, translation diagnostics, and hashing; it must not enter validated specs/contracts, verified facts, selector matching, path search/checking, ranking, search, or source matching:\n{outside_allowed_regions}"
    );
}

#[test]
fn exact_resolution_facts_are_a_one_way_proof_overlay() {
    for proof_module in [
        "crates/codestory-runtime/src/call_path_kernel.rs",
        "crates/codestory-runtime/src/indexed_source_call_path_v1.rs",
    ] {
        let source = read(proof_module);
        for forbidden in ["ResolutionCertainty", ".certainty", ".confidence"] {
            assert!(
                !source.contains(forbidden),
                "{proof_module} reads navigation-only diagnostic evidence via {forbidden}"
            );
        }
    }

    for (consumer, source) in [
        (
            "retrieval",
            // Fixture helpers rebind inherited store overlays when replacing a
            // core; that is not a product retrieval consumer of proof facts.
            read_source_tree_excluding_many("crates/codestory-retrieval/src", &["test_support.rs"]),
        ),
        (
            "packet planner",
            read_source_tree("crates/codestory-agent/src"),
        ),
        (
            "search",
            read_source_tree("crates/codestory-runtime/src/search"),
        ),
        (
            "packet runtime",
            read_source_tree("crates/codestory-runtime/src/agent"),
        ),
        (
            "context",
            read("crates/codestory-cli/src/app/agent_context/context.rs"),
        ),
        (
            "runtime navigation and graph consumers",
            read_source_tree_excluding_many(
                "crates/codestory-runtime/src",
                &[
                    "index_commit.rs",
                    "index_full.rs",
                    "index_incremental.rs",
                    "indexed_source_call_path_v1.rs",
                    "call_path_kernel.rs",
                    "proof_qualification_support.rs",
                    "semantic_republish.rs",
                    "tests.rs",
                    "v3_evidence_qualification_support.rs",
                ],
            ),
        ),
        (
            "store trail navigation",
            read("crates/codestory-store/src/storage_impl/trail.rs"),
        ),
        (
            "CLI navigation adapters",
            read_source_tree_excluding_many("crates/codestory-cli/src", &["stdio_v3/transport.rs"]),
        ),
        (
            "production proof transport",
            production_source_prefix(&read("crates/codestory-cli/src/stdio_v3/transport.rs"))
                .to_owned(),
        ),
    ] {
        for forbidden in [
            "proof_resolution",
            "proof_resolution_fact",
            "CallResolutionFact",
            "ProofResolutionStatus",
            "ResolutionEvidence",
            "get_exact_proof_resolution_fact_by_edge",
            "get_proof_resolution_facts",
            "get_proof_resolution_publication",
            "validate_proof_resolution_publication",
            "replace_proof_resolution_projection",
            "rebind_proof_resolution_publication",
        ] {
            assert!(
                !source.contains(forbidden),
                "{consumer} crossed the one-way proof overlay via {forbidden}"
            );
        }
    }

    for surface in [
        read_source_tree("crates/codestory-cli/src"),
        read("plugins/codestory/generated-mcp-catalog.json"),
        read("crates/codestory-contracts/src/api.rs"),
    ] {
        for forbidden in ["CallResolutionFact", "proof_resolution_fact"] {
            assert!(
                !surface.contains(forbidden),
                "the private exact-resolution overlay reached a public command, route, DTO, serializer, or catalog via {forbidden}"
            );
        }
    }
}

#[test]
fn public_call_path_release_surfaces_are_unique_and_legacy_unreachable() {
    let args = read("crates/codestory-cli/src/args.rs");
    assert_eq!(
        args.matches("VerifyIndexedDirectCalls(VerifyIndexedDirectCallsCommand)")
            .count(),
        1
    );

    let catalog = read("crates/codestory-cli/src/stdio_v3/catalog.rs");
    assert_eq!(
        catalog
            .matches("sources.push(proof_tool_source_v3());")
            .count(),
        1
    );

    let dispatcher = read("crates/codestory-cli/src/stdio_transport.rs");
    let stdio_catalog = read("crates/codestory-cli/src/stdio_catalog.rs");
    let legacy_tools = source_between(&stdio_catalog, "static TOOLS:", "static RESOURCES:");
    assert!(!legacy_tools.contains("prove_call_path"));
    assert!(!legacy_tools.contains("verify_indexed_direct_calls"));
    let public_proof = source_between(
        &dispatcher,
        "if crate::prove_call_path::is_proof_tool_name(name)",
        "// Public-operation retry belongs",
    );
    assert_eq!(
        public_proof.matches("build_proof_tool_result_v3").count(),
        1
    );
    assert!(!public_proof.contains("Supported"));

    for consumer in [
        "crates/codestory-runtime/Cargo.toml",
        "crates/codestory-cli/Cargo.toml",
        "crates/codestory-bench/Cargo.toml",
    ] {
        let consumer_manifest = manifest(consumer);
        let features = consumer_manifest
            .get("dependencies")
            .and_then(|table| table.get("codestory-agent"))
            .and_then(|entry| entry.get("features"))
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(
            !features.contains(&"test-support"),
            "{consumer} must not ship codestory-agent/test-support"
        );
    }
}

/// Packet planning lives in `codestory-agent`, and the crate DAG is what keeps
/// it from growing the powers it was extracted away from.
///
/// The dependency half is the load-bearing one: a planning crate that cannot
/// name `codestory-store`, `codestory-retrieval`, `codestory-indexer`,
/// `codestory-workspace`, or `codestory-runtime` cannot open storage, execute
/// retrieval, activate or retry a publication, or move readiness, because none
/// of those types exist for it. Everything it may know about pinned runtime
/// state arrives through `codestory_agent::PinnedReader`, which the runtime
/// implements.
///
/// The location half stops the extraction from being quietly undone: a planning
/// module copied back under `crates/codestory-runtime/src/agent/` would regain
/// the whole runtime namespace without anyone editing a manifest.
#[test]
fn agent_planning_crate_owns_planning_and_depends_on_contracts_only() {
    let dependencies = dependency_names("crates/codestory-agent/Cargo.toml");
    let workspace_dependencies = dependencies
        .iter()
        .filter(|name| name.starts_with("codestory-"))
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        workspace_dependencies,
        BTreeSet::from(["codestory-contracts".to_string()]),
        "codestory-agent plans against the contract types only; it reaches pinned \
         runtime state through PinnedReader, never through a producer crate"
    );

    for module in AGENT_PLANNING_MODULES {
        assert!(
            repo_root()
                .join("crates/codestory-agent/src")
                .join(module)
                .is_file(),
            "planning module {module} belongs to codestory-agent"
        );
        assert!(
            !repo_root()
                .join("crates/codestory-runtime/src/agent")
                .join(module)
                .is_file(),
            "planning module {module} must not exist a second time inside the runtime crate"
        );
    }

    // Planning reads. It never writes persisted state, and it never takes the
    // advisory locks that guard state it does not own.
    let agent_source = production_source(&read_source_tree("crates/codestory-agent/src"));
    for forbidden in [
        "fs::write",
        "fs::create_dir",
        "fs::remove_file",
        "fs::remove_dir",
        "fs::rename",
        "OpenOptions",
        "bounded_locks",
        "owned_artifacts",
    ] {
        assert!(
            !agent_source.contains(forbidden),
            "codestory-agent must not spell {forbidden}: planning owns no persisted state"
        );
    }

    // Planning modules (not eval-only `eval_probes.rs`) never open the live
    // tree. Citation source comes from a pinned excerpt or snapshot.
    let planning_source = production_source(
        &AGENT_PLANNING_MODULES
            .iter()
            .map(|module| {
                fs::read_to_string(repo_root().join("crates/codestory-agent/src").join(module))
                    .unwrap_or_else(|error| panic!("read planning module {module}: {error}"))
            })
            .collect::<Vec<_>>()
            .join("\n"),
    );
    for forbidden in ["fs::read_to_string", "std::fs::"] {
        assert!(
            !planning_source.contains(forbidden),
            "codestory-agent planning must not spell {forbidden}: live filesystem reads belong to the runtime pin"
        );
    }
}

/// A list that must stay in sync with a directory cannot be trusted to drift
/// silently: this pins `AGENT_PLANNING_MODULES` to the actual contents of
/// `crates/codestory-agent/src`, so a module added to the crate without a
/// matching allowlist entry — and therefore invisible to the location and
/// import-DAG contracts — fails here by name.
#[test]
fn agent_module_allowlist_stays_in_sync_with_the_agent_source_tree() {
    let mut files = Vec::new();
    collect_rs_files(&repo_root().join("crates/codestory-agent/src"), &mut files);
    let source_root = repo_root().join("crates/codestory-agent/src");
    let found = files
        .iter()
        .map(|path| {
            path.strip_prefix(&source_root)
                .expect("agent source file lives below the agent src root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<BTreeSet<_>>();

    let allowlist = AGENT_PLANNING_MODULES
        .iter()
        .map(|module| (*module).to_string())
        .collect::<BTreeSet<_>>();
    let exclusions = AGENT_MODULE_ALLOWLIST_EXCLUSIONS
        .iter()
        .map(|module| (*module).to_string())
        .collect::<BTreeSet<_>>();
    assert!(
        allowlist.is_disjoint(&exclusions),
        "a module cannot be both an allowlisted planning module and a named exclusion"
    );
    for exclusion in &exclusions {
        assert!(
            found.contains(exclusion),
            "stale exclusion: {exclusion} is named but no longer exists under \
             crates/codestory-agent/src"
        );
    }

    let expected = allowlist
        .union(&exclusions)
        .cloned()
        .collect::<BTreeSet<_>>();
    let unlisted = found.difference(&expected).cloned().collect::<Vec<_>>();
    let missing = allowlist.difference(&found).cloned().collect::<Vec<_>>();
    assert!(
        unlisted.is_empty(),
        "agent-crate source files missing from AGENT_PLANNING_MODULES (add each one, or name it \
         in AGENT_MODULE_ALLOWLIST_EXCLUSIONS with a reason): {unlisted:?}"
    );
    assert!(
        missing.is_empty(),
        "AGENT_PLANNING_MODULES entries with no file under crates/codestory-agent/src: {missing:?}"
    );
    assert_eq!(
        AGENT_PLANNING_MODULES.len(),
        found.len() - exclusions.len(),
        "the agent module allowlist length must equal the .rs file count under \
         crates/codestory-agent/src minus the named exclusions"
    );
}

/// Scan one line of release-compiled planning source for sibling-module
/// references (`crate::<module>` / `super::<module>`), recording each hit as an
/// import edge for the DAG guard below.
fn record_planning_import_edges<'a>(
    code: &str,
    module_names: &BTreeSet<&'a str>,
    edges: &mut BTreeSet<&'a str>,
) {
    for prefix in ["crate::", "super::"] {
        let mut rest = code;
        while let Some(position) = rest.find(prefix) {
            rest = &rest[position + prefix.len()..];
            let end = rest
                .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .unwrap_or(rest.len());
            if let Some(name) = module_names.get(&rest[..end]) {
                edges.insert(name);
            }
        }
    }
}

/// Walk the planning import graph depth-first; returns the module path of the
/// first release-code import cycle found, if any.
fn find_planning_import_cycle<'a>(
    graph: &BTreeMap<&'a str, BTreeSet<&'a str>>,
) -> Option<Vec<&'a str>> {
    fn visit<'a>(
        module: &'a str,
        graph: &BTreeMap<&'a str, BTreeSet<&'a str>>,
        finished: &mut BTreeSet<&'a str>,
        stack: &mut Vec<&'a str>,
    ) -> Option<Vec<&'a str>> {
        if let Some(position) = stack.iter().position(|entry| *entry == module) {
            let mut cycle = stack[position..].to_vec();
            cycle.push(module);
            return Some(cycle);
        }
        if finished.contains(module) {
            return None;
        }
        stack.push(module);
        if let Some(imports) = graph.get(module) {
            for import in imports {
                if let Some(cycle) = visit(import, graph, finished, stack) {
                    return Some(cycle);
                }
            }
        }
        stack.pop();
        finished.insert(module);
        None
    }

    let mut finished = BTreeSet::new();
    for module in graph.keys() {
        let mut stack = Vec::new();
        if let Some(cycle) = visit(module, graph, &mut finished, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

/// The release-compiled planning modules must remain acyclic. This assertion
/// makes a newly introduced sibling-module back-edge fail by name: the
/// release-compiled
/// source of every allowlisted planning module (top-level `#[cfg(test)]` items
/// stripped by the same helper the other source contracts use; line comments
/// dropped so prose mentioning a module path is not an edge) is scanned for
/// `crate::`/`super::` sibling references, and the resulting import graph must
/// stay acyclic.
#[test]
fn agent_planning_import_graph_stays_acyclic() {
    let module_names = AGENT_PLANNING_MODULES
        .iter()
        .map(|module| {
            module
                .strip_suffix(".rs")
                .expect("allowlist entries are .rs file names")
        })
        .collect::<BTreeSet<_>>();

    let mut graph: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for module in &module_names {
        let source = read(&format!("crates/codestory-agent/src/{module}.rs"));
        let production = production_source(&source);
        let mut imports = BTreeSet::new();
        for line in production.lines() {
            // Cut line comments (`//`, `///`, `//!`) so rustdoc links and
            // prose naming a module path do not become edges.
            let code = line.split("//").next().unwrap_or_default();
            record_planning_import_edges(code, &module_names, &mut imports);
        }
        imports.remove(*module);
        graph.insert(module, imports);
    }

    // Scanner self-check: packet_plan imports planning in release code today.
    // If the reference extraction regresses to matching nothing,
    // the acyclicity assertion below would pass vacuously; fail here instead.
    assert!(
        graph
            .get("packet_plan")
            .is_some_and(|imports| imports.contains("planning")),
        "planning import scan lost the known packet_plan -> planning edge; \
         the DAG guard is no longer reading real imports"
    );

    if let Some(cycle) = find_planning_import_cycle(&graph) {
        panic!(
            "agent planning modules must import each other along a DAG; a release-code \
             import cycle was reintroduced: {}. Break the cycle instead of extending it — \
             this is the boundary S4-10a's seam breaks established (#1673, M2 on #1865).",
            cycle.join(" -> ")
        );
    }
}

/// The eval/holdout probe hooks used to ride `#[cfg(test)]` inside
/// `codestory-runtime`, so a runtime unit test saw them and a product build did
/// not. They now live in `codestory-agent`, where `#[cfg(test)]` means "the
/// agent crate's own tests" and would silently switch the hooks off for every
/// runtime test that used to have them — quietly moving fidelity/eval output.
///
/// The `test-support` feature restores the old truth table, and this pins both
/// halves of it: `codestory-runtime` turns the feature on for its tests, and no
/// product dependency edge turns it on at all.
#[test]
fn agent_eval_hooks_stay_on_for_runtime_tests_and_off_for_product_builds() {
    let agent_manifest = manifest("crates/codestory-agent/Cargo.toml");
    assert!(
        agent_manifest
            .get("features")
            .and_then(|features| features.get("test-support"))
            .is_some(),
        "codestory-agent must declare the test-support feature the eval hooks hang from"
    );

    let runtime_manifest = manifest("crates/codestory-runtime/Cargo.toml");
    let dev_features = runtime_manifest
        .get("dev-dependencies")
        .and_then(|table| table.get("codestory-agent"))
        .and_then(|entry| entry.get("features"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    assert!(
        dev_features.contains("test-support"),
        "codestory-runtime's tests must compile codestory-agent with test-support, or the eval \
         probe hooks that used to be cfg(test) inside the runtime disappear from them"
    );

    for consumer in [
        "crates/codestory-runtime/Cargo.toml",
        "crates/codestory-cli/Cargo.toml",
        "crates/codestory-bench/Cargo.toml",
    ] {
        let consumer_manifest = manifest(consumer);
        let features = consumer_manifest
            .get("dependencies")
            .and_then(|table| table.get("codestory-agent"))
            .and_then(|entry| entry.get("features"))
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(
            !features.contains(&"test-support"),
            "{consumer} must not enable codestory-agent/test-support on a product dependency edge"
        );
    }

    // The hook itself has to hang from that feature, not from a bare cfg(test)
    // that no longer reaches the runtime's test build.
    let packet_scoring = read("crates/codestory-agent/src/packet_scoring.rs");
    assert!(
        packet_scoring.contains(
            "#[cfg(any(test, feature = \"test-support\"))]\nuse crate::eval_probes::eval_citation_rank_adjustment;"
        ) && packet_scoring.contains(
            "    #[cfg(any(test, feature = \"test-support\"))]\n    {\n        score = eval_citation_rank_adjustment("
        ),
        "the citation rank eval hook must be gated on test-support, not on cfg(test) alone"
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
    let benchmark_root = repo_root().join("crates/codestory-bench");
    for path in files {
        if !path
            .components()
            .any(|component| component.as_os_str() == "src")
        {
            continue;
        }
        if path.starts_with(&benchmark_root)
            || path == repo_root().join("crates/codestory-runtime/src/test_support.rs")
        {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read Rust source");
        if production_source_contains_git_spawn(&source) {
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
fn crate_source_git_spawns_are_limited_to_named_non_product_boundaries() {
    let mut files = Vec::new();
    collect_rs_files(&repo_root().join("crates"), &mut files);
    let actual = files
        .into_iter()
        .filter(|path| {
            path.components()
                .any(|component| component.as_os_str() == "src")
        })
        .filter(|path| {
            let source = fs::read_to_string(path).expect("read Rust source");
            production_source_contains_git_spawn(&source)
        })
        .map(|path| {
            path.strip_prefix(repo_root())
                .expect("crate source stays below repository root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        "crates/codestory-bench/src/bin/codestory_proof_availability/materialize.rs".to_owned(),
        "crates/codestory-bench/src/bin/codestory_proof_availability/multilingual_contract.rs"
            .to_owned(),
        "crates/codestory-runtime/src/test_support.rs".to_owned(),
    ]);

    assert_eq!(
        actual, expected,
        "Git process execution under crate source is limited to the feature-dark proof-availability materializer and the explicit runtime test-support helper"
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

    let one_line_braced_use = "\
#[cfg(test)]
use crate::{first, second};

fn production() {
    real_call();
}
";
    let production = production_source(one_line_braced_use);
    assert!(!production.contains("first, second"));
    assert!(
        production.contains("real_call()"),
        "a one-line braced test import must not consume following production code: {production}"
    );

    let nested_one_line_braces = "\
#[cfg(test)]
mod tests {
    fn fixture() {
        match 1 {
            _ => {}
        }
    }
}

fn production() {
    real_call();
}
";
    let production = production_source(nested_one_line_braces);
    assert!(!production.contains("_ => {}"));
    assert!(production.contains("real_call()"));
}

#[test]
fn production_git_scan_sees_code_after_a_test_module() {
    let source = "\
#[cfg(test)]
mod tests {
    fn fixture() {}
}

fn shipped() {
    Command::new(\"git\");
}
";
    assert!(production_source_contains_git_spawn(source));
}

#[test]
fn evidence_only_v3_support_is_feature_separate_from_proof_qualification() {
    const FEATURE: &str = "v3-evidence-separation-support";
    let agent_manifest = read("crates/codestory-agent/Cargo.toml");
    let runtime_manifest = read("crates/codestory-runtime/Cargo.toml");
    let cli_manifest = read("crates/codestory-cli/Cargo.toml");
    assert!(
        agent_manifest.contains("v3-evidence-separation-support"),
        "the packet v3 planner must have a proof-independent sealed feature"
    );
    assert!(
        runtime_manifest.contains(
            "v3-evidence-separation-support = [\"codestory-agent/v3-evidence-separation-support\"]"
        ),
        "the runtime packet record and projection builders must carry the sealed agent feature"
    );
    assert!(
        cli_manifest.contains(
            "v3-evidence-separation-support = [\"codestory-runtime/v3-evidence-separation-support\"]"
        ),
        "the Q1 evidence-only compile gate must not activate proof qualification"
    );
    let library = read("crates/codestory-cli/src/lib.rs");
    assert!(
        library.contains("feature = \"v3-evidence-separation-support\""),
        "the sealed evidence-only conformance facade must compile independently"
    );
    let stdio_v3 = read("crates/codestory-cli/src/stdio_v3/mod.rs");
    assert!(
        stdio_v3.contains(
            "codestory_runtime::v3_evidence_qualification_support::real_projection_fixtures"
        ),
        "four-revision conformance must consume the real runtime record/projection builders"
    );
    assert!(
        !stdio_v3.contains("serde_json::json!"),
        "four-revision conformance must not substitute hand-built JSON for product projections"
    );

    for (path, expected) in [
        ("crates/codestory-agent/Cargo.toml", BTreeSet::new()),
        (
            "crates/codestory-runtime/Cargo.toml",
            BTreeSet::from(["codestory-agent/v3-evidence-separation-support"]),
        ),
        (
            "crates/codestory-cli/Cargo.toml",
            BTreeSet::from(["codestory-runtime/v3-evidence-separation-support"]),
        ),
    ] {
        let document = manifest(path);
        let enabled = document["features"][FEATURE]
            .as_array()
            .expect("sealed evidence feature array")
            .iter()
            .map(|value| value.as_str().expect("feature edge"))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            enabled, expected,
            "unexpected sealed feature graph at {path}"
        );
        assert!(
            enabled
                .iter()
                .all(|edge| !edge.contains("proof-qualification-support")
                    && !edge.contains("test-support")),
            "{path} must not pull proof or general test support into the evidence-only gate"
        );
    }
}

#[test]
fn test_gated_module_files_follow_gated_module_trees_only() {
    let tempdir = tempfile::tempdir().expect("create fixture source root");
    let source_root = tempdir.path();
    fs::write(
        source_root.join("lib.rs"),
        "#[cfg(test)]\nmod x;\nmod y;\n#[cfg(test)]\nmod directory;\n",
    )
    .expect("write root module");
    fs::write(source_root.join("x.rs"), "mod nested;\n").expect("write gated module");
    fs::create_dir(source_root.join("x")).expect("create nested module directory");
    fs::write(source_root.join("x/nested.rs"), "").expect("write nested module");
    fs::write(source_root.join("y.rs"), "").expect("write production module");
    fs::create_dir(source_root.join("directory")).expect("create directory module");
    fs::write(source_root.join("directory/mod.rs"), "mod child;\n")
        .expect("write directory module root");
    fs::write(source_root.join("directory/child.rs"), "").expect("write directory child");
    fs::write(source_root.join("directory/fixture.rs"), "").expect("write directory fixture");

    let test_gated = test_gated_module_files(source_root);
    assert!(test_gated.contains(Path::new("x.rs")));
    assert!(test_gated.contains(Path::new("x/nested.rs")));
    assert!(test_gated.contains(Path::new("directory/mod.rs")));
    assert!(test_gated.contains(Path::new("directory/child.rs")));
    assert!(test_gated.contains(Path::new("directory/fixture.rs")));
    assert!(
        !test_gated.contains(Path::new("y.rs")),
        "ungated modules remain release-compiled"
    );
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
            && commit.contains(".publish_receipted_with_stats(&self.storage_path)"),
        "full refresh should stage, finalize, and publish snapshots through the store snapshot surface"
    );
    assert!(
        incremental_refresh.contains("SnapshotStore::clone_live_to_staged(storage_path)")
            && incremental_refresh.contains(".snapshots()\n            .finalize_staged()")
            && incremental_refresh.contains(".snapshots()\n            .refresh_detail()")
            && commit.contains(".publish_receipted_with_stats(&self.storage_path)"),
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
            && commit.contains(".publish_receipted_with_stats(&self.storage_path)")
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
        "crates/codestory-agent/src",
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
            if relative == "crates/codestory-agent/src/eval_probes.rs"
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

#[test]
fn registered_settings_are_read_only_by_their_declared_owner() {
    // CONFIG-B: the sibling contract above stops a *second spelling* of an
    // identity. It does not stop a second reading, and those are different
    // failures: a non-owner that imports the registry constant spells the
    // identity once and still decides for itself what the value means. That is
    // how CODESTORY_SEMANTIC_DOC_MAX_TOKENS=0 was 16 tokens to retrieval and
    // 128 to the runtime at the same instant.
    //
    // A read is an identity in *expression* position: the registry constant, an
    // `as` alias of it, or a file-local `const` bound to it, appearing anywhere
    // outside an import, a comment, or a string. Passing it to a helper counts —
    // `env_flag_enabled(HYBRID_RETRIEVAL_ENABLED_ENV, true)` reads the setting
    // just as surely as `std::env::var` does, and the whole point is that no
    // indirection buys a second interpretation. Naming an identity inside a
    // message stays legal: it tells an operator which variable to set and reads
    // nothing.
    //
    // `production_sources` strips only the trailing `#[cfg(test)] mod tests`,
    // so an inline test-only reader inside a production file is in scope on
    // purpose. Those are the worst kind: the runtime's semantic prefilter read
    // this setting only under `cfg(test)` and taught its own tests an encoding
    // vocabulary the store has never written.
    //
    // Scope, stated so it is not mistaken for more: the six product crates
    // `production_sources` walks. A harness outside them that *sets* a variable
    // for a child process (codestory-bench does, for the qualification gate) is
    // producing an environment, not interpreting one, and is not governed here.
    // Should a product crate ever need to write one, the owner should expose the
    // writer, and this contract will say so by failing.
    let registry = read("crates/codestory-contracts/src/config_registry.rs");
    let declared = registry_identity_constants(&registry)
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let mut violations = Vec::new();
    for (relative, source) in production_sources() {
        let masked = mask_comments_and_strings(&source);
        let bindings = registered_identity_bindings(&declared, &masked);
        if bindings.is_empty() {
            continue;
        }
        let imports = import_line_numbers(&masked);
        for (line_number, line) in masked.lines().enumerate() {
            if imports.contains(&(line_number + 1)) {
                continue;
            }
            for (token, identity) in &bindings {
                if !contains_word(line, token) {
                    continue;
                }
                let owner = codestory_contracts::config_registry::env_setting_owner(identity)
                    .expect("binding resolves to a registered identity");
                if owner == relative {
                    continue;
                }
                violations.push(format!(
                    "{relative}:{} reads {identity} (as `{token}`); the registry declares \
                     {owner} as its single reader. Consume that module's typed value instead \
                     of reading the setting here.",
                    line_number + 1
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "a registered setting may be read only by its declared owner:\n{}",
        violations.join("\n")
    );
}

#[test]
fn every_registered_identity_has_exactly_one_registry_constant() {
    // The read contract resolves identities through the registry's own
    // constants, so a setting reachable only by a literal would be invisible to
    // it. Both directions are checked: a constant with no entry, and an entry
    // with no constant.
    let registry = read("crates/codestory-contracts/src/config_registry.rs");
    let declared = registry_identity_constants(&registry);
    let mut by_identity: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (constant, identity) in &declared {
        assert!(
            codestory_contracts::config_registry::env_setting(identity).is_some(),
            "{constant} declares {identity}, which is not in ENV_SETTINGS"
        );
        by_identity
            .entry(identity.as_str())
            .or_default()
            .push(constant.as_str());
    }
    for setting in codestory_contracts::config_registry::ENV_SETTINGS {
        let constants = by_identity.get(setting.name).cloned().unwrap_or_default();
        assert_eq!(
            constants.len(),
            1,
            "{} needs exactly one registry constant, found {constants:?}",
            setting.name
        );
    }
}

/// Registry constant name to environment identity, read from the registry
/// source so the registry stays the one place a knob is declared.
fn registry_identity_constants(registry_source: &str) -> Vec<(String, String)> {
    let mut declared = Vec::new();
    let normalized = registry_source.replace(['\n', '\r'], " ");
    let mut rest = normalized.as_str();
    while let Some(index) = rest.find("pub const ") {
        rest = &rest[index + "pub const ".len()..];
        let Some((name, tail)) = rest.split_once(':') else {
            break;
        };
        let name = name.trim();
        if !name.ends_with("_ENV") {
            continue;
        }
        let Some((_, tail)) = tail.split_once('=') else {
            break;
        };
        let tail = tail.trim_start();
        let Some(value) = tail.strip_prefix('"').and_then(|tail| {
            tail.split_once('"')
                .map(|(value, _)| value)
                .filter(|value| value.starts_with("CODESTORY_"))
        }) else {
            continue;
        };
        declared.push((name.to_string(), value.to_string()));
    }
    declared
}

/// Names that resolve to a registered identity inside one source file: the
/// registry constants themselves, `as` aliases of them, and file-local `const`
/// bindings to them.
fn registered_identity_bindings(
    declared: &BTreeMap<String, String>,
    masked_source: &str,
) -> BTreeMap<String, String> {
    let registered = |identity: &String| {
        codestory_contracts::config_registry::env_setting(identity)
            .is_some()
            .then(|| identity.clone())
    };
    let mut bindings = BTreeMap::new();
    for (constant, identity) in declared {
        if contains_word(masked_source, constant)
            && let Some(identity) = registered(identity)
        {
            bindings.insert(constant.clone(), identity);
        }
    }
    for (alias, constant) in aliased_names(masked_source) {
        if let Some(identity) = declared.get(&constant).and_then(registered) {
            bindings.insert(alias, identity);
        }
    }
    bindings
}

/// `<CONSTANT> as <ALIAS>` in an import, and `const <ALIAS>: &str = ...::<CONSTANT>;`.
fn aliased_names(masked_source: &str) -> Vec<(String, String)> {
    let mut aliases = Vec::new();
    let words = masked_source
        .split(|character: char| !(character.is_alphanumeric() || character == '_'))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    for window in words.windows(3) {
        if window[1] == "as" && window[0].ends_with("_ENV") {
            aliases.push((window[2].to_string(), window[0].to_string()));
        }
    }
    for window in words.windows(6) {
        // `const NAME : & str = config_registry :: CONSTANT ;` reduces to the
        // words `const NAME str config_registry CONSTANT`, so match on the
        // const keyword and the trailing `_ENV` word.
        if window[0] == "const"
            && window.contains(&"str")
            && let Some(constant) = window.iter().rev().find(|word| word.ends_with("_ENV"))
            && *constant != window[1]
        {
            aliases.push((window[1].to_string(), (*constant).to_string()));
        }
    }
    aliases
}

/// 1-based line numbers occupied by `use` items.
fn import_line_numbers(masked_source: &str) -> BTreeSet<usize> {
    let mut lines = BTreeSet::new();
    let mut inside_import = false;
    for (index, line) in masked_source.lines().enumerate() {
        let trimmed = line.trim_start();
        let starts_import = trimmed.starts_with("use ")
            || trimmed.starts_with("pub use ")
            || trimmed.starts_with("pub(crate) use ")
            || trimmed.starts_with("pub(super) use ")
            || (trimmed.starts_with("pub(in ")
                && trimmed
                    .split_once(") ")
                    .is_some_and(|(_, rest)| rest.starts_with("use ")));
        if !inside_import && !starts_import {
            continue;
        }
        lines.insert(index + 1);
        inside_import = !line.trim_end().ends_with(';');
    }
    lines
}

fn contains_word(haystack: &str, word: &str) -> bool {
    let mut offset = 0;
    while let Some(index) = haystack[offset..].find(word) {
        let start = offset + index;
        let end = start + word.len();
        let before_is_word = haystack[..start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        let after_is_word = haystack[end..]
            .chars()
            .next()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        if !before_is_word && !after_is_word {
            return true;
        }
        offset = end;
    }
    false
}

/// Blank out comments and string literals, preserving byte offsets and line
/// breaks so a violation still reports the line it was found on. Prose that
/// names a variable and messages that tell an operator to set one are not
/// reads, and must not be mistaken for them.
fn mask_comments_and_strings(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut masked = bytes.to_vec();
    // Blanking whole comment and literal spans byte by byte keeps every offset
    // and line break where it was, and stays valid UTF-8 because the spans
    // always start and end on character boundaries.
    let blank = |masked: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut masked[from..to.min(bytes.len())] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let is_word_byte = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || !byte.is_ascii();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                let end = bytes[index..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(bytes.len(), |offset| index + offset);
                blank(&mut masked, index, end);
                index = end;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let mut depth = 1_usize;
                let mut cursor = index + 2;
                while cursor < bytes.len() && depth > 0 {
                    if bytes[cursor] == b'/' && bytes.get(cursor + 1) == Some(&b'*') {
                        depth += 1;
                        cursor += 2;
                    } else if bytes[cursor] == b'*' && bytes.get(cursor + 1) == Some(&b'/') {
                        depth -= 1;
                        cursor += 2;
                    } else {
                        cursor += 1;
                    }
                }
                blank(&mut masked, index, cursor);
                index = cursor;
            }
            b'\'' => {
                // A char or byte literal, or a lifetime. Only the literals hide
                // a quote that would otherwise open a phantom string.
                let end = match (bytes.get(index + 1), bytes.get(index + 2)) {
                    (Some(b'\\'), _) => bytes[index + 2..]
                        .iter()
                        .position(|byte| *byte == b'\'')
                        .map(|offset| index + 3 + offset),
                    (Some(_), Some(b'\'')) => Some(index + 3),
                    _ => None,
                };
                match end {
                    Some(end) => {
                        blank(&mut masked, index, end);
                        index = end;
                    }
                    None => index += 1,
                }
            }
            b'r' | b'b' if index == 0 || !is_word_byte(bytes[index - 1]) => {
                let mut cursor = index + 1;
                if bytes.get(cursor) == Some(&b'r') && bytes[index] == b'b' {
                    cursor += 1;
                }
                let mut hashes = 0;
                while bytes.get(cursor) == Some(&b'#') {
                    hashes += 1;
                    cursor += 1;
                }
                if bytes.get(cursor) != Some(&b'"') {
                    index += 1;
                    continue;
                }
                let terminator = format!("\"{}", "#".repeat(hashes));
                let end = source
                    .get(cursor + 1..)
                    .and_then(|rest| rest.find(&terminator))
                    .map_or(bytes.len(), |offset| cursor + 1 + offset + terminator.len());
                blank(&mut masked, index, end);
                index = end;
            }
            b'"' => {
                let mut cursor = index + 1;
                while cursor < bytes.len() {
                    match bytes[cursor] {
                        b'\\' => cursor += 2,
                        b'"' => {
                            cursor += 1;
                            break;
                        }
                        _ => cursor += 1,
                    }
                }
                blank(&mut masked, index, cursor);
                index = cursor.min(bytes.len());
            }
            _ => index += 1,
        }
    }
    String::from_utf8(masked).expect("blanking whole spans preserves UTF-8")
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
        "derived-reset-quarantine",
    ];
    let producer_crates = [
        "crates/codestory-workspace/src",
        "crates/codestory-store/src",
        "crates/codestory-agent/src",
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

/// `[workspace.dependencies]` is a shared version register, not a parking lot.
///
/// An entry no member consumes still reads as a supported, version-pinned
/// dependency of this workspace while pinning nothing: `proptest` sat there
/// consumed by zero crates, and `ring` outlived the one crate that used it. The
/// dead entry is the debt; this is the ratchet that keeps it retired.
#[test]
fn every_workspace_dependency_is_consumed_by_a_member() {
    fn consumes(table: &Value, name: &str) -> bool {
        let Some(table) = table.as_table() else {
            return false;
        };
        table.iter().any(|(key, value)| {
            (key == name
                && value
                    .get("workspace")
                    .and_then(Value::as_bool)
                    .unwrap_or(false))
                || consumes(value, name)
        })
    }

    let workspace = manifest("Cargo.toml");
    let declared = workspace
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Value::as_table)
        .expect("workspace dependencies");
    let members = workspace_members()
        .into_iter()
        .map(|member| manifest(&format!("{member}/Cargo.toml")))
        .collect::<Vec<_>>();

    let unconsumed = declared
        .keys()
        .filter(|name| !members.iter().any(|member| consumes(member, name)))
        .cloned()
        .collect::<Vec<_>>();

    assert!(
        unconsumed.is_empty(),
        "workspace dependencies no member declares with `workspace = true`: {}",
        unconsumed.join(", ")
    );
}

/// Every constant `codestory-llama-sys`'s build script compiles into the crate
/// must be read by workspace code.
///
/// The frozen constant set is calibrated evidence, and emitting values from it
/// that nothing reads presents unread numbers as shipped runtime behaviour —
/// the election-backoff and bulk-replay-budget constants were exactly that, and
/// three model/native identity constants sat beside them. Deleting the emission
/// changes no calibrated value: the constant-set JSON is untouched and the
/// build script still enforces the invariants those values exist for.
#[test]
fn generated_embedding_constants_are_all_read_by_workspace_code() {
    let build_script = read("crates/codestory-llama-sys/build.rs");
    let emitted = build_script
        .match_indices("pub const ")
        .chain(build_script.match_indices("pub static "))
        .filter_map(|(index, keyword)| {
            build_script[index + keyword.len()..]
                .split(|ch: char| !(ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_'))
                .next()
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .collect::<BTreeSet<String>>();
    assert!(
        emitted.contains("PER_USER_EMBEDDING_CONNECT_TIMEOUT_MS")
            && emitted.contains("MODEL_SHA256"),
        "the emitted-constant scan must actually find the generated constants: {emitted:?}"
    );

    let mut corpus = String::new();
    for member in workspace_members() {
        for directory in ["src", "tests", "benches", "examples"] {
            if repo_root().join(&member).join(directory).is_dir() {
                corpus.push_str(&read_source_tree(&format!("{member}/{directory}")));
                corpus.push('\n');
            }
        }
    }

    let unread = emitted
        .iter()
        .filter(|name| !corpus.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        unread.is_empty(),
        "generated constants no workspace source reads: {}",
        unread.join(", ")
    );
}

// The build-script decision itself, compiled from the same file `build.rs`
// includes. A build script is reachable from no test target, which is how this
// gate came to read Cargo's `DEBUG` (debug info) instead of `PROFILE` (profile
// identity) with nothing failing.
mod model_source_gate {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../codestory-llama-sys/model_source_gate.rs"
    ));

    pub(super) fn requires_embedded_model(profile: Option<&str>) -> bool {
        profile_requires_embedded_model(profile)
    }
}

/// A release build with no embedded model must fail, and must keep failing when
/// somebody turns on release debug info.
///
/// `[profile.release] debug = 1` for symbolication sets `DEBUG` to a
/// non-`"false"` value while `PROFILE` stays `release`. Keying the gate on
/// `DEBUG` therefore let a release build produce a binary with no embedded
/// model and say nothing.
#[test]
fn the_release_model_embedding_gate_keys_on_the_profile_not_debug_info() {
    assert!(model_source_gate::requires_embedded_model(Some("release")));
    for permitted in [Some("debug"), Some("test"), Some("bench"), None] {
        assert!(
            !model_source_gate::requires_embedded_model(permitted),
            "{permitted:?} is not a shipping profile"
        );
    }

    let build_script = read("crates/codestory-llama-sys/build.rs");
    assert!(
        build_script.contains("env::var(\"PROFILE\")"),
        "the gate must read the profile identity"
    );
    assert!(
        !build_script.contains("\"DEBUG\""),
        "the gate must not read the debug-info setting"
    );
    assert!(
        build_script.contains("profile_requires_embedded_model(profile.as_deref())"),
        "the gate must route the profile through the shared decision this test compiles"
    );
}

/// Every word the retired `is_gap_annotation` heuristic substring-matched.
const RETIRED_ANNOTATION_PROSE_MARKERS: [&str; 10] = [
    "fallback",
    "gap",
    "low confidence",
    "missing",
    "no relevant",
    "skipped",
    "truncated",
    "uncertain",
    "unavailable",
    "weak",
];

#[test]
fn retrieval_annotations_are_classified_by_typed_kind_not_by_prose() {
    // EV-6b (#1746). `is_gap_annotation` lowercased annotation text and looked for ten English
    // words to decide whether an annotation was an evidence gap; a match downgraded
    // `agent_confidence` from high/ready to medium/review. Annotations interpolate prompt text,
    // file paths, symbol names, error messages, and user bookmark comments, so reported
    // confidence depended on wording rather than on evidence. Classification now reads only the
    // typed kind on the DTO.
    let dto = read("crates/codestory-contracts/src/api/dto.rs");
    let dto_production = production_source(&dto);
    assert!(
        dto_production.contains("pub enum RetrievalAnnotationKindDto"),
        "retrieval annotations must carry a typed kind enum"
    );
    for variant in ["    Gap,", "    Observation,"] {
        assert!(
            dto_production.contains(variant),
            "retrieval annotation kind must be exactly Gap | Observation: missing `{variant}`"
        );
    }
    assert!(
        dto_production.contains("pub annotations: Vec<RetrievalAnnotationDto>"),
        "the retrieval trace annotation channel must be typed, not Vec<String>"
    );

    let output = read("crates/codestory-cli/src/output.rs");
    let output_production = production_source(&output);
    assert!(
        !output_production.contains("is_gap_annotation"),
        "the prose gap classifier must be gone from confidence rendering"
    );
    assert!(
        output_production.contains("annotation.kind == RetrievalAnnotationKindDto::Gap"),
        "gap notes must select annotations by typed kind"
    );

    // The confidence path must not reach for annotation prose at all. `agent_gap_notes` is the
    // only place annotations feed a confidence decision, so pin the whole function body.
    let gap_notes = source_between(
        output_production.as_str(),
        "fn agent_gap_notes(",
        "\nfn append_retrieval_gap_notes(",
    );
    assert!(
        !gap_notes.contains("to_ascii_lowercase") && !gap_notes.contains("to_lowercase"),
        "confidence gap notes must never lowercase annotation prose:\n{gap_notes}"
    );
    for marker in RETIRED_ANNOTATION_PROSE_MARKERS {
        assert!(
            !gap_notes.contains(&format!("\"{marker}\"")),
            "confidence gap notes must not substring-match the retired prose marker `{marker}`"
        );
    }

    // Every producer states the kind at the push site. Nothing may reach the channel through an
    // untyped `String`, and no helper may infer the kind for a caller.
    for path in [
        "crates/codestory-runtime/src/agent/trace.rs",
        "crates/codestory-runtime/src/agent/orchestrator.rs",
        "crates/codestory-runtime/src/agent/packet_batch.rs",
        "crates/codestory-runtime/src/agent/packet_capping.rs",
        "crates/codestory-runtime/src/agent/packet_trace.rs",
        "crates/codestory-cli/src/app/agent_context/context.rs",
        "crates/codestory-cli/src/stdio_transport.rs",
    ] {
        let source = read(path);
        let dense = production_source(&source)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("");
        let mut cursor = 0usize;
        while let Some(offset) = dense[cursor..].find("annotations.push(") {
            let after = cursor + offset + "annotations.push(".len();
            let rest = &dense[after..];
            assert!(
                rest.starts_with("RetrievalAnnotationDto::gap(")
                    || rest.starts_with("RetrievalAnnotationDto::observation(")
                    || rest.starts_with(
                        "codestory_contracts::api::RetrievalAnnotationDto::observation("
                    )
                    || rest.starts_with("codestory_contracts::api::RetrievalAnnotationDto::gap("),
                "{path} pushes a retrieval annotation without naming its kind: {}",
                &rest[..rest.len().min(80)]
            );
            cursor = after;
        }
    }

    // `TraceRecorder` is the runtime's own annotation front door: it must expose one entry point
    // per kind, not a single `annotate` that leaves the classification to a downstream reader.
    let recorder = production_source(&read("crates/codestory-runtime/src/agent/trace.rs"));
    for required in [
        "fn annotate_gap(&mut self, message: impl Into<String>)",
        "fn observe(&mut self, message: impl Into<String>)",
    ] {
        assert!(
            recorder.contains(required),
            "TraceRecorder must expose a per-kind annotation entry point: missing `{required}`"
        );
    }
    assert!(
        !recorder.contains("fn annotate(&mut self"),
        "the kind-less TraceRecorder::annotate entry point must stay retired"
    );
}

/// A sealed receipt must not claim more than the platform it runs on delivers.
///
/// READY-C (#1654) shipped `validation_receipts` documenting that "replacement,
/// truncation, in-place rewriting ... all break the seal", and two tests
/// asserting exactly that. Neither statement holds on Windows: `std::fs`
/// reports no device/inode pair and no inode-change instant there, so a
/// same-length rewrite that restores the modification time produces an
/// identical observation and is answered from the receipt. Nothing contradicted
/// the claim because `codestory-contracts` tests run only on Linux and macOS —
/// the Windows lanes in `source-proof.yml` build `codestory-workspace` and
/// `codestory-llama-sys` test targets only. The limit is therefore stated, in
/// the contract and in the docs, and this is what keeps it stated.
#[test]
fn the_sealed_receipt_states_its_windows_limit_in_the_contract_and_the_docs() {
    let receipts = read("crates/codestory-contracts/src/validation_receipts.rs");
    let production = production_source(&receipts);
    for required in [
        "pub enum SealFidelity",
        "    InodeChangeTracked,",
        "    TimestampsOnly,",
        "pub fn fidelity(&self) -> Option<SealFidelity>",
    ] {
        assert!(
            production.contains(required),
            "the receipt must report how much its observation can distinguish: missing `{required}`"
        );
    }

    // The module contract and the enum variant that carries the weaker case
    // both have to name the platform. "Some platforms report less" is the
    // implying-away this replaced.
    let module_contract = production
        .split("\nuse std::collections::HashMap;")
        .next()
        .expect("module doc comment precedes the imports");
    assert!(
        module_contract.contains("Windows"),
        "the receipt's module contract must name the platform whose seals are weaker"
    );
    let timestamps_only_doc = source_between(
        &production,
        "    /// The observation carries only",
        "    TimestampsOnly,",
    );
    assert!(
        timestamps_only_doc.contains("Windows"),
        "the timestamps-only variant must name the platform it describes"
    );

    for (path, anchor) in [
        (
            "docs/architecture/subsystems/contracts.md",
            "## Sealed validation receipts and their platform limit",
        ),
        (
            "docs/architecture/subsystems/retrieval.md",
            "#sealed-validation-receipts-and-their-platform-limit",
        ),
    ] {
        let doc = read(path);
        assert!(
            doc.contains(anchor),
            "{path} must carry the receipt platform limit: missing `{anchor}`"
        );
        assert!(
            doc.contains("Windows"),
            "{path} must name the platform the receipt is weaker on"
        );
    }
}

/// EV-6c (#1775). Every `Gap`-kind retrieval annotation a release build can emit, keyed by the
/// file that produces it and the exact argument the producer passes, in source order.
///
/// EV-6b (#1746) pinned only the harmless direction: an observation carrying all ten retired
/// prose markers no longer downgrades `agent_confidence`. The dangerous direction stayed open.
/// Swapping `trace.annotate_gap(..)` for `trace.observe(..)`, or `RetrievalAnnotationDto::gap(..)`
/// for `::observation(..)`, at any one site reclassifies a real evidence gap as routine
/// telemetry: `agent_gap_notes` drops it, the packet keeps `high`/`ready`, and the operator is
/// told an answer is grounded when the evidence behind it was never retrieved. That direction
/// *inflates* stated confidence.
///
/// The behavioural half of this pin lives with the producers: `agent::trace::tests`,
/// `agent::packet_batch::tests` and `agent::orchestrator::tests` drive the real code paths and
/// read the kind back off the published `AgentRetrievalTraceDto`. This inventory covers the
/// rest: the latency cut-offs and post-retrieval failures inside `execute_retrieval`,
/// `investigate_query_expansion`, `maybe_read_source_context` and `build_mermaid_artifacts`,
/// which no unit test can enter because reaching them needs a served retrieval and therefore a
/// fully indexed sidecar. It is fail-closed in both directions: a producer whose kind flips, a
/// producer that disappears, and a newly added gap producer all fail here.
const PRODUCTION_GAP_ANNOTATION_PRODUCERS: &[(&str, &[&str])] = &[
    (
        "crates/codestory-runtime/src/agent/orchestrator.rs",
        &[
            "annotation",
            "format!(\"Index freshness not checked: {}\", error.message)",
            "format!(\"Graph artifact bundle truncated at {} bytes; narrow focus or reduce trail depth for complete graph exports.\", GRAPH_ARTIFACT_BUNDLE_BYTE_CAP)",
            "format!(\"retrieval_primary rejected=true fail_closed=true reason={reason}\")",
            "format!(\"retrieval_primary unavailable=true fail_closed=true reason={reason}\")",
            "\"retrieval_primary skipped local nucleo investigation supplement on weak hits\"",
            "format!(\"Investigation query expansion failed; continuing with initial hits: {}\", error.message)",
            "\"Investigation discarded expansion-only hits for an unanchored natural-language query.\"",
            "\"Investigation skipped repo-text diagnostics because packet evidence must come from sidecar-backed resolvable hits or direct source reads.\"",
            "\"Investigation discarded low-confidence unanchored hits for a natural-language query.\"",
            "\"Repo-text diagnostics are disabled for packet evidence; weak unanchored hits were not promoted.\"",
            "\"Investigation low confidence gap after sidecar query expansion.\"",
            "\"Trail filter options unavailable; continuing with unsanitized filters.\"",
            "\"Neighborhood retrieval failed; continuing with trail retrieval.\"",
            "trail_truncated_annotation(idx + 1, plan.max_nodes)",
            "format!(\"Trail {} failed and was skipped.\", idx + 1)",
            "\"Latency-first cutoff skipped node occurrence lookups.\"",
            "format!(\"Node occurrence lookup failed for {}: {}\", hit.display_name, error.message)",
            "\"Latency-first cutoff skipped edge occurrence lookups.\"",
            "\"Latency-first cutoff skipped investigation query expansion.\"",
            "\"Latency-first cutoff skipped source reads.\"",
            "\"Latency-first cutoff skipped mermaid synthesis.\"",
        ],
    ),
    (
        "crates/codestory-runtime/src/agent/packet_batch.rs",
        &[
            "\"packet_subqueries skipped budget=tiny\"",
            "format!(\"packet_material_queries skipped reason=latency_budget_exhausted count={}\", pending.len())",
            "format!(\"packet_fused_subquery_batch_failed error={error:?}\")",
            "format!(\"packet_fused_blocking_cancel_retry skipped reason=latency_budget_exhausted count={}\", retry_pending.len())",
            "format!(\"packet_fused_blocking_cancel_retry_failed error={error:?}\")",
            "format!(\"packet_fused_blocking_cancel_retry exhausted count={}\", retry_outcome.retryable_queries.len())",
        ],
    ),
    ("crates/codestory-runtime/src/agent/trace.rs", &["message"]),
];

/// Collapse `source` onto one line so a producer's argument compares equal however rustfmt
/// wrapped it.
fn collapse_call_source(source: &str) -> String {
    source
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(" )", ")")
}

/// The argument expression of every `Gap`-kind annotation producer in `source`, in source order.
///
/// `fn annotate_gap(..)` is `TraceRecorder`'s declaration, not a producer, so it is skipped; the
/// one push inside its body is the producer that stands for it.
fn gap_annotation_producer_arguments(source: &str) -> Vec<String> {
    let dense = collapse_call_source(&production_source(source));
    let mut arguments = Vec::new();
    let mut cursor = 0usize;
    while cursor < dense.len() {
        let Some((offset, token)) = ["annotate_gap(", "RetrievalAnnotationDto::gap("]
            .into_iter()
            .filter_map(|token| dense[cursor..].find(token).map(|at| (at, token)))
            .min()
        else {
            break;
        };
        let start = cursor + offset;
        cursor = start + token.len();
        if dense[..start].ends_with("fn ") {
            continue;
        }
        arguments.push(balanced_call_argument(&dense, cursor));
    }
    arguments
}

/// Text between a producer's opening parenthesis (already consumed) and its match. String and
/// char literals are skipped so `query.replace('`', "'")` cannot unbalance the scan.
fn balanced_call_argument(dense: &str, after_open: usize) -> String {
    let bytes = dense.as_bytes();
    let mut depth = 1usize;
    let mut index = after_open;
    let mut in_string = false;
    let mut in_char = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string || in_char {
            match byte {
                b'\\' => index += 1,
                b'"' if in_string => in_string = false,
                b'\'' if in_char => in_char = false,
                _ => {}
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'\'' => in_char = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return dense[after_open..index].trim_end_matches(',').to_string();
                }
            }
            _ => {}
        }
        index += 1;
    }
    panic!("unbalanced annotation producer call at byte {after_open}");
}

#[test]
fn every_production_gap_annotation_producer_is_pinned_to_the_gap_kind() {
    for (path, expected) in PRODUCTION_GAP_ANNOTATION_PRODUCERS {
        let expected = expected
            .iter()
            .map(|argument| (*argument).to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            gap_annotation_producer_arguments(&read(path)),
            expected,
            "{path}: the Gap-kind annotation producers changed. Reclassifying one as an \
             observation inflates reported confidence, because `agent_gap_notes` then drops it \
             and the packet keeps its high/ready verdict; adding one needs a behavioural test \
             beside this inventory."
        );
    }

    // Fail closed: no production file may classify an annotation as a gap without being pinned
    // above, so a producer moved into a new module cannot escape the inventory.
    let pinned = PRODUCTION_GAP_ANNOTATION_PRODUCERS
        .iter()
        .map(|(path, _)| (*path).to_string())
        .collect::<BTreeSet<_>>();
    let mut discovered = BTreeSet::new();
    for member in workspace_members() {
        let source_dir = repo_root().join(&member).join("src");
        if !source_dir.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect_rs_files(&source_dir, &mut files);
        for file in files {
            let relative = file
                .strip_prefix(repo_root())
                .expect("workspace-relative path")
                .to_string_lossy()
                .replace('\\', "/");
            let source = fs::read_to_string(&file).expect("read source");
            let shipped = production_source(&source);

            // The inventory scans two entry points. Keep them the *only* two: a struct literal
            // or a bare `RetrievalAnnotationKindDto::Gap` outside the DTO would mint a gap the
            // scan cannot see, and the pin above would quietly stop covering it.
            assert!(
                !shipped.contains("RetrievalAnnotationDto {")
                    || relative == "crates/codestory-contracts/src/api/dto.rs",
                "{relative} builds a RetrievalAnnotationDto by struct literal; annotations must \
                 be minted through RetrievalAnnotationDto::gap / ::observation so the gap \
                 inventory can see them"
            );
            assert!(
                !shipped.contains("RetrievalAnnotationKindDto::Gap")
                    || matches!(
                        relative.as_str(),
                        "crates/codestory-contracts/src/api/dto.rs"
                            | "crates/codestory-cli/src/output.rs"
                    ),
                "{relative} names RetrievalAnnotationKindDto::Gap outside the DTO constructors \
                 and the single confidence consumer"
            );

            if gap_annotation_producer_arguments(&source).is_empty() {
                continue;
            }
            discovered.insert(relative);
        }
    }
    assert_eq!(
        discovered, pinned,
        "a production file emits Gap-kind annotations without being pinned in \
         PRODUCTION_GAP_ANNOTATION_PRODUCERS"
    );
}

/// Repo-text search must not be disabled by a constant on any search path.
///
/// The sidecar path -- the one the MCP tool surface and the packet both use -- once built
/// its results with `let repo_text_hits = Vec::new();` and `repo_text_enabled: false`,
/// while still honouring the caller's `repo_text` argument everywhere else. The tool
/// accepted the request, returned nothing, and advised running an index refresh to restore
/// a mode the caller was already in. It went unnoticed because nothing asserted the field:
/// across a 54-row benchmark, agents asked for repo text on 582 of 582 searches and every
/// one of the 569 that completed reported a zero count.
///
/// A literal `false` here is indistinguishable from "this path does not support repo text",
/// which is why the regression was invisible. The flag has to follow the requested mode.
#[test]
fn search_paths_do_not_hardcode_repo_text_disabled() {
    let source = read("crates/codestory-runtime/src/search_plan.rs");
    assert!(
        !source.contains("repo_text_enabled: false"),
        "search_plan.rs disables repo text with a constant; it must follow the requested \
         SearchRepoTextMode so a caller that asks for literal matches receives them"
    );
    assert!(
        source.contains("let repo_text_enabled = repo_text_mode != SearchRepoTextMode::Off;"),
        "the sidecar search path must derive repo_text_enabled from the requested mode"
    );
}
