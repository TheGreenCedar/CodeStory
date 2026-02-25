use codestory_core::{Edge, EdgeKind, Node};
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_storage::Storage;
use std::collections::HashMap;
use std::fs;
use tempfile::tempdir;

const PYTHON_SOURCE: &str =
    include_str!("fixtures/call_resolution_comprehensive/python_workflow.py");
const JAVA_SOURCE: &str = include_str!("fixtures/call_resolution_comprehensive/java_workflow.java");
const RUST_SOURCE: &str = include_str!("fixtures/call_resolution_comprehensive/rust_workflow.rs");
const JAVASCRIPT_SOURCE: &str =
    include_str!("fixtures/call_resolution_comprehensive/javascript_workflow.js");
const TYPESCRIPT_SOURCE: &str =
    include_str!("fixtures/call_resolution_comprehensive/typescript_workflow.ts");
const CPP_SOURCE: &str = include_str!("fixtures/call_resolution_comprehensive/cpp_workflow.cpp");

type ResolvedCallExpectation = (&'static str, &'static str, &'static str);
type ResolvedNameExpectation = (&'static str, &'static str);

const PYTHON_SYMBOLS: &[&str] = &["Notifier", "EmailNotifier", "Workflow", "CheckoutWorkflow"];
const JAVA_SYMBOLS: &[&str] = &[
    "Notifier",
    "Repository",
    "EmailNotifier",
    "MemoryRepository",
    "Workflow",
    "CheckoutWorkflow",
];
const RUST_SYMBOLS: &[&str] = &[
    "Notifier",
    "Repository",
    "EmailNotifier",
    "MemoryRepository",
    "Workflow",
    "CheckoutWorkflow",
];
const JAVASCRIPT_SYMBOLS: &[&str] = &["Notifier", "EmailNotifier", "Workflow", "CheckoutWorkflow"];
const TYPESCRIPT_SYMBOLS: &[&str] = &[
    "Notifier",
    "Repository",
    "EmailNotifier",
    "MemoryRepository",
    "Workflow",
    "CheckoutWorkflow",
];
const CPP_SYMBOLS: &[&str] = &[
    "Notifier",
    "Repository",
    "EmailNotifier",
    "MemoryRepository",
    "Workflow",
    "CheckoutWorkflow",
];

const PYTHON_CALLS: &[&str] = &[
    "notify_event",
    "persist",
    "audit",
    "run",
    "write_log",
    "save_record",
];
const JAVA_CALLS: &[&str] = &[
    "notifyEvent",
    "save",
    "persist",
    "audit",
    "run",
    "writeLog",
    "trackSave",
    "saveRecord",
];
const RUST_CALLS: &[&str] = &[
    "notify_event",
    "save",
    "persist",
    "audit",
    "run",
    "write_log",
    "track_save",
    "save_record",
];
const JAVASCRIPT_CALLS: &[&str] = &[
    "notifyEvent",
    "persist",
    "audit",
    "run",
    "writeLog",
    "saveRecord",
];
const TYPESCRIPT_CALLS: &[&str] = &[
    "notifyEvent",
    "save",
    "persist",
    "audit",
    "run",
    "writeLog",
    "trackSave",
    "saveRecord",
];
const CPP_CALLS: &[&str] = &[
    "notifyEvent",
    "save",
    "persist",
    "audit",
    "run",
    "writeLog",
    "trackSave",
    "saveRecord",
];

const PYTHON_RESOLVED: &[ResolvedCallExpectation] = &[];
const PYTHON_RESOLVED_BY_NAME: &[ResolvedNameExpectation] =
    &[("run", "notify_event"), ("run", "persist")];
const JAVA_RESOLVED: &[ResolvedCallExpectation] = &[
    ("run", "Notifier", "notifyEvent"),
    ("run", "Repository", "save"),
    ("run", "Workflow", "persist"),
];
const JAVA_RESOLVED_BY_NAME: &[ResolvedNameExpectation] = &[];
const RUST_RESOLVED: &[ResolvedCallExpectation] = &[
    ("run", "Notifier", "notify_event"),
    ("run", "Repository", "save"),
    ("run", "Workflow", "persist"),
];
const RUST_RESOLVED_BY_NAME: &[ResolvedNameExpectation] = &[];
const JAVASCRIPT_RESOLVED: &[ResolvedCallExpectation] = &[
    ("run", "Notifier", "notifyEvent"),
    ("run", "Workflow", "persist"),
];
const JAVASCRIPT_RESOLVED_BY_NAME: &[ResolvedNameExpectation] = &[];
const TYPESCRIPT_RESOLVED: &[ResolvedCallExpectation] = &[
    ("run", "Notifier", "notifyEvent"),
    ("run", "Repository", "save"),
    ("run", "Workflow", "persist"),
];
const TYPESCRIPT_RESOLVED_BY_NAME: &[ResolvedNameExpectation] = &[];
const CPP_RESOLVED: &[ResolvedCallExpectation] = &[
    ("run", "Notifier", "notifyEvent"),
    ("run", "Repository", "save"),
    ("run", "Workflow", "persist"),
];
const CPP_RESOLVED_BY_NAME: &[ResolvedNameExpectation] = &[];

struct RichFixtureCase {
    language: &'static str,
    filename: &'static str,
    source: &'static str,
    min_nodes: usize,
    min_call_edges: usize,
    required_symbols: &'static [&'static str],
    required_call_targets: &'static [&'static str],
    expected_resolved_calls: &'static [ResolvedCallExpectation],
    expected_resolved_names: &'static [ResolvedNameExpectation],
}

