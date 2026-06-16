use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{Edge, EdgeKind, Node, NodeKind};
use codestory_indexer::WorkspaceIndexer;
use codestory_store::Store as Storage;
use std::collections::HashMap;
use std::fs;
use tempfile::tempdir;

const PYTHON_SOURCE: &str = include_str!("fixtures/fidelity_lab/python_fidelity_lab.py");
const TYPESCRIPT_SOURCE: &str = include_str!("fixtures/fidelity_lab/typescript_fidelity_lab.ts");
const JAVASCRIPT_SOURCE: &str = include_str!("fixtures/fidelity_lab/javascript_fidelity_lab.js");
const JAVA_SOURCE: &str = include_str!("fixtures/fidelity_lab/java_fidelity_lab.java");
const CPP_SOURCE: &str = include_str!("fixtures/fidelity_lab/cpp_fidelity_lab.cpp");
const C_SOURCE: &str = include_str!("fixtures/fidelity_lab/c_fidelity_lab.c");
const RUST_SOURCE: &str = include_str!("fixtures/fidelity_lab/rust_fidelity_lab.rs");
const GO_SOURCE: &str = include_str!("fixtures/fidelity_lab/go_fidelity_lab.go");
const RUBY_SOURCE: &str = include_str!("fixtures/fidelity_lab/ruby_fidelity_lab.rb");
const PHP_SOURCE: &str = include_str!("fixtures/fidelity_lab/php_fidelity_lab.php");
const CSHARP_SOURCE: &str = include_str!("fixtures/fidelity_lab/csharp_fidelity_lab.cs");
const KOTLIN_SOURCE: &str = include_str!("fixtures/fidelity_lab/kotlin_fidelity_lab.kt");
const SWIFT_SOURCE: &str = include_str!("fixtures/fidelity_lab/swift_fidelity_lab.swift");
const DART_SOURCE: &str = include_str!("fixtures/fidelity_lab/dart_fidelity_lab.dart");
const BASH_SOURCE: &str = include_str!("fixtures/fidelity_lab/bash_fidelity_lab.sh");

type ResolvedOwnerExpectation = (&'static str, &'static str, &'static str);
type ResolvedNameExpectation = (&'static str, &'static str);
type MemberExpectation = (&'static str, &'static str);

struct FidelityCase {
    language: &'static str,
    filename: &'static str,
    source: &'static str,
    min_nodes: usize,
    min_call_edges: usize,
    min_import_edges: usize,
    required_symbols: &'static [&'static str],
    required_call_targets: &'static [&'static str],
    required_import_fragments: &'static [&'static str],
    required_member_pairs: &'static [MemberExpectation],
    min_resolved_calls: usize,
    expected_resolved_owners: &'static [ResolvedOwnerExpectation],
    expected_resolved_names: &'static [ResolvedNameExpectation],
}

const PYTHON_SYMBOLS: &[&str] = &[
    "MAX_RETRIES",
    "trace",
    "Notifier",
    "ConsoleNotifier",
    "Repository",
    "Event",
    "Workflow",
    "run",
    "run_async",
    "orchestrate",
];
const TYPESCRIPT_SYMBOLS: &[&str] = &[
    "WorkflowMode",
    "Notifier",
    "ConsoleNotifier",
    "Repository",
    "Workflow",
    "run",
    "runAsync",
    "orchestrateTs",
];
const JAVASCRIPT_SYMBOLS: &[&str] = &[
    "Notifier",
    "ConsoleNotifier",
    "Repository",
    "Workflow",
    "run",
    "runAsync",
    "orchestrateJs",
];
const JAVA_SYMBOLS: &[&str] = &[
    "Notifier",
    "ConsoleNotifier",
    "Repository",
    "Event",
    "Workflow",
    "run",
    "runAsync",
    "orchestrateJava",
];
const CPP_SYMBOLS: &[&str] = &[
    "Notifier",
    "ConsoleNotifier",
    "Repository",
    "Event",
    "Workflow",
    "run",
    "runAsync",
    "orchestrate_cpp",
];
const C_SYMBOLS: &[&str] = &[
    "Event",
    "Notifier",
    "Repository",
    "repository_track",
    "repository_save",
    "workflow_run",
    "orchestrate_c",
];
const RUST_SYMBOLS: &[&str] = &[
    "Notifier",
    "ConsoleNotifier",
    "Event",
    "Repository",
    "MemoryRepository",
    "Workflow",
    "run",
    "run_async",
    "orchestrate_rust",
];
const GO_SYMBOLS: &[&str] = &[
    "Notifier",
    "ConsoleNotifier",
    "Repository",
    "Event",
    "Workflow",
    "Notify",
    "Save",
    "Run",
    "decorate",
    "orchestrateGo",
];
const RUBY_SYMBOLS: &[&str] = &[
    "Notifier",
    "ConsoleNotifier",
    "Repository",
    "Workflow",
    "notify",
    "save",
    "run",
    "decorate",
    "orchestrate_ruby",
];
const PHP_SYMBOLS: &[&str] = &[
    "Notifier",
    "ConsoleNotifier",
    "Repository",
    "Event",
    "Workflow",
    "notify",
    "save",
    "run",
    "decorate",
    "orchestrate_php",
];
const CSHARP_SYMBOLS: &[&str] = &[
    "INotifier",
    "ConsoleNotifier",
    "Repository",
    "Event",
    "Workflow",
    "Program",
    "Notify",
    "Save",
    "Run",
    "Decorate",
    "Main",
];
const KOTLIN_SYMBOLS: &[&str] = &[
    "Notifier",
    "ConsoleNotifier",
    "Repository",
    "Event",
    "Workflow",
    "notify",
    "save",
    "run",
    "decorate",
    "orchestrateKotlin",
];
const SWIFT_SYMBOLS: &[&str] = &[
    "Notifier",
    "ConsoleNotifier",
    "Repository",
    "Event",
    "Workflow",
    "notify",
    "save",
    "run",
    "decorate",
    "orchestrateSwift",
];
const DART_SYMBOLS: &[&str] = &[
    "Notifier",
    "ConsoleNotifier",
    "Repository",
    "Event",
    "Workflow",
    "notify",
    "save",
    "run",
    "decorate",
    "orchestrateDart",
];
const BASH_SYMBOLS: &[&str] = &[
    "notify",
    "save",
    "decorate",
    "run",
    "orchestrate_bash",
    "event",
];

