use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{Edge, EdgeKind, Node, NodeKind};
use codestory_indexer::WorkspaceIndexer;
use codestory_store::Store as Storage;
use std::collections::{HashMap, HashSet};
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

        // Resolved CALL targets must be real nodes in this fixture, not
        // dangling ids — folded in from the deleted integrity-only test.
        let node_ids: HashSet<_> = nodes.iter().map(|node| node.id).collect();
        for edge in call_edges.iter() {
            if let Some(resolved_id) = edge.resolved_target {
                assert!(
                    node_ids.contains(&resolved_id),
                    "Case `{}`: resolved CALL points to missing node id {:?}",
                    case.language,
                    resolved_id
                );
            }
        }

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
fn go_package_initializer_calls_are_file_owned_without_deferred_body_calls() -> anyhow::Result<()> {
    let source = r#"package startup

type action func()
func wrap(fn action) action { return fn }
func build() action { return deferred }
func combine(left, right action) action { return left }
func deferred() {}
var first = wrap(func() {
    deferred()
})
var inline = wrap(func() { deferred() })
var reference = wrap(deferred)
var (
    grouped = wrap(build())
    nested = combine(wrap(build()), wrap(build()))
)
var sameA, sameB = wrap(deferred), wrap(deferred)
var held = func() { deferred() }
func ordinary() { var local = wrap(deferred); _ = local; wrap(deferred); deferred() }; var adjacent = wrap(func() { deferred() })
"#;
    let (nodes, edges) = index_single_file("initializers.go", source)?;
    let files: Vec<_> = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::FILE)
        .collect();
    assert_eq!(
        files.len(),
        1,
        "the real file node must own package initialization"
    );
    let file = files[0];
    let by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let named_function = |name: &str| {
        let matches: Vec<_> = nodes
            .iter()
            .filter(|node| {
                node.kind == NodeKind::FUNCTION && is_matching_name(&node.serialized_name, name)
            })
            .collect();
        assert_eq!(matches.len(), 1, "expected one declared function {name}");
        matches[0]
    };
    let function_name = |id| {
        ["wrap", "build", "combine", "deferred", "ordinary"]
            .into_iter()
            .find(|name| named_function(name).id == id)
            .unwrap_or_else(|| {
                panic!(
                    "CALL target must be an actual declared function: {:?}",
                    by_id.get(&id)
                )
            })
    };
    let functions: Vec<_> = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::FUNCTION)
        .collect();
    assert_eq!(
        functions.len(),
        5,
        "initializer bindings and function literals must not fabricate FUNCTION nodes: {functions:?}"
    );
    for binding in [
        "first",
        "inline",
        "reference",
        "grouped",
        "nested",
        "sameA",
        "sameB",
        "held",
        "adjacent",
    ] {
        assert!(
            !has_node_with_kind(&nodes, NodeKind::FUNCTION, binding),
            "{binding} is a variable, not a callable declaration"
        );
    }
    let file_calls: Vec<_> = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.effective_source() == file.id)
        .collect();
    assert_eq!(
        file_calls.len(),
        13,
        "package initializer calls must survive real WorkspaceIndexer persistence as FILE-owned edges; callbacks/reference arguments add no calls. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    let mut observed = std::collections::BTreeMap::new();
    let mut callsites = HashSet::new();
    for edge in &file_calls {
        assert_eq!(
            edge.source, file.id,
            "no fabricated callable owns initialization"
        );
        assert_eq!(edge.file_node_id, Some(file.id));
        assert_ne!(edge.source, edge.target);
        let target = edge.effective_target();
        assert_eq!(by_id[&target].kind, NodeKind::FUNCTION);
        *observed
            .entry((
                edge.line.expect("initializer CALL needs its physical line"),
                function_name(target),
            ))
            .or_insert(0usize) += 1;
        assert!(
            callsites.insert(
                edge.callsite_identity
                    .as_deref()
                    .expect("each initializer CALL needs a distinct callsite identity")
            ),
            "same-line calls must not collapse: {edge:?}"
        );
    }
    let line_of = |prefix: &str| -> u32 {
        source
            .lines()
            .position(|line| line.trim_start().starts_with(prefix))
            .unwrap() as u32
            + 1
    };
    let mut expected = std::collections::BTreeMap::new();
    for (prefix, callee, count) in [
        ("var first =", "wrap", 1),
        ("var inline =", "wrap", 1),
        ("var reference =", "wrap", 1),
        ("grouped =", "wrap", 1),
        ("grouped =", "build", 1),
        ("nested =", "combine", 1),
        ("nested =", "wrap", 2),
        ("nested =", "build", 2),
        ("var sameA, sameB =", "wrap", 2),
        ("func ordinary()", "wrap", 1),
    ] {
        expected.insert((line_of(prefix), callee), count);
    }
    assert_eq!(
        observed, expected,
        "nested arguments, grouped declarations and distinct same-line initializers retain the actual evaluated callsites; deferred literal bodies are excluded"
    );
    let ordinary = named_function("ordinary");
    let mut ordinary_calls = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.effective_source() == ordinary.id)
        .map(|edge| {
            assert_eq!(edge.line, Some(line_of("func ordinary()")));
            function_name(edge.effective_target())
        })
        .collect::<Vec<_>>();
    ordinary_calls.sort_unstable();
    assert_eq!(
        ordinary_calls,
        ["deferred", "wrap", "wrap"],
        "local var initializers and ordinary calls retain their function owner"
    );
    assert_no_self_call_edges("Go package initialization", &edges);
    Ok(())
}

