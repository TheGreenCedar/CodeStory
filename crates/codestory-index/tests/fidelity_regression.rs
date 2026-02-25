use codestory_core::{Edge, EdgeKind, Node};
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_storage::Storage;
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

type ResolvedOwnerExpectation = (&'static str, &'static str, &'static str);
type ResolvedNameExpectation = (&'static str, &'static str);

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
    min_resolved_calls: usize,
    expected_resolved_owners: &'static [ResolvedOwnerExpectation],
    expected_resolved_names: &'static [ResolvedNameExpectation],
}

const PYTHON_SYMBOLS: &[&str] = &[
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

const PYTHON_CALLS: &[&str] = &["notify", "save", "decorate", "run"];
const TYPESCRIPT_CALLS: &[&str] = &["identity", "notify", "save", "decorate", "run"];
const JAVASCRIPT_CALLS: &[&str] = &["identity", "notify", "save", "decorate", "run"];
const JAVA_CALLS: &[&str] = &["identity", "notifyEvent", "save", "decorate", "run"];
const CPP_CALLS: &[&str] = &["identity", "notifyEvent", "save", "decorate", "run"];
const C_CALLS: &[&str] = &["repository_track", "workflow_run"];
const RUST_CALLS: &[&str] = &["identity", "notify", "save", "decorate", "run"];

const PYTHON_IMPORTS: &[&str] = &[];
const TYPESCRIPT_IMPORTS: &[&str] = &["fs", "path"];
const JAVASCRIPT_IMPORTS: &[&str] = &["fs", "path"];
const JAVA_IMPORTS: &[&str] = &["java.util.concurrent", "java.util.function"];
const CPP_IMPORTS: &[&str] = &["future", "functional", "string"];
const C_IMPORTS: &[&str] = &["stdio", "string", "stddef"];
const RUST_IMPORTS: &[&str] = &["std::collections", "std::future"];

const PYTHON_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[];
const TYPESCRIPT_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] =
    &[("run", "Notifier", "notify"), ("run", "Repository", "save")];
const JAVASCRIPT_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[("run", "Notifier", "notify")];
const JAVA_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("run", "Notifier", "notifyEvent"),
    ("run", "Repository", "save"),
];
const CPP_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[
    ("run", "Notifier", "notifyEvent"),
    ("run", "Repository", "save"),
];
const C_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] = &[];
const RUST_RESOLVED_OWNERS: &[ResolvedOwnerExpectation] =
    &[("run", "Notifier", "notify"), ("run", "Repository", "save")];

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
            min_resolved_calls: 0,
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
            min_resolved_calls: 2,
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
            min_resolved_calls: 1,
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
            min_resolved_calls: 2,
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
            min_resolved_calls: 2,
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
            min_resolved_calls: 0,
            expected_resolved_owners: C_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
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
            min_resolved_calls: 2,
            expected_resolved_owners: RUST_RESOLVED_OWNERS,
            expected_resolved_names: EMPTY_RESOLVED_NAMES,
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

    let refresh_info = codestory_project::RefreshInfo {
        files_to_index: vec![file_path],
        files_to_remove: vec![],
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

fn has_call_target_name(edges: &[Edge], nodes: &[Node], target_name: &str) -> bool {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .any(|node| is_matching_name(&node.serialized_name, target_name))
}

fn has_import_target_fragment(edges: &[Edge], nodes: &[Node], target_fragment: &str) -> bool {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::IMPORT)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .any(|node| node.serialized_name.contains(target_fragment))
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

        let resolved_calls = call_edges
            .iter()
            .filter(|edge| edge.resolved_target.is_some())
            .count();
        assert!(
            resolved_calls >= case.min_resolved_calls,
            "Case `{}`: expected at least {} resolved CALL edges, got {}",
            case.language,
            case.min_resolved_calls,
            resolved_calls
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