const PYTHON_CALLS: &[&str] = &["notify", "save", "decorate", "run"];
const TYPESCRIPT_CALLS: &[&str] = &["identity", "notify", "save", "decorate", "run"];
const JAVASCRIPT_CALLS: &[&str] = &["identity", "notify", "save", "decorate", "run"];
const JAVA_CALLS: &[&str] = &["identity", "notifyEvent", "save", "decorate", "run"];
const CPP_CALLS: &[&str] = &["identity", "notifyEvent", "save", "decorate", "run"];
const C_CALLS: &[&str] = &["repository_track", "workflow_run"];
const RUST_CALLS: &[&str] = &["identity", "notify", "save", "decorate", "run"];
const GO_CALLS: &[&str] = &["Notify", "Save", "decorate", "Run"];
const RUBY_CALLS: &[&str] = &["notify", "save", "decorate", "run"];
const PHP_CALLS: &[&str] = &["notify", "save", "decorate", "run"];
const CSHARP_CALLS: &[&str] = &["Notify", "Save", "Decorate", "Run"];
const KOTLIN_CALLS: &[&str] = &["notify", "save", "decorate", "run"];
const SWIFT_CALLS: &[&str] = &["notify", "save", "decorate"];
const DART_CALLS: &[&str] = &["notify", "save", "decorate"];
const BASH_CALLS: &[&str] = &["notify", "save", "decorate", "run"];

const PYTHON_IMPORTS: &[&str] = &[];
const TYPESCRIPT_IMPORTS: &[&str] = &["fs", "path"];
const JAVASCRIPT_IMPORTS: &[&str] = &["fs", "path"];
const JAVA_IMPORTS: &[&str] = &["java.util.concurrent", "java.util.function"];
const CPP_IMPORTS: &[&str] = &["future", "functional", "string"];
const C_IMPORTS: &[&str] = &["stdio", "string", "stddef"];
const RUST_IMPORTS: &[&str] = &["std::collections", "std::future"];
const GO_IMPORTS: &[&str] = &["fmt"];
const RUBY_IMPORTS: &[&str] = &["logger"];
const PHP_IMPORTS: &[&str] = &["Random\\Randomizer"];
const CSHARP_IMPORTS: &[&str] = &["System"];
const KOTLIN_IMPORTS: &[&str] = &["kotlin.math.abs"];
const SWIFT_IMPORTS: &[&str] = &["Foundation"];
const DART_IMPORTS: &[&str] = &["dart:math"];
const BASH_IMPORTS: &[&str] = &["./logger.sh"];