#[test]
fn go_package_initializer_selectors_preserve_unresolved_target_policy() -> anyhow::Result<()> {
    let source = r#"package startup
import external "example.invalid/library"
type holder struct{}
func (h holder) Build() int { return 1 }
func Build() int { return 2 }
func wrap(value int) int { return value }
var imported = external.Build(wrap(1))
var object holder
var selected = object.Build()
func ordinary() { var external holder; var local = external.Build(); _ = local }
"#;
    let (nodes, edges) = index_single_file("selectors.go", source)?;
    let by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let file = nodes
        .iter()
        .find(|node| node.kind == NodeKind::FILE)
        .unwrap();
    let file_calls: Vec<_> = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.effective_source() == file.id)
        .collect();
    assert_eq!(
        file_calls.len(),
        3,
        "outer selectors and nested eager argument calls survive: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    let selectors: Vec<_> = file_calls
        .iter()
        .filter(|edge| {
            edge.callsite_identity
                .as_deref()
                .is_some_and(|site| site.contains("syntax:go-selector-call"))
        })
        .collect();
    assert_eq!(selectors.len(), 2);
    for edge in selectors {
        assert_eq!(edge.source, file.id);
        assert_eq!(edge.file_node_id, Some(file.id));
        assert_eq!(
            edge.resolved_target, None,
            "FILE ownership cannot invent receiver/import binding: {edge:?}"
        );
        assert_eq!(by_id[&edge.effective_target()].kind, NodeKind::UNKNOWN);
        assert!(matches!(edge.line, Some(7 | 9)));
    }
    let wrap = file_calls
        .iter()
        .find(|edge| is_matching_name(&by_id[&edge.effective_target()].serialized_name, "wrap"))
        .unwrap();
    assert_eq!(wrap.line, Some(7));
    assert_eq!(by_id[&wrap.effective_target()].kind, NodeKind::FUNCTION);
    let ordinary = nodes
        .iter()
        .find(|node| {
            node.kind == NodeKind::FUNCTION && is_matching_name(&node.serialized_name, "ordinary")
        })
        .unwrap();
    let local_calls: Vec<_> = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.effective_source() == ordinary.id)
        .collect();
    assert_eq!(
        local_calls.len(),
        1,
        "shadowed local receiver remains callable-owned"
    );
    assert_eq!(local_calls[0].line, Some(10));
    assert!(!file_calls.iter().any(|edge| edge.line == Some(10)));
    assert_no_self_call_edges("Go initializer selectors", &edges);
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