fn rich_fixture_cases() -> Vec<RichFixtureCase> {
    vec![
        RichFixtureCase {
            language: "python",
            filename: "workflow.py",
            source: PYTHON_SOURCE,
            min_nodes: 18,
            min_call_edges: 6,
            required_symbols: PYTHON_SYMBOLS,
            required_call_targets: PYTHON_CALLS,
            expected_resolved_calls: PYTHON_RESOLVED,
            expected_resolved_names: PYTHON_RESOLVED_BY_NAME,
        },
        RichFixtureCase {
            language: "java",
            filename: "Workflow.java",
            source: JAVA_SOURCE,
            min_nodes: 14,
            min_call_edges: 6,
            required_symbols: JAVA_SYMBOLS,
            required_call_targets: JAVA_CALLS,
            expected_resolved_calls: JAVA_RESOLVED,
            expected_resolved_names: JAVA_RESOLVED_BY_NAME,
        },
        RichFixtureCase {
            language: "rust",
            filename: "workflow.rs",
            source: RUST_SOURCE,
            min_nodes: 14,
            min_call_edges: 6,
            required_symbols: RUST_SYMBOLS,
            required_call_targets: RUST_CALLS,
            expected_resolved_calls: RUST_RESOLVED,
            expected_resolved_names: RUST_RESOLVED_BY_NAME,
        },
        RichFixtureCase {
            language: "javascript",
            filename: "workflow.js",
            source: JAVASCRIPT_SOURCE,
            min_nodes: 14,
            min_call_edges: 6,
            required_symbols: JAVASCRIPT_SYMBOLS,
            required_call_targets: JAVASCRIPT_CALLS,
            expected_resolved_calls: JAVASCRIPT_RESOLVED,
            expected_resolved_names: JAVASCRIPT_RESOLVED_BY_NAME,
        },
        RichFixtureCase {
            language: "typescript",
            filename: "workflow.ts",
            source: TYPESCRIPT_SOURCE,
            min_nodes: 14,
            min_call_edges: 6,
            required_symbols: TYPESCRIPT_SYMBOLS,
            required_call_targets: TYPESCRIPT_CALLS,
            expected_resolved_calls: TYPESCRIPT_RESOLVED,
            expected_resolved_names: TYPESCRIPT_RESOLVED_BY_NAME,
        },
        RichFixtureCase {
            language: "cpp",
            filename: "workflow.cpp",
            source: CPP_SOURCE,
            min_nodes: 14,
            min_call_edges: 6,
            required_symbols: CPP_SYMBOLS,
            required_call_targets: CPP_CALLS,
            expected_resolved_calls: CPP_RESOLVED,
            expected_resolved_names: CPP_RESOLVED_BY_NAME,
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
fn test_common_method_calls_do_not_resolve_globally() -> anyhow::Result<()> {
    // Create a file that defines a project method named `push`, plus calls to common stdlib-ish
    // methods (`Vec::push`, `Vec::sort`, `Vec::dedup`). We should not resolve these calls to
    // unrelated project methods by name alone.
    let source = r#"
struct NavigationHistory;
impl NavigationHistory {
    fn push(&mut self, _x: i32) {}
}

mod tests {
    pub fn test_deduplication() {}
}

fn main() {
    let mut v = vec![1, 1, 2];
    v.push(3);
    v.sort();
    v.dedup();
}
"#;
    let (nodes, edges) = index_single_file("main.rs", source)?;

    let node_name = |id| {
        nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.serialized_name.as_str())
    };

    let mut found = 0;
    for edge in edges.iter().filter(|e| e.kind == EdgeKind::CALL) {
        let Some(name) = node_name(edge.target) else {
            continue;
        };
        if matches!(name, "push" | "sort" | "dedup") {
            found += 1;
            assert!(
                edge.resolved_target.is_none(),
                "CALL edge to `{name}` should remain unresolved (got {:?})",
                edge.resolved_target
            );
        }
    }

    assert!(found >= 3, "expected CALL edges for push/sort/dedup");
    Ok(())
}

#[test]
fn test_run_incremental_clone_does_not_resolve_to_unrelated_field_clone() -> anyhow::Result<()> {
    // Regression guard for graph trail fidelity: `run_incremental` should not resolve generic
    // `.clone()` calls to unrelated project methods like `Field::clone`.
    let source = r#"
use std::sync::Arc;

struct Field;
impl Field {
    fn clone(&self) -> Self { Field }
}

struct WorkspaceIndexer {
    root: Arc<String>,
}

impl WorkspaceIndexer {
    fn run_incremental(&self) {
        let _root = self.root.clone();
        let _tmp = Arc::new(String::from("x")).clone();
    }
}
"#;

    let (nodes, edges) = index_single_file("main.rs", source)?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();

    let run_node_ids: Vec<_> = nodes
        .iter()
        .filter(|node| is_matching_name(&node.serialized_name, "run_incremental"))
        .map(|node| node.id)
        .collect();
    assert!(
        !run_node_ids.is_empty(),
        "expected run_incremental symbol to be indexed"
    );

    let field_clone_id = nodes
        .iter()
        .find(|node| is_matching_owned_method(&node.serialized_name, "Field", "clone"))
        .map(|node| node.id);
    assert!(
        field_clone_id.is_some(),
        "expected Field::clone symbol to be indexed"
    );
    let field_clone_id = field_clone_id.expect("field clone id should be present");

    let mut clone_edges_from_run = 0usize;
    for edge in edges.iter().filter(|edge| edge.kind == EdgeKind::CALL) {
        if !run_node_ids.contains(&edge.source) {
            continue;
        }
        let Some(target_node) = node_by_id.get(&edge.target) else {
            continue;
        };
        if !is_matching_name(&target_node.serialized_name, "clone") {
            continue;
        }

        clone_edges_from_run += 1;
        assert_ne!(
            edge.resolved_target,
            Some(field_clone_id),
            "clone call from run_incremental must not resolve to Field::clone"
        );
    }

    assert!(
        clone_edges_from_run >= 1,
        "expected at least one clone call edge from run_incremental"
    );
    Ok(())
}

#[test]
fn test_comprehensive_polymorphic_call_resolution_fixtures() -> anyhow::Result<()> {
    for case in rich_fixture_cases() {
        let (nodes, edges) = index_single_file(case.filename, case.source)?;

        assert!(
            nodes.len() >= case.min_nodes,
            "Case `{}`: expected at least {} nodes, got {}",
            case.language,
            case.min_nodes,
            nodes.len()
        );

        let call_edges = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::CALL)
            .count();
        assert!(
            call_edges >= case.min_call_edges,
            "Case `{}`: expected at least {} CALL edges, got {}",
            case.language,
            case.min_call_edges,
            call_edges
        );

        for symbol in case.required_symbols {
            assert!(
                has_node_name(&nodes, symbol),
                "Case `{}`: missing symbol node `{}`. Node names: {:?}",
                case.language,
                symbol,
                nodes
                    .iter()
                    .map(|node| node.serialized_name.clone())
                    .collect::<Vec<_>>()
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

        for (caller, owner, method) in case.expected_resolved_calls {
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