const PYTHON_MEMBERS: &[MemberExpectation] = &[];
const TYPESCRIPT_MEMBERS: &[MemberExpectation] = &[
    ("ConsoleNotifier", "notify"),
    ("Repository", "save"),
    ("Workflow", "run"),
];
const JAVASCRIPT_MEMBERS: &[MemberExpectation] = &[
    ("ConsoleNotifier", "notify"),
    ("Repository", "save"),
    ("Workflow", "run"),
];
const JAVA_MEMBERS: &[MemberExpectation] = &[
    ("Notifier", "notifyEvent"),
    ("ConsoleNotifier", "notifyEvent"),
    ("Repository", "save"),
    ("Workflow", "run"),
];
const CPP_MEMBERS: &[MemberExpectation] = &[
    ("Notifier", "notifyEvent"),
    ("ConsoleNotifier", "notifyEvent"),
    ("Repository", "save"),
    ("Workflow", "run"),
];
const C_MEMBERS: &[MemberExpectation] = &[];
const RUST_MEMBERS: &[MemberExpectation] = &[
    ("ConsoleNotifier", "notify"),
    ("MemoryRepository", "save"),
    ("Workflow", "run"),
];
const GO_MEMBERS: &[MemberExpectation] = &[
    ("Notifier", "Notify"),
    ("ConsoleNotifier", "Notify"),
    ("Repository", "Save"),
    ("Workflow", "Run"),
];
const RUBY_MEMBERS: &[MemberExpectation] = &[
    ("Notifier", "notify"),
    ("ConsoleNotifier", "notify"),
    ("Repository", "save"),
    ("Workflow", "run"),
    ("Workflow", "decorate"),
];
const PHP_MEMBERS: &[MemberExpectation] = &[
    ("Notifier", "notify"),
    ("ConsoleNotifier", "notify"),
    ("Repository", "save"),
    ("Workflow", "run"),
    ("Workflow", "decorate"),
];
const CSHARP_MEMBERS: &[MemberExpectation] = &[
    ("INotifier", "Notify"),
    ("ConsoleNotifier", "Notify"),
    ("Repository", "Save"),
    ("Workflow", "Run"),
    ("Workflow", "Decorate"),
    ("Program", "Main"),
];
const KOTLIN_MEMBERS: &[MemberExpectation] = &[
    ("Notifier", "notify"),
    ("ConsoleNotifier", "notify"),
    ("Repository", "save"),
    ("Workflow", "run"),
    ("Workflow", "decorate"),
];
const SWIFT_MEMBERS: &[MemberExpectation] = &[
    ("Notifier", "notify"),
    ("ConsoleNotifier", "notify"),
    ("Repository", "save"),
    ("Workflow", "run"),
    ("Workflow", "decorate"),
];
const DART_MEMBERS: &[MemberExpectation] = &[
    ("Notifier", "notify"),
    ("ConsoleNotifier", "notify"),
    ("Repository", "save"),
    ("Workflow", "run"),
    ("Workflow", "decorate"),
];
const BASH_MEMBERS: &[MemberExpectation] = &[];

const PYTHON_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("notify", "ConsoleNotifier", "write_log"),
    ("save", "Repository", "track"),
    ("run", "Notifier", "notify"),
    ("run", "Repository", "save"),
    ("run", "Workflow", "decorate"),
    ("run_async", "Workflow", "run"),
];
const TYPESCRIPT_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("notify", "ConsoleNotifier", "writeLog"),
    ("save", "Repository", "track"),
    ("run", "Notifier", "notify"),
    ("run", "Repository", "save"),
    ("run", "Workflow", "identity"),
    ("run", "Workflow", "decorate"),
    ("runAsync", "Workflow", "run"),
    ("orchestrateTs", "Workflow", "run"),
];
const JAVASCRIPT_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("notify", "ConsoleNotifier", "writeLog"),
    ("save", "Repository", "track"),
    ("run", "Workflow", "identity"),
    ("run", "Workflow", "decorate"),
    ("runAsync", "Workflow", "run"),
    ("orchestrateJs", "Workflow", "run"),
];
const JAVA_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("run", "Notifier", "notifyEvent"),
    ("run", "Repository", "save"),
    ("orchestrateJava", "Workflow", "run"),
];
const CPP_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("run", "Notifier", "notifyEvent"),
    ("run", "Repository", "save"),
    ("orchestrate_cpp", "Workflow", "run"),
];
const C_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[];
const C_RESOLVED_NAMES: &[ResolvedNameExpectation] = &[
    ("repository_save", "repository_track"),
    ("orchestrate_c", "workflow_run"),
];
const RUST_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("run", "Notifier", "notify"),
    ("run", "Repository", "save"),
    ("orchestrate_rust", "Workflow", "run"),
];
const GO_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("Run", "Notifier", "Notify"),
    ("Run", "Repository", "Save"),
    ("orchestrateGo", "Workflow", "Run"),
];
const RUBY_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("run", "Workflow", "decorate"),
    ("orchestrate_ruby", "Workflow", "run"),
];
const PHP_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("run", "Notifier", "notify"),
    ("run", "Repository", "save"),
    ("run", "Workflow", "decorate"),
    ("orchestrate_php", "Workflow", "run"),
];
const CSHARP_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("Run", "INotifier", "Notify"),
    ("Run", "Repository", "Save"),
    ("Run", "Workflow", "Decorate"),
    ("Main", "Workflow", "Run"),
];
const KOTLIN_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("run", "Notifier", "notify"),
    ("run", "Repository", "save"),
    ("orchestrateKotlin", "Workflow", "run"),
];
const SWIFT_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("run", "Notifier", "notify"),
    ("run", "Repository", "save"),
    ("orchestrateSwift", "Workflow", "run"),
];
const DART_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("run", "Notifier", "notify"),
    ("run", "Repository", "save"),
    ("orchestrateDart", "Workflow", "run"),
];
const BASH_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[];
const BASH_RESOLVED_NAMES: &[ResolvedNameExpectation] = &[
    ("run", "notify"),
    ("run", "save"),
    ("run", "decorate"),
    ("orchestrate_bash", "run"),
];