#[test]
#[cfg(unix)]
fn test_trailing_space_extension_indexes_without_abort() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    let migrations = root.join("migrations");
    fs::create_dir(&migrations)?;
    let odd_path = migrations.join("x.sql ");
    let normal_path = root.join("ok.sql");
    fs::write(&odd_path, "CREATE TABLE x (id INTEGER);\n")?;
    fs::write(&normal_path, "CREATE TABLE u (id INTEGER);\n")?;

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();
    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![odd_path, normal_path],
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    let nodes = storage.get_nodes()?;
    assert!(
        nodes.iter().any(|node| node.serialized_name == "public.u"),
        "the normal sql file must still index"
    );
    // `x.sql ` is admitted by the trimming extension lookup, so dispatch must
    // route it through the same normalization: the file indexes as SQL rather
    // than degrading to a tolerated coverage gap.
    assert!(
        nodes.iter().any(|node| node.serialized_name == "public.x"),
        "the whitespace-extension file must index as SQL, got: {nodes:?}"
    );
    Ok(())
}

#[test]
fn public_export_navigation_is_bound_and_refreshes_target_only_edits() -> anyhow::Result<()> {
    use codestory_store::{IndexPublicationMode, IndexPublicationRecord};
    let dir = tempdir()?;
    let root = dir.path();
    let inputs = [
        (
            "owner.ts",
            "export class _Painter { draw() {} }\nexport function _paint() {}\n",
        ),
        (
            "entry.ts",
            "import { _Painter as Local } from './owner.js';\nexport { Local as Painter };\nexport { _paint as paint } from './owner.js';\nexport function run() { new Local().draw(); }\n",
        ),
        ("local.js", "class _Local {}\nexport { _Local as Local };\n"),
        ("a.ts", "export { _Painter as Shared } from './owner.js';\n"),
        ("b.ts", "export { _paint as Shared } from './owner.js';\n"),
        (
            "barrel.ts",
            "export { Painter as Chained } from './entry.js';\nexport * from './owner.js';\n",
        ),
        (
            "unsupported.ts",
            "import Default from './owner.js';\nexport { Default as NotProven };\nexport type { _Painter as TypeOnly } from './owner.js';\n",
        ),
    ];
    for (path, source) in inputs {
        fs::write(root.join(path), source)?;
    }
    let mut store = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let refresh =
        |paths: Vec<std::path::PathBuf>, existing_file_ids| codestory_workspace::RefreshInfo {
            mode: codestory_workspace::BuildMode::Incremental,
            files_to_index: paths,
            files_to_remove: vec![],
            existing_file_ids,
        };
    indexer.run_incremental(
        &mut store,
        &refresh(
            inputs.iter().map(|(path, _)| root.join(path)).collect(),
            HashMap::new(),
        ),
        &EventBus::new(),
        None,
    )?;
    let publication = IndexPublicationRecord {
        generation: 1,
        generation_id: "exports".into(),
        run_id: "run".into(),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: 1,
    };
    codestory_indexer::rematerialize_proof_resolution_projection(&mut store, &publication)?;
    store.put_index_publication(&publication)?;
    for (name, canonical) in [
        ("Painter", "_Painter"),
        ("paint", "_paint"),
        ("Local", "_Local"),
    ] {
        let found = store.public_export_aliases(name)?;
        assert!(found.ready, "{name}");
        assert_eq!(
            found.rows.len(),
            1,
            "{name}: {found:?}; nodes: {:?}",
            store.get_nodes()?
        );
        assert!(
            store
                .get_node(found.rows[0].target_node_id.unwrap())?
                .unwrap()
                .serialized_name
                .ends_with(canonical)
        );
    }
    let painter = store.public_export_aliases("Painter")?.rows[0]
        .target_node_id
        .unwrap();
    let draw = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.callsite.raw_target == "draw")
        .collect::<Vec<_>>();
    assert!(
        !draw.is_empty(),
        "negative control must contain a real callsite"
    );
    assert!(
        draw.iter().all(|fact| fact.status
            != codestory_contracts::proof_resolution::ProofResolutionStatus::Exact),
        "navigation aliases cannot broaden exact-call admission"
    );
    assert_eq!(store.public_export_aliases("Shared")?.rows.len(), 2);
    store
        .get_connection()
        .execute_batch("SAVEPOINT cache_eviction; DELETE FROM index_artifact_cache;")?;
    assert_eq!(
        store.public_export_aliases("Painter")?.rows[0].target_node_id,
        Some(painter),
        "parser eviction cannot erase durable query authority"
    );
    let missing =
        codestory_indexer::rematerialize_proof_resolution_projection(&mut store, &publication)
            .expect_err("missing parser inputs cannot certify an empty replacement");
    assert!(missing.to_string().contains("coverage is missing"));
    assert_eq!(
        store.public_export_aliases("Painter")?.rows.len(),
        1,
        "failed staged reconstruction preserves prior rows"
    );
    store
        .get_connection()
        .execute_batch("ROLLBACK TO cache_eviction; RELEASE cache_eviction;")?;
    for name in ["Chained", "NotProven", "TypeOnly", "Invented"] {
        assert!(store.public_export_aliases(name)?.rows.is_empty(), "{name}");
    }
    // Only the target is indexed. An unchanged exporter must not retain its old binding.
    fs::write(
        root.join("owner.ts"),
        "export class Other {}\nexport function _paint() {}\n",
    )?;
    let existing = store
        .get_files()?
        .into_iter()
        .map(|file| (file.path, file.id))
        .collect();
    indexer.run_incremental(
        &mut store,
        &refresh(vec![root.join("owner.ts")], existing),
        &EventBus::new(),
        None,
    )?;
    let mut next = publication.clone();
    next.generation = 2;
    next.generation_id = "next".into();
    codestory_indexer::rematerialize_proof_resolution_projection(&mut store, &next)?;
    store.put_index_publication(&next)?;
    assert!(store.public_export_aliases("Painter")?.rows.is_empty());
    assert!(
        store.get_node(painter)?.is_none()
            || store.get_node(painter)?.unwrap().serialized_name != "_Painter"
    );
    // A competing concrete .js source makes .js -> .ts navigation ambiguous.
    fs::write(
        root.join("owner.js"),
        "export class _Painter {}\nexport function _paint() {}\n",
    )?;
    let existing = store
        .get_files()?
        .into_iter()
        .map(|file| (file.path, file.id))
        .collect();
    indexer.run_incremental(
        &mut store,
        &refresh(vec![root.join("owner.js")], existing),
        &EventBus::new(),
        None,
    )?;
    next.generation = 3;
    next.generation_id = "third".into();
    codestory_indexer::rematerialize_proof_resolution_projection(&mut store, &next)?;
    store.put_index_publication(&next)?;
    let ambiguous = store.public_export_aliases("paint")?;
    assert!(ambiguous.ready);
    assert!(!ambiguous.complete);
    assert!(ambiguous.rows.is_empty());
    // Drift after extraction must reject the staged rebuild, not invent an empty binding.
    fs::write(root.join("owner.js"), "export function changed() {}\n")?;
    assert!(
        codestory_indexer::rematerialize_proof_resolution_projection(&mut store, &next).is_err()
    );
    Ok(())
}