const EMPTY_RESOLVED_NAMES: &[ResolvedNameExpectation] = &[];

fn fidelity_cases() -> Vec<FidelityCase> {
    vec![
        FidelityCase {
            language: "python",
            filename: "fidelity.py",
            source: PYTHON_SOURCE,
            min_nodes: 16,
            min_call_edges: 8,
            min_import_edges: 1,
            required_symbols: PYTHON_SYMBOLS,
            required_call_targets: PYTHON_CALLS,
            required_import_fragments: PYTHON_IMPORTS,
            required_member_pairs: PYTHON_MEMBERS,
            min_resolved_calls: 6,
            expected_resolved_owners: PYTHON_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "typescript",
            filename: "fidelity.ts",
            source: TYPESCRIPT_SOURCE,
            min_nodes: 14,
            min_call_edges: 8,
            min_import_edges: 2,
            required_symbols: TYPESCRIPT_SYMBOLS,
            required_call_targets: TYPESCRIPT_CALLS,
            required_import_fragments: TYPESCRIPT_IMPORTS,
            required_member_pairs: TYPESCRIPT_MEMBERS,
            min_resolved_calls: 8,
            expected_resolved_owners: TYPESCRIPT_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "javascript",
            filename: "fidelity.js",
            source: JAVASCRIPT_SOURCE,
            min_nodes: 12,
            min_call_edges: 8,
            min_import_edges: 2,
            required_symbols: JAVASCRIPT_SYMBOLS,
            required_call_targets: JAVASCRIPT_CALLS,
            required_import_fragments: JAVASCRIPT_IMPORTS,
            required_member_pairs: JAVASCRIPT_MEMBERS,
            min_resolved_calls: 6,
            expected_resolved_owners: JAVASCRIPT_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "java",
            filename: "Fidelity.java",
            source: JAVA_SOURCE,
            min_nodes: 14,
            min_call_edges: 8,
            min_import_edges: 2,
            required_symbols: JAVA_SYMBOLS,
            required_call_targets: JAVA_CALLS,
            required_import_fragments: JAVA_IMPORTS,
            required_member_pairs: JAVA_MEMBERS,
            min_resolved_calls: 3,
            expected_resolved_owners: JAVA_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "cpp",
            filename: "fidelity.cpp",
            source: CPP_SOURCE,
            min_nodes: 12,
            min_call_edges: 8,
            min_import_edges: 3,
            required_symbols: CPP_SYMBOLS,
            required_call_targets: CPP_CALLS,
            required_import_fragments: CPP_IMPORTS,
            required_member_pairs: CPP_MEMBERS,
            min_resolved_calls: 3,
            expected_resolved_owners: CPP_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "c",
            filename: "fidelity.c",
            source: C_SOURCE,
            min_nodes: 8,
            min_call_edges: 4,
            min_import_edges: 3,
            required_symbols: C_SYMBOLS,
            required_call_targets: C_CALLS,
            required_import_fragments: C_IMPORTS,
            required_member_pairs: C_MEMBERS,
            min_resolved_calls: 2,
            expected_resolved_owners: C_RESOLVED_OWNERS,
            expected_resolved_names: C_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "rust",
            filename: "fidelity.rs",
            source: RUST_SOURCE,
            min_nodes: 14,
            min_call_edges: 8,
            min_import_edges: 2,
            required_symbols: RUST_SYMBOLS,
            required_call_targets: RUST_CALLS,
            required_import_fragments: RUST_IMPORTS,
            required_member_pairs: RUST_MEMBERS,
            min_resolved_calls: 3,
            expected_resolved_owners: RUST_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "go",
            filename: "fidelity.go",
            source: GO_SOURCE,
            min_nodes: 12,
            min_call_edges: 4,
            min_import_edges: 1,
            required_symbols: GO_SYMBOLS,
            required_call_targets: GO_CALLS,
            required_import_fragments: GO_IMPORTS,
            required_member_pairs: GO_MEMBERS,
            min_resolved_calls: 3,
            expected_resolved_owners: GO_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "ruby",
            filename: "fidelity.rb",
            source: RUBY_SOURCE,
            min_nodes: 12,
            min_call_edges: 4,
            min_import_edges: 1,
            required_symbols: RUBY_SYMBOLS,
            required_call_targets: RUBY_CALLS,
            required_import_fragments: RUBY_IMPORTS,
            required_member_pairs: RUBY_MEMBERS,
            min_resolved_calls: 2,
            expected_resolved_owners: RUBY_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "php",
            filename: "fidelity.php",
            source: PHP_SOURCE,
            min_nodes: 12,
            min_call_edges: 4,
            min_import_edges: 1,
            required_symbols: PHP_SYMBOLS,
            required_call_targets: PHP_CALLS,
            required_import_fragments: PHP_IMPORTS,
            required_member_pairs: PHP_MEMBERS,
            min_resolved_calls: 4,
            expected_resolved_owners: PHP_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "csharp",
            filename: "fidelity.cs",
            source: CSHARP_SOURCE,
            min_nodes: 12,
            min_call_edges: 4,
            min_import_edges: 1,
            required_symbols: CSHARP_SYMBOLS,
            required_call_targets: CSHARP_CALLS,
            required_import_fragments: CSHARP_IMPORTS,
            required_member_pairs: CSHARP_MEMBERS,
            min_resolved_calls: 4,
            expected_resolved_owners: CSHARP_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "kotlin",
            filename: "fidelity.kt",
            source: KOTLIN_SOURCE,
            min_nodes: 10,
            min_call_edges: 4,
            min_import_edges: 1,
            required_symbols: KOTLIN_SYMBOLS,
            required_call_targets: KOTLIN_CALLS,
            required_import_fragments: KOTLIN_IMPORTS,
            required_member_pairs: KOTLIN_MEMBERS,
            min_resolved_calls: 3,
            expected_resolved_owners: KOTLIN_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "swift",
            filename: "fidelity.swift",
            source: SWIFT_SOURCE,
            min_nodes: 10,
            min_call_edges: 3,
            min_import_edges: 1,
            required_symbols: SWIFT_SYMBOLS,
            required_call_targets: SWIFT_CALLS,
            required_import_fragments: SWIFT_IMPORTS,
            required_member_pairs: SWIFT_MEMBERS,
            min_resolved_calls: 3,
            expected_resolved_owners: SWIFT_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "dart",
            filename: "fidelity.dart",
            source: DART_SOURCE,
            min_nodes: 10,
            min_call_edges: 3,
            min_import_edges: 1,
            required_symbols: DART_SYMBOLS,
            required_call_targets: DART_CALLS,
            required_import_fragments: DART_IMPORTS,
            required_member_pairs: DART_MEMBERS,
            min_resolved_calls: 3,
            expected_resolved_owners: DART_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
        },
        FidelityCase {
            language: "bash",
            filename: "fidelity.sh",
            source: BASH_SOURCE,
            min_nodes: 6,
            min_call_edges: 4,
            min_import_edges: 1,
            required_symbols: BASH_SYMBOLS,
            required_call_targets: BASH_CALLS,
            required_import_fragments: BASH_IMPORTS,
            required_member_pairs: BASH_MEMBERS,
            min_resolved_calls: 4,
            expected_resolved_owners: BASH_RESOLVED_OWNERS,
            expected_resolved_names: BASH_RESOLVED_NAMES,
        },
    ]
}

fn index_single_file(filename: &str, contents: &str) -> anyhow::Result<(Vec<Node>, Vec<Edge>)> {
    let dir = tempdir()?;
    let root = dir.path();
    let file_path = root.join(filename);
    fs::write(&file_path, contents)?;

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();

    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![file_path],
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    let errors = storage.get_errors(None)?;
    anyhow::ensure!(
        errors.is_empty(),
        "Indexing errors for `{filename}`: {errors:?}"
    );

    Ok((storage.get_nodes()?, storage.get_edges()?))
}

fn is_matching_name(serialized_name: &str, wanted_name: &str) -> bool {
    if serialized_name == wanted_name
        || serialized_name.starts_with(&format!("{wanted_name}<"))
        || serialized_name.ends_with(&format!(".{wanted_name}"))
        || serialized_name.ends_with(&format!("::{wanted_name}"))
    {
        return true;
    }

    let dotted_tail = serialized_name
        .rsplit_once('.')
        .map(|(_, tail)| tail)
        .unwrap_or(serialized_name);
    if dotted_tail == wanted_name || dotted_tail.starts_with(&format!("{wanted_name}<")) {
        return true;
    }

    let rust_tail = serialized_name
        .rsplit_once("::")
        .map(|(_, tail)| tail)
        .unwrap_or(serialized_name);
    rust_tail == wanted_name || rust_tail.starts_with(&format!("{wanted_name}<"))
}

fn is_matching_owned_method(serialized_name: &str, owner: &str, method: &str) -> bool {
    serialized_name == format!("{owner}.{method}")
        || serialized_name == format!("{owner}::{method}")
        || serialized_name.ends_with(&format!(".{owner}.{method}"))
        || serialized_name.ends_with(&format!("::{owner}::{method}"))
}

fn has_node_name(nodes: &[Node], target_name: &str) -> bool {
    nodes
        .iter()
        .any(|node| is_matching_name(&node.serialized_name, target_name))
}