#[test]
fn public_export_supported_bindings_coexist_with_unsupported_coverage() -> anyhow::Result<()> {
    use codestory_store::{IndexPublicationMode, IndexPublicationRecord};
    let cases = [
        (
            "supported",
            "export class _Owner {}\n",
            "export { _Owner as Public } from './owner.js';\n",
            1,
            false,
        ),
        (
            "exporter_const",
            "export class _Owner {}\n",
            "export { _Owner as Public } from './owner.js';\nexport const version = 1;\n",
            1,
            true,
        ),
        (
            "target_const",
            "export class _Owner {}\nexport const version = 1;\n",
            "export { _Owner as Public } from './owner.js';\n",
            1,
            true,
        ),
        (
            "mixed_named_import",
            "export class _Owner {}\nexport const version = 1;\nexport interface Other { size: number }\nexport default function unrelated() {}\n",
            "import { _Owner as Local } from './owner.js';\nexport { Local as Public };\nexport const version = 2;\nexport declare namespace Other { const size: number; }\nexport type { Other } from './types';\nexport * from './unavailable';\n",
            1,
            true,
        ),
        (
            "same_file",
            "class _Owner {}\nexport { _Owner as Public };\nexport const version = 1;\n",
            "export const other = 2;\n",
            1,
            true,
        ),
        (
            "namespace_reexport_has_no_local_binding",
            "class _Owner {}\nexport { _Owner as Public };\nexport * as _Owner from './unavailable';\n",
            "export const version = 1;\n",
            1,
            true,
        ),
        (
            "public_name_conflict",
            "export class _Owner {}\n",
            "export { _Owner as Public } from './owner.js';\nexport const Public = 1;\n",
            0,
            true,
        ),
        (
            "target_name_conflict",
            "export class _Owner {}\nexport const _Owner = 1;\n",
            "export { _Owner as Public } from './owner.js';\n",
            0,
            true,
        ),
        (
            "public_namespace_conflict",
            "export class _Owner {}\n",
            "export { _Owner as Public } from './owner.js';\nexport declare namespace Public { const size: number; }\n",
            0,
            true,
        ),
        (
            "target_namespace_conflict",
            "export class _Owner {}\nexport declare namespace _Owner { const size: number; }\n",
            "export { _Owner as Public } from './owner.js';\n",
            0,
            true,
        ),
        (
            "public_namespace_export_conflict",
            "export class _Owner {}\n",
            "export { _Owner as Public } from './owner.js';\nexport * as Public from './unavailable';\n",
            0,
            true,
        ),
        (
            "local_namespace_conflict",
            "class _Owner {}\ndeclare namespace _Owner { const size: number; }\nexport { _Owner as Public };\n",
            "export const version = 1;\n",
            0,
            true,
        ),
        (
            "function_with_unsupported_exports",
            "export function _Owner() {}\nexport const version = 1;\nexport default class Other {}\n",
            "export { _Owner as Public } from './owner.js';\nexport * from './unavailable';\nexport type { Other } from './types';\n",
            1,
            true,
        ),
        (
            "duplicate_alias",
            "export class _Owner {}\n",
            "export { _Owner as Public } from './owner.js';\nexport { _Owner as Public } from './owner.js';\n",
            0,
            true,
        ),
        (
            "missing_target",
            "export class Other {}\n",
            "export { _Owner as Public } from './owner.js';\n",
            0,
            true,
        ),
        (
            "partial_exporter",
            "export class _Owner {}\n",
            "export { _Owner as Public } from './owner.js';\nfunction broken( {\n",
            0,
            true,
        ),
        (
            "type_star_coexistence",
            "export class _Owner {}\n",
            "export { _Owner as Public } from './owner.js';\nexport type * from './types';\n",
            1,
            true,
        ),
        (
            "partial_target",
            "export class _Owner {}\nfunction broken( {\n",
            "export { _Owner as Public } from './owner.js';\n",
            0,
            true,
        ),
        (
            "unsupported_target",
            "export const _Owner = 1;\n",
            "export { _Owner as Public } from './owner.js';\n",
            0,
            true,
        ),
        (
            "quoted_public_conflict",
            "class _Owner {}\nclass Other {}\nexport { _Owner as Public };\nexport { Other as \"Public\" };\n",
            "export const version = 1;\n",
            0,
            true,
        ),
        (
            "quoted_public_unrelated",
            "class _Owner {}\nclass Other {}\nexport { _Owner as Public };\nexport { Other as \"Different\" };\n",
            "export const version = 1;\n",
            1,
            true,
        ),
        (
            "quoted_empty_public_unrelated",
            "class _Owner {}\nclass Other {}\nexport { _Owner as Public };\nexport { Other as \"\" };\n",
            "export const version = 1;\n",
            1,
            true,
        ),
        (
            "single_quoted_public_conflict",
            "class _Owner {}\nclass Other {}\nexport { _Owner as Public };\nexport { Other as 'Public' };\n",
            "export const version = 1;\n",
            0,
            true,
        ),
        (
            "quoted_local_conflict",
            "export class _Owner {}\nexport class Other {}\n",
            "export { _Owner as Public } from './owner.js';\nexport { \"Other\" as Public } from './owner.js';\n",
            0,
            true,
        ),
        (
            "quoted_local_unrelated",
            "export class _Owner {}\nexport class Other {}\n",
            "export { _Owner as Public } from './owner.js';\nexport { \"Other\" as Different } from './owner.js';\n",
            1,
            true,
        ),
        (
            "quoted_alias_never_resolves",
            "export class _Owner {}\n",
            "export { _Owner as \"Public\" } from './owner.js';\n",
            0,
            true,
        ),
        (
            "quoted_namespace_conflict",
            "export class _Owner {}\n",
            "export { _Owner as Public } from './owner.js';\nexport * as \"Public\" from './other.js';\n",
            0,
            true,
        ),
        (
            "quoted_namespace_unrelated",
            "export class _Owner {}\n",
            "export { _Owner as Public } from './owner.js';\nexport * as \"Different\" from './other.js';\n",
            1,
            true,
        ),
        (
            "escaped_json_public_conflict",
            "class _Owner {}\nclass Other {}\nexport { _Owner as Public };\nexport { Other as \"Pub\\u006cic\" };\n",
            "export const version = 1;\n",
            0,
            true,
        ),
        (
            "escaped_json_public_unrelated",
            "class _Owner {}\nclass Other {}\nexport { _Owner as Public };\nexport { Other as \"Diffe\\u0072ent\" };\n",
            "export const version = 1;\n",
            1,
            true,
        ),
        (
            "unknown_single_quoted_public",
            "class _Owner {}\nclass Other {}\nexport { _Owner as Public };\nexport { Other as 'Pub\\u006cic' };\n",
            "export const version = 1;\n",
            0,
            true,
        ),
        (
            "unknown_identifier_public",
            "class _Owner {}\nclass Other {}\nexport { _Owner as Public };\nexport { Other as Pub\\u006cic };\n",
            "export const version = 1;\n",
            0,
            true,
        ),
        (
            "unknown_local_declaration",
            "class _Owner {}\nconst _Ow\\u006eer = 1;\nexport { _Owner as Public };\n",
            "export const version = 1;\n",
            0,
            true,
        ),
        (
            "unknown_local_does_not_poison_direct_reexport",
            "export class _Owner {}\n",
            "const Diffe\\u0072ent = 1;\nexport { _Owner as Public } from './owner.js';\nexport const version = 1;\n",
            1,
            true,
        ),
        (
            "quoted_target_export_conflict",
            "export class _Owner {}\nclass Other {}\nexport { Other as \"_Owner\" };\n",
            "export { _Owner as Public } from './owner.js';\n",
            0,
            true,
        ),
    ];
    let mut failures = Vec::new();
    for (name, target, exporter, expected, partial_coverage) in cases {
        let dir = tempdir()?;
        let root = dir.path();
        fs::write(root.join("owner.ts"), target)?;
        fs::write(root.join("entry.ts"), exporter)?;
        let mut store = Storage::new_in_memory()?;
        WorkspaceIndexer::new(root.to_path_buf()).run_incremental(
            &mut store,
            &codestory_workspace::RefreshInfo {
                mode: codestory_workspace::BuildMode::Incremental,
                files_to_index: vec![root.join("owner.ts"), root.join("entry.ts")],
                files_to_remove: vec![],
                existing_file_ids: HashMap::new(),
            },
            &EventBus::new(),
            None,
        )?;
        let publication = IndexPublicationRecord {
            generation: 1,
            generation_id: name.into(),
            run_id: name.into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        codestory_indexer::rematerialize_proof_resolution_projection(&mut store, &publication)?;
        store.put_index_publication(&publication)?;
        let found = store.public_export_aliases("Public")?;
        if !matches!(name, "partial_exporter" | "partial_target") {
            assert!(
                store.get_files()?.iter().all(|file| file.complete),
                "valid fixture must have complete parser coverage: {name}"
            );
        }
        eprintln!(
            "mixed export matrix {name}: expected={expected} actual={} ready={} complete={}",
            found.rows.len(),
            found.ready,
            found.complete
        );
        if !found.ready || found.rows.len() != expected || partial_coverage && found.complete {
            failures.push(name);
        }
        for row in found.rows {
            assert_eq!(
                store
                    .get_node(row.target_node_id.unwrap())?
                    .unwrap()
                    .serialized_name,
                "_Owner"
            );
        }
    }
    assert!(
        failures.is_empty(),
        "mixed-export matrix failures: {failures:?}"
    );
    Ok(())
}

#[test]
fn public_export_type_star_grammar_preserves_value_authority() -> anyhow::Result<()> {
    use codestory_store::{IndexPublicationMode, IndexPublicationRecord};
    let cases = [
        ("plain", "export type * from './owner.js';\n", true, true),
        (
            "namespace",
            "export type * as Types from './owner.js';\n",
            true,
            true,
        ),
        (
            "type_only",
            "export type * as Public from './owner.js';\n",
            true,
            false,
        ),
        ("missing_from", "export type * './owner.js';\n", false, true),
        (
            "missing_namespace_name",
            "export type * as from './owner.js';\n",
            false,
            true,
        ),
        (
            "unclosed_string",
            "export type * from './owner.js\n",
            false,
            true,
        ),
        (
            "unrelated_error",
            "export type * from './owner.js';\nfunction broken( {\n",
            false,
            true,
        ),
    ];
    let mut failures = Vec::new();
    for extension in ["ts", "tsx"] {
        for (name, type_export, parsed, named_exports) in cases {
            let dir = tempdir()?;
            let root = dir.path();
            let owner = root.join(format!("owner.{extension}"));
            let entry = root.join(format!("entry.{extension}"));
            fs::write(
                &owner,
                "export class _Owner {}\nexport function _run() {}\n",
            )?;
            let mut source = String::new();
            if named_exports {
                source.push_str("export { _Owner as Public, _run as Run } from './owner.js';\n");
            }
            source.push_str(type_export);
            fs::write(&entry, source)?;
            let mut store = Storage::new_in_memory()?;
            WorkspaceIndexer::new(root.to_path_buf()).run_incremental(
                &mut store,
                &codestory_workspace::RefreshInfo {
                    mode: codestory_workspace::BuildMode::Incremental,
                    files_to_index: vec![owner, entry.clone()],
                    files_to_remove: vec![],
                    existing_file_ids: HashMap::new(),
                },
                &EventBus::new(),
                None,
            )?;
            let publication = IndexPublicationRecord {
                generation: 1,
                generation_id: format!("{extension}-{name}"),
                run_id: name.into(),
                mode: IndexPublicationMode::Full,
                published_at_epoch_ms: 1,
            };
            codestory_indexer::rematerialize_proof_resolution_projection(&mut store, &publication)?;
            store.put_index_publication(&publication)?;
            let actual_parsed = store.get_file_by_path(&entry)?.unwrap().complete;
            let class = store.public_export_aliases("Public")?;
            let function = store.public_export_aliases("Run")?;
            let namespace = store.public_export_aliases("Types")?;
            let expected = usize::from(parsed && named_exports);
            eprintln!(
                "type-star matrix {extension}/{name}: parsed={actual_parsed} expected={parsed} class={} function={} expected={expected}",
                class.rows.len(),
                function.rows.len()
            );
            if actual_parsed != parsed
                || !class.ready
                || class.complete
                || class.rows.len() != expected
                || function.rows.len() != expected
                || !namespace.rows.is_empty()
            {
                failures.push(format!("{extension}/{name}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "type-star matrix failures: {failures:?}"
    );
    Ok(())
}

#[path = "fidelity_regression/go_callable_value_references.rs"]
mod go_callable_value_references;