fn has_node_with_kind(nodes: &[Node], target_kind: NodeKind, target_name: &str) -> bool {
    nodes.iter().any(|node| {
        node.kind == target_kind && is_matching_name(&node.serialized_name, target_name)
    })
}

fn has_call_target_name(edges: &[Edge], nodes: &[Node], target_name: &str) -> bool {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .any(|node| is_matching_name(&node.serialized_name, target_name))
}

fn has_call_from_owner_to_target(
    edges: &[Edge],
    nodes: &[Node],
    owner_name: &str,
    target_name: &str,
) -> bool {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .any(|edge| {
            let Some(source) = node_by_id.get(&edge.source) else {
                return false;
            };
            let Some(target) = node_by_id.get(&edge.target) else {
                return false;
            };
            is_matching_name(&source.serialized_name, owner_name)
                && is_matching_name(&target.serialized_name, target_name)
        })
}

fn has_edge_between_names(
    edges: &[Edge],
    nodes: &[Node],
    kind: EdgeKind,
    source_name: &str,
    target_name: &str,
) -> bool {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    edges.iter().filter(|edge| edge.kind == kind).any(|edge| {
        let Some(source) = node_by_id.get(&edge.source) else {
            return false;
        };
        let Some(target) = node_by_id.get(&edge.target) else {
            return false;
        };
        is_matching_name(&source.serialized_name, source_name)
            && is_matching_name(&target.serialized_name, target_name)
    })
}

fn has_import_target_fragment(edges: &[Edge], nodes: &[Node], target_fragment: &str) -> bool {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::IMPORT)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .any(|node| node.serialized_name.contains(target_fragment))
}

fn assert_no_self_call_edges(case_name: &str, edges: &[Edge]) {
    let self_edges: Vec<_> = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == edge.target)
        .collect();
    assert!(
        self_edges.is_empty(),
        "Case `{case_name}`: persisted CALL self-edges should have been dropped, found {}",
        self_edges.len()
    );
}

fn describe_call_edges(edges: &[Edge], nodes: &[Node]) -> Vec<String> {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .map(|edge| {
            let source = node_by_id
                .get(&edge.source)
                .map(|n| n.serialized_name.clone())
                .unwrap_or_else(|| format!("<missing:{}>", edge.source.0));
            let unresolved_target = node_by_id
                .get(&edge.target)
                .map(|n| n.serialized_name.clone())
                .unwrap_or_else(|| format!("<missing:{}>", edge.target.0));
            let resolved = edge
                .resolved_target
                .and_then(|resolved_id| node_by_id.get(&resolved_id).copied())
                .map(|n| n.serialized_name.clone())
                .unwrap_or_else(|| "<none>".to_string());
            format!("{source} -> {unresolved_target} (resolved: {resolved})")
        })
        .collect()
}

fn assert_resolved_call_to_method_owner(
    case_name: &str,
    nodes: &[Node],
    edges: &[Edge],
    caller_name: &str,
    owner_name: &str,
    method_name: &str,
) {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let found = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter_map(|edge| {
            let source = node_by_id.get(&edge.source)?;
            if !is_matching_name(&source.serialized_name, caller_name) {
                return None;
            }
            let resolved_id = edge.resolved_target?;
            let resolved_node = node_by_id.get(&resolved_id)?;
            Some(resolved_node.serialized_name.as_str())
        })
        .any(|resolved_name| is_matching_owned_method(resolved_name, owner_name, method_name));

    assert!(
        found,
        "Case `{case_name}`: expected CALL from `{caller_name}` to resolve to `{owner_name}::{method_name}`. Calls: {:?}",
        describe_call_edges(edges, nodes)
    );
}

fn assert_resolved_call_to_name(
    case_name: &str,
    nodes: &[Node],
    edges: &[Edge],
    caller_name: &str,
    callee_name: &str,
) {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let found = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter_map(|edge| {
            let source = node_by_id.get(&edge.source)?;
            if !is_matching_name(&source.serialized_name, caller_name) {
                return None;
            }
            let resolved_id = edge.resolved_target?;
            let resolved_node = node_by_id.get(&resolved_id)?;
            Some(resolved_node.serialized_name.as_str())
        })
        .any(|resolved_name| is_matching_name(resolved_name, callee_name));

    assert!(
        found,
        "Case `{case_name}`: expected CALL from `{caller_name}` to resolve to `{callee_name}`. Calls: {:?}",
        describe_call_edges(edges, nodes)
    );
}

#[test]
fn test_fidelity_lab_graph_shape_and_semantics() -> anyhow::Result<()> {
    for case in fidelity_cases() {
        let (nodes, edges) = index_single_file(case.filename, case.source)?;

        assert!(
            nodes.len() >= case.min_nodes,
            "Case `{}`: expected at least {} nodes, got {}",
            case.language,
            case.min_nodes,
            nodes.len()
        );

        let call_edges: Vec<_> = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::CALL)
            .collect();
        assert!(
            call_edges.len() >= case.min_call_edges,
            "Case `{}`: expected at least {} CALL edges, got {}",
            case.language,
            case.min_call_edges,
            call_edges.len()
        );
        assert_no_self_call_edges(case.language, &edges);

        let import_edges: Vec<_> = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::IMPORT)
            .collect();
        assert!(
            import_edges.len() >= case.min_import_edges,
            "Case `{}`: expected at least {} IMPORT edges, got {}",
            case.language,
            case.min_import_edges,
            import_edges.len()
        );

        for symbol in case.required_symbols {
            assert!(
                has_node_name(&nodes, symbol),
                "Case `{}`: missing symbol node `{}`",
                case.language,
                symbol
            );
        }

        for target in case.required_call_targets {
            assert!(
                has_call_target_name(&edges, &nodes, target),
                "Case `{}`: missing CALL edge target `{}`",
                case.language,
                target
            );
        }

        for import_fragment in case.required_import_fragments {
            assert!(
                has_import_target_fragment(&edges, &nodes, import_fragment),
                "Case `{}`: missing IMPORT target containing `{}`",
                case.language,
                import_fragment
            );
        }

        for (owner, member) in case.required_member_pairs {
            assert!(
                has_edge_between_names(&edges, &nodes, EdgeKind::MEMBER, owner, member),
                "Case `{}`: missing MEMBER edge `{}` -> `{}`",
                case.language,
                owner,
                member
            );
        }

        assert!(
            edges
                .iter()
                .filter(|edge| edge.kind == EdgeKind::CALL)
                .all(|edge| edge.source != edge.target),
            "Case `{}`: CALL graph contains self-edge markers: {:?}",
            case.language,
            describe_call_edges(&edges, &nodes)
        );

        let resolved_calls = call_edges
            .iter()
            .filter(|edge| edge.resolved_target.is_some())
            .count();
        assert!(
            resolved_calls >= case.min_resolved_calls,
            "Case `{}`: expected at least {} resolved CALL edges, got {}. Calls: {:?}",
            case.language,
            case.min_resolved_calls,
            resolved_calls,
            describe_call_edges(&edges, &nodes)
        );

        for (caller, owner, method) in case.expected_resolved_owners {
            assert_resolved_call_to_method_owner(
                case.language,
                &nodes,
                &edges,
                caller,
                owner,
                method,
            );
        }

        for (caller, callee) in case.expected_resolved_names {
            assert_resolved_call_to_name(case.language, &nodes, &edges, caller, callee);
        }
    }

    Ok(())
}

#[test]
fn test_context_agnostic_calls_bind_to_enclosing_callable_without_self_edges() -> anyhow::Result<()>
{
    let cases = [
        (
            "python",
            "context.py",
            r#"
def callee() -> int:
    return 1

def caller(flag: bool) -> int:
    value = callee()
    if callee():
        while value < 0:
            callee()
    return callee() if flag else value
"#,
            "caller",
            "callee",
            3usize,
        ),
        (
            "typescript",
            "context.ts",
            r#"
function callee(): number { return 1; }

function caller(flag: boolean): number {
    const value = callee();
    if (callee()) {
        while (value < 0) {
            callee();
        }
    }
    switch (value) {
        case 0:
            return callee();
        default:
            return flag ? callee() : value;
    }
}
"#,
            "caller",
            "callee",
            5usize,
        ),
        (
            "javascript",
            "context.js",
            r#"
function callee() { return 1; }

function caller(flag) {
    const value = callee();
    if (callee()) {
        while (value < 0) {
            callee();
        }
    }
    switch (value) {
        case 0:
            return callee();
        default:
            return flag ? callee() : value;
    }
}
"#,
            "caller",
            "callee",
            5usize,
        ),
        (
            "cpp",
            "context.cpp",
            r#"
int callee() { return 1; }

int caller(bool flag) {
    int value = callee();
    if (callee()) {
        while (value < 0) {
            callee();
        }
    }
    switch (value) {
        case 0:
            return callee();
        default:
            return flag ? callee() : value;
    }
}
"#,
            "caller",
            "callee",
            5usize,
        ),
        (
            "c",
            "context.c",
            r#"
int callee(void) { return 1; }

int caller(int flag) {
    int value = callee();
    if (callee()) {
        while (value < 0) {
            callee();
        }
    }
    switch (value) {
        case 0:
            return callee();
        default:
            return flag ? callee() : value;
    }
}
"#,
            "caller",
            "callee",
            5usize,
        ),
        (
            "rust",
            "context.rs",
            r#"
fn callee() -> bool { true }

fn caller(flag: bool) -> bool {
    let value = callee();
    if callee() {
        while false {
            callee();
        }
    }
    match value {
        true => callee(),
        false => flag,
    }
}
"#,
            "caller",
            "callee",
            3usize,
        ),
    ];

    for (case_name, filename, source, caller_name, callee_name, min_call_count) in cases {
        let (nodes, edges) = index_single_file(filename, source)?;
        assert_no_self_call_edges(case_name, &edges);

        let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
        let caller_edges: Vec<_> = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::CALL)
            .filter(|edge| {
                node_by_id
                    .get(&edge.source)
                    .is_some_and(|node| is_matching_name(&node.serialized_name, caller_name))
            })
            .collect();

        assert!(
            caller_edges.len() >= min_call_count,
            "Case `{case_name}`: expected at least {min_call_count} attributed CALL edges from `{caller_name}`, got {}. Calls: {:?}",
            caller_edges.len(),
            describe_call_edges(&edges, &nodes)
        );

        let target_hits = caller_edges
            .iter()
            .filter_map(|edge| node_by_id.get(&edge.target))
            .filter(|node| is_matching_name(&node.serialized_name, callee_name))
            .count();
        assert!(
            target_hits >= min_call_count,
            "Case `{case_name}`: expected `{caller_name}` to own at least {min_call_count} CALL edges to `{callee_name}`, got {target_hits}. Calls: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }

    Ok(())
}

#[test]
fn test_fidelity_lab_resolved_targets_point_to_existing_nodes() -> anyhow::Result<()> {
    for case in fidelity_cases() {
        let (nodes, edges) = index_single_file(case.filename, case.source)?;
        let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();

        for edge in edges.iter().filter(|edge| edge.kind == EdgeKind::CALL) {
            if let Some(resolved_id) = edge.resolved_target {
                assert!(
                    node_by_id.contains_key(&resolved_id),
                    "Case `{}`: resolved CALL points to missing node id {:?}",
                    case.language,
                    resolved_id
                );
            }
        }
    }

    Ok(())
}

#[test]
fn test_nested_call_attribution_follows_enclosing_callable() -> anyhow::Result<()> {
    for (filename, source, caller, callee) in [
        ("nested.py", PYTHON_SOURCE, "decorate", "sqrt"),
        ("nested.ts", TYPESCRIPT_SOURCE, "run", "identity"),
        ("nested.js", JAVASCRIPT_SOURCE, "run", "identity"),
        ("nested.rs", RUST_SOURCE, "run", "identity"),
        ("nested.cpp", CPP_SOURCE, "run", "identity"),
        ("nested.c", C_SOURCE, "repository_track", "ALIAS_LEN"),
    ] {
        let (nodes, edges) = index_single_file(filename, source)?;
        assert!(
            has_call_from_owner_to_target(&edges, &nodes, caller, callee),
            "Expected `{caller}` to own CALL edge to `{callee}`. Calls: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }

    Ok(())
}

#[test]
fn test_rust_struct_enum_and_local_bindings_are_indexed() -> anyhow::Result<()> {
    let (nodes, _) = index_single_file("locals.rs", RUST_SOURCE)?;
    assert!(has_node_name(&nodes, "MemoryRepository"));
    assert!(has_node_name(&nodes, "Event"));
    assert!(has_node_name(&nodes, "ConsoleNotifier"));
    assert!(has_node_with_kind(&nodes, NodeKind::VARIABLE, "mapped"));
    Ok(())
}

#[test]
fn test_rust_enum_variants_are_indexed_as_owned_constants() -> anyhow::Result<()> {
    let source = r#"
enum Subcommand {
    Exec(ExecCli),
    Review(ReviewCommand),
}
"#;
    let (nodes, edges) = index_single_file("subcommand.rs", source)?;

    assert!(has_node_with_kind(
        &nodes,
        NodeKind::ENUM_CONSTANT,
        "Subcommand::Exec"
    ));
    assert!(has_node_with_kind(
        &nodes,
        NodeKind::ENUM_CONSTANT,
        "Subcommand::Review"
    ));
    assert!(has_edge_between_names(
        &edges,
        &nodes,
        EdgeKind::MEMBER,
        "Subcommand",
        "Subcommand::Exec"
    ));
    Ok(())
}

#[test]
fn test_python_module_constants_and_decorators_are_preserved() -> anyhow::Result<()> {
    let source = r#"
MAX_RETRIES = 3

def trace(fn):
    return fn

@trace
def run():
    return MAX_RETRIES
"#;
    let (nodes, edges) = index_single_file("constants.py", source)?;
    assert!(has_node_name(&nodes, "MAX_RETRIES"));
    assert!(has_edge_between_names(
        &edges,
        &nodes,
        EdgeKind::CALL,
        "run",
        "trace"
    ));
    Ok(())
}

#[test]
fn test_typescript_type_alias_and_enum_are_indexed() -> anyhow::Result<()> {
    let source = r#"
type Props = {
    label: string;
};

enum Tone {
    Primary = "primary",
}
"#;
    let (nodes, _) = index_single_file("types.ts", source)?;
    assert!(has_node_with_kind(&nodes, NodeKind::TYPEDEF, "Props"));
    assert!(has_node_with_kind(&nodes, NodeKind::ENUM, "Tone"));
    Ok(())
}
