use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{Edge, EdgeKind, Node, NodeId, ResolutionCertainty};
use codestory_indexer::WorkspaceIndexer;
use codestory_store::Store as Storage;
use std::collections::{HashMap, HashSet};
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

const PYTHON_RESOLVED: &[ResolvedCallExpectation] = &[
    ("run", "Notifier", "notify_event"),
    ("run", "Workflow", "persist"),
    ("run", "Workflow", "audit"),
    ("run_async", "Workflow", "run"),
];
const PYTHON_RESOLVED_BY_NAME: &[ResolvedNameExpectation] = &[];
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
    ("run", "Workflow", "audit"),
];
const RUST_RESOLVED_BY_NAME: &[ResolvedNameExpectation] = &[];
const JAVASCRIPT_RESOLVED: &[ResolvedCallExpectation] = &[
    ("notifyEvent", "EmailNotifier", "writeLog"),
    ("run", "Workflow", "persist"),
    ("run", "Workflow", "audit"),
    ("runAsync", "Workflow", "run"),
    ("persist", "CheckoutWorkflow", "saveRecord"),
];
const JAVASCRIPT_RESOLVED_BY_NAME: &[ResolvedNameExpectation] = &[];
const TYPESCRIPT_RESOLVED: &[ResolvedCallExpectation] = &[
    ("notifyEvent", "EmailNotifier", "writeLog"),
    ("save", "MemoryRepository", "trackSave"),
    ("run", "Workflow", "identity"),
    ("run", "Notifier", "notifyEvent"),
    ("run", "Repository", "save"),
    ("run", "Workflow", "persist"),
    ("run", "Workflow", "audit"),
    ("runAsync", "Workflow", "run"),
    ("persist", "CheckoutWorkflow", "saveRecord"),
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

fn index_files(files: &[(&str, &str)]) -> anyhow::Result<(Vec<Node>, Vec<Edge>)> {
    let dir = tempdir()?;
    let root = dir.path();
    let mut files_to_index = Vec::with_capacity(files.len());
    for (filename, contents) in files {
        let file_path = root.join(filename);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&file_path, contents)?;
        files_to_index.push(file_path);
    }

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();

    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index,
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };
    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;
    let errors = storage.get_errors(None)?;
    anyhow::ensure!(
        errors.is_empty(),
        "Indexing errors for {:?}: {errors:?}",
        files
            .iter()
            .map(|(filename, _)| *filename)
            .collect::<Vec<_>>()
    );

    Ok((storage.get_nodes()?, storage.get_edges()?))
}

fn index_single_file(filename: &str, contents: &str) -> anyhow::Result<(Vec<Node>, Vec<Edge>)> {
    index_files(&[(filename, contents)])
}

fn is_matching_name(serialized_name: &str, wanted_name: &str) -> bool {
    if serialized_name == wanted_name
        || serialized_name.starts_with(&format!("{wanted_name}<"))
        || serialized_name.ends_with(&format!(".{wanted_name}"))
        || serialized_name.ends_with(&format!("::{wanted_name}"))
        || serialized_name.ends_with(&format!(" {wanted_name}"))
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

fn byte_offset_for_line_col(source: &str, line: u32, col: u32) -> Option<usize> {
    if line == 0 || col == 0 {
        return None;
    }

    let mut current_line = 1u32;
    let mut line_start = 0usize;
    if current_line < line {
        for (idx, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                current_line += 1;
                line_start = idx + 1;
                if current_line == line {
                    break;
                }
            }
        }
    }

    (current_line == line).then_some(line_start + col as usize - 1)
}

fn snippet_for_node<'a>(source: &'a str, node: &Node) -> Option<&'a str> {
    let start = byte_offset_for_line_col(source, node.start_line?, node.start_col?)?;
    let end = byte_offset_for_line_col(source, node.end_line?, node.end_col?)?;
    (start <= end && end <= source.len()).then_some(&source[start..end])
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
            let target_col = node_by_id
                .get(&edge.target)
                .and_then(|node| node.start_col)
                .map(|col| col.to_string())
                .unwrap_or_else(|| "<none>".to_string());
            let resolved = edge
                .resolved_target
                .and_then(|resolved_id| node_by_id.get(&resolved_id).copied())
                .map(|n| n.serialized_name.clone())
                .unwrap_or_else(|| "<none>".to_string());
            format!(
                "{source} -> {unresolved_target} (target_col: {target_col}, callsite: {:?}, resolved: {resolved})",
                edge.callsite_identity
            )
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

fn assert_no_resolved_call_to_method_owner(
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
        !found,
        "Case `{case_name}`: did not expect CALL from `{caller_name}` to resolve to `{owner_name}::{method_name}`. Calls: {:?}",
        describe_call_edges(edges, nodes)
    );
}

fn file_path_for_node<'a>(nodes_by_id: &HashMap<NodeId, &'a Node>, node: &Node) -> Option<&'a str> {
    node.file_node_id
        .and_then(|file_id| nodes_by_id.get(&file_id).copied())
        .map(|file| file.serialized_name.as_str())
}

fn assert_resolved_call_to_method_owner_in_file(
    case_name: &str,
    nodes: &[Node],
    edges: &[Edge],
    caller_name: &str,
    owner_name: &str,
    method_name: &str,
    file_suffix: &str,
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
            Some(*resolved_node)
        })
        .any(|resolved_node| {
            is_matching_owned_method(&resolved_node.serialized_name, owner_name, method_name)
                && file_path_for_node(&node_by_id, resolved_node)
                    .map(|path| path.replace('\\', "/").ends_with(file_suffix))
                    .unwrap_or(false)
        });

    assert!(
        found,
        "Case `{case_name}`: expected CALL from `{caller_name}` to resolve to `{owner_name}::{method_name}` in `{file_suffix}`. Calls: {:?}",
        describe_call_edges(edges, nodes)
    );
}

struct ResolvedCallCountInFile<'a> {
    caller_name: &'a str,
    owner_name: &'a str,
    method_name: &'a str,
    file_suffix: &'a str,
    expected_count: usize,
}

fn assert_resolved_call_count_to_method_owner_in_file(
    case_name: &str,
    nodes: &[Node],
    edges: &[Edge],
    expected: ResolvedCallCountInFile<'_>,
) {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let count = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter_map(|edge| {
            let source = node_by_id.get(&edge.source)?;
            if !is_matching_name(&source.serialized_name, expected.caller_name) {
                return None;
            }
            let resolved_id = edge.resolved_target?;
            let resolved_node = node_by_id.get(&resolved_id)?;
            Some(*resolved_node)
        })
        .filter(|resolved_node| {
            is_matching_owned_method(
                &resolved_node.serialized_name,
                expected.owner_name,
                expected.method_name,
            ) && file_path_for_node(&node_by_id, resolved_node)
                .map(|path| path.replace('\\', "/").ends_with(expected.file_suffix))
                .unwrap_or(false)
        })
        .count();

    assert_eq!(
        count,
        expected.expected_count,
        "Case `{case_name}`: expected {} CALL(s) from `{}` to `{}::{}` in `{}`. Calls: {:?}",
        expected.expected_count,
        expected.caller_name,
        expected.owner_name,
        expected.method_name,
        expected.file_suffix,
        describe_call_edges(edges, nodes)
    );
}

fn assert_no_resolved_call_to_method_owner_in_file(
    case_name: &str,
    nodes: &[Node],
    edges: &[Edge],
    caller_name: &str,
    owner_name: &str,
    method_name: &str,
    file_suffix: &str,
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
            Some(*resolved_node)
        })
        .any(|resolved_node| {
            is_matching_owned_method(&resolved_node.serialized_name, owner_name, method_name)
                && file_path_for_node(&node_by_id, resolved_node)
                    .map(|path| path.replace('\\', "/").ends_with(file_suffix))
                    .unwrap_or(false)
        });

    assert!(
        !found,
        "Case `{case_name}`: did not expect CALL from `{caller_name}` to resolve to `{owner_name}::{method_name}` in `{file_suffix}`. Calls: {:?}",
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
fn test_rust_self_field_receiver_calls_resolve_to_declared_field_owner() -> anyhow::Result<()> {
    let source = r#"
struct AppController;
impl AppController {
    fn run_indexing_blocking_without_runtime_refresh(&self) {}
}

struct ProjectService;
impl ProjectService {
    fn run_indexing_blocking_without_runtime_refresh(&self) {}
}

struct IndexService {
    controller: AppController,
}
impl IndexService {
    fn run_indexing_blocking_without_runtime_refresh(&self) {
        self.controller.run_indexing_blocking_without_runtime_refresh();
    }
}

struct RuntimeContext {
    index: IndexService,
}
impl RuntimeContext {
    fn ensure_open_from_summary(&self) {
        self.index.run_indexing_blocking_without_runtime_refresh();
    }
}
"#;

    let (nodes, edges) = index_single_file("main.rs", source)?;

    assert_resolved_call_to_method_owner(
        "rust self field receiver",
        &nodes,
        &edges,
        "RuntimeContext::ensure_open_from_summary",
        "IndexService",
        "run_indexing_blocking_without_runtime_refresh",
    );
    assert_resolved_call_to_method_owner(
        "rust self field receiver",
        &nodes,
        &edges,
        "IndexService::run_indexing_blocking_without_runtime_refresh",
        "AppController",
        "run_indexing_blocking_without_runtime_refresh",
    );

    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let project_method_id = nodes
        .iter()
        .find(|node| {
            is_matching_owned_method(
                &node.serialized_name,
                "ProjectService",
                "run_indexing_blocking_without_runtime_refresh",
            )
        })
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected ProjectService method node"))?;
    let index_service_ids = nodes
        .iter()
        .filter(|node| {
            is_matching_owned_method(
                &node.serialized_name,
                "IndexService",
                "run_indexing_blocking_without_runtime_refresh",
            )
        })
        .map(|node| node.id)
        .collect::<Vec<_>>();
    assert!(
        !index_service_ids.is_empty(),
        "expected IndexService method node"
    );

    for edge in edges.iter().filter(|edge| edge.kind == EdgeKind::CALL) {
        if !index_service_ids.contains(&edge.source) {
            continue;
        }
        let Some(target_node) = node_by_id.get(&edge.target) else {
            continue;
        };
        if !is_matching_name(
            &target_node.serialized_name,
            "run_indexing_blocking_without_runtime_refresh",
        ) {
            continue;
        }
        assert_ne!(
            edge.resolved_target,
            Some(project_method_id),
            "self.controller call must not resolve to unrelated ProjectService method. Calls: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }
    Ok(())
}

#[test]
fn test_python_self_receiver_call_resolves_to_enclosing_class_method() -> anyhow::Result<()> {
    let source = r#"
class OtherWorker:
    def handle(self):
        pass

class Worker:
    def handle(self):
        pass

    def run(self):
        self.handle()
"#;

    let (nodes, edges) = index_single_file("workflow.py", source)?;
    assert_resolved_call_to_method_owner(
        "python self receiver",
        &nodes,
        &edges,
        "run",
        "Worker",
        "handle",
    );
    assert_no_resolved_call_to_method_owner(
        "python self receiver",
        &nodes,
        &edges,
        "run",
        "OtherWorker",
        "handle",
    );

    Ok(())
}

#[test]
fn test_python_self_receiver_shadowing_import_does_not_resolve_to_import() -> anyhow::Result<()> {
    let external_source = r#"
class Widget:
    def save(self):
        pass
"#;
    let workflow_source = r#"
from external import Widget

class Widget:
    def run(self):
        self.save()
"#;

    let (nodes, edges) = index_files(&[
        ("external.py", external_source),
        ("workflow.py", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "python self receiver shadowing import",
        &nodes,
        &edges,
        "run",
        "Widget",
        "save",
        "external.py",
    );

    Ok(())
}

#[test]
fn test_python_annotated_receiver_call_resolves_to_declared_parameter_type() -> anyhow::Result<()> {
    let source = r#"
class Event:
    pass

class Archive:
    def save(self, event: Event):
        pass

class Repository:
    def save(self, event: Event):
        pass

def run(repo: Repository[Event], event: Event):
    repo.save(event)
"#;

    let (nodes, edges) = index_single_file("workflow.py", source)?;
    assert_resolved_call_to_method_owner(
        "python annotated receiver",
        &nodes,
        &edges,
        "run",
        "Repository",
        "save",
    );
    assert_no_resolved_call_to_method_owner(
        "python annotated receiver",
        &nodes,
        &edges,
        "run",
        "Archive",
        "save",
    );

    Ok(())
}

#[test]
fn test_python_receiver_call_without_precise_annotation_stays_unresolved() -> anyhow::Result<()> {
    let source = r#"
class Repository:
    def save(self):
        pass

class Archive:
    def save(self):
        pass

def unannotated(repo):
    repo.save()

def union_typed(repo: Repository | None):
    repo.save()
"#;

    let (nodes, edges) = index_single_file("workflow.py", source)?;
    for caller in ["unannotated", "union_typed"] {
        assert_no_resolved_call_to_method_owner(
            "python imprecise receiver",
            &nodes,
            &edges,
            caller,
            "Repository",
            "save",
        );
        assert_no_resolved_call_to_method_owner(
            "python imprecise receiver",
            &nodes,
            &edges,
            caller,
            "Archive",
            "save",
        );
    }

    Ok(())
}

#[test]
fn test_python_imported_annotated_receiver_call_resolves_to_imported_owner_method()
-> anyhow::Result<()> {
    let notifier_source = r#"
class Notifier:
    def notify_event(self):
        pass
"#;
    let shadow_source = r#"
class Notifier:
    def notify_event(self):
        pass
"#;
    let workflow_source = r#"
from notifier import Notifier

def run(notifier: Notifier):
    notifier.notify_event()

def untyped(notifier):
    notifier.notify_event()
"#;
    let missing_import_source = r#"
from missing_notifier import Notifier

def run(notifier: Notifier):
    notifier.notify_event()
"#;
    let misplaced_import_source = r#"
from notifier import Notifier

def run(notifier: Notifier):
    notifier.notify_event()
"#;
    let unimported_annotation_source = r#"
def run(notifier: Notifier):
    notifier.notify_event()
"#;
    let duplicate_import_source = r#"
from notifier import Notifier
from shadow import Notifier

def run(notifier: Notifier):
    notifier.notify_event()
"#;
    let local_shadow_import_source = r#"
from notifier import Notifier

class Notifier:
    def notify_event(self):
        pass

def run(notifier: Notifier):
    notifier.notify_event()
"#;
    let assignment_shadow_import_source = r#"
from notifier import Notifier

Notifier = object

def run(notifier: Notifier):
    notifier.notify_event()
"#;
    let subscript_assignment_source = r#"
from notifier import Notifier

registry = {}
registry[Notifier] = object

def run(notifier: Notifier):
    notifier.notify_event()
"#;
    let aliased_import_source = r#"
from notifier import Notifier as Mailer

def run(mailer: Mailer):
    mailer.notify_event()
"#;
    let shadow_alias_source = r#"
class Mailer:
    def notify_event(self):
        pass
"#;
    let multiline_import_source = r#"
from notifier import (
    Notifier,
)

def run(notifier: Notifier):
    notifier.notify_event()
"#;
    let ambiguous_notifier_source = r#"
class Notifier:
    def notify_event(self):
        pass

class Notifier:
    def notify_event(self):
        pass
"#;

    let (nodes, edges) = index_files(&[
        ("notifier.py", notifier_source),
        ("shadow.py", shadow_source),
        ("workflow.py", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "python imported annotated receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notify_event",
        "notifier.py",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "python imported annotated receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notify_event",
        "shadow.py",
    );
    assert_no_resolved_call_to_method_owner(
        "python imported untyped receiver",
        &nodes,
        &edges,
        "untyped",
        "Notifier",
        "notify_event",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("shadow.py", shadow_source),
        ("workflow.py", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "python missing imported owner",
        &missing_nodes,
        &missing_edges,
        "run",
        "Notifier",
        "notify_event",
    );

    let (misplaced_nodes, misplaced_edges) = index_files(&[
        ("other/notifier.py", notifier_source),
        ("workflow.py", misplaced_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "python misplaced imported owner",
        &misplaced_nodes,
        &misplaced_edges,
        "run",
        "Notifier",
        "notify_event",
    );

    let (unimported_nodes, unimported_edges) = index_files(&[
        ("other/notifier.py", notifier_source),
        ("workflow.py", unimported_annotation_source),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "python unimported annotated owner",
        &unimported_nodes,
        &unimported_edges,
        "run",
        "Notifier",
        "notify_event",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("notifier.py", notifier_source),
        ("shadow.py", shadow_source),
        ("workflow.py", duplicate_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "python duplicate imported owner",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notify_event",
    );

    let (local_shadow_nodes, local_shadow_edges) = index_files(&[
        ("notifier.py", notifier_source),
        ("workflow.py", local_shadow_import_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "python local shadowed imported owner",
        &local_shadow_nodes,
        &local_shadow_edges,
        "run",
        "Notifier",
        "notify_event",
        "workflow.py",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "python local shadowed imported owner",
        &local_shadow_nodes,
        &local_shadow_edges,
        "run",
        "Notifier",
        "notify_event",
        "notifier.py",
    );

    let (assignment_shadow_nodes, assignment_shadow_edges) = index_files(&[
        ("notifier.py", notifier_source),
        ("workflow.py", assignment_shadow_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "python assignment shadowed imported owner",
        &assignment_shadow_nodes,
        &assignment_shadow_edges,
        "run",
        "Notifier",
        "notify_event",
        "notifier.py",
    );

    let (subscript_assignment_nodes, subscript_assignment_edges) = index_files(&[
        ("notifier.py", notifier_source),
        ("workflow.py", subscript_assignment_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "python subscript assignment does not shadow import",
        &subscript_assignment_nodes,
        &subscript_assignment_edges,
        "run",
        "Notifier",
        "notify_event",
        "notifier.py",
    );

    let (alias_nodes, alias_edges) = index_files(&[
        ("notifier.py", notifier_source),
        ("shadow_alias.py", shadow_alias_source),
        ("workflow.py", aliased_import_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "python aliased imported owner",
        &alias_nodes,
        &alias_edges,
        "run",
        "Notifier",
        "notify_event",
        "notifier.py",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "python aliased imported owner",
        &alias_nodes,
        &alias_edges,
        "run",
        "Mailer",
        "notify_event",
        "shadow_alias.py",
    );

    let (multiline_nodes, multiline_edges) = index_files(&[
        ("notifier.py", notifier_source),
        ("shadow.py", shadow_source),
        ("workflow.py", multiline_import_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "python multiline imported owner",
        &multiline_nodes,
        &multiline_edges,
        "run",
        "Notifier",
        "notify_event",
        "notifier.py",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "python multiline imported owner",
        &multiline_nodes,
        &multiline_edges,
        "run",
        "Notifier",
        "notify_event",
        "shadow.py",
    );

    let (ambiguous_nodes, ambiguous_edges) = index_files(&[
        ("notifier.py", ambiguous_notifier_source),
        ("workflow.py", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "python ambiguous imported owner",
        &ambiguous_nodes,
        &ambiguous_edges,
        "run",
        "Notifier",
        "notify_event",
    );

    Ok(())
}

#[test]
fn test_python_instance_property_receiver_call_resolves_to_same_file_owner() -> anyhow::Result<()> {
    let source = r#"
class OtherWorkflow:
    def run(self):
        pass

class Workflow:
    def run(self):
        pass

class Pipeline:
    def __init__(self):
        self.workflow = Workflow()

    def run(self):
        self.workflow.run()
"#;

    let (nodes, edges) = index_single_file("workflow.py", source)?;
    assert_resolved_call_to_method_owner(
        "python instance property receiver",
        &nodes,
        &edges,
        "Pipeline.run",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "python instance property receiver",
        &nodes,
        &edges,
        "Pipeline.run",
        "OtherWorkflow",
        "run",
    );

    Ok(())
}

#[test]
fn test_python_imported_instance_property_receiver_resolves_to_imported_owner_method()
-> anyhow::Result<()> {
    let workflow_source = r#"
class Workflow:
    def run(self):
        pass
"#;
    let shadow_source = r#"
class Workflow:
    def run(self):
        pass
"#;
    let source = r#"
from workflow import Workflow
from workflow import Workflow as RemoteWorkflow

class NamedPipeline:
    def __init__(self):
        self.workflow = Workflow()

    def run(self):
        self.workflow.run()

class AliasedPipeline:
    def __init__(self):
        self.workflow = RemoteWorkflow()

    def run(self):
        self.workflow.run()
"#;
    let missing_source = r#"
from missing_workflow import Workflow

class Pipeline:
    def __init__(self):
        self.workflow = Workflow()

    def run(self):
        self.workflow.run()
"#;
    let duplicate_source = r#"
from workflow import Workflow
from shadow import Workflow

class Pipeline:
    def __init__(self):
        self.workflow = Workflow()

    def run(self):
        self.workflow.run()
"#;

    let (nodes, edges) = index_files(&[
        ("workflow.py", workflow_source),
        ("shadow.py", shadow_source),
        ("main.py", source),
    ])?;
    for caller in ["NamedPipeline.run", "AliasedPipeline.run"] {
        assert_resolved_call_to_method_owner_in_file(
            "python imported property receiver",
            &nodes,
            &edges,
            caller,
            "Workflow",
            "run",
            "workflow.py",
        );
        assert_no_resolved_call_to_method_owner_in_file(
            "python imported property receiver",
            &nodes,
            &edges,
            caller,
            "Workflow",
            "run",
            "shadow.py",
        );
    }

    let (missing_nodes, missing_edges) = index_files(&[
        ("workflow.py", workflow_source),
        ("main.py", missing_source),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "python missing imported property owner",
        &missing_nodes,
        &missing_edges,
        "Pipeline.run",
        "Workflow",
        "run",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("workflow.py", workflow_source),
        ("shadow.py", shadow_source),
        ("main.py", duplicate_source),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "python duplicate imported property owner",
        &duplicate_nodes,
        &duplicate_edges,
        "Pipeline.run",
        "Workflow",
        "run",
    );

    Ok(())
}

#[test]
fn test_python_instance_property_receiver_stays_fail_closed_for_uncertain_owners()
-> anyhow::Result<()> {
    let source = r#"
class Workflow:
    def run(self):
        pass

class Archive:
    def run(self):
        pass

class FactoryPipeline:
    def __init__(self):
        self.workflow = make_workflow()

    def run(self):
        self.workflow.run()

class MixedPipeline:
    def __init__(self):
        self.workflow = Workflow()

    def reset(self):
        self.workflow = Archive()

    def run(self):
        self.workflow.run()

class LocalShadowPipeline:
    def __init__(self):
        Workflow = make_factory()
        self.workflow = Workflow()

    def run(self):
        self.workflow.run()

class FutureAssignmentPipeline:
    def run(self):
        self.workflow.run()
        self.workflow = Workflow()

class StaticAssignmentPipeline:
    @staticmethod
    def configure(self):
        self.workflow = Workflow()

    def run(self):
        self.workflow.run()

class ClassAssignmentPipeline:
    @classmethod
    def configure(cls):
        cls.workflow = Workflow()

    def run(self):
        self.workflow.run()

class NestedAssignmentPipeline:
    def __init__(self):
        def configure(self):
            self.workflow = Archive()
        self.workflow = Workflow()

    def run(self):
        self.workflow.run()
"#;

    let (nodes, edges) = index_single_file("workflow.py", source)?;
    for caller in [
        "FactoryPipeline.run",
        "MixedPipeline.run",
        "LocalShadowPipeline.run",
        "FutureAssignmentPipeline.run",
        "StaticAssignmentPipeline.run",
        "ClassAssignmentPipeline.run",
    ] {
        assert_no_resolved_call_to_method_owner(
            "python uncertain property receiver",
            &nodes,
            &edges,
            caller,
            "Workflow",
            "run",
        );
        assert_no_resolved_call_to_method_owner(
            "python uncertain property receiver",
            &nodes,
            &edges,
            caller,
            "Archive",
            "run",
        );
    }
    assert_resolved_call_to_method_owner(
        "python property receiver ignores nested assignment",
        &nodes,
        &edges,
        "NestedAssignmentPipeline.run",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "python property receiver ignores nested assignment",
        &nodes,
        &edges,
        "NestedAssignmentPipeline.run",
        "Archive",
        "run",
    );

    Ok(())
}

#[test]
fn test_python_constructor_local_receiver_call_resolves_to_same_file_owner() -> anyhow::Result<()> {
    let source = r#"
class Archive:
    def persist(self):
        pass

class Workflow:
    def persist(self):
        pass

def run():
    workflow = Workflow()
    workflow.persist()
"#;

    let (nodes, edges) = index_single_file("workflow.py", source)?;
    assert_resolved_call_to_method_owner(
        "python constructor local receiver",
        &nodes,
        &edges,
        "run",
        "Workflow",
        "persist",
    );
    assert_no_resolved_call_to_method_owner(
        "python constructor local receiver",
        &nodes,
        &edges,
        "run",
        "Archive",
        "persist",
    );

    Ok(())
}

#[test]
fn test_python_annotated_factory_local_resolves_declared_interface_not_implementation()
-> anyhow::Result<()> {
    let adapters_source = r#"
class BaseAdapter:
    def send(self, request):
        pass

class HTTPAdapter(BaseAdapter):
    def send(self, request):
        pass
"#;
    let sessions_source = r#"
from .adapters import BaseAdapter

class Session:
    def get_adapter(self, request) -> BaseAdapter:
        raise NotImplementedError

    def send(self, request):
        adapter = self.get_adapter(request)
        return adapter.send(request)
"#;

    let (nodes, edges) = index_files(&[
        ("requests/adapters.py", adapters_source),
        ("requests/sessions.py", sessions_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "python annotated factory interface receiver",
        &nodes,
        &edges,
        "Session.send",
        "BaseAdapter",
        "send",
        "requests/adapters.py",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "python annotated factory does not guess implementation",
        &nodes,
        &edges,
        "Session.send",
        "HTTPAdapter",
        "send",
        "requests/adapters.py",
    );

    Ok(())
}

#[test]
fn test_python_factory_receiver_without_one_precise_return_type_stays_unresolved()
-> anyhow::Result<()> {
    let source = r#"
class BaseAdapter:
    def send(self, request):
        pass

class HTTPAdapter(BaseAdapter):
    def send(self, request):
        pass

class UnannotatedSession:
    def get_adapter(self, request):
        return BaseAdapter()

    def send(self, request):
        adapter = self.get_adapter(request)
        return adapter.send(request)

class UnionSession:
    def get_adapter(self, request) -> BaseAdapter | HTTPAdapter:
        return BaseAdapter()

    def send(self, request):
        adapter = self.get_adapter(request)
        return adapter.send(request)

class ReassignedSession:
    def get_adapter(self, request) -> BaseAdapter:
        return BaseAdapter()

    def send(self, request, runtime_adapter):
        adapter = self.get_adapter(request)
        adapter = runtime_adapter
        return adapter.send(request)
"#;

    let (nodes, edges) = index_single_file("requests/sessions.py", source)?;
    for caller in [
        "UnannotatedSession.send",
        "UnionSession.send",
        "ReassignedSession.send",
    ] {
        for owner in ["BaseAdapter", "HTTPAdapter"] {
            assert_no_resolved_call_to_method_owner(
                "python imprecise factory return",
                &nodes,
                &edges,
                caller,
                owner,
                "send",
            );
        }
    }

    Ok(())
}

#[test]
fn test_python_imported_constructor_local_receiver_resolves_to_imported_owner_method()
-> anyhow::Result<()> {
    let workflow_source = r#"
class Workflow:
    def run(self):
        pass
"#;
    let shadow_source = r#"
class Workflow:
    def run(self):
        pass
"#;
    let source = r#"
from workflow import Workflow
from workflow import Workflow as RemoteWorkflow

def named():
    workflow = Workflow()
    workflow.run()

def aliased():
    workflow = RemoteWorkflow()
    workflow.run()
"#;
    let missing_source = r#"
from missing_workflow import Workflow

def run():
    workflow = Workflow()
    workflow.run()
"#;
    let duplicate_source = r#"
from workflow import Workflow
from shadow import Workflow

def run():
    workflow = Workflow()
    workflow.run()
"#;

    let (nodes, edges) = index_files(&[
        ("workflow.py", workflow_source),
        ("shadow.py", shadow_source),
        ("main.py", source),
    ])?;
    for caller in ["named", "aliased"] {
        assert_resolved_call_to_method_owner_in_file(
            "python imported constructor local receiver",
            &nodes,
            &edges,
            caller,
            "Workflow",
            "run",
            "workflow.py",
        );
        assert_no_resolved_call_to_method_owner_in_file(
            "python imported constructor local receiver",
            &nodes,
            &edges,
            caller,
            "Workflow",
            "run",
            "shadow.py",
        );
    }

    let (missing_nodes, missing_edges) = index_files(&[
        ("workflow.py", workflow_source),
        ("main.py", missing_source),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "python missing imported constructor owner",
        &missing_nodes,
        &missing_edges,
        "run",
        "Workflow",
        "run",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("workflow.py", workflow_source),
        ("shadow.py", shadow_source),
        ("main.py", duplicate_source),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "python duplicate imported constructor owner",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Workflow",
        "run",
    );

    Ok(())
}

#[test]
fn test_python_with_item_constructor_receiver_resolves_to_exact_owner_method() -> anyhow::Result<()>
{
    let sessions_source = r#"
class Session:
    def __enter__(self) -> Self:
        return self

    def request(self, method, url):
        pass
"#;
    let shadow_source = r#"
class Session:
    def __enter__(self) -> Self:
        return self

    def request(self, method, url):
        pass
"#;
    let api_source = r#"
from . import sessions

def request(method, url):
    """Send one outbound request."""
    # Session owns request preparation and adapter selection.
    # Closing the context releases session-owned resources.
    with sessions.Session() as session:
        return session.request(method=method, url=url)
"#;

    let (nodes, edges) = index_files(&[
        ("src/requests/sessions.py", sessions_source),
        ("src/shadow/sessions.py", shadow_source),
        ("src/requests/api.py", api_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "python with-item imported module constructor",
        &nodes,
        &edges,
        "request",
        "Session",
        "request",
        "src/requests/sessions.py",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "python with-item imported module constructor",
        &nodes,
        &edges,
        "request",
        "Session",
        "request",
        "src/shadow/sessions.py",
    );

    Ok(())
}

#[test]
fn test_python_with_item_constructor_receiver_fails_closed_on_imprecise_bindings()
-> anyhow::Result<()> {
    let sessions_source = r#"
class Session:
    def __enter__(self) -> Self:
        return self

    def request(self, method, url):
        pass

class UnannotatedSession:
    def request(self, method, url):
        pass

class OtherSession:
    def request(self, method, url):
        pass

class WrongReturnSession:
    def __enter__(self) -> OtherSession:
        return OtherSession()

    def request(self, method, url):
        pass

class StaticEnterSession:
    @staticmethod
    def __enter__() -> Self:
        return StaticEnterSession()

    def request(self, method, url):
        pass

class DuplicateEnterSession:
    def __enter__(self) -> Self:
        return self

    def __enter__(self) -> Self:
        return self

    def request(self, method, url):
        pass

class AsyncEnterSession:
    async def __enter__(self) -> Self:
        return self

    def request(self, method, url):
        pass
"#;
    let api_source = r#"
from . import sessions
from .sessions import Session

def unannotated_context(method, url):
    with sessions.UnannotatedSession() as session:
        return session.request(method, url)

def wrong_return_context(method, url):
    with sessions.WrongReturnSession() as session:
        return session.request(method, url)

def static_enter_context(method, url):
    with sessions.StaticEnterSession() as session:
        return session.request(method, url)

def duplicate_enter_context(method, url):
    with sessions.DuplicateEnterSession() as session:
        return session.request(method, url)

def parameter_shadowed_module(sessions, method, url):
    with sessions.Session() as session:
        return session.request(method, url)

def parameter_shadowed_type(Session, method, url):
    with Session() as session:
        return session.request(method, url)

def defaulted_parameter_shadowed_module(method, url, sessions=alternate):
    with sessions.Session() as session:
        return session.request(method, url)

def defaulted_parameter_shadowed_type(method, url, Session=Other):
    with Session() as session:
        return session.request(method, url)

def imported_bare_type_context(method, url):
    with Session() as session:
        return session.request(method, url)

def constructor_argument_context(method, url):
    with sessions.Session(option := make_option()) as session:
        return session.request(method, url)

def multi_with_item_context(method, url):
    with sessions.Session() as session, sessions.Session() as other:
        return session.request(method, url)

async def async_with_context(method, url):
    async with sessions.AsyncEnterSession() as session:
        return session.request(method, url)

def async_enter_context(method, url):
    with sessions.AsyncEnterSession() as session:
        return session.request(method, url)

def except_alias_context(method, url):
    with sessions.Session() as session:
        try:
            raise RuntimeError()
        except RuntimeError as session:
            return session.request(method, url)

def walrus_branch_context(method, url):
    with sessions.Session() as session:
        if session := make_session():
            return session.request(method, url)

def comprehension_context(method, url):
    with sessions.Session() as session:
        return [session.request(method, url) for session in sessions]

def enclosing_except_context(method, url):
    try:
        raise RuntimeError()
    except RuntimeError:
        with sessions.Session() as session:
            return session.request(method, url)

def untyped_factory(method, url):
    with make_session() as session:
        return session.request(method, url)

def context_expression(method, url):
    with sessions.open_session() as session:
        return session.request(method, url)

def subscript_alias(method, url):
    with sessions.Session() as state["session"]:
        return state["session"].request(method, url)

def shadowed_alias(method, url):
    with sessions.Session() as session:
        session = make_session()
        return session.request(method, url)

def loop_shadow(method, url):
    with sessions.Session() as session:
        for session in pool:
            return session.request(method, url)

def nested_scope(method, url):
    with sessions.Session() as session:
        def inner():
            return session.request(method, url)
        return inner()

def shadowed_module(method, url):
    sessions = load_sessions()
    with sessions.Session() as session:
        return session.request(method, url)
"#;

    let (nodes, edges) = index_files(&[
        ("src/requests/sessions.py", sessions_source),
        ("src/requests/api.py", api_source),
    ])?;
    for caller in [
        "unannotated_context",
        "wrong_return_context",
        "static_enter_context",
        "duplicate_enter_context",
        "parameter_shadowed_module",
        "parameter_shadowed_type",
        "defaulted_parameter_shadowed_module",
        "defaulted_parameter_shadowed_type",
        "imported_bare_type_context",
        "constructor_argument_context",
        "multi_with_item_context",
        "async_with_context",
        "async_enter_context",
        "except_alias_context",
        "walrus_branch_context",
        "comprehension_context",
        "enclosing_except_context",
        "untyped_factory",
        "context_expression",
        "subscript_alias",
        "shadowed_alias",
        "loop_shadow",
        "nested_scope",
        "shadowed_module",
    ] {
        for owner in [
            "Session",
            "UnannotatedSession",
            "OtherSession",
            "WrongReturnSession",
            "StaticEnterSession",
            "DuplicateEnterSession",
            "AsyncEnterSession",
        ] {
            assert_no_resolved_call_to_method_owner_in_file(
                "python with-item fail-closed guard",
                &nodes,
                &edges,
                caller,
                owner,
                "request",
                "src/requests/sessions.py",
            );
        }
    }

    Ok(())
}

#[test]
fn test_python_constructor_local_visibility_and_shadows_fail_closed() -> anyhow::Result<()> {
    let workflow_source = r#"
class Workflow:
    def run(self):
        pass
"#;
    let shadow_source = r#"
class Workflow:
    def run(self):
        pass

class ShadowWorkflow:
    def run(self):
        pass
"#;
    let source = r#"
from workflow import Workflow

def future_binding():
    workflow.run()
    workflow = Workflow()

def factory_shadow(workflow: Workflow):
    workflow = make_workflow()
    workflow.run()

def tuple_factory_shadow(workflow: Workflow):
    workflow, _ = make_workflow_pair()
    workflow.run()

def constructor_name_shadow():
    Workflow = make_factory()
    workflow = Workflow()
    workflow.run()

def local_from_import_shadow():
    from shadow import Workflow
    workflow = Workflow()
    workflow.run()

def local_import_alias_shadow():
    import shadow as Workflow
    workflow = Workflow()
    workflow.run()

def local_from_import_alias_shadow():
    from shadow import ShadowWorkflow as Workflow
    workflow = Workflow()
    workflow.run()

def local_class_shadow():
    class Workflow:
        def run(self):
            pass
    workflow = Workflow()
    workflow.run()

def future_local_class_shadow():
    workflow = Workflow()
    class Workflow:
        def run(self):
            pass
    workflow.run()
"#;

    let (nodes, edges) = index_files(&[
        ("workflow.py", workflow_source),
        ("shadow.py", shadow_source),
        ("main.py", source),
    ])?;
    for caller in [
        "future_binding",
        "factory_shadow",
        "tuple_factory_shadow",
        "constructor_name_shadow",
        "local_from_import_shadow",
        "local_import_alias_shadow",
        "local_from_import_alias_shadow",
        "future_local_class_shadow",
    ] {
        assert_no_resolved_call_to_method_owner_in_file(
            "python constructor local visibility guard",
            &nodes,
            &edges,
            caller,
            "Workflow",
            "run",
            "workflow.py",
        );
        assert_no_resolved_call_to_method_owner_in_file(
            "python constructor local visibility guard",
            &nodes,
            &edges,
            caller,
            "Workflow",
            "run",
            "shadow.py",
        );
        assert_no_resolved_call_to_method_owner_in_file(
            "python constructor local visibility guard",
            &nodes,
            &edges,
            caller,
            "ShadowWorkflow",
            "run",
            "shadow.py",
        );
    }
    assert_resolved_call_to_method_owner_in_file(
        "python constructor local class shadow",
        &nodes,
        &edges,
        "local_class_shadow",
        "Workflow",
        "run",
        "main.py",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "python constructor local class shadow",
        &nodes,
        &edges,
        "local_class_shadow",
        "Workflow",
        "run",
        "workflow.py",
    );

    Ok(())
}

#[test]
fn test_python_same_line_imported_receiver_calls_keep_distinct_owners() -> anyhow::Result<()> {
    let repository_source = r#"
class Repository:
    def save(self):
        pass
"#;
    let archive_source = r#"
class Archive:
    def save(self):
        pass
"#;
    let workflow_source = r#"
from repository import Repository
from archive import Archive

def run(repo: Repository, archive: Archive):
    repo.save(); archive.save()
"#;

    let (nodes, edges) = index_files(&[
        ("repository.py", repository_source),
        ("archive.py", archive_source),
        ("workflow.py", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "python same-line imported receivers",
        &nodes,
        &edges,
        "run",
        "Repository",
        "save",
        "repository.py",
    );
    assert_resolved_call_to_method_owner_in_file(
        "python same-line imported receivers",
        &nodes,
        &edges,
        "run",
        "Archive",
        "save",
        "archive.py",
    );

    Ok(())
}

#[test]
fn test_python_nested_callable_receiver_call_does_not_attach_to_outer() -> anyhow::Result<()> {
    let source = r#"
class Repository:
    def save(self):
        pass

    def flush(self):
        pass

class Archive:
    def save(self):
        pass

def outer(repo: Repository):
    def inner():
        repo.save()

    repo.flush()
    return inner
"#;

    let (nodes, edges) = index_single_file("workflow.py", source)?;
    assert_resolved_call_to_method_owner(
        "python nested receiver",
        &nodes,
        &edges,
        "outer",
        "Repository",
        "flush",
    );
    assert_no_resolved_call_to_method_owner(
        "python nested receiver",
        &nodes,
        &edges,
        "outer",
        "Repository",
        "save",
    );
    assert_no_resolved_call_to_method_owner(
        "python nested receiver",
        &nodes,
        &edges,
        "outer",
        "Archive",
        "save",
    );

    Ok(())
}

#[test]
fn test_python_returned_call_projects_one_inner_member_call() -> anyhow::Result<()> {
    let source = r#"
class App:
    def ensure_sync(self, function):
        return function

    def dispatch(self):
        pass

    def caller(self):
        return self.ensure_sync(self.dispatch)()
"#;

    let (nodes, edges) = index_single_file("app.py", source)?;
    let ensure_sync = nodes
        .iter()
        .find(|node| is_matching_owned_method(&node.serialized_name, "App", "ensure_sync"))
        .expect("ensure_sync declaration");
    let projected = edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::CALL
                && edge.line == Some(10)
                && edge.effective_target() == ensure_sync.id
        })
        .collect::<Vec<_>>();
    assert_eq!(
        projected.len(),
        1,
        "duplicate returned-call projection: {projected:#?}"
    );
    Ok(())
}

#[test]
fn test_python_parenthesized_member_call_is_transparent_but_returned_call_is_not()
-> anyhow::Result<()> {
    let source = r#"
class App:
    def run(self):
        pass

    def ensure_sync(self, function):
        return function

    def caller(self):
        (self.run)()
        return self.ensure_sync(self.run)()
"#;

    let (nodes, edges) = index_single_file("app.py", source)?;
    let run = nodes
        .iter()
        .find(|node| is_matching_owned_method(&node.serialized_name, "App", "run"))
        .expect("run declaration");
    let ensure_sync = nodes
        .iter()
        .find(|node| is_matching_owned_method(&node.serialized_name, "App", "ensure_sync"))
        .expect("ensure_sync declaration");

    let parenthesized = edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::CALL
                && edge.line == Some(10)
                && edge.effective_target() == run.id
        })
        .count();
    assert_eq!(parenthesized, 1, "parenthesized member call: {edges:#?}");

    let returned = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.line == Some(11))
        .collect::<Vec<_>>();
    assert_eq!(
        returned
            .iter()
            .filter(|edge| edge.effective_target() == ensure_sync.id)
            .count(),
        1,
        "returned-call inner member must project once: {returned:#?}"
    );
    assert_eq!(
        returned
            .iter()
            .filter(|edge| edge.effective_target() == run.id)
            .count(),
        0,
        "callable-return invocation must not project as the argument member: {returned:#?}"
    );

    Ok(())
}

#[test]
fn test_go_imported_receiver_uses_qualified_import_owner() -> anyhow::Result<()> {
    let notifier_source = r#"
package notifier

type Notifier interface {
    Notify()
}
"#;
    let workflow_source = r#"
package workflow

import mail "example.com/project/notifier"

type Notifier interface {
    Notify()
}

func Run(n mail.Notifier) {
    n.Notify()
}
"#;
    let other_notifier_source = r#"
package other

type Notifier interface {
    Notify()
}
"#;

    let (nodes, edges) = index_files(&[
        ("project/notifier/notifier.go", notifier_source),
        ("other/notifier/notifier.go", other_notifier_source),
        ("workflow.go", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "go imported receiver",
        &nodes,
        &edges,
        "Run",
        "Notifier",
        "Notify",
        "project/notifier/notifier.go",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "go imported receiver",
        &nodes,
        &edges,
        "Run",
        "Notifier",
        "Notify",
        "workflow.go",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "go imported receiver",
        &nodes,
        &edges,
        "Run",
        "Notifier",
        "Notify",
        "other/notifier/notifier.go",
    );

    let (missing_import_nodes, missing_import_edges) = index_files(&[
        ("other/notifier/notifier.go", other_notifier_source),
        ("workflow.go", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "go imported receiver missing package",
        &missing_import_nodes,
        &missing_import_edges,
        "Run",
        "Notifier",
        "Notify",
        "other/notifier/notifier.go",
    );

    let no_import_workflow_source = r#"
package workflow

func Run(n Notifier) {
    n.Notify()
}
"#;
    let (no_import_nodes, no_import_edges) = index_files(&[
        ("project/notifier/notifier.go", notifier_source),
        ("workflow.go", no_import_workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "go unimported receiver type stays unresolved",
        &no_import_nodes,
        &no_import_edges,
        "Run",
        "Notifier",
        "Notify",
        "project/notifier/notifier.go",
    );

    Ok(())
}

#[test]
fn test_go_same_file_composite_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let workflow_source = r#"
package workflow

type Event struct{}

type Notifier interface {
    Notify(Event)
}

type Repository struct{}

func (Repository) Save(Event) {}

type Workflow struct {
    notifier Notifier
    repo     Repository
}

func (w Workflow) Run(event Event) {
    w.notifier.Notify(event)
    w.repo.Save(event)
}

type OtherWorkflow struct{}

func (OtherWorkflow) Run(Event) {}

type NoRun struct{}

func makeWorkflow() Workflow {
    return Workflow{}
}

func wrap(Workflow) any {
    return Workflow{}
}

func orchestrate() {
    workflow := Workflow{}
    workflow.Run(Event{})
}

func keyedComposite() {
    workflow := Workflow{repo: Repository{}}
    workflow.Run(Event{})
}

func pointerComposite() {
    workflow := &Workflow{}
    workflow.Run(Event{})
}

func factoryOnly() {
    workflow := makeWorkflow()
    workflow.Run(Event{})
}

func assignmentComposite() {
    workflow := makeWorkflow()
    workflow = Workflow{}
    workflow.Run(Event{})
}

func reassignedFactory() {
    workflow := Workflow{}
    workflow = makeWorkflow()
    workflow.Run(Event{})
}

func redeclaredFactory() {
    workflow := Workflow{}
    workflow, err := makeWorkflow()
    _ = err
    workflow.Run(Event{})
}

func redeclaredComposite() {
    workflow := NoRun{}
    workflow, ok := Workflow{}, true
    _ = ok
    workflow.Run(Event{})
}

func wrappedComposite() {
    workflow := wrap(Workflow{})
    workflow.Run(Event{})
}

func directOnly() {
    makeWorkflow()
}

func erased(workflow any) {
    workflow.Run(Event{})
}

func orderAware() {
    workflow.Run(Event{})
    workflow := Workflow{}
    workflow.Run(Event{})
}

func innerShadow(workflow Workflow) {
    if true {
        workflow := NoRun{}
        workflow.Run(Event{})
    }
}

func outerParamStillWorks(workflow Workflow) {
    if true {
        inner := NoRun{}
        inner.Run(Event{})
    }
    workflow.Run(Event{})
}
"#;
    let (nodes, edges) = index_files(&[("workflow.go", workflow_source)])?;
    assert_resolved_call_to_method_owner_in_file(
        "go same-file composite receiver",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "Run",
        "workflow.go",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "go same-file composite receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "orchestrate",
            owner_name: "Workflow",
            method_name: "Run",
            file_suffix: "workflow.go",
            expected_count: 1,
        },
    );
    assert_resolved_call_to_method_owner_in_file(
        "go pointer composite receiver",
        &nodes,
        &edges,
        "pointerComposite",
        "Workflow",
        "Run",
        "workflow.go",
    );
    assert_resolved_call_to_method_owner_in_file(
        "go assignment composite receiver",
        &nodes,
        &edges,
        "assignmentComposite",
        "Workflow",
        "Run",
        "workflow.go",
    );
    assert_resolved_call_to_method_owner_in_file(
        "go multi-name redeclared composite receiver",
        &nodes,
        &edges,
        "redeclaredComposite",
        "Workflow",
        "Run",
        "workflow.go",
    );
    assert_resolved_call_to_method_owner_in_file(
        "go method receiver field interface",
        &nodes,
        &edges,
        "Run",
        "Notifier",
        "Notify",
        "workflow.go",
    );
    assert_resolved_call_to_method_owner_in_file(
        "go method receiver field struct",
        &nodes,
        &edges,
        "Run",
        "Repository",
        "Save",
        "workflow.go",
    );
    assert_resolved_call_to_method_owner_in_file(
        "go keyed composite receiver",
        &nodes,
        &edges,
        "keyedComposite",
        "Workflow",
        "Run",
        "workflow.go",
    );
    assert_no_resolved_call_to_method_owner(
        "go same-file composite receiver avoids other owner",
        &nodes,
        &edges,
        "orchestrate",
        "OtherWorkflow",
        "Run",
    );
    assert_no_resolved_call_to_method_owner(
        "go factory receiver stays unresolved",
        &nodes,
        &edges,
        "factoryOnly",
        "Workflow",
        "Run",
    );
    assert_no_resolved_call_to_method_owner(
        "go reassigned factory receiver invalidates stale owner",
        &nodes,
        &edges,
        "reassignedFactory",
        "Workflow",
        "Run",
    );
    assert_no_resolved_call_to_method_owner(
        "go redeclared factory receiver invalidates stale owner",
        &nodes,
        &edges,
        "redeclaredFactory",
        "Workflow",
        "Run",
    );
    assert_no_resolved_call_to_method_owner(
        "go wrapper call containing composite stays unresolved",
        &nodes,
        &edges,
        "wrappedComposite",
        "Workflow",
        "Run",
    );
    assert_resolved_call_to_name(
        "go direct call remains resolved",
        &nodes,
        &edges,
        "directOnly",
        "makeWorkflow",
    );
    assert_no_resolved_call_to_method_owner(
        "go erased receiver stays unresolved",
        &nodes,
        &edges,
        "erased",
        "Workflow",
        "Run",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "go composite binding only applies after declaration",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "orderAware",
            owner_name: "Workflow",
            method_name: "Run",
            file_suffix: "workflow.go",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner(
        "go local composite shadows typed parameter",
        &nodes,
        &edges,
        "innerShadow",
        "Workflow",
        "Run",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "go outer typed parameter survives unrelated inner local",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "outerParamStillWorks",
            owner_name: "Workflow",
            method_name: "Run",
            file_suffix: "workflow.go",
            expected_count: 1,
        },
    );

    let imported_owner_source = r#"
package external

type Event struct{}

type Workflow struct{}

func (Workflow) Run(Event) {}
"#;
    let imported_caller_source = r#"
package app

import external "example.com/project/external"

func orchestrate() {
    workflow := Workflow{}
    workflow.Run(external.Event{})
}
"#;
    let (imported_nodes, imported_edges) = index_files(&[
        ("project/external/workflow.go", imported_owner_source),
        ("app/workflow.go", imported_caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "go unqualified composite receiver does not use imported cross-file owner",
        &imported_nodes,
        &imported_edges,
        "orchestrate",
        "Workflow",
        "Run",
        "project/external/workflow.go",
    );

    Ok(())
}

#[test]
fn test_go_imported_composite_receiver_call_resolves_to_qualified_import_owner()
-> anyhow::Result<()> {
    let external_source = r#"
package external

type Event struct{}

type Workflow struct{}

func (Workflow) Run(Event) {}
"#;
    let other_source = r#"
package other

type Event struct{}

type Workflow struct{}

func (Workflow) Run(Event) {}
"#;
    let caller_source = r#"
package app

import external "example.com/project/external"

func orchestrate() {
    workflow := external.Workflow{}
    workflow.Run(external.Event{})
}

func pointerComposite() {
    workflow := &external.Workflow{}
    workflow.Run(external.Event{})
}
"#;
    let missing_import_source = r#"
package app

import missing "example.com/project/missing"

func orchestrate() {
    workflow := missing.Workflow{}
    workflow.Run(missing.Event{})
}
"#;
    let duplicate_alias_source = r#"
package app

import (
    external "example.com/project/external"
    external "example.com/project/other"
)

func orchestrate() {
    workflow := external.Workflow{}
    workflow.Run(external.Event{})
}
"#;

    let (nodes, edges) = index_files(&[
        ("project/external/workflow.go", external_source),
        ("project/other/workflow.go", other_source),
        ("app/workflow.go", caller_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "go qualified imported composite receiver",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "Run",
        "project/external/workflow.go",
    );
    assert_resolved_call_to_method_owner_in_file(
        "go pointer qualified imported composite receiver",
        &nodes,
        &edges,
        "pointerComposite",
        "Workflow",
        "Run",
        "project/external/workflow.go",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "go qualified imported composite avoids other owner",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "Run",
        "project/other/workflow.go",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("project/external/workflow.go", external_source),
        ("app/workflow.go", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "go missing qualified imported composite receiver",
        &missing_nodes,
        &missing_edges,
        "orchestrate",
        "Workflow",
        "Run",
        "project/external/workflow.go",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("project/external/workflow.go", external_source),
        ("project/other/workflow.go", other_source),
        ("app/workflow.go", duplicate_alias_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "go duplicate qualified imported composite receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "orchestrate",
        "Workflow",
        "Run",
        "project/external/workflow.go",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "go duplicate qualified imported composite receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "orchestrate",
        "Workflow",
        "Run",
        "project/other/workflow.go",
    );

    Ok(())
}

#[test]
fn test_go_imported_method_receiver_field_resolves_to_qualified_import_owner() -> anyhow::Result<()>
{
    let notifier_source = r#"
package notifier

type Notifier interface {
    Notify()
}

type PointerNotifier struct{}

func (*PointerNotifier) Notify() {}
"#;
    let duplicate_notifier_source = r#"
package notifier

type Notifier interface {
    Notify()
}
"#;
    let other_notifier_source = r#"
package other

type Notifier interface {
    Notify()
}
"#;
    let workflow_source = r#"
package workflow

import mail "example.com/project/notifier"

type Notifier interface {
    Notify()
}

type Workflow struct {
    notifier mail.Notifier
    pointer  *mail.PointerNotifier
}

func (w Workflow) Run() {
    w.notifier.Notify()
    w.pointer.Notify()
}
"#;
    let missing_import_source = r#"
package workflow

import mail "example.com/project/missing"

type Workflow struct {
    notifier mail.Notifier
}

func (w Workflow) Run() {
    w.notifier.Notify()
}
"#;
    let no_import_source = r#"
package workflow

type Workflow struct {
    notifier mail.Notifier
}

func (w Workflow) Run() {
    w.notifier.Notify()
}
"#;

    let (nodes, edges) = index_files(&[
        ("project/notifier/notifier.go", notifier_source),
        ("other/notifier/notifier.go", other_notifier_source),
        ("workflow.go", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "go imported method receiver field",
        &nodes,
        &edges,
        "Run",
        "Notifier",
        "Notify",
        "project/notifier/notifier.go",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "go imported method receiver field avoids local same-name owner",
        &nodes,
        &edges,
        "Run",
        "Notifier",
        "Notify",
        "workflow.go",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "go imported method receiver field avoids other package owner",
        &nodes,
        &edges,
        "Run",
        "Notifier",
        "Notify",
        "other/notifier/notifier.go",
    );
    assert_resolved_call_to_method_owner_in_file(
        "go imported pointer method receiver field",
        &nodes,
        &edges,
        "Run",
        "PointerNotifier",
        "Notify",
        "project/notifier/notifier.go",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("project/notifier/notifier.go", notifier_source),
        (
            "project/notifier/alternate_notifier.go",
            duplicate_notifier_source,
        ),
        ("workflow.go", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "go duplicate imported method receiver field owner",
        &duplicate_nodes,
        &duplicate_edges,
        "Run",
        "Notifier",
        "Notify",
        "project/notifier/notifier.go",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "go duplicate imported method receiver field owner",
        &duplicate_nodes,
        &duplicate_edges,
        "Run",
        "Notifier",
        "Notify",
        "project/notifier/alternate_notifier.go",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("project/notifier/notifier.go", notifier_source),
        ("workflow.go", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "go imported method receiver field missing package",
        &missing_nodes,
        &missing_edges,
        "Run",
        "Notifier",
        "Notify",
        "project/notifier/notifier.go",
    );

    let (no_import_nodes, no_import_edges) = index_files(&[
        ("project/notifier/notifier.go", notifier_source),
        ("workflow.go", no_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "go qualified field type without import stays unresolved",
        &no_import_nodes,
        &no_import_edges,
        "Run",
        "Notifier",
        "Notify",
        "project/notifier/notifier.go",
    );

    Ok(())
}

#[test]
fn test_java_typed_receiver_call_resolves_to_declared_parameter_type() -> anyhow::Result<()> {
    let source = r#"
interface EventListener {
    void handleEvent();
}

class ConcreteListener {
    void handleEvent() {}
}

class AuditListener {
    void handleEvent() {}
}

class EventBus {
    void dispatchTo(EventListener listener, ConcreteListener concrete) {
        listener.handleEvent(); concrete.handleEvent();
    }
}
"#;

    let (nodes, edges) = index_single_file("Test.java", source)?;
    assert_resolved_call_to_method_owner(
        "java typed receiver",
        &nodes,
        &edges,
        "dispatchTo",
        "EventListener",
        "handleEvent",
    );
    assert_resolved_call_to_method_owner(
        "java typed receiver",
        &nodes,
        &edges,
        "dispatchTo",
        "ConcreteListener",
        "handleEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "java typed receiver",
        &nodes,
        &edges,
        "dispatchTo",
        "AuditListener",
        "handleEvent",
    );

    Ok(())
}

#[test]
fn test_java_same_file_local_receiver_call_resolves_to_declared_owner_method() -> anyhow::Result<()>
{
    let source = r#"
package com.acme.workflow;

class Event {}

class Workflow {
    void run(Event event) {}
}

class OtherWorkflow {
    void run(Event event) {}
}

class NoRun {}

class CloseableWorkflow implements AutoCloseable {
    void run(Event event) {}

    public void close() {}
}

class WorkflowException extends Exception {
    void run(Event event) {}
}

class Entry {
    static Workflow makeWorkflow() {
        return new Workflow();
    }

    static Object wrap(Workflow workflow) {
        return workflow;
    }

    static void orchestrate() {
        Workflow workflow = new Workflow();
        workflow.run(new Event());
    }

    static void typedFactory() {
        Workflow workflow = makeWorkflow();
        workflow.run(new Event());
    }

    static void staticQualified() {
        Workflow workflow = Entry.makeWorkflow();
        workflow.run(new Event());
    }

    static void staticFullyQualified() {
        Workflow workflow = com.acme.workflow.Entry.makeWorkflow();
        workflow.run(new Event());
    }

    static void varConstructor() {
        var workflow = new Workflow();
        workflow.run(new Event());
    }

    static void varFactory() {
        var workflow = makeWorkflow();
        workflow.run(new Event());
    }

    static void directConstructor() {
        new Workflow().run(new Event());
    }

    static void objectWrapped() {
        Object workflow = wrap(new Workflow());
        workflow.run(new Event());
    }

    static void directOnly() {
        makeWorkflow();
    }

    static void erased(Object workflow) {
        workflow.run(new Event());
    }

    static void orderAware() {
        workflow.run(new Event());
        Workflow workflow = new Workflow();
        workflow.run(new Event());
    }

    static void innerShadow(Workflow workflow) {
        if (true) {
            NoRun workflow = new NoRun();
            workflow.run(new Event());
        }
    }

    static void outerParamStillWorks(Workflow workflow) {
        if (true) {
            NoRun other = new NoRun();
            other.run(new Event());
        }
        workflow.run(new Event());
    }

    static void enhancedFor(java.util.List<Workflow> workflows) {
        for (Workflow workflow : workflows) {
            workflow.run(new Event());
        }
    }

    static void enhancedForDoesNotLeak(java.util.List<Workflow> workflows) {
        for (Workflow workflow : workflows) {}
        workflow.run(new Event());
    }

    static void tryResource() {
        try (CloseableWorkflow workflow = new CloseableWorkflow()) {
            workflow.run(new Event());
        } catch (Exception ignored) {}
    }

    static void catchParameter() {
        try {
            throw new WorkflowException();
        } catch (WorkflowException workflow) {
            workflow.run(new Event());
        }
    }
}
"#;

    let (nodes, edges) = index_single_file("src/com/acme/workflow/Entry.java", source)?;
    assert_resolved_call_to_method_owner_in_file(
        "java same-file local receiver",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/com/acme/workflow/Entry.java",
    );
    assert_resolved_call_to_method_owner_in_file(
        "java typed factory local receiver",
        &nodes,
        &edges,
        "typedFactory",
        "Workflow",
        "run",
        "src/com/acme/workflow/Entry.java",
    );
    assert_resolved_call_to_name(
        "java class-qualified static call remains resolved",
        &nodes,
        &edges,
        "staticQualified",
        "makeWorkflow",
    );
    assert_resolved_call_to_name(
        "java fully-qualified static call remains resolved",
        &nodes,
        &edges,
        "staticFullyQualified",
        "makeWorkflow",
    );
    assert_resolved_call_to_method_owner_in_file(
        "java var direct constructor local receiver",
        &nodes,
        &edges,
        "varConstructor",
        "Workflow",
        "run",
        "src/com/acme/workflow/Entry.java",
    );
    assert_no_resolved_call_to_method_owner(
        "java var factory local receiver stays unresolved",
        &nodes,
        &edges,
        "varFactory",
        "Workflow",
        "run",
    );
    assert_resolved_call_to_method_owner_in_file(
        "java direct constructor receiver",
        &nodes,
        &edges,
        "directConstructor",
        "Workflow",
        "run",
        "src/com/acme/workflow/Entry.java",
    );
    assert_no_resolved_call_to_method_owner(
        "java object-typed wrapper stays unresolved",
        &nodes,
        &edges,
        "objectWrapped",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "java erased receiver stays unresolved",
        &nodes,
        &edges,
        "erased",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "java local receiver avoids other owner",
        &nodes,
        &edges,
        "orchestrate",
        "OtherWorkflow",
        "run",
    );
    assert_resolved_call_to_name(
        "java direct call remains resolved",
        &nodes,
        &edges,
        "directOnly",
        "makeWorkflow",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "java local binding only applies after declaration",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "orderAware",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "src/com/acme/workflow/Entry.java",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner(
        "java local declaration shadows typed parameter",
        &nodes,
        &edges,
        "innerShadow",
        "Workflow",
        "run",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "java outer typed parameter survives unrelated inner local",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "outerParamStillWorks",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "src/com/acme/workflow/Entry.java",
            expected_count: 1,
        },
    );
    assert_resolved_call_to_method_owner_in_file(
        "java enhanced-for receiver",
        &nodes,
        &edges,
        "enhancedFor",
        "Workflow",
        "run",
        "src/com/acme/workflow/Entry.java",
    );
    assert_no_resolved_call_to_method_owner(
        "java enhanced-for receiver does not leak",
        &nodes,
        &edges,
        "enhancedForDoesNotLeak",
        "Workflow",
        "run",
    );
    assert_resolved_call_to_method_owner_in_file(
        "java try-with-resources receiver",
        &nodes,
        &edges,
        "tryResource",
        "CloseableWorkflow",
        "run",
        "src/com/acme/workflow/Entry.java",
    );
    assert_resolved_call_to_method_owner_in_file(
        "java catch parameter receiver",
        &nodes,
        &edges,
        "catchParameter",
        "WorkflowException",
        "run",
        "src/com/acme/workflow/Entry.java",
    );

    Ok(())
}

#[test]
fn test_java_same_file_field_receiver_call_resolves_to_declared_owner_method() -> anyhow::Result<()>
{
    let source = r#"
package com.acme.workflow;

class Event {}

interface Notifier {
    void notifyEvent(Event event);
}

class ConsoleNotifier implements Notifier {
    public void notifyEvent(Event event) {}
}

class Repository {
    void save(Event event) {}
}

class Workflow {
    private final Notifier notifier;
    private Repository repository;
    private Object erased;

    Workflow(Notifier notifier, Repository repository) {
        this.notifier = notifier;
        this.repository = repository;
        this.erased = notifier;
    }

    void run(Event event) {
        notifier.notifyEvent(event);
        this.repository.save(event);
        this.decorate(event);
    }

    void localShadowsField(Event event) {
        Repository notifier = new Repository();
        notifier.save(event);
        notifier.notifyEvent(event);
    }

    void erasedField(Event event) {
        erased.notifyEvent(event);
    }

    void decorate(Event event) {}
}

class OtherWorkflow {
    void decorate(Event event) {}
}
"#;

    let (nodes, edges) = index_single_file("src/com/acme/workflow/Workflow.java", source)?;
    assert_resolved_call_to_method_owner_in_file(
        "java bare field receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/workflow/Workflow.java",
    );
    assert_resolved_call_to_method_owner_in_file(
        "java this field receiver",
        &nodes,
        &edges,
        "run",
        "Repository",
        "save",
        "src/com/acme/workflow/Workflow.java",
    );
    assert_resolved_call_to_method_owner_in_file(
        "java this self receiver",
        &nodes,
        &edges,
        "run",
        "Workflow",
        "decorate",
        "src/com/acme/workflow/Workflow.java",
    );
    assert_no_resolved_call_to_method_owner(
        "java self receiver avoids same-named owner",
        &nodes,
        &edges,
        "run",
        "OtherWorkflow",
        "decorate",
    );
    assert_resolved_call_to_method_owner_in_file(
        "java local receiver shadows field receiver",
        &nodes,
        &edges,
        "localShadowsField",
        "Repository",
        "save",
        "src/com/acme/workflow/Workflow.java",
    );
    assert_no_resolved_call_to_method_owner(
        "java local receiver shadow prevents field fallback",
        &nodes,
        &edges,
        "localShadowsField",
        "Notifier",
        "notifyEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "java erased field receiver stays unresolved",
        &nodes,
        &edges,
        "erasedField",
        "Notifier",
        "notifyEvent",
    );

    let owner_source = r#"
package com.acme.model;

public class Workflow {
    public void run() {}
}
"#;
    let caller_source = r#"
package com.acme.app;

class Entry {
    private Workflow workflow;

    void call() {
        workflow.run();
    }
}
"#;
    let (cross_file_nodes, cross_file_edges) = index_files(&[
        ("src/com/acme/model/Workflow.java", owner_source),
        ("src/com/acme/app/Entry.java", caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "java field receiver does not use unimported cross-file owner",
        &cross_file_nodes,
        &cross_file_edges,
        "call",
        "Workflow",
        "run",
        "src/com/acme/model/Workflow.java",
    );

    Ok(())
}

#[test]
fn test_java_imported_field_receiver_call_resolves_to_exact_imported_owner_method()
-> anyhow::Result<()> {
    let mail_notifier_source = r#"
package com.acme.mail;

public interface Notifier {
    void notifyEvent(String value);
}
"#;
    let other_notifier_source = r#"
package com.acme.other;

public interface Notifier {
    void notifyEvent(String value);
}
"#;
    let workflow_source = r#"
package com.acme.workflow;

import com.acme.mail.Notifier;

class Workflow {
    private Notifier notifier;

    void run() {
        notifier.notifyEvent("ready");
    }
}
"#;
    let duplicate_import_source = r#"
package com.acme.workflow;

import com.acme.mail.Notifier;
import com.acme.other.Notifier;

class Workflow {
    private Notifier notifier;

    void run() {
        notifier.notifyEvent("ready");
    }
}
"#;

    let (nodes, edges) = index_files(&[
        ("src/com/acme/mail/Notifier.java", mail_notifier_source),
        ("src/com/acme/other/Notifier.java", other_notifier_source),
        ("src/com/acme/workflow/Workflow.java", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "java imported field receiver exact package",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.java",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "java imported field receiver exact package",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/other/Notifier.java",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.java", mail_notifier_source),
        ("src/com/acme/other/Notifier.java", other_notifier_source),
        (
            "src/com/acme/workflow/Workflow.java",
            duplicate_import_source,
        ),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "java duplicate imported field receiver local name",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.java",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "java duplicate imported field receiver local name",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/other/Notifier.java",
    );

    Ok(())
}

#[test]
fn test_java_imported_typed_receiver_call_resolves_to_exact_imported_owner_method()
-> anyhow::Result<()> {
    let mail_notifier_source = r#"
package com.acme.mail;

public interface Notifier {
    void notifyEvent(String value);
}
"#;
    let other_notifier_source = r#"
package com.acme.other;

public interface Notifier {
    void notifyEvent(String value);
}
"#;
    let workflow_source = r#"
package com.acme.workflow;

import com.acme.mail.Notifier;

class Workflow {
    void run(Notifier notifier) {
        notifier.notifyEvent("ready");
    }
}
"#;
    let missing_import_source = r#"
package com.acme.workflow;

import com.acme.missing.Notifier;

class Workflow {
    void run(Notifier notifier) {
        notifier.notifyEvent("ready");
    }
}
"#;
    let duplicate_import_source = r#"
package com.acme.workflow;

import com.acme.mail.Notifier;
import com.acme.other.Notifier;

class Workflow {
    void run(Notifier notifier) {
        notifier.notifyEvent("ready");
    }
}
"#;
    let local_shadow_source = r#"
package com.acme.workflow;

import com.acme.mail.Notifier;

interface Notifier {
    void notifyEvent(String value);
}

class Workflow {
    void run(Notifier notifier) {
        notifier.notifyEvent("ready");
    }
}
"#;

    let (nodes, edges) = index_files(&[
        ("src/com/acme/mail/Notifier.java", mail_notifier_source),
        ("src/com/acme/other/Notifier.java", other_notifier_source),
        ("src/com/acme/workflow/Workflow.java", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "java imported receiver exact package",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.java",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "java imported receiver exact package",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Notifier",
            method_name: "notifyEvent",
            file_suffix: "src/com/acme/mail/Notifier.java",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "java imported receiver exact package",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/other/Notifier.java",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.java", mail_notifier_source),
        ("src/com/acme/workflow/Workflow.java", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "java missing imported receiver package",
        &missing_nodes,
        &missing_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.java",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.java", mail_notifier_source),
        ("src/com/acme/other/Notifier.java", other_notifier_source),
        (
            "src/com/acme/workflow/Workflow.java",
            duplicate_import_source,
        ),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "java duplicate imported receiver local name",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.java",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "java duplicate imported receiver local name",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/other/Notifier.java",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.java", mail_notifier_source),
        ("src/com/acme/workflow/Workflow.java", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "java local receiver shadows imported type",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/workflow/Workflow.java",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "java local receiver shadows imported type",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.java",
    );

    Ok(())
}

#[test]
fn test_kotlin_imported_typed_receiver_call_resolves_to_exact_imported_owner_method()
-> anyhow::Result<()> {
    let mail_notifier_source = r#"
package com.acme.mail

interface Notifier {
    fun notifyEvent(value: String)
}
"#;
    let other_notifier_source = r#"
package com.acme.other

interface Notifier {
    fun notifyEvent(value: String)
}
"#;
    let workflow_source = r#"
package com.acme.workflow

import com.acme.mail.Notifier

fun run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let alias_workflow_source = r#"
package com.acme.workflow

import com.acme.mail.Notifier as MailNotifier

fun run(notifier: MailNotifier) {
    notifier.notifyEvent("ready")
}
"#;
    let missing_import_source = r#"
package com.acme.workflow

import com.acme.missing.Notifier

fun run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let duplicate_import_source = r#"
package com.acme.workflow

import com.acme.mail.Notifier
import com.acme.other.Notifier

fun run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let wildcard_import_source = r#"
package com.acme.workflow

import com.acme.mail.*

fun run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let no_import_source = r#"
package com.acme.workflow

fun run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let local_shadow_source = r#"
package com.acme.workflow

import com.acme.mail.Notifier

interface Notifier {
    fun notifyEvent(value: String)
}

fun run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;

    let (nodes, edges) = index_files(&[
        ("src/com/acme/mail/Notifier.kt", mail_notifier_source),
        ("src/com/acme/other/Notifier.kt", other_notifier_source),
        ("src/com/acme/workflow/Workflow.kt", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "kotlin imported receiver exact package",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.kt",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "kotlin imported receiver exact package",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Notifier",
            method_name: "notifyEvent",
            file_suffix: "src/com/acme/mail/Notifier.kt",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin imported receiver exact package",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/other/Notifier.kt",
    );

    let (alias_nodes, alias_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.kt", mail_notifier_source),
        ("src/com/acme/other/Notifier.kt", other_notifier_source),
        ("src/com/acme/workflow/Workflow.kt", alias_workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "kotlin aliased imported receiver exact package",
        &alias_nodes,
        &alias_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.kt",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin aliased imported receiver exact package",
        &alias_nodes,
        &alias_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/other/Notifier.kt",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.kt", mail_notifier_source),
        ("src/com/acme/workflow/Workflow.kt", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin missing imported receiver package",
        &missing_nodes,
        &missing_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.kt",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.kt", mail_notifier_source),
        ("src/com/acme/other/Notifier.kt", other_notifier_source),
        ("src/com/acme/workflow/Workflow.kt", duplicate_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin duplicate imported receiver local name",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.kt",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin duplicate imported receiver local name",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/other/Notifier.kt",
    );

    let (wildcard_nodes, wildcard_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.kt", mail_notifier_source),
        ("src/com/acme/workflow/Workflow.kt", wildcard_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin wildcard imported receiver stays unresolved",
        &wildcard_nodes,
        &wildcard_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.kt",
    );

    let (no_import_nodes, no_import_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.kt", mail_notifier_source),
        ("src/com/acme/workflow/Workflow.kt", no_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin unimported receiver type stays unresolved",
        &no_import_nodes,
        &no_import_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.kt",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.kt", mail_notifier_source),
        ("src/com/acme/workflow/Workflow.kt", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "kotlin local receiver shadows imported type",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/workflow/Workflow.kt",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin local receiver shadows imported type",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.kt",
    );

    Ok(())
}

#[test]
fn test_kotlin_same_file_constructor_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let workflow_source = r#"
package app

class Workflow {
    fun run(value: String) {}
}

class OtherWorkflow {
    fun run(value: String) {}
}

fun makeWorkflow(): Workflow {
    return Workflow()
}

fun orchestrate() {
    val workflow = Workflow()
    workflow.run("ready")
}

fun factoryOnly() {
    val workflow = makeWorkflow()
    workflow.run("ready")
}

fun erased(workflow: Any) {
    workflow.run("ready")
}

fun innerShadow(workflow: Any, enabled: Boolean) {
    if (enabled) {
        val workflow = Workflow()
        workflow.run("inner")
    }
    workflow.run("outer")
}

fun orderAware() {
    workflow.run("early")
    val workflow = Workflow()
    workflow.run("late")
}
"#;
    let (nodes, edges) = index_files(&[("src/app/Workflow.kt", workflow_source)])?;
    assert_resolved_call_to_method_owner_in_file(
        "kotlin same-file constructor receiver",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/app/Workflow.kt",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "kotlin same-file constructor receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "orchestrate",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "src/app/Workflow.kt",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin same-file constructor receiver avoids other owner",
        &nodes,
        &edges,
        "orchestrate",
        "OtherWorkflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin factory receiver stays unresolved",
        &nodes,
        &edges,
        "factoryOnly",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin erased receiver stays unresolved",
        &nodes,
        &edges,
        "erased",
        "Workflow",
        "run",
    );
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let factory_call_resolved = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter_map(|edge| {
            let source = node_by_id.get(&edge.source)?;
            if !is_matching_name(&source.serialized_name, "factoryOnly") {
                return None;
            }
            let resolved_id = edge.resolved_target?;
            let resolved_node = node_by_id.get(&resolved_id)?;
            Some(resolved_node.serialized_name.as_str())
        })
        .any(|resolved_name| is_matching_name(resolved_name, "makeWorkflow"));
    assert!(
        factory_call_resolved,
        "Case `kotlin factory direct call remains resolved`: expected direct CALL from `factoryOnly` to resolve to `makeWorkflow`. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "kotlin inner constructor binding stays block-scoped",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "innerShadow",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "src/app/Workflow.kt",
            expected_count: 1,
        },
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "kotlin constructor binding only applies after declaration",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "orderAware",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "src/app/Workflow.kt",
            expected_count: 1,
        },
    );

    let imported_owner_source = r#"
package other

class Workflow {
    fun run(value: String) {}
}
"#;
    let other_imported_owner_source = r#"
package alternate

class Workflow {
    fun run(value: String) {}
}
"#;
    let imported_caller_source = r#"
package app

import other.Workflow

fun orchestrate() {
    val workflow = Workflow()
    workflow.run("ready")
}
"#;
    let alias_imported_caller_source = r#"
package app

import other.Workflow as RemoteWorkflow

fun orchestrate() {
    val workflow = RemoteWorkflow()
    workflow.run("ready")
}
"#;
    let missing_import_source = r#"
package app

import missing.Workflow

fun orchestrate() {
    val workflow = Workflow()
    workflow.run("ready")
}
"#;
    let duplicate_import_source = r#"
package app

import other.Workflow
import alternate.Workflow

fun orchestrate() {
    val workflow = Workflow()
    workflow.run("ready")
}
"#;
    let wildcard_import_source = r#"
package app

import other.*

fun orchestrate() {
    val workflow = Workflow()
    workflow.run("ready")
}
"#;
    let no_import_source = r#"
package app

fun orchestrate() {
    val workflow = Workflow()
    workflow.run("ready")
}
"#;
    let local_shadow_source = r#"
package app

import other.Workflow

class Workflow {
    fun run(value: String) {}
}

fun orchestrate() {
    val workflow = Workflow()
    workflow.run("ready")
}
"#;
    let alias_local_shadow_source = r#"
package app

import other.Workflow as RemoteWorkflow

class RemoteWorkflow {
    fun run(value: String) {}
}

fun orchestrate() {
    val workflow = RemoteWorkflow()
    workflow.run("ready")
}
"#;
    let (imported_nodes, imported_edges) = index_files(&[
        ("src/other/Workflow.kt", imported_owner_source),
        ("src/alternate/Workflow.kt", other_imported_owner_source),
        ("src/app/UseWorkflow.kt", imported_caller_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "kotlin imported constructor receiver exact package",
        &imported_nodes,
        &imported_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/other/Workflow.kt",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "kotlin imported constructor receiver exact package",
        &imported_nodes,
        &imported_edges,
        ResolvedCallCountInFile {
            caller_name: "orchestrate",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "src/other/Workflow.kt",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin imported constructor receiver avoids other package",
        &imported_nodes,
        &imported_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/alternate/Workflow.kt",
    );

    let (alias_nodes, alias_edges) = index_files(&[
        ("src/other/Workflow.kt", imported_owner_source),
        ("src/alternate/Workflow.kt", other_imported_owner_source),
        ("src/app/UseWorkflow.kt", alias_imported_caller_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "kotlin aliased imported constructor receiver exact package",
        &alias_nodes,
        &alias_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/other/Workflow.kt",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin aliased imported constructor receiver avoids other package",
        &alias_nodes,
        &alias_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/alternate/Workflow.kt",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("src/other/Workflow.kt", imported_owner_source),
        ("src/app/UseWorkflow.kt", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin missing imported constructor package",
        &missing_nodes,
        &missing_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/other/Workflow.kt",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("src/other/Workflow.kt", imported_owner_source),
        ("src/alternate/Workflow.kt", other_imported_owner_source),
        ("src/app/UseWorkflow.kt", duplicate_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin duplicate imported constructor local name",
        &duplicate_nodes,
        &duplicate_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/other/Workflow.kt",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin duplicate imported constructor local name",
        &duplicate_nodes,
        &duplicate_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/alternate/Workflow.kt",
    );

    let (wildcard_nodes, wildcard_edges) = index_files(&[
        ("src/other/Workflow.kt", imported_owner_source),
        ("src/app/UseWorkflow.kt", wildcard_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin wildcard imported constructor receiver stays unresolved",
        &wildcard_nodes,
        &wildcard_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/other/Workflow.kt",
    );

    let (no_import_nodes, no_import_edges) = index_files(&[
        ("src/other/Workflow.kt", imported_owner_source),
        ("src/app/UseWorkflow.kt", no_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin unimported constructor receiver stays unresolved",
        &no_import_nodes,
        &no_import_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/other/Workflow.kt",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("src/other/Workflow.kt", imported_owner_source),
        ("src/app/UseWorkflow.kt", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "kotlin local constructor receiver shadows imported type",
        &shadow_nodes,
        &shadow_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/app/UseWorkflow.kt",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin local constructor receiver shadows imported type",
        &shadow_nodes,
        &shadow_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/other/Workflow.kt",
    );

    let (alias_shadow_nodes, alias_shadow_edges) = index_files(&[
        ("src/other/Workflow.kt", imported_owner_source),
        ("src/app/UseWorkflow.kt", alias_local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "kotlin local constructor receiver shadows imported alias",
        &alias_shadow_nodes,
        &alias_shadow_edges,
        "orchestrate",
        "RemoteWorkflow",
        "run",
        "src/app/UseWorkflow.kt",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin local constructor receiver shadows imported alias",
        &alias_shadow_nodes,
        &alias_shadow_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/other/Workflow.kt",
    );

    Ok(())
}

#[test]
fn test_kotlin_same_file_property_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let source = r#"
package app

class Event

interface Notifier {
    fun notifyEvent(event: Event)
}

class Repository {
    fun save(event: Event) {}
}

class Workflow(private val constructorNotifier: Notifier) {
    private val notifier: Notifier = constructorNotifier
    private val repository: Repository = Repository()
    private val erased: Any = constructorNotifier

    fun run(event: Event) {
        notifier.notifyEvent(event)
        this.repository.save(event)
        this.decorate(event)
        constructorNotifier.notifyEvent(event)
    }

    fun parameterShadowsProperty(notifier: Repository, event: Event) {
        notifier.save(event)
        notifier.notifyEvent(event)
    }

    fun thisPropertyDespiteParameter(repository: Notifier, event: Event) {
        this.repository.save(event)
        repository.notifyEvent(event)
    }

    fun localConstructorShadowsProperty(event: Event) {
        val notifier = Repository()
        notifier.save(event)
        notifier.notifyEvent(event)
    }

    fun localFactoryShadowsProperty(event: Event) {
        val notifier = makeNotifier()
        notifier.notifyEvent(event)
    }

    fun localAnyShadowsProperty(event: Event) {
        val notifier: Any = makeNotifier()
        notifier.notifyEvent(event)
    }

    fun erasedProperty(event: Event) {
        erased.notifyEvent(event)
    }

    fun decorate(event: Event) {}
}

class OtherWorkflow {
    fun decorate(event: Event) {}
}

fun makeNotifier(): Notifier = object : Notifier {
    override fun notifyEvent(event: Event) {}
}
"#;

    let (nodes, edges) = index_single_file("src/app/Workflow.kt", source)?;
    assert_resolved_call_count_to_method_owner_in_file(
        "kotlin same-file property and primary constructor property receivers",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Notifier",
            method_name: "notifyEvent",
            file_suffix: "src/app/Workflow.kt",
            expected_count: 2,
        },
    );
    assert_resolved_call_to_method_owner_in_file(
        "kotlin this property receiver",
        &nodes,
        &edges,
        "run",
        "Repository",
        "save",
        "src/app/Workflow.kt",
    );
    assert_resolved_call_to_method_owner_in_file(
        "kotlin this self receiver",
        &nodes,
        &edges,
        "run",
        "Workflow",
        "decorate",
        "src/app/Workflow.kt",
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin self receiver avoids same-named owner",
        &nodes,
        &edges,
        "run",
        "OtherWorkflow",
        "decorate",
    );
    assert_resolved_call_to_method_owner_in_file(
        "kotlin parameter shadows property receiver",
        &nodes,
        &edges,
        "parameterShadowsProperty",
        "Repository",
        "save",
        "src/app/Workflow.kt",
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin parameter shadow prevents property fallback",
        &nodes,
        &edges,
        "parameterShadowsProperty",
        "Notifier",
        "notifyEvent",
    );
    assert_resolved_call_to_method_owner_in_file(
        "kotlin explicit this property ignores same-named parameter",
        &nodes,
        &edges,
        "thisPropertyDespiteParameter",
        "Repository",
        "save",
        "src/app/Workflow.kt",
    );
    assert_resolved_call_to_method_owner_in_file(
        "kotlin same-named parameter still resolves separately",
        &nodes,
        &edges,
        "thisPropertyDespiteParameter",
        "Notifier",
        "notifyEvent",
        "src/app/Workflow.kt",
    );
    assert_resolved_call_to_method_owner_in_file(
        "kotlin local constructor shadows property receiver",
        &nodes,
        &edges,
        "localConstructorShadowsProperty",
        "Repository",
        "save",
        "src/app/Workflow.kt",
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin local constructor shadow prevents property fallback",
        &nodes,
        &edges,
        "localConstructorShadowsProperty",
        "Notifier",
        "notifyEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin local factory shadow prevents property fallback",
        &nodes,
        &edges,
        "localFactoryShadowsProperty",
        "Notifier",
        "notifyEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin local Any shadow prevents property fallback",
        &nodes,
        &edges,
        "localAnyShadowsProperty",
        "Notifier",
        "notifyEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin erased property receiver stays unresolved",
        &nodes,
        &edges,
        "erasedProperty",
        "Notifier",
        "notifyEvent",
    );

    let owner_source = r#"
package other

class Workflow {
    fun run() {}
}
"#;
    let caller_source = r#"
package app

class Entry {
    private val workflow: Workflow = TODO()

    fun call() {
        workflow.run()
    }
}
"#;
    let (cross_file_nodes, cross_file_edges) = index_files(&[
        ("src/other/Workflow.kt", owner_source),
        ("src/app/Entry.kt", caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin property receiver does not use unimported cross-file owner",
        &cross_file_nodes,
        &cross_file_edges,
        "call",
        "Workflow",
        "run",
        "src/other/Workflow.kt",
    );

    Ok(())
}

#[test]
fn test_kotlin_imported_property_receiver_call_resolves_to_exact_imported_owner_method()
-> anyhow::Result<()> {
    let mail_notifier_source = r#"
package com.acme.mail

interface Notifier {
    fun notifyEvent(value: String)
}
"#;
    let other_notifier_source = r#"
package com.acme.other

interface Notifier {
    fun notifyEvent(value: String)
}
"#;
    let workflow_source = r#"
package com.acme.workflow

import com.acme.mail.Notifier

class Workflow(private val notifier: Notifier) {
    fun run() {
        notifier.notifyEvent("ready")
    }
}
"#;
    let alias_workflow_source = r#"
package com.acme.workflow

import com.acme.mail.Notifier as MailNotifier

class Workflow(private val notifier: MailNotifier) {
    fun run() {
        notifier.notifyEvent("ready")
    }
}
"#;
    let duplicate_import_source = r#"
package com.acme.workflow

import com.acme.mail.Notifier
import com.acme.other.Notifier

class Workflow(private val notifier: Notifier) {
    fun run() {
        notifier.notifyEvent("ready")
    }
}
"#;

    let (nodes, edges) = index_files(&[
        ("src/com/acme/mail/Notifier.kt", mail_notifier_source),
        ("src/com/acme/other/Notifier.kt", other_notifier_source),
        ("src/com/acme/workflow/Workflow.kt", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "kotlin imported property receiver exact package",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.kt",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin imported property receiver exact package",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/other/Notifier.kt",
    );

    let (alias_nodes, alias_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.kt", mail_notifier_source),
        ("src/com/acme/other/Notifier.kt", other_notifier_source),
        ("src/com/acme/workflow/Workflow.kt", alias_workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "kotlin aliased imported property receiver exact package",
        &alias_nodes,
        &alias_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.kt",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin aliased imported property receiver exact package",
        &alias_nodes,
        &alias_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/other/Notifier.kt",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("src/com/acme/mail/Notifier.kt", mail_notifier_source),
        ("src/com/acme/other/Notifier.kt", other_notifier_source),
        ("src/com/acme/workflow/Workflow.kt", duplicate_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin duplicate imported property receiver local name",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/mail/Notifier.kt",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "kotlin duplicate imported property receiver local name",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/com/acme/other/Notifier.kt",
    );

    Ok(())
}

#[test]
fn test_csharp_precise_receiver_calls_resolve_without_global_member_fallback() -> anyhow::Result<()>
{
    let source = r#"
namespace Acme.Workflow;

interface INotifier
{
    void Notify(Event evt);
}

class ConsoleNotifier : INotifier
{
    public void Notify(Event evt) {}
}

class Repository
{
    public void Save(Event evt) {}
}

class Event {}

class Workflow
{
    private readonly INotifier notifier;
    private readonly Repository repository;

    public Workflow(INotifier notifier, Repository repository)
    {
        this.notifier = notifier;
        this.repository = repository;
    }

    public void Run(Event evt)
    {
        notifier.Notify(evt);
        this.repository.Save(evt);
        this.Decorate(evt);
    }

    public void Decorate(Event evt) {}
}

class OtherWorkflow
{
    public void Run(Event evt) {}
}

class Program
{
    static Workflow MakeWorkflow()
    {
        return new Workflow(new ConsoleNotifier(), new Repository());
    }

    static object Wrap(Workflow workflow)
    {
        return workflow;
    }

    static void Temporary()
    {
        new Workflow(new ConsoleNotifier(), new Repository()).Run(new Event());
    }

    static void Local()
    {
        Workflow workflow = MakeWorkflow();
        workflow.Run(new Event());
    }

    static void VarConstructor()
    {
        var workflow = new Workflow(new ConsoleNotifier(), new Repository());
        workflow.Run(new Event());
    }

    static void StaticQualified()
    {
        Workflow workflow = Program.MakeWorkflow();
        workflow.Run(new Event());
    }

    static void StaticFullyQualified()
    {
        Workflow workflow = Acme.Workflow.Program.MakeWorkflow();
        workflow.Run(new Event());
    }

    static void VarFactory()
    {
        var workflow = MakeWorkflow();
        workflow.Run(new Event());
    }

    static void DynamicWrapped()
    {
        dynamic workflow = Wrap(new Workflow(new ConsoleNotifier(), new Repository()));
        workflow.Run(new Event());
    }

    static void ExternalQualifiedLocal()
    {
        Acme.External.Workflow workflow = null;
        workflow.Run(new Event());
    }

    static void ExternalQualifiedConstructor()
    {
        new Acme.External.Workflow().Run(new Event());
    }

    static void ExternalQualifiedStatic()
    {
        Acme.External.Workflow.Run(new Event());
    }

    static void DirectOnly()
    {
        MakeWorkflow();
    }

    static void OrderAware()
    {
        workflow.Run(new Event());
        Workflow workflow = MakeWorkflow();
        workflow.Run(new Event());
    }
}
"#;
    let (nodes, edges) = index_files(&[("src/Acme/Workflow/Program.cs", source)])?;

    assert_resolved_call_to_method_owner_in_file(
        "csharp direct constructor receiver resolves local owner",
        &nodes,
        &edges,
        "Temporary",
        "Workflow",
        "Run",
        "src/Acme/Workflow/Program.cs",
    );
    assert_resolved_call_to_method_owner_in_file(
        "csharp explicit local receiver resolves local owner",
        &nodes,
        &edges,
        "Local",
        "Workflow",
        "Run",
        "src/Acme/Workflow/Program.cs",
    );
    assert_resolved_call_to_method_owner_in_file(
        "csharp var direct constructor receiver resolves local owner",
        &nodes,
        &edges,
        "VarConstructor",
        "Workflow",
        "Run",
        "src/Acme/Workflow/Program.cs",
    );
    assert_resolved_call_to_method_owner_in_file(
        "csharp static type receiver call resolves same-file owner",
        &nodes,
        &edges,
        "StaticQualified",
        "Program",
        "MakeWorkflow",
        "src/Acme/Workflow/Program.cs",
    );
    assert_resolved_call_to_method_owner_in_file(
        "csharp fully qualified static type receiver call resolves same-file owner",
        &nodes,
        &edges,
        "StaticFullyQualified",
        "Program",
        "MakeWorkflow",
        "src/Acme/Workflow/Program.cs",
    );
    assert_resolved_call_to_method_owner_in_file(
        "csharp field receiver resolves interface owner",
        &nodes,
        &edges,
        "Run",
        "INotifier",
        "Notify",
        "src/Acme/Workflow/Program.cs",
    );
    assert_resolved_call_to_method_owner_in_file(
        "csharp this field receiver resolves class owner",
        &nodes,
        &edges,
        "Run",
        "Repository",
        "Save",
        "src/Acme/Workflow/Program.cs",
    );
    assert_resolved_call_to_method_owner_in_file(
        "csharp this receiver resolves enclosing class owner",
        &nodes,
        &edges,
        "Run",
        "Workflow",
        "Decorate",
        "src/Acme/Workflow/Program.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp var factory receiver stays fail-closed without return-type evidence",
        &nodes,
        &edges,
        "VarFactory",
        "Workflow",
        "Run",
        "src/Acme/Workflow/Program.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp dynamic receiver stays fail-closed",
        &nodes,
        &edges,
        "DynamicWrapped",
        "Workflow",
        "Run",
        "src/Acme/Workflow/Program.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp qualified external local receiver does not collapse to same-file short owner",
        &nodes,
        &edges,
        "ExternalQualifiedLocal",
        "Workflow",
        "Run",
        "src/Acme/Workflow/Program.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp qualified external constructor receiver does not collapse to same-file short owner",
        &nodes,
        &edges,
        "ExternalQualifiedConstructor",
        "Workflow",
        "Run",
        "src/Acme/Workflow/Program.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp qualified external static receiver does not collapse to same-file short owner",
        &nodes,
        &edges,
        "ExternalQualifiedStatic",
        "Workflow",
        "Run",
        "src/Acme/Workflow/Program.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp precise local owner avoids same-name global fallback",
        &nodes,
        &edges,
        "Local",
        "OtherWorkflow",
        "Run",
        "src/Acme/Workflow/Program.cs",
    );
    assert_resolved_call_to_method_owner_in_file(
        "csharp direct static call still resolves by name",
        &nodes,
        &edges,
        "DirectOnly",
        "Program",
        "MakeWorkflow",
        "src/Acme/Workflow/Program.cs",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "csharp local receiver is order-aware",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "OrderAware",
            owner_name: "Workflow",
            method_name: "Run",
            file_suffix: "src/Acme/Workflow/Program.cs",
            expected_count: 1,
        },
    );

    let unique_fallback_source = r#"
namespace Acme.Unique;

class Event {}

class Workflow
{
    public void Run(Event evt) {}
}

class Program
{
    static Workflow MakeWorkflow()
    {
        return new Workflow();
    }

    static void VarFactory()
    {
        var workflow = MakeWorkflow();
        workflow.Run(new Event());
    }

    static void DynamicFactory()
    {
        dynamic workflow = MakeWorkflow();
        workflow.Run(new Event());
    }
}
"#;
    let (unique_nodes, unique_edges) =
        index_files(&[("src/Acme/Unique/Program.cs", unique_fallback_source)])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp uninferred var receiver stays fail-closed with unique global method",
        &unique_nodes,
        &unique_edges,
        "VarFactory",
        "Workflow",
        "Run",
        "src/Acme/Unique/Program.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp dynamic receiver stays fail-closed with unique global method",
        &unique_nodes,
        &unique_edges,
        "DynamicFactory",
        "Workflow",
        "Run",
        "src/Acme/Unique/Program.cs",
    );

    Ok(())
}

#[test]
fn test_csharp_using_alias_typed_receiver_call_resolves_to_exact_imported_owner_method()
-> anyhow::Result<()> {
    let mail_notifier_source = r#"
namespace Acme.Mail;

public interface Notifier
{
    void Notify(string value);
}
"#;
    let other_notifier_source = r#"
namespace Acme.Other;

public interface Notifier
{
    void Notify(string value);
}
"#;
    let workflow_source = r#"
using Mailer = Acme.Mail.Notifier;

namespace Acme.Workflow;

class Workflow
{
    void Run(Mailer notifier)
    {
        notifier.Notify("ready");
    }
}
"#;
    let block_namespace_alias_source = r#"
namespace Acme.Workflow
{
    using Mailer = Acme.Mail.Notifier;

    class Workflow
    {
        void Run(Mailer notifier)
        {
            notifier.Notify("ready");
        }
    }
}
"#;
    let missing_import_source = r#"
using Mailer = Acme.Missing.Notifier;

namespace Acme.Workflow;

class Workflow
{
    void Run(Mailer notifier)
    {
        notifier.Notify("ready");
    }
}
"#;
    let duplicate_alias_source = r#"
using Mailer = Acme.Mail.Notifier;
using Mailer = Acme.Other.Notifier;

namespace Acme.Workflow;

class Workflow
{
    void Run(Mailer notifier)
    {
        notifier.Notify("ready");
    }
}
"#;
    let local_shadow_source = r#"
using Mailer = Acme.Mail.Notifier;

namespace Acme.Workflow;

interface Mailer
{
    void Notify(string value);
}

class Workflow
{
    void Run(Mailer notifier)
    {
        notifier.Notify("ready");
    }
}
"#;
    let block_namespace_shadow_source = r#"
using Mailer = Acme.Mail.Notifier;

namespace Acme.Workflow
{
    interface Mailer
    {
        void Notify(string value);
    }

    class Workflow
    {
        void Run(Mailer notifier)
        {
            notifier.Notify("ready");
        }
    }
}
"#;
    let plain_using_source = r#"
using Acme.Mail;

namespace Acme.Workflow;

class Workflow
{
    void Run(Notifier notifier)
    {
        notifier.Notify("ready");
    }
}
"#;
    let duplicate_plain_using_source = r#"
using Acme.Mail;
using Acme.Other;

namespace Acme.Workflow;

class Workflow
{
    void Run(Notifier notifier)
    {
        notifier.Notify("ready");
    }
}
"#;
    let system_plain_using_source = r#"
using System;
using Acme.Mail;

namespace Acme.Workflow;

class Workflow
{
    void Run(Notifier notifier)
    {
        notifier.Notify("ready");
    }
}
"#;
    let local_plain_shadow_source = r#"
using Acme.Mail;

namespace Acme.Workflow;

interface Notifier
{
    void Notify(string value);
}

class Workflow
{
    void Run(Notifier notifier)
    {
        notifier.Notify("ready");
    }
}
"#;
    let static_plain_using_source = r#"
using Acme.Mail;

namespace Acme.Workflow;

class Workflow
{
    void Run()
    {
        Notifier.Notify("ready");
    }
}
"#;

    let (nodes, edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Other/Notifier.cs", other_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "csharp using alias imported receiver exact namespace",
        &nodes,
        &edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "csharp using alias imported receiver exact namespace",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "Run",
            owner_name: "Notifier",
            method_name: "Notify",
            file_suffix: "src/Acme/Mail/Notifier.cs",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp using alias imported receiver exact namespace",
        &nodes,
        &edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Other/Notifier.cs",
    );

    let (block_alias_nodes, block_alias_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Other/Notifier.cs", other_notifier_source),
        (
            "src/Acme/Workflow/Workflow.cs",
            block_namespace_alias_source,
        ),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "csharp block namespace using alias imported receiver",
        &block_alias_nodes,
        &block_alias_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp block namespace using alias imported receiver",
        &block_alias_nodes,
        &block_alias_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Other/Notifier.cs",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp missing using alias imported receiver",
        &missing_nodes,
        &missing_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Other/Notifier.cs", other_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", duplicate_alias_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp duplicate using alias imported receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp duplicate using alias imported receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Other/Notifier.cs",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "csharp local receiver shadows using alias",
        &shadow_nodes,
        &shadow_edges,
        "Run",
        "Mailer",
        "Notify",
        "src/Acme/Workflow/Workflow.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp local receiver shadows using alias",
        &shadow_nodes,
        &shadow_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );

    let (block_shadow_nodes, block_shadow_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        (
            "src/Acme/Workflow/Workflow.cs",
            block_namespace_shadow_source,
        ),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "csharp block namespace local receiver shadows using alias",
        &block_shadow_nodes,
        &block_shadow_edges,
        "Run",
        "Mailer",
        "Notify",
        "src/Acme/Workflow/Workflow.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp block namespace local receiver shadows using alias",
        &block_shadow_nodes,
        &block_shadow_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );

    let (plain_nodes, plain_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Other/Notifier.cs", other_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", plain_using_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "csharp plain namespace using resolves exact imported owner",
        &plain_nodes,
        &plain_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp plain namespace using avoids other namespace",
        &plain_nodes,
        &plain_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Other/Notifier.cs",
    );

    let (duplicate_plain_nodes, duplicate_plain_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Other/Notifier.cs", other_notifier_source),
        (
            "src/Acme/Workflow/Workflow.cs",
            duplicate_plain_using_source,
        ),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp duplicate plain namespace using stays unresolved",
        &duplicate_plain_nodes,
        &duplicate_plain_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp duplicate plain namespace using stays unresolved",
        &duplicate_plain_nodes,
        &duplicate_plain_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Other/Notifier.cs",
    );

    let (system_plain_nodes, system_plain_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Other/Notifier.cs", other_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", system_plain_using_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp system plus plain namespace using stays unresolved",
        &system_plain_nodes,
        &system_plain_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp system plus plain namespace using stays unresolved",
        &system_plain_nodes,
        &system_plain_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Other/Notifier.cs",
    );

    let (local_plain_shadow_nodes, local_plain_shadow_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", local_plain_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "csharp local receiver shadows plain namespace using",
        &local_plain_shadow_nodes,
        &local_plain_shadow_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Workflow/Workflow.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp local receiver shadows plain namespace using",
        &local_plain_shadow_nodes,
        &local_plain_shadow_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );

    let (static_plain_nodes, static_plain_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", static_plain_using_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp plain namespace using does not resolve static receiver",
        &static_plain_nodes,
        &static_plain_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );

    Ok(())
}

#[test]
fn test_csharp_using_alias_field_and_local_receiver_call_resolves_to_exact_imported_owner_method()
-> anyhow::Result<()> {
    let mail_notifier_source = r#"
namespace Acme.Mail;

public interface Notifier
{
    void Notify(string value);
}
"#;
    let other_notifier_source = r#"
namespace Acme.Other;

public interface Notifier
{
    void Notify(string value);
}
"#;
    let workflow_source = r#"
using Mailer = Acme.Mail.Notifier;

namespace Acme.Workflow;

class Other
{
    public void Notify(string value) {}
}

class Workflow
{
    private readonly Mailer notifier;

    void Run(string value)
    {
        notifier.Notify(value);
        this.notifier.Notify(value);
    }

    void Local(string value)
    {
        Mailer localNotifier = null;
        localNotifier.Notify(value);
    }

    void ParameterNameShadowsAlias(Other Mailer)
    {
        Mailer.Notify("ready");
    }

    void ParameterShadowsField(Other notifier, string value)
    {
        notifier.Notify(value);
        this.notifier.Notify(value);
    }
}
"#;
    let missing_import_source = r#"
using Mailer = Acme.Missing.Notifier;

namespace Acme.Workflow;

class Workflow
{
    private readonly Mailer notifier;

    void Run(string value)
    {
        notifier.Notify(value);
    }
}
"#;
    let duplicate_alias_source = r#"
using Mailer = Acme.Mail.Notifier;
using Mailer = Acme.Other.Notifier;

namespace Acme.Workflow;

class Workflow
{
    private readonly Mailer notifier;

    void Run(string value)
    {
        notifier.Notify(value);
    }
}
"#;
    let local_shadow_source = r#"
using Mailer = Acme.Mail.Notifier;

namespace Acme.Workflow;

interface Mailer
{
    void Notify(string value);
}

class Workflow
{
    private readonly Mailer notifier;

    void Run(string value)
    {
        notifier.Notify(value);
    }
}
"#;

    let (nodes, edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Other/Notifier.cs", other_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", workflow_source),
    ])?;
    assert_resolved_call_count_to_method_owner_in_file(
        "csharp using alias imported field receiver exact namespace",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "Run",
            owner_name: "Notifier",
            method_name: "Notify",
            file_suffix: "src/Acme/Mail/Notifier.cs",
            expected_count: 2,
        },
    );
    assert_resolved_call_to_method_owner_in_file(
        "csharp using alias imported local receiver exact namespace",
        &nodes,
        &edges,
        "Local",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp using alias imported field receiver avoids other namespace",
        &nodes,
        &edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Other/Notifier.cs",
    );
    assert_resolved_call_to_method_owner_in_file(
        "csharp parameter name shadows using alias static receiver",
        &nodes,
        &edges,
        "ParameterNameShadowsAlias",
        "Other",
        "Notify",
        "src/Acme/Workflow/Workflow.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp parameter name shadows using alias static receiver",
        &nodes,
        &edges,
        "ParameterNameShadowsAlias",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "csharp parameter name shadows bare field receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "ParameterShadowsField",
            owner_name: "Other",
            method_name: "Notify",
            file_suffix: "src/Acme/Workflow/Workflow.cs",
            expected_count: 1,
        },
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "csharp explicit this field ignores parameter shadow",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "ParameterShadowsField",
            owner_name: "Notifier",
            method_name: "Notify",
            file_suffix: "src/Acme/Mail/Notifier.cs",
            expected_count: 1,
        },
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp missing using alias imported field receiver",
        &missing_nodes,
        &missing_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Other/Notifier.cs", other_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", duplicate_alias_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp duplicate using alias imported field receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp duplicate using alias imported field receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Other/Notifier.cs",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.cs", mail_notifier_source),
        ("src/Acme/Workflow/Workflow.cs", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "csharp local receiver shadows using alias field type",
        &shadow_nodes,
        &shadow_edges,
        "Run",
        "Mailer",
        "Notify",
        "src/Acme/Workflow/Workflow.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp local receiver shadows using alias field type",
        &shadow_nodes,
        &shadow_edges,
        "Run",
        "Notifier",
        "Notify",
        "src/Acme/Mail/Notifier.cs",
    );

    Ok(())
}

#[test]
fn test_ruby_same_file_constructor_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let source = r#"
class Workflow
  def run(event)
  end
end

class OtherWorkflow
  def run(event)
  end
end

def make_workflow
  Workflow.new
end

def direct_constructor
  Workflow.new.run(:ready)
end

def local_constructor
  workflow = Workflow.new
  workflow.run(:ready)
end

def factory_returned
  workflow = make_workflow
  workflow.run(:ready)
end

def reassigned
  workflow = Workflow.new
  workflow.run(:ready)
  workflow = make_workflow
  workflow.run(:ready)
end

def order_aware
  workflow.run(:ready)
  workflow = Workflow.new
  workflow.run(:ready)
end

def operator_reassigned
  workflow = Workflow.new
  workflow ||= make_workflow
  workflow.run(:ready)
end

def non_constructor_segment
  Workflow.newer.run(:ready)
end
"#;
    let (nodes, edges) = index_files(&[("src/workflow.rb", source)])?;

    assert_resolved_call_to_method_owner_in_file(
        "ruby direct constructor receiver resolves local owner",
        &nodes,
        &edges,
        "direct_constructor",
        "Workflow",
        "run",
        "src/workflow.rb",
    );
    assert_resolved_call_to_method_owner_in_file(
        "ruby local constructor receiver resolves local owner",
        &nodes,
        &edges,
        "local_constructor",
        "Workflow",
        "run",
        "src/workflow.rb",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby factory receiver stays fail-closed without return-type evidence",
        &nodes,
        &edges,
        "factory_returned",
        "Workflow",
        "run",
        "src/workflow.rb",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby local owner avoids same-name global fallback",
        &nodes,
        &edges,
        "local_constructor",
        "OtherWorkflow",
        "run",
        "src/workflow.rb",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "ruby local constructor binding is invalidated by later factory assignment",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "reassigned",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "src/workflow.rb",
            expected_count: 1,
        },
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "ruby local constructor receiver is order-aware",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "order_aware",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "src/workflow.rb",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby local operator reassignment stays fail-closed",
        &nodes,
        &edges,
        "operator_reassigned",
        "Workflow",
        "run",
        "src/workflow.rb",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby direct receiver constructor requires exact new segment",
        &nodes,
        &edges,
        "non_constructor_segment",
        "Workflow",
        "run",
        "src/workflow.rb",
    );

    let owner_source = r#"
class Workflow
  def run(event)
  end
end
"#;
    let caller_source = r#"
def orchestrate
  workflow = Workflow.new
  workflow.run(:ready)
end
"#;
    let (cross_nodes, cross_edges) = index_files(&[
        ("src/workflow.rb", owner_source),
        ("src/use_workflow.rb", caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby constructor receiver does not use cross-file owner",
        &cross_nodes,
        &cross_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/workflow.rb",
    );

    Ok(())
}

#[test]
fn test_ruby_require_relative_constructor_receiver_resolves_to_imported_owner_method()
-> anyhow::Result<()> {
    let workflow_source = r#"
class Workflow
  def run(event)
  end
end
"#;
    let shadow_source = r#"
class Workflow
  def run(event)
  end
end
"#;
    let wrong_owner_source = r#"
class OtherWorkflow
  def run(event)
  end
end
"#;
    let caller_source = r#"
require_relative "./workflow"

def direct_constructor
  Workflow.new.run(:ready)
end

def local_constructor
  workflow = Workflow.new
  workflow.run(:ready)
end

class Entry
  def initialize
    @workflow = Workflow.new
  end

  def run
    @workflow.run(:ready)
  end
end
"#;
    let missing_source = r#"
require_relative "./missing_workflow"

def run
  Workflow.new.run(:ready)
end
"#;
    let duplicate_require_source = r#"
require_relative "./workflow"
require_relative "./shadow"

def run
  Workflow.new.run(:ready)
end
"#;
    let local_shadow_source = r#"
require_relative "./workflow"

class Workflow
  def run(event)
  end
end

def run
  Workflow.new.run(:ready)
end
"#;
    let assignment_shadow_source = r#"
require_relative "./workflow"

Workflow = Class.new

def run
  Workflow.new.run(:ready)
end
"#;

    let (nodes, edges) = index_files(&[
        ("src/workflow.rb", workflow_source),
        ("src/shadow.rb", shadow_source),
        ("src/use_workflow.rb", caller_source),
    ])?;
    for caller in ["direct_constructor", "local_constructor", "Entry.run"] {
        assert_resolved_call_to_method_owner_in_file(
            "ruby require_relative constructor receiver",
            &nodes,
            &edges,
            caller,
            "Workflow",
            "run",
            "src/workflow.rb",
        );
        assert_no_resolved_call_to_method_owner_in_file(
            "ruby require_relative constructor receiver",
            &nodes,
            &edges,
            caller,
            "Workflow",
            "run",
            "src/shadow.rb",
        );
    }

    let (missing_nodes, missing_edges) = index_files(&[
        ("src/workflow.rb", workflow_source),
        ("src/use_workflow.rb", missing_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby missing require_relative owner",
        &missing_nodes,
        &missing_edges,
        "run",
        "Workflow",
        "run",
        "src/workflow.rb",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("src/workflow.rb", workflow_source),
        ("src/shadow.rb", shadow_source),
        ("src/use_workflow.rb", duplicate_require_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby duplicate require_relative owner",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Workflow",
        "run",
        "src/workflow.rb",
    );

    let (wrong_owner_nodes, wrong_owner_edges) = index_files(&[
        ("src/workflow.rb", wrong_owner_source),
        ("src/use_workflow.rb", caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby require_relative owner mismatch stays unresolved",
        &wrong_owner_nodes,
        &wrong_owner_edges,
        "direct_constructor",
        "OtherWorkflow",
        "run",
        "src/workflow.rb",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby require_relative owner mismatch stays unresolved",
        &wrong_owner_nodes,
        &wrong_owner_edges,
        "direct_constructor",
        "Workflow",
        "run",
        "src/workflow.rb",
    );

    let (assignment_shadow_nodes, assignment_shadow_edges) = index_files(&[
        ("src/workflow.rb", workflow_source),
        ("src/use_workflow.rb", assignment_shadow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby constant assignment shadows require_relative owner",
        &assignment_shadow_nodes,
        &assignment_shadow_edges,
        "run",
        "Workflow",
        "run",
        "src/workflow.rb",
    );

    let (local_shadow_nodes, local_shadow_edges) = index_files(&[
        ("src/workflow.rb", workflow_source),
        ("src/use_workflow.rb", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "ruby local class shadows require_relative owner",
        &local_shadow_nodes,
        &local_shadow_edges,
        "run",
        "Workflow",
        "run",
        "src/use_workflow.rb",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby local class shadows require_relative owner",
        &local_shadow_nodes,
        &local_shadow_edges,
        "run",
        "Workflow",
        "run",
        "src/workflow.rb",
    );

    Ok(())
}

#[test]
fn test_ruby_same_file_instance_variable_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let source = r#"
class Workflow
  def run(event)
  end
end

class OtherWorkflow
  def run(event)
  end
end

def make_workflow
  Workflow.new
end

class Entry
  def initialize
    @workflow = Workflow.new
  end

  def run
    @workflow.run(:ready)
  end

  def local_constructor
    @workflow = Workflow.new
    @workflow.run(:ready)
  end
end

class ReassignedEntry
  def initialize
    @workflow = Workflow.new
  end

  def replace
    @workflow = make_workflow
    @workflow.run(:ready)
  end
end

class OperatorEntry
  def initialize
    @workflow = Workflow.new
  end

  def replace
    @workflow ||= make_workflow
    @workflow.run(:ready)
  end
end
"#;
    let class_body_source = r#"
class Workflow
  def run(event)
  end
end

class Entry
  @workflow = Workflow.new

  def run
    @workflow.run(:ready)
  end
end
"#;
    let singleton_source = r#"
class Workflow
  def run(event)
  end
end

class Entry
  def self.configure
    @workflow = Workflow.new
  end

  def run
    @workflow.run(:ready)
  end
end
"#;
    let (nodes, edges) = index_files(&[("src/entry.rb", source)])?;

    assert_resolved_call_to_method_owner_in_file(
        "ruby initialized instance variable receiver",
        &nodes,
        &edges,
        "run",
        "Workflow",
        "run",
        "src/entry.rb",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby instance variable operator assignment stays fail-closed",
        &nodes,
        &edges,
        "replace",
        "Workflow",
        "run",
        "src/entry.rb",
    );

    let (class_body_nodes, class_body_edges) =
        index_files(&[("src/class_body.rb", class_body_source)])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby class-body instance variable does not authorize instance receiver",
        &class_body_nodes,
        &class_body_edges,
        "run",
        "Workflow",
        "run",
        "src/class_body.rb",
    );

    let (singleton_nodes, singleton_edges) =
        index_files(&[("src/singleton.rb", singleton_source)])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby singleton instance variable does not authorize instance receiver",
        &singleton_nodes,
        &singleton_edges,
        "run",
        "Workflow",
        "run",
        "src/singleton.rb",
    );
    assert_resolved_call_to_method_owner_in_file(
        "ruby local instance variable constructor receiver",
        &nodes,
        &edges,
        "local_constructor",
        "Workflow",
        "run",
        "src/entry.rb",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby instance variable factory reassignment stays fail-closed",
        &nodes,
        &edges,
        "replace",
        "Workflow",
        "run",
        "src/entry.rb",
    );
    let owner_source = r#"
class Workflow
  def run(event)
  end
end
"#;
    let caller_source = r#"
class Entry
  def initialize
    @workflow = Workflow.new
  end

  def run
    @workflow.run(:ready)
  end
end
"#;
    let (cross_file_nodes, cross_file_edges) = index_files(&[
        ("src/workflow.rb", owner_source),
        ("src/entry.rb", caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby instance variable receiver does not use cross-file owner",
        &cross_file_nodes,
        &cross_file_edges,
        "run",
        "Workflow",
        "run",
        "src/workflow.rb",
    );

    let mixed_source = r#"
class Workflow
  def run(event)
  end
end

class OtherWorkflow
  def run(event)
  end
end

class MixedEntry
  def initialize
    @workflow = Workflow.new
    @workflow = OtherWorkflow.new
  end

  def run
    @workflow.run(:ready)
  end
end
"#;
    let (mixed_nodes, mixed_edges) = index_files(&[("src/mixed.rb", mixed_source)])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby mixed instance variable owners stay fail-closed",
        &mixed_nodes,
        &mixed_edges,
        "run",
        "Workflow",
        "run",
        "src/mixed.rb",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "ruby mixed instance variable owners stay fail-closed",
        &mixed_nodes,
        &mixed_edges,
        "run",
        "OtherWorkflow",
        "run",
        "src/mixed.rb",
    );

    Ok(())
}

#[test]
fn test_php_same_file_constructor_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let source = r#"
<?php

namespace App;

class Event {}

interface Notifier
{
    public function notify(Event $event): void;
}

class ConsoleNotifier implements Notifier
{
    public function notify(Event $event): void {}
}

class Repository
{
    public function save(Event $event): void {}
}

class Workflow
{
    public function __construct(
        private Notifier $notifier,
        private Repository $repository
    ) {
    }

    public function run(Event $event): void
    {
        $this->notifier->notify($event);
        $this->repository->save($event);
        $this->decorate($event);
    }

    private function decorate(Event $event): string
    {
        return 'ready';
    }
}

class OtherWorkflow
{
    public function run(Event $event): void {}
}

class UntypedWorkflow
{
    private $notifier;

    public function check(Event $event): void
    {
        $this->notifier->notify($event);
    }
}

class ExplicitPropertyWorkflow
{
    private Notifier $notifier;

    public function process(Event $event): void
    {
        $this->notifier->notify($event);
    }
}

function make_workflow(): Workflow
{
    return new Workflow(new ConsoleNotifier(), new Repository());
}

function direct_constructor(): void
{
    (new Workflow(new ConsoleNotifier(), new Repository()))->run(new Event());
}

function local_constructor(): void
{
    $workflow = new Workflow(new ConsoleNotifier(), new Repository());
    $workflow->run(new Event());
}

function local_nullsafe_constructor(): void
{
    $workflow = new Workflow(new ConsoleNotifier(), new Repository());
    $workflow?->run(new Event());
}

function factory_returned(): void
{
    $workflow = make_workflow();
    $workflow->run(new Event());
}

function factory_nullsafe_returned(): void
{
    $workflow = make_workflow();
    $workflow?->run(new Event());
}

function reassigned(): void
{
    $workflow = new Workflow(new ConsoleNotifier(), new Repository());
    $workflow->run(new Event());
    $workflow = make_workflow();
    $workflow->run(new Event());
}

function order_aware(): void
{
    $workflow->run(new Event());
    $workflow = new Workflow(new ConsoleNotifier(), new Repository());
    $workflow->run(new Event());
}

function untyped_property(): void
{
    $workflow = new UntypedWorkflow();
    $workflow->check(new Event());
}

function explicit_property(): void
{
    $workflow = new ExplicitPropertyWorkflow();
    $workflow->process(new Event());
}
"#;
    let (nodes, edges) = index_files(&[("src/App/workflow.php", source)])?;

    assert_resolved_call_to_method_owner_in_file(
        "php direct constructor receiver resolves local owner",
        &nodes,
        &edges,
        "direct_constructor",
        "Workflow",
        "run",
        "src/App/workflow.php",
    );
    assert_resolved_call_to_method_owner_in_file(
        "php local constructor receiver resolves local owner",
        &nodes,
        &edges,
        "local_constructor",
        "Workflow",
        "run",
        "src/App/workflow.php",
    );
    assert_resolved_call_to_method_owner_in_file(
        "php nullsafe local constructor receiver resolves local owner",
        &nodes,
        &edges,
        "local_nullsafe_constructor",
        "Workflow",
        "run",
        "src/App/workflow.php",
    );
    assert_resolved_call_to_method_owner_in_file(
        "php this receiver resolves enclosing class owner",
        &nodes,
        &edges,
        "run",
        "Workflow",
        "decorate",
        "src/App/workflow.php",
    );
    assert_resolved_call_to_method_owner_in_file(
        "php typed promoted property receiver resolves interface owner",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notify",
        "src/App/workflow.php",
    );
    assert_resolved_call_to_method_owner_in_file(
        "php typed promoted property receiver resolves class owner",
        &nodes,
        &edges,
        "run",
        "Repository",
        "save",
        "src/App/workflow.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php untyped property receiver stays fail-closed",
        &nodes,
        &edges,
        "check",
        "Notifier",
        "notify",
        "src/App/workflow.php",
    );
    assert_resolved_call_to_method_owner_in_file(
        "php typed explicit property receiver resolves interface owner",
        &nodes,
        &edges,
        "process",
        "Notifier",
        "notify",
        "src/App/workflow.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php factory receiver stays fail-closed without return-type evidence",
        &nodes,
        &edges,
        "factory_returned",
        "Workflow",
        "run",
        "src/App/workflow.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php nullsafe factory receiver stays fail-closed without return-type evidence",
        &nodes,
        &edges,
        "factory_nullsafe_returned",
        "Workflow",
        "run",
        "src/App/workflow.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php local owner avoids same-name global fallback",
        &nodes,
        &edges,
        "local_constructor",
        "OtherWorkflow",
        "run",
        "src/App/workflow.php",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "php local constructor binding is invalidated by later factory assignment",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "reassigned",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "src/App/workflow.php",
            expected_count: 1,
        },
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "php local constructor receiver is order-aware",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "order_aware",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "src/App/workflow.php",
            expected_count: 1,
        },
    );

    let owner_source = r#"
<?php

namespace App;

class Event {}

class Workflow
{
    public function run(Event $event): void {}
}
"#;
    let caller_source = r#"
<?php

namespace App;

function orchestrate(): void
{
    $workflow = new Workflow();
    $workflow->run(new Event());
}
"#;
    let (cross_nodes, cross_edges) = index_files(&[
        ("src/App/workflow.php", owner_source),
        ("src/App/use_workflow.php", caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "php constructor receiver does not use cross-file owner",
        &cross_nodes,
        &cross_edges,
        "orchestrate",
        "Workflow",
        "run",
        "src/App/workflow.php",
    );

    Ok(())
}

#[test]
fn test_php_use_alias_typed_receiver_call_resolves_to_exact_imported_owner_method()
-> anyhow::Result<()> {
    let mail_notifier_source = r#"
<?php

namespace Acme\Mail;

interface Notifier
{
    public function notifyEvent(string $value): void;
}
"#;
    let other_notifier_source = r#"
<?php

namespace Acme\Other;

interface Notifier
{
    public function notifyEvent(string $value): void;
}
"#;
    let workflow_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Mail\Notifier as Mailer;

function run(Mailer $notifier): void
{
    $notifier->notifyEvent('ready');
}
"#;
    let bracketed_namespace_source = r#"
<?php

namespace Acme\Workflow {
    use Acme\Mail\Notifier as Mailer;

    function run(Mailer $notifier): void
    {
        $notifier->notifyEvent('ready');
    }
}
"#;
    let namespace_leak_source = r#"
<?php

namespace Acme\Alpha;

use Acme\Mail\Notifier as Mailer;

function alpha(Mailer $notifier): void
{
    $notifier->notifyEvent('ready');
}

namespace Acme\Workflow;

function run(Mailer $notifier): void
{
    $notifier->notifyEvent('ready');
}
"#;
    let missing_import_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Missing\Notifier as Mailer;

function run(Mailer $notifier): void
{
    $notifier->notifyEvent('ready');
}
"#;
    let duplicate_alias_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Mail\Notifier as Mailer;
use Acme\Other\Notifier as Mailer;

function run(Mailer $notifier): void
{
    $notifier->notifyEvent('ready');
}
"#;
    let local_shadow_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Mail\Notifier as Mailer;

interface Mailer
{
    public function notifyEvent(string $value): void;
}

function run(Mailer $notifier): void
{
    $notifier->notifyEvent('ready');
}
"#;
    let plain_use_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Mail\Notifier;

function run(Notifier $notifier): void
{
    $notifier->notifyEvent('ready');
}
"#;
    let duplicate_plain_use_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Mail\Notifier;
use Acme\Other\Notifier;

function run(Notifier $notifier): void
{
    $notifier->notifyEvent('ready');
}
"#;
    let const_plain_use_source = r#"
<?php

namespace Acme\Workflow;

use const Acme\Mail\Notifier;

function run(Notifier $notifier): void
{
    $notifier->notifyEvent('ready');
}
"#;
    let grouped_plain_use_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Mail\{Notifier};

function run(Notifier $notifier): void
{
    $notifier->notifyEvent('ready');
}
"#;
    let uppercase_alias_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Mail\Notifier AS Mailer;

function run(Mailer $notifier): void
{
    $notifier?->notifyEvent('ready');
}
"#;
    let uppercase_function_import_source = r#"
<?php

namespace Acme\Workflow;

use FUNCTION Acme\Mail\Notifier AS Mailer;

function run(Mailer $notifier): void
{
    $notifier->notifyEvent('ready');
}
"#;

    let (nodes, edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Other/Notifier.php", other_notifier_source),
        ("src/Acme/Workflow/workflow.php", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "php use alias imported receiver exact namespace",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "php use alias imported receiver exact namespace",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Notifier",
            method_name: "notifyEvent",
            file_suffix: "src/Acme/Mail/Notifier.php",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php use alias imported receiver exact namespace",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Other/Notifier.php",
    );

    let (bracketed_nodes, bracketed_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Other/Notifier.php", other_notifier_source),
        ("src/Acme/Workflow/workflow.php", bracketed_namespace_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "php bracketed namespace use alias imported receiver",
        &bracketed_nodes,
        &bracketed_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php bracketed namespace use alias imported receiver",
        &bracketed_nodes,
        &bracketed_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Other/Notifier.php",
    );

    let (leak_nodes, leak_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Other/Notifier.php", other_notifier_source),
        ("src/Acme/Workflow/workflow.php", namespace_leak_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "php unbracketed namespace use alias stays in segment",
        &leak_nodes,
        &leak_edges,
        "alpha",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php unbracketed namespace use alias does not leak",
        &leak_nodes,
        &leak_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php unbracketed namespace use alias does not leak",
        &leak_nodes,
        &leak_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Other/Notifier.php",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Workflow/workflow.php", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "php missing use alias imported receiver",
        &missing_nodes,
        &missing_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Other/Notifier.php", other_notifier_source),
        ("src/Acme/Workflow/workflow.php", duplicate_alias_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "php duplicate use alias imported receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php duplicate use alias imported receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Other/Notifier.php",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Workflow/workflow.php", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "php local receiver shadows use alias",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Mailer",
        "notifyEvent",
        "src/Acme/Workflow/workflow.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php local receiver shadows use alias",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );

    let (plain_nodes, plain_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Other/Notifier.php", other_notifier_source),
        ("src/Acme/Workflow/workflow.php", plain_use_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "php plain use resolves exact imported owner",
        &plain_nodes,
        &plain_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php plain use avoids other namespace",
        &plain_nodes,
        &plain_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Other/Notifier.php",
    );

    let (duplicate_plain_nodes, duplicate_plain_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Other/Notifier.php", other_notifier_source),
        ("src/Acme/Workflow/workflow.php", duplicate_plain_use_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "php duplicate plain use stays unresolved",
        &duplicate_plain_nodes,
        &duplicate_plain_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php duplicate plain use stays unresolved",
        &duplicate_plain_nodes,
        &duplicate_plain_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Other/Notifier.php",
    );

    let (const_plain_nodes, const_plain_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Workflow/workflow.php", const_plain_use_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "php const plain use is not treated as a type import",
        &const_plain_nodes,
        &const_plain_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );

    let (grouped_plain_nodes, grouped_plain_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Workflow/workflow.php", grouped_plain_use_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "php grouped plain use stays unresolved",
        &grouped_plain_nodes,
        &grouped_plain_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );

    let (uppercase_nodes, uppercase_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Other/Notifier.php", other_notifier_source),
        ("src/Acme/Workflow/workflow.php", uppercase_alias_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "php uppercase use alias nullsafe receiver resolves imported owner",
        &uppercase_nodes,
        &uppercase_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php uppercase use alias nullsafe receiver avoids other namespace",
        &uppercase_nodes,
        &uppercase_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Other/Notifier.php",
    );

    let (function_import_nodes, function_import_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        (
            "src/Acme/Workflow/workflow.php",
            uppercase_function_import_source,
        ),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "php uppercase function use alias is not treated as a type alias",
        &function_import_nodes,
        &function_import_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );

    Ok(())
}

const PHP_LOOP_MARKER_PREFIX: &str = "receiver-binding:loop-element@";

const PHP_LISTENER_INTERFACE_SOURCE: &str = r#"
<?php

namespace Acme\Events;

interface Listener
{
    public function handle(string $payload): void;
}
"#;

const PHP_AUDITOR_CLASS_SOURCE: &str = r#"
<?php

namespace Acme\Events;

class Auditor
{
    public function __construct() {}

    public function handle(string $payload): void {}
}
"#;

fn call_edges_from_caller_at_line<'a>(
    nodes: &[Node],
    edges: &'a [Edge],
    caller_name: &str,
    line: u32,
) -> Vec<&'a Edge> {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.line == Some(line))
        .filter(|edge| {
            node_by_id
                .get(&edge.source)
                .is_some_and(|source| is_matching_name(&source.serialized_name, caller_name))
        })
        .collect()
}

fn callsite_has_segment(edge: &Edge, segment: &str) -> bool {
    edge.callsite_identity
        .as_deref()
        .is_some_and(|identity| identity.split('|').any(|part| part == segment))
}

fn callsite_has_segment_with_prefix(edge: &Edge, prefix: &str) -> bool {
    edge.callsite_identity
        .as_deref()
        .is_some_and(|identity| identity.split('|').any(|part| part.starts_with(prefix)))
}

fn assert_no_call_self_edges(case_name: &str, edges: &[Edge]) {
    let self_edges = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == edge.target)
        .count();
    assert_eq!(
        self_edges, 0,
        "Case `{case_name}`: persisted CALL self-edges should have been dropped"
    );
}

fn assert_no_loop_marker_from_caller(
    case_name: &str,
    nodes: &[Node],
    edges: &[Edge],
    caller: &str,
) {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let marked = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter(|edge| {
            node_by_id
                .get(&edge.source)
                .is_some_and(|source| is_matching_name(&source.serialized_name, caller))
        })
        .filter(|edge| callsite_has_segment_with_prefix(edge, PHP_LOOP_MARKER_PREFIX))
        .count();
    assert_eq!(
        marked,
        0,
        "Case `{case_name}`: no CALL edge from `{caller}` may carry a loop-element binding marker. Calls: {:?}",
        describe_call_edges(edges, nodes)
    );
}

#[test]
fn test_php_foreach_without_provable_element_binding_emits_no_marker_and_no_resolution()
-> anyhow::Result<()> {
    let hostile_source = r#"
<?php

namespace Acme\App;

use Acme\Events\Listener;

function no_docblock(array $listeners): void
{
    foreach ($listeners as $listener) {
        $listener->handle('ready');
    }
}

function unresolvable_type(array $listeners): void
{
    /** @var list<MissingType> $listeners */
    foreach ($listeners as $listener) {
        $listener->handle('ready');
    }
}

function union_docblock(array $listeners): void
{
    /** @var list<Listener|Auditor> $listeners */
    foreach ($listeners as $listener) {
        $listener->handle('ready');
    }
}

function wrong_variable_docblock(array $listeners): void
{
    /** @var list<Listener> $others */
    foreach ($listeners as $listener) {
        $listener->handle('ready');
    }
}

function outside_scope(array $listeners): void
{
    /** @var list<Listener> $listeners */
    foreach ($listeners as $listener) {
        $listener->handle('ready');
    }
    $listener->handle('after');
}

function commented_and_quoted(array $listeners): void
{
    // foreach ($listeners as $listener) { $listener->handle('ready'); }
    $note = 'foreach ($listeners as $listener) { $listener->handle("x"); }';
    echo $note;
}

function nested_shadowing(array $listeners, array $inner): void
{
    /** @var list<Listener> $listeners */
    foreach ($listeners as $listener) {
        foreach ($inner as $listener) {
            $listener->handle('inner');
        }
    }
}
"#;
    let (nodes, edges) = index_files(&[
        (
            "src/Acme/Events/Listener.php",
            PHP_LISTENER_INTERFACE_SOURCE,
        ),
        ("src/Acme/App/hostile.php", hostile_source),
    ])?;

    for hostile_caller in [
        "no_docblock",
        "unresolvable_type",
        "union_docblock",
        "wrong_variable_docblock",
        "commented_and_quoted",
        "nested_shadowing",
    ] {
        assert_no_resolved_call_to_method_owner_in_file(
            "php foreach hostile fails closed",
            &nodes,
            &edges,
            hostile_caller,
            "Listener",
            "handle",
            "src/Acme/Events/Listener.php",
        );
        assert_no_loop_marker_from_caller(
            "php foreach hostile fails closed",
            &nodes,
            &edges,
            hostile_caller,
        );
    }

    // The scope hostile is per-callsite: the in-loop call proves the binding,
    // the same-spelled call after the loop body must not inherit it.
    let after_loop_edges = call_edges_from_caller_at_line(&nodes, &edges, "outside_scope", 45);
    assert!(
        !after_loop_edges.is_empty(),
        "expected the after-loop callsite to keep its unannotated placeholder edge. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    for edge in &after_loop_edges {
        assert_eq!(
            edge.resolved_target, None,
            "a call on the element after the loop body ends must stay unresolved"
        );
        assert!(
            !callsite_has_segment_with_prefix(edge, PHP_LOOP_MARKER_PREFIX),
            "a call on the element after the loop body ends must not carry the loop marker: {:?}",
            edge.callsite_identity
        );
    }

    // The comment/string hostile parses no call at all.
    let commented_edges: Vec<_> =
        call_edges_from_caller_at_line(&nodes, &edges, "commented_and_quoted", 50)
            .into_iter()
            .chain(call_edges_from_caller_at_line(
                &nodes,
                &edges,
                "commented_and_quoted",
                51,
            ))
            .collect();
    assert!(
        commented_edges.is_empty(),
        "foreach text inside comments and strings must produce no CALL edges. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );

    assert_no_call_self_edges("php foreach hostile fails closed", &edges);
    Ok(())
}

#[test]
fn test_php_foreach_var_list_docblock_binds_element_and_lands_combined_loop_marker()
-> anyhow::Result<()> {
    let dispatch_source = r#"
<?php

namespace Acme\App;

use Acme\Events\Listener;

function dispatch_all(array $listeners): void
{
    /** @var list<Listener> $listeners */
    foreach ($listeners as $listener) {
        $listener->handle('ready');
    }
}

function dispatch_suffixed(array $listeners): void
{
    /** @var Listener[] $listeners */
    foreach ($listeners as $listener) {
        $listener->handle('ready');
    }
}
"#;
    let (nodes, edges) = index_files(&[
        (
            "src/Acme/Events/Listener.php",
            PHP_LISTENER_INTERFACE_SOURCE,
        ),
        ("src/Acme/App/dispatch.php", dispatch_source),
    ])?;

    assert_resolved_call_to_method_owner_in_file(
        "php foreach list docblock element resolves interface method",
        &nodes,
        &edges,
        "dispatch_all",
        "Listener",
        "handle",
        "src/Acme/Events/Listener.php",
    );
    assert_resolved_call_to_method_owner_in_file(
        "php foreach suffixed docblock element resolves interface method",
        &nodes,
        &edges,
        "dispatch_suffixed",
        "Listener",
        "handle",
        "src/Acme/Events/Listener.php",
    );

    // The `list<T>` loop: foreach spans lines 11-13, the callsite is line 12.
    let loop_edges = call_edges_from_caller_at_line(&nodes, &edges, "dispatch_all", 12);
    assert_eq!(
        loop_edges.len(),
        1,
        "expected exactly one CALL edge for the loop callsite. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    let loop_edge = loop_edges[0];
    assert!(callsite_has_segment(loop_edge, "syntax:php-member-call"));
    assert!(callsite_has_segment(loop_edge, "receiver-owner:Listener"));
    assert!(callsite_has_segment(
        loop_edge,
        r"receiver-module:Acme\Events.Listener"
    ));
    assert!(
        callsite_has_segment(loop_edge, "receiver-binding:loop-element@11-13"),
        "the combined loop marker must carry the exact foreach statement span: {:?}",
        loop_edge.callsite_identity
    );
    assert_eq!(loop_edge.certainty, Some(ResolutionCertainty::Certain));
    assert!(loop_edge.resolved_target.is_some());

    // The `T[]` loop: foreach spans lines 19-21, the callsite is line 20.
    let suffixed_edges = call_edges_from_caller_at_line(&nodes, &edges, "dispatch_suffixed", 20);
    assert_eq!(suffixed_edges.len(), 1);
    assert!(
        callsite_has_segment(suffixed_edges[0], "receiver-binding:loop-element@19-21"),
        "the suffixed-array loop marker must carry its own foreach span: {:?}",
        suffixed_edges[0].callsite_identity
    );
    assert_eq!(
        suffixed_edges[0].certainty,
        Some(ResolutionCertainty::Certain)
    );

    assert_no_call_self_edges("php foreach docblock binds element", &edges);

    // Property-backed collections: a `$this->handlers` foreach binds through
    // its own property declaration's docblock; a sibling property's typed
    // docblock never leaks onto a different property's loop.
    let property_source = r#"
<?php

namespace Acme\App;

use Acme\Events\Listener;

class Dispatcher
{
    /** @var Listener[] */
    private array $handlers = [];

    public function flush(string $payload): void
    {
        foreach ($this->handlers as $handler) {
            $handler->handle($payload);
        }
    }
}

class AmbiguousDispatcher
{
    /** @var Listener[] */
    private array $handlers = [];

    /** @var string[] */
    private array $handlers2 = [];

    public function flush(string $payload): void
    {
        foreach ($this->handlers2 as $handler) {
            $handler->handle($payload);
        }
    }
}
"#;
    let (property_nodes, property_edges) = index_files(&[
        (
            "src/Acme/Events/Listener.php",
            PHP_LISTENER_INTERFACE_SOURCE,
        ),
        ("src/Acme/App/Dispatcher.php", property_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "php property-collection foreach element resolves interface method",
        &property_nodes,
        &property_edges,
        "flush",
        "Listener",
        "handle",
        "src/Acme/Events/Listener.php",
    );
    // Dispatcher::flush: foreach spans lines 15-17, callsite line 16.
    let property_loop_edges =
        call_edges_from_caller_at_line(&property_nodes, &property_edges, "Dispatcher.flush", 16);
    assert_eq!(
        property_loop_edges.len(),
        1,
        "expected one CALL edge for the property-collection loop callsite. Calls: {:?}",
        describe_call_edges(&property_edges, &property_nodes)
    );
    assert!(callsite_has_segment(
        property_loop_edges[0],
        "receiver-binding:loop-element@15-17"
    ));
    assert_eq!(
        property_loop_edges[0].certainty,
        Some(ResolutionCertainty::Certain)
    );
    // AmbiguousDispatcher iterates a string[] property; the Listener docblock
    // on the sibling property must not bind it.
    assert_no_loop_marker_from_caller(
        "php ambiguous property collection fails closed",
        &property_nodes,
        &property_edges,
        "AmbiguousDispatcher.flush",
    );

    Ok(())
}

#[test]
fn test_php_foreach_loop_marker_lands_regardless_of_which_spec_pass_claims_the_callsite()
-> anyhow::Result<()> {
    // Order 1 — the foreach pass annotates first: a same-named typed
    // parameter also targets the loop callsite, but the element binding
    // shadows it inside the body, so the edge carries the foreach owner AND
    // the marker.
    let foreach_first_source = r#"
<?php

namespace Acme\App;

use Acme\Events\Auditor;
use Acme\Events\Listener;

function relay(Auditor $listener, array $listeners): void
{
    /** @var list<Listener> $listeners */
    foreach ($listeners as $listener) {
        $listener->handle('ready');
    }
}
"#;
    let (foreach_first_nodes, foreach_first_edges) = index_files(&[
        (
            "src/Acme/Events/Listener.php",
            PHP_LISTENER_INTERFACE_SOURCE,
        ),
        ("src/Acme/Events/Auditor.php", PHP_AUDITOR_CLASS_SOURCE),
        ("src/Acme/App/relay.php", foreach_first_source),
    ])?;
    let relay_edges =
        call_edges_from_caller_at_line(&foreach_first_nodes, &foreach_first_edges, "relay", 13);
    assert_eq!(
        relay_edges.len(),
        1,
        "expected one CALL edge for the shadowed loop callsite. Calls: {:?}",
        describe_call_edges(&foreach_first_edges, &foreach_first_nodes)
    );
    let relay_edge = relay_edges[0];
    assert!(
        !callsite_has_segment(relay_edge, "receiver-owner:Auditor"),
        "the loop element shadows the same-named parameter inside the body: {:?}",
        relay_edge.callsite_identity
    );
    assert!(callsite_has_segment(relay_edge, "receiver-owner:Listener"));
    assert!(callsite_has_segment(
        relay_edge,
        "receiver-binding:loop-element@12-14"
    ));
    assert_resolved_call_to_method_owner_in_file(
        "php foreach-first order resolves to the element interface",
        &foreach_first_nodes,
        &foreach_first_edges,
        "relay",
        "Listener",
        "handle",
        "src/Acme/Events/Listener.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php foreach-first order does not resolve to the shadowed parameter type",
        &foreach_first_nodes,
        &foreach_first_edges,
        "relay",
        "Auditor",
        "handle",
        "src/Acme/Events/Auditor.php",
    );

    // Order 2 — another spec annotates first: a local constructor binding for
    // the same variable claims the callsite before the foreach pass runs; the
    // marker must still be appended to that already-annotated edge.
    let other_first_source = r#"
<?php

namespace Acme\App;

use Acme\Events\Auditor;
use Acme\Events\Listener;

function primed(array $listeners): void
{
    $listener = new Auditor();
    $listener->handle('primed');
    /** @var list<Listener> $listeners */
    foreach ($listeners as $listener) {
        $listener->handle('ready');
    }
}
"#;
    let (other_first_nodes, other_first_edges) = index_files(&[
        (
            "src/Acme/Events/Listener.php",
            PHP_LISTENER_INTERFACE_SOURCE,
        ),
        ("src/Acme/Events/Auditor.php", PHP_AUDITOR_CLASS_SOURCE),
        ("src/Acme/App/primed.php", other_first_source),
    ])?;
    let loop_edges =
        call_edges_from_caller_at_line(&other_first_nodes, &other_first_edges, "primed", 15);
    assert_eq!(
        loop_edges.len(),
        1,
        "the losing foreach spec must not spawn a competing edge. Calls: {:?}",
        describe_call_edges(&other_first_edges, &other_first_nodes)
    );
    let loop_edge = loop_edges[0];
    assert!(
        callsite_has_segment(loop_edge, "receiver-owner:Auditor"),
        "the first annotating spec keeps the owner annotation: {:?}",
        loop_edge.callsite_identity
    );
    assert!(!callsite_has_segment(loop_edge, "receiver-owner:Listener"));
    assert!(
        callsite_has_segment(loop_edge, "receiver-binding:loop-element@14-16"),
        "the loop marker must still be appended to the already-annotated edge: {:?}",
        loop_edge.callsite_identity
    );
    let pre_loop_edges =
        call_edges_from_caller_at_line(&other_first_nodes, &other_first_edges, "primed", 12);
    assert_eq!(pre_loop_edges.len(), 1);
    assert!(
        !callsite_has_segment_with_prefix(pre_loop_edges[0], PHP_LOOP_MARKER_PREFIX),
        "the same-spelled callsite before the loop must not carry the marker: {:?}",
        pre_loop_edges[0].callsite_identity
    );

    // Order 2, resolved variant — the earlier binding resolves in-file and
    // replaces the placeholder entirely; the marker must land on the resolved
    // edge itself.
    let resolved_first_source = r#"
<?php

namespace Acme\App;

class LocalAuditor
{
    public function handle(string $payload): void {}
}

function tracked(array $listeners): void
{
    $listener = new LocalAuditor();
    /** @var list<LocalAuditor> $listeners */
    foreach ($listeners as $listener) {
        $listener->handle('ready');
    }
}
"#;
    let (resolved_first_nodes, resolved_first_edges) =
        index_files(&[("src/Acme/App/tracked.php", resolved_first_source)])?;
    assert_resolved_call_to_method_owner_in_file(
        "php resolved-first order keeps the in-file resolution",
        &resolved_first_nodes,
        &resolved_first_edges,
        "tracked",
        "LocalAuditor",
        "handle",
        "src/Acme/App/tracked.php",
    );
    let tracked_edges =
        call_edges_from_caller_at_line(&resolved_first_nodes, &resolved_first_edges, "tracked", 16);
    assert_eq!(
        tracked_edges.len(),
        1,
        "expected one resolved edge for the loop callsite. Calls: {:?}",
        describe_call_edges(&resolved_first_edges, &resolved_first_nodes)
    );
    assert!(tracked_edges[0].resolved_target.is_some());
    assert!(
        callsite_has_segment(tracked_edges[0], "receiver-binding:loop-element@15-17"),
        "the loop marker must land on the resolved edge that replaced the placeholder: {:?}",
        tracked_edges[0].callsite_identity
    );

    assert_no_call_self_edges(
        "php foreach marker order-independence",
        &foreach_first_edges,
    );
    assert_no_call_self_edges("php foreach marker order-independence", &other_first_edges);
    assert_no_call_self_edges(
        "php foreach marker order-independence",
        &resolved_first_edges,
    );
    Ok(())
}

#[test]
fn test_php_construction_placeholder_annotates_owner_and_resolves_to_constructor()
-> anyhow::Result<()> {
    let construction_source = r#"
<?php

namespace Acme\App;

use Acme\Events\Auditor;

class Widget
{
    public function __construct(string $label) {}
}

class Plain
{
    public function touch(): void {}
}

function build_all(): void
{
    $widget = new Widget('w');
    $auditor = new Auditor();
    $plain = new Plain();
    $unknown = new Mystery();
}

function hostile_texts(): void
{
    // new Widget('comment');
    $note = "new Widget('quoted')";
    echo $note;
}
"#;
    let (nodes, edges) = index_files(&[
        ("src/Acme/Events/Auditor.php", PHP_AUDITOR_CLASS_SOURCE),
        ("src/Acme/App/build.php", construction_source),
    ])?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();

    // Hostile first: `new` text in comments and strings parses no callsite.
    for hostile_line in [28, 29] {
        assert!(
            call_edges_from_caller_at_line(&nodes, &edges, "hostile_texts", hostile_line)
                .is_empty(),
            "`new` inside comments and strings must produce no CALL edges. Calls: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }

    // Hostile: a bare `new` name unknown to the tables carries only the
    // namespace-derived module guess, and with no matching class in that
    // namespace the resolution fails closed.
    let mystery_edges = call_edges_from_caller_at_line(&nodes, &edges, "build_all", 23);
    assert_eq!(mystery_edges.len(), 1);
    assert!(callsite_has_segment(mystery_edges[0], "syntax:php-new"));
    assert!(callsite_has_segment(
        mystery_edges[0],
        "receiver-owner:Mystery"
    ));
    assert!(callsite_has_segment(
        mystery_edges[0],
        r"receiver-module:Acme\App.Mystery"
    ));
    assert_eq!(
        mystery_edges[0].resolved_target, None,
        "a same-namespace construction with no matching class must fail closed"
    );
    assert_eq!(mystery_edges[0].certainty, None);

    // Hostile: an annotated construction of a class with no declared
    // constructor stays unresolved.
    let plain_edges = call_edges_from_caller_at_line(&nodes, &edges, "build_all", 22);
    assert_eq!(plain_edges.len(), 1);
    assert!(callsite_has_segment(plain_edges[0], "syntax:php-new"));
    assert!(callsite_has_segment(plain_edges[0], "receiver-owner:Plain"));
    assert_eq!(
        plain_edges[0].resolved_target, None,
        "a class without `__construct` must fail the construction resolution closed"
    );

    // Same-file construction: placeholder named by the callee TYPE text,
    // `syntax:php-new`-marked, owner-annotated, resolved to the constructor
    // with resolver-set certainty.
    let widget_edges = call_edges_from_caller_at_line(&nodes, &edges, "build_all", 20);
    assert_eq!(widget_edges.len(), 1);
    let widget_edge = widget_edges[0];
    assert_eq!(
        node_by_id
            .get(&widget_edge.target)
            .map(|target| target.serialized_name.as_str()),
        Some("Widget"),
        "the construction placeholder must be named by the callee type text"
    );
    assert!(callsite_has_segment(widget_edge, "syntax:php-new"));
    assert!(callsite_has_segment(widget_edge, "receiver-owner:Widget"));
    let widget_resolved = widget_edge
        .resolved_target
        .and_then(|id| node_by_id.get(&id))
        .map(|node| node.serialized_name.as_str());
    assert_eq!(
        widget_resolved,
        Some("Widget.__construct"),
        "same-file construction must resolve to the owner's constructor. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert_eq!(widget_edge.certainty, Some(ResolutionCertainty::Certain));

    // Imported construction resolves through the annotated receiver module.
    let auditor_edges = call_edges_from_caller_at_line(&nodes, &edges, "build_all", 21);
    assert_eq!(auditor_edges.len(), 1);
    let auditor_edge = auditor_edges[0];
    assert!(callsite_has_segment(auditor_edge, "syntax:php-new"));
    assert!(callsite_has_segment(auditor_edge, "receiver-owner:Auditor"));
    assert!(callsite_has_segment(
        auditor_edge,
        r"receiver-module:Acme\Events.Auditor"
    ));
    let auditor_resolved = auditor_edge
        .resolved_target
        .and_then(|id| node_by_id.get(&id))
        .map(|node| node.serialized_name.as_str());
    assert_eq!(
        auditor_resolved,
        Some("Auditor.__construct"),
        "imported construction must resolve to the imported owner's constructor. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert_eq!(auditor_edge.certainty, Some(ResolutionCertainty::Certain));

    assert_no_call_self_edges("php construction placeholders", &edges);

    // Outside any namespace declaration there is no namespace to guess from:
    // an unknown bare construction stays entirely unannotated.
    let global_namespace_source = r#"
<?php

function bare_build(): void
{
    $thing = new Mystery();
}
"#;
    let (global_nodes, global_edges) =
        index_files(&[("src/scripts/bare.php", global_namespace_source)])?;
    let bare_edges = call_edges_from_caller_at_line(&global_nodes, &global_edges, "bare_build", 6);
    assert_eq!(bare_edges.len(), 1);
    assert!(callsite_has_segment(bare_edges[0], "syntax:php-new"));
    assert!(
        !callsite_has_segment_with_prefix(bare_edges[0], "receiver-owner:"),
        "a global-namespace unknown construction must stay unannotated: {:?}",
        bare_edges[0].callsite_identity
    );
    assert_eq!(bare_edges[0].resolved_target, None);

    Ok(())
}

#[test]
fn test_php_multiline_named_argument_construction_resolves_same_namespace_constructor()
-> anyhow::Result<()> {
    let record_source = r#"
<?php

namespace Acme\Logs;

class Record
{
    public function __construct(
        public Stamp $stamp,
        public string $channel
    ) {
    }
}

class Stamp
{
    public function __construct() {}
}
"#;
    // `Record` and `Stamp` are referenced unqualified from a sibling file of
    // the same namespace — no `use` statement, exactly the Monolog
    // `new LogRecord(...)` shape — across a multi-line, named-argument
    // construction with a nested construction inside one argument.
    let factory_source = r#"
<?php

namespace Acme\Logs;

function make_record(string $channel): Record
{
    $record = new Record(
        stamp: new Stamp(),
        channel: $channel,
    );
    return $record;
}

function miss_record(): void
{
    $ghost = new MissingRecord(
        channel: 'app',
    );
}
"#;
    let (nodes, edges) = index_files(&[
        ("src/Acme/Logs/Record.php", record_source),
        ("src/Acme/Logs/factory.php", factory_source),
    ])?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();

    // Hostile first: the same-namespace miss carries the namespace-derived
    // annotation but resolves nothing.
    let miss_edges = call_edges_from_caller_at_line(&nodes, &edges, "miss_record", 17);
    assert_eq!(
        miss_edges.len(),
        1,
        "expected one placeholder for the missing-class construction. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert!(callsite_has_segment(miss_edges[0], "syntax:php-new"));
    assert!(callsite_has_segment(
        miss_edges[0],
        "receiver-owner:MissingRecord"
    ));
    assert!(callsite_has_segment(
        miss_edges[0],
        r"receiver-module:Acme\Logs.MissingRecord"
    ));
    assert_eq!(
        miss_edges[0].resolved_target, None,
        "an unqualified name with no matching same-namespace class must fail closed"
    );
    assert_eq!(miss_edges[0].certainty, None);

    // The outer multi-line named-argument construction: annotated with the
    // namespace-derived module and resolved certain to the promoted-property
    // constructor in the sibling file.
    let record_edges = call_edges_from_caller_at_line(&nodes, &edges, "make_record", 8);
    assert_eq!(
        record_edges.len(),
        1,
        "expected one CALL edge for the outer multi-line construction. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    let record_edge = record_edges[0];
    assert_eq!(
        node_by_id
            .get(&record_edge.target)
            .map(|target| target.serialized_name.as_str()),
        Some("Record"),
        "the multi-line construction placeholder must be named by the callee type text"
    );
    assert!(callsite_has_segment(record_edge, "syntax:php-new"));
    assert!(callsite_has_segment(record_edge, "receiver-owner:Record"));
    assert!(callsite_has_segment(
        record_edge,
        r"receiver-module:Acme\Logs.Record"
    ));
    assert_eq!(
        record_edge
            .resolved_target
            .and_then(|id| node_by_id.get(&id))
            .map(|node| node.serialized_name.as_str()),
        Some("Record.__construct"),
        "the same-namespace construction must resolve to the sibling file's constructor. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert_eq!(record_edge.certainty, Some(ResolutionCertainty::Certain));

    // The nested construction inside the outer named argument gets its own
    // annotated, certain edge — nesting shadows nothing.
    let stamp_edges = call_edges_from_caller_at_line(&nodes, &edges, "make_record", 9);
    assert_eq!(
        stamp_edges.len(),
        1,
        "expected one CALL edge for the nested construction. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    let stamp_edge = stamp_edges[0];
    assert!(callsite_has_segment(stamp_edge, "syntax:php-new"));
    assert!(callsite_has_segment(stamp_edge, "receiver-owner:Stamp"));
    assert!(callsite_has_segment(
        stamp_edge,
        r"receiver-module:Acme\Logs.Stamp"
    ));
    assert_eq!(
        stamp_edge
            .resolved_target
            .and_then(|id| node_by_id.get(&id))
            .map(|node| node.serialized_name.as_str()),
        Some("Stamp.__construct"),
        "the nested construction must resolve to its own constructor. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert_eq!(stamp_edge.certainty, Some(ResolutionCertainty::Certain));

    assert_no_call_self_edges("php multiline same-namespace construction", &edges);
    Ok(())
}

#[test]
fn test_javascript_same_file_receiver_call_resolves_to_declared_owner_method() -> anyhow::Result<()>
{
    let source = r#"
class ConsoleNotifier {
    notify(value) {
        this.writeLog(value);
    }

    writeLog(value) {}
}

class Workflow {
    run(value) {
        this.decorate(value);
    }

    decorate(value) {}
}

class OtherWorkflow {
    run(value) {}
}

function orchestrate() {
    const workflow = new Workflow();
    workflow.run("ready");
}

function factory() {
    const workflow = makeWorkflow();
    workflow.run("ready");
}

function scopedOne() {
    const workflow = new Workflow();
    workflow.run("ready");
}

function scopedTwo(workflow) {
    workflow.run("ready");
}
"#;

    let (nodes, edges) = index_single_file("main.js", source)?;
    assert_resolved_call_to_method_owner(
        "javascript this receiver",
        &nodes,
        &edges,
        "ConsoleNotifier.notify",
        "ConsoleNotifier",
        "writeLog",
    );
    assert_resolved_call_to_method_owner(
        "javascript this receiver",
        &nodes,
        &edges,
        "Workflow.run",
        "Workflow",
        "decorate",
    );
    assert_resolved_call_to_method_owner(
        "javascript constructor receiver",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "run",
    );
    assert_resolved_call_to_method_owner(
        "javascript scoped constructor receiver",
        &nodes,
        &edges,
        "scopedOne",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "javascript factory receiver stays unresolved",
        &nodes,
        &edges,
        "factory",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "javascript parameter receiver stays unresolved",
        &nodes,
        &edges,
        "scopedTwo",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "javascript constructor receiver avoids same-name owner",
        &nodes,
        &edges,
        "orchestrate",
        "OtherWorkflow",
        "run",
    );

    Ok(())
}

#[test]
fn test_javascript_property_assigned_functions_preserve_receiver_and_import_boundaries()
-> anyhow::Result<()> {
    let source = r#"
const send = require('wire-send');

const app = {};
app.handle = function handle(req, res) {
    return [req].map(() => this.router.handle(req, res));
};
app.regular = function regular(req, res) {
    return [req].map(function () {
        return this.router.handle(req, res);
    });
};
app.use = function use(path) {
    var router = this.router;
    return [path].map(function () {
        return router.use(path);
    });
};
app.arrowAlias = function arrowAlias(path) {
    let router = this.router;
    return [path].map(() => router.use(path));
};
app.reassigned = function reassigned(path, other) {
    var router = this.router;
    router = other;
    return router.use(path);
};
app.blockReassigned = function blockReassigned(path, other) {
    let router = this.router;
    {
        router = other;
    }
    return router.use(path);
};
app.closureReassigned = function closureReassigned(path, other) {
    let router = this.router;
    (() => { router = other; })();
    return router.use(path);
};
app.dormantClosure = function dormantClosure(path, other) {
    let router = this.router;
    const mutate = () => { router = other; };
    return router.use(path);
};
app.localShadowClosure = function localShadowClosure(path, other) {
    let router = this.router;
    (() => {
        let router = other;
        router = other;
    })();
    return router.use(path);
};
app.parameterShadowClosure = function parameterShadowClosure(path, other) {
    let router = this.router;
    ((router) => { router = other; })(other);
    return router.use(path);
};
app.laterDirectWrite = function laterDirectWrite(path, other) {
    let router = this.router;
    const result = router.use(path);
    router = other;
    return result;
};
app.hoistedClosure = function hoistedClosure(path, other) {
    let router = this.router;
    mutate();
    return router.use(path);
    function mutate() { router = other; }
};
app.hoistedShadowClosure = function hoistedShadowClosure(path, other) {
    let router = this.router;
    mutate(other);
    return router.use(path);
    function mutate(router) { router = other; }
};
app.loopReassigned = function loopReassigned(path, other) {
    let router = this.router;
    for (let index = 0; index < 2; index++) {
        if (index > 0) router.use(path);
        router = other;
    }
};
app.loneLoopLaterWrite = function loneLoopLaterWrite(path, other) {
    let router = this.router;
    const result = router.use(path);
    for (let index = 0; index < 2; index++) {
        router = other;
    }
    return result;
};
app.shadowed = function shadowed(path, other) {
    var router = this.router;
    return (function (router) {
        return router.use(path);
    })(other);
};
app.foreign = function foreign(path, other) {
    var router = other.router;
    return router.use(path);
};

const res = {};
res.end = function end(body) {
    return body;
};
res.send = function send(body) {
    return this.end(body);
};
res.sendFile = function sendFile(path) {
    return send(path);
};
"#;

    let (nodes, edges) = index_single_file("server.js", source)?;
    assert_resolved_call_to_method_owner(
        "javascript property-assigned this receiver",
        &nodes,
        &edges,
        "res.send",
        "res",
        "end",
    );
    assert_no_resolved_call_to_method_owner(
        "javascript imported bare call avoids same-name receiver method",
        &nodes,
        &edges,
        "res.sendFile",
        "res",
        "send",
    );

    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let external_router_call = edges.iter().find(|edge| {
        if edge.kind != EdgeKind::CALL {
            return false;
        }
        let Some(source) = node_by_id.get(&edge.source) else {
            return false;
        };
        let Some(target) = node_by_id.get(&edge.target) else {
            return false;
        };
        is_matching_name(&source.serialized_name, "app.handle")
            && is_matching_name(&target.serialized_name, "handle")
    });
    let external_router_call = external_router_call.unwrap_or_else(|| {
        panic!(
            "expected exact app.handle external callsite. Calls: {:?}",
            describe_call_edges(&edges, &nodes)
        )
    });
    assert_eq!(external_router_call.resolved_target, None);
    assert!(
        external_router_call
            .callsite_identity
            .as_deref()
            .is_some_and(|identity| identity.contains("receiver-owner:app.router")),
        "external callsite should retain its receiver provenance without a resolved target: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    let alias_edges = edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::CALL
                && edge
                    .callsite_identity
                    .as_deref()
                    .is_some_and(|identity| identity.contains("receiver-owner:app.router"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        alias_edges.len(),
        8,
        "only lexical `this.router` and its exact local alias may carry app.router provenance: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert!(
        alias_edges.iter().all(|edge| {
            node_by_id
                .get(&edge.source)
                .is_none_or(|source| !is_matching_name(&source.serialized_name, "app.regular"))
        }),
        "a nested regular function owns its own `this` and must not inherit app.router provenance: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    let alias_use = alias_edges
        .iter()
        .find(|edge| {
            node_by_id
                .get(&edge.source)
                .is_some_and(|source| is_matching_name(&source.serialized_name, "app.use"))
                && node_by_id
                    .get(&edge.target)
                    .is_some_and(|target| is_matching_name(&target.serialized_name, "use"))
        })
        .expect("exact router alias call");
    assert_eq!(alias_use.resolved_target, None);
    assert!(
        node_by_id
            .get(&alias_use.source)
            .is_some_and(|source| is_matching_name(&source.serialized_name, "app.use")),
        "alias provenance should stay attached to the owning property-assigned callable: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert!(
        alias_edges.iter().any(|edge| {
            node_by_id
                .get(&edge.source)
                .is_some_and(|source| is_matching_name(&source.serialized_name, "app.arrowAlias"))
        }),
        "a nested arrow must retain the exact lexical router alias: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert!(
        edges
            .iter()
            .filter(|edge| {
                edge.kind == EdgeKind::CALL
                    && node_by_id.get(&edge.source).is_some_and(|source| {
                        is_matching_name(&source.serialized_name, "app.blockReassigned")
                    })
            })
            .all(|edge| {
                !edge
                    .callsite_identity
                    .as_deref()
                    .is_some_and(|identity| identity.contains("receiver-owner:app.router"))
                    && edge.resolved_target.is_none_or(|target| {
                        node_by_id.get(&target).is_none_or(|resolved| {
                            !is_matching_name(&resolved.serialized_name, "app.router.use")
                        })
                    })
            }),
        "an unconditional nested-block write must invalidate router alias provenance: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    for caller in [
        "app.closureReassigned",
        "app.dormantClosure",
        "app.hoistedClosure",
        "app.loopReassigned",
    ] {
        assert!(
            edges
                .iter()
                .filter(|edge| {
                    edge.kind == EdgeKind::CALL
                        && node_by_id
                            .get(&edge.source)
                            .is_some_and(|source| is_matching_name(&source.serialized_name, caller))
                })
                .all(|edge| !edge
                    .callsite_identity
                    .as_deref()
                    .is_some_and(|identity| identity.contains("receiver-owner:app.router"))),
            "a prior nested closure write to the captured outer alias must invalidate provenance even when invocation is unknown in `{caller}`: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }
    for caller in [
        "app.localShadowClosure",
        "app.parameterShadowClosure",
        "app.laterDirectWrite",
        "app.hoistedShadowClosure",
        "app.loneLoopLaterWrite",
    ] {
        assert!(
            alias_edges.iter().any(|edge| {
                node_by_id
                    .get(&edge.source)
                    .is_some_and(|source| is_matching_name(&source.serialized_name, caller))
            }),
            "a nested closure write to its own shadow must not invalidate the outer alias in `{caller}`: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }

    Ok(())
}

#[test]
fn test_javascript_runtime_import_boundaries_follow_exact_binding_scope_and_alias()
-> anyhow::Result<()> {
    let source = r#"
const direct = require('wire-direct');
const { send: dispatch } = require('wire-dispatch');
const relay = require('wire-relay').send;
let assigned;
assigned = require('wire-assigned');
let reassigned;
reassigned = require('wire-reassigned');
reassigned = (value) => value;
let dynamic;
dynamic = require(moduleName);
let destructured;
({ destructured } = require('wire-destructured'));
const holder = {};
holder.member = require('wire-member');
let parameterBound;
const captured = require('wire-captured');
const dormant = require('wire-dormant');
const shadowLocal = require('wire-shadow-local');
const shadowParameter = require('wire-shadow-parameter');
let laterWrite = require('wire-later-write');
let hoistedWrite = require('wire-hoisted-write');
let hoistedShadow = require('wire-hoisted-shadow');
let loopWrite = require('wire-loop-write');
let loneLoopWrite = require('wire-lone-loop-write');

function local(value) { return value; }
(() => { captured = local; })();
const mutateDormant = () => { dormant = local; };
(() => {
    let shadowLocal = local;
    shadowLocal = local;
})();
((shadowParameter) => { shadowParameter = local; })(local);

class Response {
    direct(body) { return body; }
    dispatch(body) { return body; }
    relay(body) { return body; }
    assigned(body) { return body; }
    reassigned(body) { return body; }
    dynamic(body) { return body; }
    destructured(body) { return body; }
}

function externalCalls(body) {
    direct(body);
    dispatch(body);
    relay(body);
    assigned(body);
    reassigned(body);
    dynamic(body);
    destructured(body);
}

function memberExternal(body) {
    holder.member(body);
}

function parameterAssignment(parameterBound, body) {
    parameterBound = require('wire-parameter');
    parameterBound(body);
}

function shadowedLocal(body) {
    const dispatch = (value) => value;
    dispatch(body);
}

function shadowedDeclaration(body) {
    function relay(value) { return value; }
    relay(body);
}

const NamedClass = class dispatch {
    static namedClassCall(body) { return dispatch(body); }
};

const AnonymousClass = class {
    static anonymousClassCall(body) { return dispatch(body); }
};

function outerClassCall(body) {
    return dispatch(body);
}

function capturedExternal(body) {
    return captured(body);
}

function dormantExternal(body) {
    return dormant(body);
}

function shadowLocalExternal(body) {
    return shadowLocal(body);
}

function shadowParameterExternal(body) {
    return shadowParameter(body);
}

function laterWriteExternal(body) {
    return laterWrite(body);
}

laterWrite = local;

function hoistedWriteExternal(body) {
    return hoistedWrite(body);
}

mutateHoistedWrite();
function mutateHoistedWrite() {
    hoistedWrite = local;
}

function hoistedShadowExternal(body) {
    return hoistedShadow(body);
}

mutateHoistedShadow(local);
function mutateHoistedShadow(hoistedShadow) {
    hoistedShadow = local;
}

function loopWriteExternal(body) {
    for (let index = 0; index < 2; index++) {
        if (index > 0) loopWrite(body);
        loopWrite = local;
    }
}

function loneLoopWriteExternal(body) {
    const result = loneLoopWrite(body);
    for (let index = 0; index < 2; index++) {
        loneLoopWrite = local;
    }
    return result;
}
"#;
    let (nodes, edges) = index_single_file("neutral.js", source)?;
    for method in ["direct", "dispatch", "relay", "assigned"] {
        assert_no_resolved_call_to_method_owner(
            "exact CommonJS aliases stay external",
            &nodes,
            &edges,
            "externalCalls",
            "Response",
            method,
        );
    }

    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let mut external_markers = edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::CALL
                && node_by_id.get(&edge.source).is_some_and(|source| {
                    is_matching_name(&source.serialized_name, "externalCalls")
                })
                && edge
                    .callsite_identity
                    .as_deref()
                    .is_some_and(|identity| identity.contains("syntax:js-runtime-import-call"))
        })
        .map(|edge| {
            node_by_id
                .get(&edge.target)
                .expect("runtime import call target")
                .serialized_name
                .clone()
        })
        .collect::<Vec<_>>();
    external_markers.sort();
    assert_eq!(
        external_markers,
        ["assigned", "direct", "dispatch", "relay"],
        "declarator aliases and one simple assignment binding need exact external-boundary markers: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    for (caller, reason) in [
        ("memberExternal", "member-assignment"),
        ("parameterAssignment", "parameter-shadowed assignment"),
    ] {
        assert!(
            edges
                .iter()
                .filter(|edge| {
                    edge.kind == EdgeKind::CALL
                        && node_by_id
                            .get(&edge.source)
                            .is_some_and(|source| is_matching_name(&source.serialized_name, caller))
                })
                .all(|edge| !edge
                    .callsite_identity
                    .as_deref()
                    .is_some_and(|identity| identity.contains("syntax:js-runtime-import-call"))),
            "{reason} must not create a bare-binding marker: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }
    for caller in ["shadowedLocal", "shadowedDeclaration"] {
        let calls = edges.iter().filter(|edge| {
            edge.kind == EdgeKind::CALL
                && node_by_id
                    .get(&edge.source)
                    .is_some_and(|source| is_matching_name(&source.serialized_name, caller))
        });
        assert!(
            calls.into_iter().all(|edge| !edge
                .callsite_identity
                .as_deref()
                .is_some_and(|identity| identity.contains("syntax:js-runtime-import-call"))),
            "shadowed local binding in `{caller}` must not inherit the file import marker: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }
    for (caller, method) in [
        ("shadowedLocal", "dispatch"),
        ("shadowedDeclaration", "relay"),
    ] {
        assert_no_resolved_call_to_method_owner(
            "shadowed import name avoids unrelated receiver method",
            &nodes,
            &edges,
            caller,
            "Response",
            method,
        );
    }
    for caller in ["shadowedLocal", "shadowedDeclaration"] {
        assert!(
            edges
                .iter()
                .filter(|edge| edge.kind == EdgeKind::CALL)
                .any(|edge| {
                    node_by_id
                        .get(&edge.source)
                        .is_some_and(|source| is_matching_name(&source.serialized_name, caller))
                        && edge.resolved_target.is_some_and(|target| {
                            node_by_id.get(&target).is_some_and(|resolved| {
                                resolved.kind == codestory_contracts::graph::NodeKind::FUNCTION
                                    && matches!(
                                        resolved.serialized_name.as_str(),
                                        "dispatch" | "relay"
                                    )
                            })
                        })
                }),
            "local callable shadow in `{caller}` should resolve locally: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }
    for (caller, should_inherit_import) in [
        ("namedClassCall", false),
        ("anonymousClassCall", true),
        ("outerClassCall", true),
    ] {
        let import_marked = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::CALL)
            .any(|edge| {
                node_by_id
                    .get(&edge.source)
                    .is_some_and(|source| is_matching_name(&source.serialized_name, caller))
                    && edge
                        .callsite_identity
                        .as_deref()
                        .is_some_and(|identity| identity.contains("syntax:js-runtime-import-call"))
            });
        assert_eq!(
            import_marked,
            should_inherit_import,
            "named class expressions shadow only within their class; anonymous classes and outer calls retain the import: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }
    for (caller, should_inherit_import) in [
        ("capturedExternal", false),
        ("dormantExternal", false),
        ("shadowLocalExternal", true),
        ("shadowParameterExternal", true),
        ("laterWriteExternal", true),
        ("hoistedWriteExternal", false),
        ("hoistedShadowExternal", true),
        ("loopWriteExternal", false),
        ("loneLoopWriteExternal", true),
    ] {
        let import_marked = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::CALL)
            .any(|edge| {
                node_by_id
                    .get(&edge.source)
                    .is_some_and(|source| is_matching_name(&source.serialized_name, caller))
                    && edge
                        .callsite_identity
                        .as_deref()
                        .is_some_and(|identity| identity.contains("syntax:js-runtime-import-call"))
            });
        assert_eq!(
            import_marked,
            should_inherit_import,
            "outer-binding closure writes invalidate runtime-import provenance conservatively, while local and parameter shadows do not in `{caller}`: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }
    let import_binding_ids = edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::IMPORT
                && node_by_id
                    .get(&edge.target)
                    .is_some_and(|target| target.serialized_name.contains("wire-dispatch"))
        })
        .map(|edge| edge.source)
        .collect::<HashSet<_>>();
    assert!(
        !import_binding_ids.is_empty(),
        "expected the outer CommonJS import binding"
    );
    let named_class_calls = edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::CALL
                && node_by_id.get(&edge.source).is_some_and(|source| {
                    is_matching_name(&source.serialized_name, "namedClassCall")
                })
        })
        .collect::<Vec<_>>();
    assert!(
        !named_class_calls.is_empty()
            && named_class_calls.iter().all(|edge| edge
                .resolved_target
                .is_none_or(|target| !import_binding_ids.contains(&target))),
        "the named class binding must not resolve to the outer CommonJS import: {:?}",
        describe_call_edges(&edges, &nodes)
    );

    Ok(())
}

#[test]
fn test_script_same_line_runtime_import_marks_only_the_unshadowed_occurrence() -> anyhow::Result<()>
{
    for extension in ["js", "ts", "tsx"] {
        for imported_first in [false, true] {
            let imported = "function outside() { dispatch(2); }";
            let shadowed = "function shadow() { const dispatch = (value) => value; dispatch(1); }";
            let source = if imported_first {
                format!("const dispatch = require('opaque-module'); {imported} {shadowed}\n")
            } else {
                format!("const dispatch = require('opaque-module'); {shadowed} {imported}\n")
            };
            let (nodes, edges) = index_single_file(&format!("neutral.{extension}"), &source)?;
            let marked = edges
                .iter()
                .filter(|edge| {
                    edge.kind == EdgeKind::CALL
                        && edge.callsite_identity.as_deref().is_some_and(|identity| {
                            identity
                                .split('|')
                                .any(|part| part == "syntax:js-runtime-import-call")
                        })
                })
                .collect::<Vec<_>>();
            assert_eq!(
                marked.len(),
                1,
                "{extension}/imported_first={imported_first}: {:?}",
                describe_call_edges(&edges, &nodes)
            );
            assert!(marked[0].resolved_target.is_none());
        }
    }
    Ok(())
}

#[test]
fn test_typescript_family_member_calls_never_inherit_runtime_import_markers() -> anyhow::Result<()>
{
    for (filename, source) in [
        (
            "neutral.ts",
            r#"
const dispatch = require('wire-dispatch');
class Response { dispatch(body: string) { return body; } }
function invoke(response: Response, body: string) {
    dispatch(body);
    response.dispatch(body);
}
"#,
        ),
        (
            "neutral.tsx",
            r#"
const dispatch = require('wire-dispatch');
class Response { dispatch(body: string) { return body; } }
function Widget({ response, body }: { response: Response; body: string }) {
    dispatch(body);
    response.dispatch(body);
    return <span>{body}</span>;
}
"#,
        ),
    ] {
        let (nodes, edges) = index_single_file(filename, source)?;
        let marked = edges
            .iter()
            .filter(|edge| {
                edge.kind == EdgeKind::CALL
                    && edge
                        .callsite_identity
                        .as_deref()
                        .is_some_and(|identity| identity.contains("syntax:js-runtime-import-call"))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            marked.len(),
            1,
            "only the exact bare import call in `{filename}` should carry the marker: {:?}",
            describe_call_edges(&edges, &nodes)
        );
        let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
        assert_eq!(
            node_by_id
                .get(&marked[0].target)
                .map(|target| target.serialized_name.as_str()),
            Some("dispatch"),
            "the member expression target must not inherit the bare import marker"
        );
    }
    Ok(())
}

#[test]
fn test_javascript_imported_constructor_receiver_call_resolves_to_imported_owner_method()
-> anyhow::Result<()> {
    let main_source = r#"
import { Workflow } from "./workflow.js";

class LocalWorkflow {
    run(value) {}
}

function orchestrateLocal() {
    const workflow = new LocalWorkflow();
    workflow.run("ready");
}

function orchestrateRemote() {
    const workflow = new Workflow();
    workflow.run("ready");
}
"#;
    let default_import_source = r#"
import Workflow from "./workflow.js";

function orchestrateRemote() {
    const workflow = new Workflow();
    workflow.run("ready");
}
"#;
    let default_alias_source = r#"
import RemoteWorkflow from "./workflow.js";

function orchestrateRemote() {
    const workflow = new RemoteWorkflow();
    workflow.run("ready");
}
"#;
    let aliased_source = r#"
import { Workflow as RemoteWorkflow } from "./workflow.js";

function orchestrateRemote() {
    const workflow = new RemoteWorkflow();
    workflow.run("ready");
}
"#;
    let missing_import_source = r#"
import { Workflow } from "./missing.js";

function orchestrateRemote() {
    const workflow = new Workflow();
    workflow.run("ready");
}
"#;
    let duplicate_import_source = r#"
import { Workflow } from "./workflow.js";
import { Workflow } from "./other.js";

function orchestrateRemote() {
    const workflow = new Workflow();
    workflow.run("ready");
}
"#;
    let local_shadow_source = r#"
import { Workflow } from "./workflow.js";

class Workflow {
    run(value) {}
}

function orchestrateRemote() {
    const workflow = new Workflow();
    workflow.run("ready");
}
"#;
    let future_binding_source = r#"
import { Workflow } from "./workflow.js";

function beforeDeclaration() {
    workflow.run("ready");
    const workflow = new Workflow();
}
"#;
    let function_shadow_source = r#"
import { Workflow } from "./workflow.js";

function orchestrateRemote() {
    class Workflow {
        run(value) {}
    }

    const workflow = new Workflow();
    workflow.run("ready");
}
"#;
    let qualified_constructor_source = r#"
import * as remote from "./workflow.js";

class Workflow {
    run(value) {}
}

function orchestrateRemote() {
    const workflow = new remote.Workflow();
    workflow.run("ready");
}
"#;
    let workflow_source = r#"
export class Workflow {
    run(value) {}
}
"#;
    let default_workflow_source = r#"
export default class Workflow {
    run(value) {}
}
"#;
    let other_workflow_source = r#"
export class Workflow {
    run(value) {}
}
"#;

    let (nodes, edges) =
        index_files(&[("main.js", main_source), ("workflow.js", workflow_source)])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript same-file constructor receiver still resolves",
        &nodes,
        &edges,
        "orchestrateLocal",
        "LocalWorkflow",
        "run",
        "main.js",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript imported constructor receiver avoids local file",
        &nodes,
        &edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "main.js",
    );
    assert_resolved_call_to_method_owner_in_file(
        "javascript named imported constructor receiver resolves imported owner",
        &nodes,
        &edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (default_nodes, default_edges) = index_files(&[
        ("main.js", default_import_source),
        ("workflow.js", default_workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript default imported constructor receiver resolves imported owner",
        &default_nodes,
        &default_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (default_alias_nodes, default_alias_edges) = index_files(&[
        ("main.js", default_alias_source),
        ("workflow.js", default_workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript default import alias constructor receiver resolves exported owner",
        &default_alias_nodes,
        &default_alias_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (aliased_nodes, aliased_edges) = index_files(&[
        ("main.js", aliased_source),
        ("workflow.js", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript aliased imported constructor receiver resolves imported owner",
        &aliased_nodes,
        &aliased_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("main.js", missing_import_source),
        ("workflow.js", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript missing imported constructor receiver stays unresolved",
        &missing_nodes,
        &missing_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("main.js", duplicate_import_source),
        ("workflow.js", workflow_source),
        ("other.js", other_workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript duplicate imported constructor receiver avoids first owner",
        &duplicate_nodes,
        &duplicate_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.js",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript duplicate imported constructor receiver avoids second owner",
        &duplicate_nodes,
        &duplicate_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "other.js",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("main.js", local_shadow_source),
        ("workflow.js", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript local class shadows imported constructor receiver",
        &shadow_nodes,
        &shadow_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "main.js",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript local class shadow avoids imported constructor owner",
        &shadow_nodes,
        &shadow_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (future_nodes, future_edges) = index_files(&[
        ("main.js", future_binding_source),
        ("workflow.js", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript constructor receiver does not use future binding",
        &future_nodes,
        &future_edges,
        "beforeDeclaration",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (function_shadow_nodes, function_shadow_edges) = index_files(&[
        ("main.js", function_shadow_source),
        ("workflow.js", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript function local class shadows imported constructor receiver",
        &function_shadow_nodes,
        &function_shadow_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "main.js",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript function local class shadow avoids imported owner",
        &function_shadow_nodes,
        &function_shadow_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (qualified_nodes, qualified_edges) = index_files(&[
        ("main.js", qualified_constructor_source),
        ("workflow.js", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript qualified constructor receiver avoids local same-name owner",
        &qualified_nodes,
        &qualified_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "main.js",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript qualified constructor receiver stays unresolved",
        &qualified_nodes,
        &qualified_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.js",
    );

    Ok(())
}

#[test]
fn test_javascript_class_property_receiver_call_resolves_to_same_file_owner_method()
-> anyhow::Result<()> {
    let source = r#"
class Workflow {
    run(value) {}
}

class OtherWorkflow {
    run(value) {}
}

class Entry {
    constructor() {
        this.workflow = new Workflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;
    let private_source = r#"
class Workflow {
    run(value) {}
}

class OtherWorkflow {
    run(value) {}
}

class Entry {
    #workflow;

    constructor() {
        this.#workflow = new Workflow();
    }

    run(value) {
        this.#workflow.run(value);
    }
}

class InitializerEntry {
    #workflow = new Workflow();

    run(value) {
        this.#workflow.run(value);
    }
}

class StaticEntry {
    static #workflow = new Workflow();

    run(value) {
        this.#workflow.run(value);
    }
}
"#;

    let (nodes, edges) = index_single_file("main.js", source)?;
    assert_resolved_call_to_method_owner(
        "javascript property receiver",
        &nodes,
        &edges,
        "Entry.run",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "javascript property receiver avoids same-name owner",
        &nodes,
        &edges,
        "Entry.run",
        "OtherWorkflow",
        "run",
    );

    let (private_nodes, private_edges) = index_single_file("private.js", private_source)?;
    assert_resolved_call_to_method_owner(
        "javascript private property receiver",
        &private_nodes,
        &private_edges,
        "Entry.run",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "javascript private property receiver avoids same-name owner",
        &private_nodes,
        &private_edges,
        "Entry.run",
        "OtherWorkflow",
        "run",
    );
    assert_resolved_call_to_method_owner(
        "javascript private field initializer receiver",
        &private_nodes,
        &private_edges,
        "InitializerEntry.run",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "javascript static private field initializer stays unresolved",
        &private_nodes,
        &private_edges,
        "StaticEntry.run",
        "Workflow",
        "run",
    );

    Ok(())
}

#[test]
fn test_javascript_imported_class_property_receiver_call_resolves_to_imported_owner_method()
-> anyhow::Result<()> {
    let named_source = r#"
import { Workflow } from "./workflow.js";

class Entry {
    constructor() {
        this.workflow = new Workflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;
    let default_source = r#"
import Workflow from "./workflow.js";

class Entry {
    constructor() {
        this.workflow = new Workflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;
    let default_alias_source = r#"
import RemoteWorkflow from "./workflow.js";

class Entry {
    constructor() {
        this.workflow = new RemoteWorkflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;
    let aliased_source = r#"
import { Workflow as RemoteWorkflow } from "./workflow.js";

class Entry {
    constructor() {
        this.workflow = new RemoteWorkflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;
    let private_initializer_source = r#"
import { Workflow } from "./workflow.js";

class Entry {
    #workflow = new Workflow();

    run(value) {
        this.#workflow.run(value);
    }
}
"#;
    let missing_source = r#"
import { Workflow } from "./missing.js";

class Entry {
    constructor() {
        this.workflow = new Workflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;
    let duplicate_source = r#"
import { Workflow } from "./workflow.js";
import { Workflow } from "./other.js";

class Entry {
    constructor() {
        this.workflow = new Workflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;
    let local_shadow_source = r#"
import { Workflow } from "./workflow.js";

class Workflow {
    run(value) {}
}

class Entry {
    constructor() {
        this.workflow = new Workflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;
    let qualified_source = r#"
import * as remote from "./workflow.js";

class Entry {
    constructor() {
        this.workflow = new remote.Workflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;
    let workflow_source = r#"
export class Workflow {
    run(value) {}
}
"#;
    let default_workflow_source = r#"
export default class Workflow {
    run(value) {}
}
"#;
    let other_workflow_source = r#"
export class Workflow {
    run(value) {}
}
"#;

    let (named_nodes, named_edges) =
        index_files(&[("main.js", named_source), ("workflow.js", workflow_source)])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript named imported property receiver resolves imported owner",
        &named_nodes,
        &named_edges,
        "Entry.run",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (default_nodes, default_edges) = index_files(&[
        ("main.js", default_source),
        ("workflow.js", default_workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript default imported property receiver resolves imported owner",
        &default_nodes,
        &default_edges,
        "Entry.run",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (default_alias_nodes, default_alias_edges) = index_files(&[
        ("main.js", default_alias_source),
        ("workflow.js", default_workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript default import alias property receiver resolves exported owner",
        &default_alias_nodes,
        &default_alias_edges,
        "Entry.run",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (aliased_nodes, aliased_edges) = index_files(&[
        ("main.js", aliased_source),
        ("workflow.js", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript aliased imported property receiver resolves imported owner",
        &aliased_nodes,
        &aliased_edges,
        "Entry.run",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (private_initializer_nodes, private_initializer_edges) = index_files(&[
        ("main.js", private_initializer_source),
        ("workflow.js", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript imported private initializer property receiver resolves imported owner",
        &private_initializer_nodes,
        &private_initializer_edges,
        "Entry.run",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("main.js", missing_source),
        ("workflow.js", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript missing imported property receiver stays unresolved",
        &missing_nodes,
        &missing_edges,
        "Entry.run",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("main.js", duplicate_source),
        ("workflow.js", workflow_source),
        ("other.js", other_workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript duplicate imported property receiver avoids first owner",
        &duplicate_nodes,
        &duplicate_edges,
        "Entry.run",
        "Workflow",
        "run",
        "workflow.js",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript duplicate imported property receiver avoids second owner",
        &duplicate_nodes,
        &duplicate_edges,
        "Entry.run",
        "Workflow",
        "run",
        "other.js",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("main.js", local_shadow_source),
        ("workflow.js", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "javascript local class shadows imported property receiver",
        &shadow_nodes,
        &shadow_edges,
        "Entry.run",
        "Workflow",
        "run",
        "main.js",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript local class property shadow avoids imported owner",
        &shadow_nodes,
        &shadow_edges,
        "Entry.run",
        "Workflow",
        "run",
        "workflow.js",
    );

    let (qualified_nodes, qualified_edges) = index_files(&[
        ("main.js", qualified_source),
        ("workflow.js", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript qualified property constructor stays unresolved",
        &qualified_nodes,
        &qualified_edges,
        "Entry.run",
        "Workflow",
        "run",
        "workflow.js",
    );

    Ok(())
}

#[test]
fn test_javascript_class_property_receiver_call_stays_fail_closed() -> anyhow::Result<()> {
    let source = r#"
class Workflow {
    run(value) {}
}

class OtherWorkflow {
    run(value) {}
}

class FactoryEntry {
    constructor() {
        this.workflow = makeWorkflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}

class MixedEntry {
    constructor() {
        this.workflow = new Workflow();
        this.workflow = new OtherWorkflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}

class ReassignedUnknownEntry {
    constructor() {
        this.workflow = new Workflow();
        this.workflow = makeWorkflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}

class ErasedEntry {
    constructor(workflow) {
        this.workflow = workflow;
    }

    run(value) {
        this.workflow.run(value);
    }
}

class LocalShadowEntry {
    run(value) {
        const workflow = new Workflow();
        this.workflow.run(value);
    }
}

class StaticEntry {
    static setup() {
        this.workflow = new Workflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}

class QualifiedEntry {
    constructor() {
        this.workflow = new Namespace.Workflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;

    let (nodes, edges) = index_single_file("main.js", source)?;
    for source_name in [
        "FactoryEntry.run",
        "MixedEntry.run",
        "ReassignedUnknownEntry.run",
        "ErasedEntry.run",
        "LocalShadowEntry.run",
        "StaticEntry.run",
        "QualifiedEntry.run",
    ] {
        assert_no_resolved_call_to_method_owner(
            "javascript property receiver stays unresolved",
            &nodes,
            &edges,
            source_name,
            "Workflow",
            "run",
        );
        assert_no_resolved_call_to_method_owner(
            "javascript property receiver avoids ambiguous same-name owner",
            &nodes,
            &edges,
            source_name,
            "OtherWorkflow",
            "run",
        );
    }

    Ok(())
}

#[test]
fn test_javascript_property_receiver_duplicate_same_file_targets_stay_fail_closed()
-> anyhow::Result<()> {
    let source = r#"
class Workflow {
    run(value) {}
    run(value, options) {}
}

class Entry {
    constructor() {
        this.workflow = new Workflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;

    let (nodes, edges) = index_single_file("main.js", source)?;
    assert_no_resolved_call_to_method_owner(
        "javascript property receiver duplicate method target stays unresolved",
        &nodes,
        &edges,
        "Entry.run",
        "Workflow",
        "run",
    );

    Ok(())
}

#[test]
fn test_javascript_property_receiver_does_not_resolve_to_cross_file_owner() -> anyhow::Result<()> {
    let main_source = r#"
class Entry {
    constructor() {
        this.workflow = new Workflow();
    }

    run(value) {
        this.workflow.run(value);
    }
}
"#;
    let workflow_source = r#"
export class Workflow {
    run(value) {}
}
"#;

    let (nodes, edges) =
        index_files(&[("main.js", main_source), ("workflow.js", workflow_source)])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "javascript unimported property receiver avoids cross-file owner",
        &nodes,
        &edges,
        "Entry.run",
        "Workflow",
        "run",
        "workflow.js",
    );

    Ok(())
}

#[test]
fn test_typescript_same_file_receiver_call_resolves_to_declared_owner_method() -> anyhow::Result<()>
{
    let source = r#"
class ConsoleNotifier {
    notify(value: string): void {
        this.writeLog(value);
    }

    writeLog(value: string): void {}
}

class Workflow<T> {
    run(value: T): void {
        this.decorate(value);
    }

    decorate(value: T): void {}
}

class OtherWorkflow {
    run(value: string): void {}
}

function orchestrate(): void {
    const workflow = new Workflow<string>();
    workflow.run("ready");
}

function factory(): void {
    const workflow = makeWorkflow();
    workflow.run("ready");
}

function makeWorkflow(): Workflow<string> {
    return new Workflow<string>();
}

function scopedOne(): void {
    const workflow = new Workflow<string>();
    workflow.run("ready");
}

function scopedTwo(workflow: any): void {
    workflow.run("ready");
}
"#;

    let (nodes, edges) = index_single_file("main.ts", source)?;
    assert_resolved_call_to_method_owner(
        "typescript this receiver",
        &nodes,
        &edges,
        "ConsoleNotifier.notify",
        "ConsoleNotifier",
        "writeLog",
    );
    assert_resolved_call_to_method_owner(
        "typescript this receiver",
        &nodes,
        &edges,
        "Workflow.run",
        "Workflow",
        "decorate",
    );
    assert_resolved_call_to_method_owner(
        "typescript constructor receiver",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "run",
    );
    assert_resolved_call_to_method_owner(
        "typescript scoped constructor receiver",
        &nodes,
        &edges,
        "scopedOne",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "typescript factory receiver stays unresolved",
        &nodes,
        &edges,
        "factory",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "typescript any receiver stays unresolved",
        &nodes,
        &edges,
        "scopedTwo",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "typescript constructor receiver avoids same-name owner",
        &nodes,
        &edges,
        "orchestrate",
        "OtherWorkflow",
        "run",
    );

    Ok(())
}

#[test]
fn test_typescript_class_property_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let source = r#"
interface Notifier {
    notifyEvent(value: string): void;
}

class Repository {
    save(value: string): void {}
}

class OtherRepository {
    save(value: string): void {}
}

class PrivateRepository {
    persist(value: string): void {}
}

class Workflow {
    private notifier: Notifier;
    private repository: Repository;
    #privateRepository: PrivateRepository;
    private loose: any;
    private unknownOwner: UnknownOwner;

    constructor(notifier: Notifier, repository: Repository, loose: any) {
        this.notifier = notifier;
        this.repository = repository;
        this.#privateRepository = new PrivateRepository();
        this.loose = loose;
    }

    run(value: string): void {
        this.notifier.notifyEvent(value);
        this.repository.save(value);
        this.#privateRepository.persist(value);
        this.decorate(value);
    }

    decorate(value: string): void {}

    parameterDoesNotAffectThis(repository: Notifier, value: string): void {
        this.repository.save(value);
    }

    erasedProperty(value: string): void {
        this.loose.notifyEvent(value);
    }

    unknownProperty(value: string): void {
        this.unknownOwner.notifyEvent(value);
    }
}
"#;

    let (nodes, edges) = index_single_file("main.ts", source)?;
    assert_resolved_call_to_method_owner(
        "typescript class property receiver",
        &nodes,
        &edges,
        "Workflow.run",
        "Notifier",
        "notifyEvent",
    );
    assert_resolved_call_to_method_owner(
        "typescript class property receiver",
        &nodes,
        &edges,
        "Workflow.run",
        "PrivateRepository",
        "persist",
    );
    assert_resolved_call_to_method_owner(
        "typescript private class property receiver",
        &nodes,
        &edges,
        "Workflow.run",
        "Repository",
        "save",
    );
    assert_resolved_call_to_method_owner(
        "typescript this method receiver remains supported",
        &nodes,
        &edges,
        "Workflow.run",
        "Workflow",
        "decorate",
    );
    assert_resolved_call_to_method_owner(
        "typescript explicit this property ignores parameter shadow",
        &nodes,
        &edges,
        "Workflow.parameterDoesNotAffectThis",
        "Repository",
        "save",
    );
    assert_no_resolved_call_to_method_owner(
        "typescript class property receiver avoids same-name method owner",
        &nodes,
        &edges,
        "Workflow.run",
        "OtherRepository",
        "save",
    );
    assert_no_resolved_call_to_method_owner(
        "typescript any property receiver stays unresolved",
        &nodes,
        &edges,
        "Workflow.erasedProperty",
        "Notifier",
        "notifyEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "typescript unknown property receiver stays unresolved",
        &nodes,
        &edges,
        "Workflow.unknownProperty",
        "Notifier",
        "notifyEvent",
    );

    Ok(())
}

#[test]
fn test_typescript_imported_constructor_receiver_call_resolves_to_imported_owner_method()
-> anyhow::Result<()> {
    let main_source = r#"
import { Workflow } from "./workflow";

class LocalWorkflow {
    run(value: string): void {}
}

function orchestrateLocal(): void {
    const workflow = new LocalWorkflow();
    workflow.run("ready");
}

function orchestrateRemote(): void {
    const workflow = new Workflow();
    workflow.run("ready");
}
"#;
    let aliased_source = r#"
import { Workflow as RemoteWorkflow } from "./workflow";

function orchestrateRemote(): void {
    const workflow = new RemoteWorkflow();
    workflow.run("ready");
}
"#;
    let namespace_source = r#"
import * as remote from "./workflow";

function orchestrateRemote(): void {
    const workflow = new remote.Workflow();
    workflow.run("ready");
}
"#;
    let missing_namespace_source = r#"
function orchestrateRemote(): void {
    const workflow = new remote.Workflow();
    workflow.run("ready");
}
"#;
    let duplicate_namespace_source = r#"
import * as remote from "./workflow";
import * as remote from "./other";

function orchestrateRemote(): void {
    const workflow = new remote.Workflow();
    workflow.run("ready");
}
"#;
    let missing_import_source = r#"
import { Workflow } from "./missing";

function orchestrateRemote(): void {
    const workflow = new Workflow();
    workflow.run("ready");
}
"#;
    let duplicate_import_source = r#"
import { Workflow } from "./workflow";
import { Workflow } from "./other";

function orchestrateRemote(): void {
    const workflow = new Workflow();
    workflow.run("ready");
}
"#;
    let local_shadow_source = r#"
import { Workflow } from "./workflow";

class Workflow {
    run(value: string): void {}
}

function orchestrateRemote(): void {
    const workflow = new Workflow();
    workflow.run("ready");
}
"#;
    let future_binding_source = r#"
import { Workflow } from "./workflow";

function beforeDeclaration(): void {
    workflow.run("ready");
    const workflow = new Workflow();
}
"#;
    let function_shadow_source = r#"
import { Workflow } from "./workflow";

function orchestrateRemote(): void {
    class Workflow {
        run(value: string): void {}
    }

    const workflow = new Workflow();
    workflow.run("ready");
}
"#;
    let block_factory_shadow_source = r#"
import { Workflow } from "./workflow";

declare function makeWorkflow(): unknown;

function scopedShadow(workflow: Workflow): void {
    {
        const workflow = makeWorkflow();
        workflow.run("ready");
    }
}
"#;
    let workflow_source = r#"
export class Workflow {
    run(value: string): void {}
}
"#;
    let other_workflow_source = r#"
export class Workflow {
    run(value: string): void {}
}
"#;

    let (nodes, edges) =
        index_files(&[("main.ts", main_source), ("workflow.ts", workflow_source)])?;
    assert_resolved_call_to_method_owner_in_file(
        "typescript same-file constructor receiver still resolves",
        &nodes,
        &edges,
        "orchestrateLocal",
        "LocalWorkflow",
        "run",
        "main.ts",
    );
    assert_resolved_call_to_method_owner_in_file(
        "typescript imported constructor receiver",
        &nodes,
        &edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.ts",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "typescript imported constructor receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "orchestrateRemote",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "workflow.ts",
            expected_count: 1,
        },
    );

    let (aliased_nodes, aliased_edges) = index_files(&[
        ("main.ts", aliased_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "typescript aliased imported constructor receiver",
        &aliased_nodes,
        &aliased_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.ts",
    );

    let (namespace_nodes, namespace_edges) = index_files(&[
        ("main.ts", namespace_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "typescript namespace imported constructor receiver",
        &namespace_nodes,
        &namespace_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.ts",
    );

    let (missing_namespace_nodes, missing_namespace_edges) = index_files(&[
        ("main.ts", missing_namespace_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript missing namespace constructor receiver stays unresolved",
        &missing_namespace_nodes,
        &missing_namespace_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.ts",
    );

    let (duplicate_namespace_nodes, duplicate_namespace_edges) = index_files(&[
        ("main.ts", duplicate_namespace_source),
        ("workflow.ts", workflow_source),
        ("other.ts", other_workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript duplicate namespace constructor receiver avoids first owner",
        &duplicate_namespace_nodes,
        &duplicate_namespace_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.ts",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript duplicate namespace constructor receiver avoids second owner",
        &duplicate_namespace_nodes,
        &duplicate_namespace_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "other.ts",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("main.ts", missing_import_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript missing imported constructor receiver",
        &missing_nodes,
        &missing_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.ts",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("main.ts", duplicate_import_source),
        ("workflow.ts", workflow_source),
        ("other.ts", other_workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript duplicate imported constructor receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.ts",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript duplicate imported constructor receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "other.ts",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("main.ts", local_shadow_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript local constructor shadow avoids imported owner",
        &shadow_nodes,
        &shadow_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.ts",
    );

    let (future_nodes, future_edges) = index_files(&[
        ("main.ts", future_binding_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript constructor receiver does not use future binding",
        &future_nodes,
        &future_edges,
        "beforeDeclaration",
        "Workflow",
        "run",
        "workflow.ts",
    );

    let (function_shadow_nodes, function_shadow_edges) = index_files(&[
        ("main.ts", function_shadow_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "typescript function local class shadows imported constructor receiver",
        &function_shadow_nodes,
        &function_shadow_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "main.ts",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript function local class shadow avoids imported owner",
        &function_shadow_nodes,
        &function_shadow_edges,
        "orchestrateRemote",
        "Workflow",
        "run",
        "workflow.ts",
    );

    let (block_shadow_nodes, block_shadow_edges) = index_files(&[
        ("main.ts", block_factory_shadow_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript block local factory receiver suppresses parameter fallback",
        &block_shadow_nodes,
        &block_shadow_edges,
        "scopedShadow",
        "Workflow",
        "run",
        "workflow.ts",
    );

    Ok(())
}

#[test]
fn test_typescript_class_property_receiver_does_not_resolve_to_unimported_cross_file_owner()
-> anyhow::Result<()> {
    let main_source = r#"
class Workflow {
    private repository: Repository;

    run(value: string): void {
        this.repository.save(value);
    }
}
"#;
    let repository_source = r#"
export class Repository {
    save(value: string): void {}
}
"#;

    let (nodes, edges) = index_files(&[
        ("main.ts", main_source),
        ("repository.ts", repository_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript unimported cross-file property receiver stays unresolved",
        &nodes,
        &edges,
        "Workflow.run",
        "Repository",
        "save",
        "repository.ts",
    );

    Ok(())
}

#[test]
fn test_tsx_same_file_receiver_call_resolves_to_declared_owner_method() -> anyhow::Result<()> {
    let source = r#"
class WidgetWorkflow {
    run(value: string): string {
        return this.decorate(value);
    }

    decorate(value: string): string {
        return value;
    }
}

export function Widget(): JSX.Element {
    const workflow = new WidgetWorkflow();
    workflow.run("ready");
    return <div>{workflow.run("again")}</div>;
}
"#;

    let (nodes, edges) = index_single_file("widget.tsx", source)?;
    assert_resolved_call_to_method_owner(
        "tsx this receiver",
        &nodes,
        &edges,
        "WidgetWorkflow.run",
        "WidgetWorkflow",
        "decorate",
    );
    assert_resolved_call_to_method_owner(
        "tsx constructor receiver",
        &nodes,
        &edges,
        "Widget",
        "WidgetWorkflow",
        "run",
    );

    Ok(())
}

#[test]
fn test_tsx_imported_constructor_receiver_call_resolves_to_imported_owner_method()
-> anyhow::Result<()> {
    let widget_source = r#"
import { WidgetWorkflow } from "./workflow";

export function Widget(): JSX.Element {
    const workflow = new WidgetWorkflow();
    return <div>{workflow.run("ready")}</div>;
}
"#;
    let workflow_source = r#"
export class WidgetWorkflow {
    run(value: string): string {
        return value;
    }
}
"#;
    let aliased_widget_source = r#"
import { WidgetWorkflow as RemoteWidgetWorkflow } from "./workflow";

export function Widget(): JSX.Element {
    const workflow = new RemoteWidgetWorkflow();
    return <div>{workflow.run("ready")}</div>;
}
"#;
    let namespace_widget_source = r#"
import * as remote from "./workflow";

export function Widget(): JSX.Element {
    const workflow = new remote.WidgetWorkflow();
    return <div>{workflow.run("ready")}</div>;
}
"#;

    let (nodes, edges) = index_files(&[
        ("widget.tsx", widget_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "tsx imported constructor receiver",
        &nodes,
        &edges,
        "Widget",
        "WidgetWorkflow",
        "run",
        "workflow.ts",
    );

    let (aliased_nodes, aliased_edges) = index_files(&[
        ("widget.tsx", aliased_widget_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "tsx aliased imported constructor receiver",
        &aliased_nodes,
        &aliased_edges,
        "Widget",
        "WidgetWorkflow",
        "run",
        "workflow.ts",
    );

    let (namespace_nodes, namespace_edges) = index_files(&[
        ("widget.tsx", namespace_widget_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "tsx namespace imported constructor receiver",
        &namespace_nodes,
        &namespace_edges,
        "Widget",
        "WidgetWorkflow",
        "run",
        "workflow.ts",
    );

    Ok(())
}

#[test]
fn test_typescript_tsx_class_property_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let source = r#"
class WidgetRepository {
    save(value: string): string {
        return value;
    }
}

class PrivateWidgetRepository {
    persist(value: string): string {
        return value;
    }
}

class WidgetWorkflow {
    private repository: WidgetRepository;
    #privateRepository: PrivateWidgetRepository;

    run(value: string): string {
        this.#privateRepository = new PrivateWidgetRepository();
        return this.repository.save(this.#privateRepository.persist(value));
    }
}

export function Widget(): JSX.Element {
    return <div>ready</div>;
}
"#;

    let (nodes, edges) = index_single_file("widget.tsx", source)?;
    assert_resolved_call_to_method_owner(
        "tsx class property receiver",
        &nodes,
        &edges,
        "WidgetWorkflow.run",
        "WidgetRepository",
        "save",
    );
    assert_resolved_call_to_method_owner(
        "tsx private class property receiver",
        &nodes,
        &edges,
        "WidgetWorkflow.run",
        "PrivateWidgetRepository",
        "persist",
    );

    Ok(())
}

#[test]
fn test_typescript_tsx_imported_class_property_receiver_call_resolves_to_imported_owner_method()
-> anyhow::Result<()> {
    let notifier_source = r#"
export interface Notifier {
    notifyEvent(value: string): void;
}
"#;
    let repository_source = r#"
export interface Repository {
    save(value: string): string;
}
"#;
    let other_notifier_source = r#"
export interface Notifier {
    notifyEvent(value: string): void;
}
"#;
    let other_repository_source = r#"
export interface Repository {
    save(value: string): string;
}
"#;
    let widget_source = r#"
import type { Notifier as WidgetNotifier } from "./notifier";
import type { Repository } from "./repository";

class WidgetWorkflow {
    #privateNotifier: WidgetNotifier;
    private repository: Repository;

    run(value: string): string {
        this.#privateNotifier.notifyEvent(value);
        return this.repository.save(value);
    }
}

export function Widget(): JSX.Element {
    return <div>ready</div>;
}
"#;
    let missing_import_source = r#"
import type { Repository } from "./repository";

class WidgetWorkflow {
    private repository: Repository;

    run(value: string): string {
        return this.repository.save(value);
    }
}

export function Widget(): JSX.Element {
    return <div>ready</div>;
}
"#;
    let local_shadow_source = r#"
import type { Repository as RemoteRepository } from "./repository";

interface Repository {
    save(value: string): string;
}

class WidgetWorkflow {
    private repository: Repository;

    run(value: string): string {
        return this.repository.save(value);
    }
}

export function Widget(): JSX.Element {
    return <div>ready</div>;
}
"#;

    let (nodes, edges) = index_files(&[
        ("notifier.ts", notifier_source),
        ("repository.ts", repository_source),
        ("other/notifier.ts", other_notifier_source),
        ("other/repository.ts", other_repository_source),
        ("widget.tsx", widget_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "tsx imported private class property receiver",
        &nodes,
        &edges,
        "WidgetWorkflow.run",
        "Notifier",
        "notifyEvent",
        "notifier.ts",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "tsx imported private class property receiver avoids same-name owner",
        &nodes,
        &edges,
        "WidgetWorkflow.run",
        "Notifier",
        "notifyEvent",
        "other/notifier.ts",
    );
    assert_resolved_call_to_method_owner_in_file(
        "tsx imported class property receiver",
        &nodes,
        &edges,
        "WidgetWorkflow.run",
        "Repository",
        "save",
        "repository.ts",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "tsx imported class property receiver avoids same-name owner",
        &nodes,
        &edges,
        "WidgetWorkflow.run",
        "Repository",
        "save",
        "other/repository.ts",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("repository.ts", repository_source),
        ("widget.tsx", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "tsx local shadowed class property receiver",
        &shadow_nodes,
        &shadow_edges,
        "WidgetWorkflow.run",
        "Repository",
        "save",
        "widget.tsx",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "tsx local shadowed class property receiver avoids imported owner",
        &shadow_nodes,
        &shadow_edges,
        "WidgetWorkflow.run",
        "Repository",
        "save",
        "repository.ts",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("other/repository.ts", other_repository_source),
        ("widget.tsx", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "tsx missing imported class property receiver",
        &missing_nodes,
        &missing_edges,
        "WidgetWorkflow.run",
        "Repository",
        "save",
        "other/repository.ts",
    );

    Ok(())
}

#[test]
fn test_swift_imported_typed_receiver_call_resolves_to_module_owner_method() -> anyhow::Result<()> {
    let mail_notifier_source = r#"
public protocol Notifier {
    func notifyEvent(_ value: String)
}
"#;
    let mail_duplicate_source = r#"
public protocol Notifier {
    func notifyEvent(_ value: String)
}
"#;
    let other_notifier_source = r#"
public protocol Notifier {
    func notifyEvent(_ value: String)
}
"#;
    let mail_repository_source = r#"
public protocol Repository {
    func save(_ value: String)
}
"#;
    let workflow_source = r#"
import MailKit

func run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let module_qualified_source = r#"
import MailKit
import OtherKit

func run(notifier: MailKit.Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let module_qualified_local_shadow_source = r#"
import MailKit

class Notifier {
    func notifyEvent(_ value: String) {}
}

func run(notifier: MailKit.Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let scoped_import_source = r#"
import class MailKit.Notifier

func run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let scoped_unrelated_source = r#"
import class MailKit.Notifier

func run(repository: Repository) {
    repository.save("ready")
}
"#;
    let missing_import_source = r#"
import MissingKit

func run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let no_import_source = r#"
func run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let ambiguous_import_source = r#"
import MailKit
import OtherKit

func run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;
    let local_shadow_source = r#"
import MailKit

class Notifier {
    func notifyEvent(_ value: String) {}
}

func run(notifier: Notifier) {
    notifier.notifyEvent("ready")
}
"#;

    let (nodes, edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/OtherKit/Notifier.swift", other_notifier_source),
        ("Sources/App/App.swift", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift imported typed receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "swift imported typed receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Notifier",
            method_name: "notifyEvent",
            file_suffix: "Sources/MailKit/Notifier.swift",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift imported typed receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/OtherKit/Notifier.swift",
    );

    let (qualified_nodes, qualified_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/OtherKit/Notifier.swift", other_notifier_source),
        ("Sources/App/App.swift", module_qualified_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift module-qualified typed receiver",
        &qualified_nodes,
        &qualified_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift module-qualified typed receiver",
        &qualified_nodes,
        &qualified_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/OtherKit/Notifier.swift",
    );

    let (qualified_shadow_nodes, qualified_shadow_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        (
            "Sources/App/App.swift",
            module_qualified_local_shadow_source,
        ),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift module-qualified receiver ignores same-file shadow",
        &qualified_shadow_nodes,
        &qualified_shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift module-qualified receiver ignores same-file shadow",
        &qualified_shadow_nodes,
        &qualified_shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/App/App.swift",
    );

    let (scoped_nodes, scoped_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/App/App.swift", scoped_import_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift scoped imported receiver type",
        &scoped_nodes,
        &scoped_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );

    let (scoped_unrelated_nodes, scoped_unrelated_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/MailKit/Repository.swift", mail_repository_source),
        ("Sources/App/App.swift", scoped_unrelated_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift scoped import does not imply whole-module import",
        &scoped_unrelated_nodes,
        &scoped_unrelated_edges,
        "run",
        "Repository",
        "save",
        "Sources/MailKit/Repository.swift",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/App/App.swift", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift missing imported receiver module",
        &missing_nodes,
        &missing_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );

    let (no_import_nodes, no_import_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/App/App.swift", no_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift unimported receiver type stays unresolved",
        &no_import_nodes,
        &no_import_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );

    let (cross_package_nodes, cross_package_edges) = index_files(&[
        (
            "Packages/Dependency/Sources/MailKit/Notifier.swift",
            mail_notifier_source,
        ),
        ("Packages/App/Sources/App/App.swift", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift imported receiver stays within package root",
        &cross_package_nodes,
        &cross_package_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Packages/Dependency/Sources/MailKit/Notifier.swift",
    );

    let (ambiguous_nodes, ambiguous_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/OtherKit/Notifier.swift", other_notifier_source),
        ("Sources/App/App.swift", ambiguous_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift ambiguous imported receiver module",
        &ambiguous_nodes,
        &ambiguous_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift ambiguous imported receiver module",
        &ambiguous_nodes,
        &ambiguous_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/OtherKit/Notifier.swift",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        (
            "Sources/MailKit/AlternateNotifier.swift",
            mail_duplicate_source,
        ),
        ("Sources/App/App.swift", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift duplicate imported receiver module owner",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift duplicate imported receiver module owner",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/AlternateNotifier.swift",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/App/App.swift", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift local receiver shadows imported module",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/App/App.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift local receiver shadows imported module",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );

    Ok(())
}

#[test]
fn test_swift_same_file_constructor_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let workflow_source = r#"
class Workflow {
    func run(value: String) {}
}

class OtherWorkflow {
    func run(value: String) {}
}

func makeWorkflow() -> Workflow {
    return Workflow()
}

func orchestrate() {
    let workflow = Workflow()
    workflow.run(value: "ready")
}

func factoryOnly() {
    let workflow = makeWorkflow()
    workflow.run(value: "ready")
}

func directOnly() {
    makeWorkflow()
}

func erased(workflow: Any) {
    workflow.run(value: "ready")
}

func innerShadow(workflow: Any, enabled: Bool) {
    if enabled {
        let workflow = Workflow()
        workflow.run(value: "inner")
    }
    workflow.run(value: "outer")
}

func orderAware() {
    workflow.run(value: "early")
    let workflow = Workflow()
    workflow.run(value: "late")
}
"#;
    let (nodes, edges) = index_files(&[("Sources/App/Workflow.swift", workflow_source)])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift same-file constructor receiver",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "run",
        "Sources/App/Workflow.swift",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "swift same-file constructor receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "orchestrate",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "Sources/App/Workflow.swift",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner(
        "swift same-file constructor receiver avoids other owner",
        &nodes,
        &edges,
        "orchestrate",
        "OtherWorkflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "swift factory receiver stays unresolved",
        &nodes,
        &edges,
        "factoryOnly",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "swift erased receiver stays unresolved",
        &nodes,
        &edges,
        "erased",
        "Workflow",
        "run",
    );
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let direct_call_resolved = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter_map(|edge| {
            let source = node_by_id.get(&edge.source)?;
            if !is_matching_name(&source.serialized_name, "directOnly") {
                return None;
            }
            let resolved_id = edge.resolved_target?;
            let resolved_node = node_by_id.get(&resolved_id)?;
            Some(resolved_node.serialized_name.as_str())
        })
        .any(|resolved_name| is_matching_name(resolved_name, "makeWorkflow"));
    assert!(
        direct_call_resolved,
        "Case `swift direct call remains resolved`: expected direct CALL from `directOnly` to resolve to `makeWorkflow`. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "swift inner constructor binding stays block-scoped",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "innerShadow",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "Sources/App/Workflow.swift",
            expected_count: 1,
        },
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "swift constructor binding only applies after declaration",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "orderAware",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "Sources/App/Workflow.swift",
            expected_count: 1,
        },
    );

    let imported_owner_source = r#"
public class Workflow {
    public init() {}
    public func run(value: String) {}
}
"#;
    let imported_caller_source = r#"
func orchestrate() {
    let workflow = Workflow()
    workflow.run(value: "ready")
}
"#;
    let (imported_nodes, imported_edges) = index_files(&[
        ("Sources/WorkflowKit/Workflow.swift", imported_owner_source),
        ("Sources/App/App.swift", imported_caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift unimported constructor receiver does not use cross-file owner",
        &imported_nodes,
        &imported_edges,
        "orchestrate",
        "Workflow",
        "run",
        "Sources/WorkflowKit/Workflow.swift",
    );

    Ok(())
}

#[test]
fn test_swift_imported_constructor_receiver_call_resolves_to_module_owner_method()
-> anyhow::Result<()> {
    let workflow_source = r#"
public class Workflow {
    public init() {}
    public func run(value: String) {}
}
"#;
    let other_workflow_source = r#"
public class Workflow {
    public init() {}
    public func run(value: String) {}
}
"#;
    let caller_source = r#"
import WorkflowKit

func orchestrate() {
    let workflow = Workflow()
    workflow.run(value: "ready")
}
"#;
    let module_qualified_source = r#"
import WorkflowKit
import OtherKit

class Workflow {
    func run(value: String) {}
}

func orchestrate() {
    let workflow = WorkflowKit.Workflow()
    workflow.run(value: "ready")
}
"#;
    let scoped_import_source = r#"
import class WorkflowKit.Workflow

func orchestrate() {
    let workflow = Workflow()
    workflow.run(value: "ready")
}
"#;
    let ambiguous_import_source = r#"
import WorkflowKit
import OtherKit

func orchestrate() {
    let workflow = Workflow()
    workflow.run(value: "ready")
}
"#;
    let missing_import_source = r#"
import MissingKit

func orchestrate() {
    let workflow = Workflow()
    workflow.run(value: "ready")
}
"#;
    let local_shadow_source = r#"
import WorkflowKit

class Workflow {
    func run(value: String) {}
}

func orchestrate() {
    let workflow = Workflow()
    workflow.run(value: "ready")
}
"#;

    let (nodes, edges) = index_files(&[
        ("Sources/WorkflowKit/Workflow.swift", workflow_source),
        ("Sources/OtherKit/Workflow.swift", other_workflow_source),
        ("Sources/App/App.swift", caller_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift imported constructor receiver exact module",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "run",
        "Sources/WorkflowKit/Workflow.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift imported constructor receiver avoids other module",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "run",
        "Sources/OtherKit/Workflow.swift",
    );

    let (qualified_nodes, qualified_edges) = index_files(&[
        ("Sources/WorkflowKit/Workflow.swift", workflow_source),
        ("Sources/OtherKit/Workflow.swift", other_workflow_source),
        ("Sources/App/App.swift", module_qualified_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift module-qualified constructor receiver exact module",
        &qualified_nodes,
        &qualified_edges,
        "orchestrate",
        "Workflow",
        "run",
        "Sources/WorkflowKit/Workflow.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift module-qualified constructor receiver avoids same-file shadow",
        &qualified_nodes,
        &qualified_edges,
        "orchestrate",
        "Workflow",
        "run",
        "Sources/App/App.swift",
    );

    let (scoped_nodes, scoped_edges) = index_files(&[
        ("Sources/WorkflowKit/Workflow.swift", workflow_source),
        ("Sources/App/App.swift", scoped_import_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift scoped imported constructor receiver exact module",
        &scoped_nodes,
        &scoped_edges,
        "orchestrate",
        "Workflow",
        "run",
        "Sources/WorkflowKit/Workflow.swift",
    );

    let (ambiguous_nodes, ambiguous_edges) = index_files(&[
        ("Sources/WorkflowKit/Workflow.swift", workflow_source),
        ("Sources/OtherKit/Workflow.swift", other_workflow_source),
        ("Sources/App/App.swift", ambiguous_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift ambiguous imported constructor receiver",
        &ambiguous_nodes,
        &ambiguous_edges,
        "orchestrate",
        "Workflow",
        "run",
        "Sources/WorkflowKit/Workflow.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift ambiguous imported constructor receiver",
        &ambiguous_nodes,
        &ambiguous_edges,
        "orchestrate",
        "Workflow",
        "run",
        "Sources/OtherKit/Workflow.swift",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("Sources/WorkflowKit/Workflow.swift", workflow_source),
        ("Sources/App/App.swift", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift missing imported constructor receiver",
        &missing_nodes,
        &missing_edges,
        "orchestrate",
        "Workflow",
        "run",
        "Sources/WorkflowKit/Workflow.swift",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("Sources/WorkflowKit/Workflow.swift", workflow_source),
        ("Sources/App/App.swift", local_shadow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift local constructor shadow avoids imported owner",
        &shadow_nodes,
        &shadow_edges,
        "orchestrate",
        "Workflow",
        "run",
        "Sources/WorkflowKit/Workflow.swift",
    );

    Ok(())
}

#[test]
fn test_swift_same_file_property_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let source = r#"
class Event {}

protocol Notifier {
    func notifyEvent(_ event: Event)
}

class Repository {
    func save(_ event: Event) {}
}

class Workflow {
    private let notifier: Notifier
    private let repository: Repository = Repository()
    private let erased: Any

    init(notifier: Notifier) {
        self.notifier = notifier
        self.erased = notifier
    }

    func run(event: Event) {
        notifier.notifyEvent(event)
        self.repository.save(event)
        self.decorate(event)
    }

    func parameterShadowsProperty(notifier: Repository, event: Event) {
        notifier.save(event)
        notifier.notifyEvent(event)
    }

    func selfPropertyDespiteParameter(repository: Notifier, event: Event) {
        self.repository.save(event)
        repository.notifyEvent(event)
    }

    func localConstructorShadowsProperty(event: Event) {
        let notifier = Repository()
        notifier.save(event)
        notifier.notifyEvent(event)
    }

    func localFactoryShadowsProperty(event: Event) {
        let notifier = makeNotifier()
        notifier.notifyEvent(event)
    }

    func localAnyShadowsProperty(event: Event) {
        let notifier: Any = makeNotifier()
        notifier.notifyEvent(event)
    }

    func erasedProperty(event: Event) {
        erased.notifyEvent(event)
    }

    func decorate(_ event: Event) {}
}

class OtherWorkflow {
    func decorate(_ event: Event) {}
}

func makeNotifier() -> Notifier {
    fatalError()
}
"#;

    let (nodes, edges) = index_single_file("Sources/App/Workflow.swift", source)?;
    assert_resolved_call_to_method_owner_in_file(
        "swift same-file property receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/App/Workflow.swift",
    );
    assert_resolved_call_to_method_owner_in_file(
        "swift self property receiver",
        &nodes,
        &edges,
        "run",
        "Repository",
        "save",
        "Sources/App/Workflow.swift",
    );
    assert_resolved_call_to_method_owner_in_file(
        "swift self receiver",
        &nodes,
        &edges,
        "run",
        "Workflow",
        "decorate",
        "Sources/App/Workflow.swift",
    );
    assert_no_resolved_call_to_method_owner(
        "swift self receiver avoids same-named owner",
        &nodes,
        &edges,
        "run",
        "OtherWorkflow",
        "decorate",
    );
    assert_resolved_call_to_method_owner_in_file(
        "swift parameter shadows property receiver",
        &nodes,
        &edges,
        "parameterShadowsProperty",
        "Repository",
        "save",
        "Sources/App/Workflow.swift",
    );
    assert_no_resolved_call_to_method_owner(
        "swift parameter shadow prevents property fallback",
        &nodes,
        &edges,
        "parameterShadowsProperty",
        "Notifier",
        "notifyEvent",
    );
    assert_resolved_call_to_method_owner_in_file(
        "swift explicit self property ignores same-named parameter",
        &nodes,
        &edges,
        "selfPropertyDespiteParameter",
        "Repository",
        "save",
        "Sources/App/Workflow.swift",
    );
    assert_resolved_call_to_method_owner_in_file(
        "swift same-named parameter still resolves separately",
        &nodes,
        &edges,
        "selfPropertyDespiteParameter",
        "Notifier",
        "notifyEvent",
        "Sources/App/Workflow.swift",
    );
    assert_resolved_call_to_method_owner_in_file(
        "swift local constructor shadows property receiver",
        &nodes,
        &edges,
        "localConstructorShadowsProperty",
        "Repository",
        "save",
        "Sources/App/Workflow.swift",
    );
    assert_no_resolved_call_to_method_owner(
        "swift local constructor shadow prevents property fallback",
        &nodes,
        &edges,
        "localConstructorShadowsProperty",
        "Notifier",
        "notifyEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "swift local factory shadow prevents property fallback",
        &nodes,
        &edges,
        "localFactoryShadowsProperty",
        "Notifier",
        "notifyEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "swift local Any shadow prevents property fallback",
        &nodes,
        &edges,
        "localAnyShadowsProperty",
        "Notifier",
        "notifyEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "swift erased property receiver stays unresolved",
        &nodes,
        &edges,
        "erasedProperty",
        "Notifier",
        "notifyEvent",
    );

    let owner_source = r#"
public class Workflow {
    public func run() {}
}
"#;
    let caller_source = r#"
class Entry {
    private let workflow: Workflow

    init(workflow: Workflow) {
        self.workflow = workflow
    }

    func call() {
        workflow.run()
    }
}
"#;
    let (cross_file_nodes, cross_file_edges) = index_files(&[
        ("Sources/WorkflowKit/Workflow.swift", owner_source),
        ("Sources/App/Entry.swift", caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift property receiver does not use unimported cross-file owner",
        &cross_file_nodes,
        &cross_file_edges,
        "call",
        "Workflow",
        "run",
        "Sources/WorkflowKit/Workflow.swift",
    );

    Ok(())
}

#[test]
fn test_swift_imported_property_receiver_call_resolves_to_module_owner_method() -> anyhow::Result<()>
{
    let mail_notifier_source = r#"
public protocol Notifier {
    func notifyEvent(_ value: String)
}
"#;
    let mail_duplicate_source = r#"
public protocol Notifier {
    func notifyEvent(_ value: String)
}
"#;
    let other_notifier_source = r#"
public protocol Notifier {
    func notifyEvent(_ value: String)
}
"#;
    let mail_repository_source = r#"
public protocol Repository {
    func save(_ value: String)
}
"#;
    let workflow_source = r#"
import MailKit

class Workflow {
    private let notifier: Notifier

    init(notifier: Notifier) {
        self.notifier = notifier
    }

    func run() {
        notifier.notifyEvent("ready")
    }
}
"#;
    let module_qualified_source = r#"
import MailKit
import OtherKit

class Workflow {
    private let notifier: MailKit.Notifier

    init(notifier: MailKit.Notifier) {
        self.notifier = notifier
    }

    func run() {
        notifier.notifyEvent("ready")
    }
}
"#;
    let module_qualified_local_shadow_source = r#"
import MailKit

class Notifier {
    func notifyEvent(_ value: String) {}
}

class Workflow {
    private let notifier: MailKit.Notifier

    init(notifier: MailKit.Notifier) {
        self.notifier = notifier
    }

    func run() {
        notifier.notifyEvent("ready")
    }
}
"#;
    let scoped_import_source = r#"
import class MailKit.Notifier

class Workflow {
    private let notifier: Notifier

    init(notifier: Notifier) {
        self.notifier = notifier
    }

    func run() {
        notifier.notifyEvent("ready")
    }
}
"#;
    let scoped_unrelated_source = r#"
import class MailKit.Notifier

class Workflow {
    private let repository: Repository

    init(repository: Repository) {
        self.repository = repository
    }

    func run() {
        repository.save("ready")
    }
}
"#;
    let ambiguous_import_source = r#"
import MailKit
import OtherKit

class Workflow {
    private let notifier: Notifier

    init(notifier: Notifier) {
        self.notifier = notifier
    }

    func run() {
        notifier.notifyEvent("ready")
    }
}
"#;
    let missing_import_source = r#"
import MissingKit

class Workflow {
    private let notifier: Notifier

    init(notifier: Notifier) {
        self.notifier = notifier
    }

    func run() {
        notifier.notifyEvent("ready")
    }
}
"#;

    let (nodes, edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/OtherKit/Notifier.swift", other_notifier_source),
        ("Sources/App/Workflow.swift", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift imported property receiver exact module",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift imported property receiver exact module",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/OtherKit/Notifier.swift",
    );

    let (qualified_nodes, qualified_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/OtherKit/Notifier.swift", other_notifier_source),
        ("Sources/App/Workflow.swift", module_qualified_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift module-qualified property receiver exact module",
        &qualified_nodes,
        &qualified_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift module-qualified property receiver exact module",
        &qualified_nodes,
        &qualified_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/OtherKit/Notifier.swift",
    );

    let (qualified_shadow_nodes, qualified_shadow_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        (
            "Sources/App/Workflow.swift",
            module_qualified_local_shadow_source,
        ),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift module-qualified property receiver ignores same-file shadow",
        &qualified_shadow_nodes,
        &qualified_shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift module-qualified property receiver ignores same-file shadow",
        &qualified_shadow_nodes,
        &qualified_shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/App/Workflow.swift",
    );

    let (scoped_nodes, scoped_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/App/Workflow.swift", scoped_import_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "swift scoped imported property receiver type",
        &scoped_nodes,
        &scoped_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );

    let (scoped_unrelated_nodes, scoped_unrelated_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/MailKit/Repository.swift", mail_repository_source),
        ("Sources/App/Workflow.swift", scoped_unrelated_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift scoped property import does not imply whole-module import",
        &scoped_unrelated_nodes,
        &scoped_unrelated_edges,
        "run",
        "Repository",
        "save",
        "Sources/MailKit/Repository.swift",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/App/Workflow.swift", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift missing imported property receiver module",
        &missing_nodes,
        &missing_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );

    let (ambiguous_nodes, ambiguous_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        ("Sources/OtherKit/Notifier.swift", other_notifier_source),
        ("Sources/App/Workflow.swift", ambiguous_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift ambiguous imported property receiver module",
        &ambiguous_nodes,
        &ambiguous_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift ambiguous imported property receiver module",
        &ambiguous_nodes,
        &ambiguous_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/OtherKit/Notifier.swift",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("Sources/MailKit/Notifier.swift", mail_notifier_source),
        (
            "Sources/MailKit/AlternateNotifier.swift",
            mail_duplicate_source,
        ),
        ("Sources/App/Workflow.swift", workflow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "swift duplicate imported property receiver module owner",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/Notifier.swift",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "swift duplicate imported property receiver module owner",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "Sources/MailKit/AlternateNotifier.swift",
    );

    Ok(())
}

#[test]
fn test_typescript_imported_typed_receiver_call_resolves_to_imported_owner_method()
-> anyhow::Result<()> {
    let notifier_source = r#"
export interface Notifier {
    notifyEvent(value: string): void;
}
"#;
    let repository_source = r#"
export interface Repository {
    save(value: string): void;
}
"#;
    let archive_source = r#"
export interface Archive {
    save(value: string): void;
}
"#;
    let other_notifier_source = r#"
export interface Notifier {
    notifyEvent(value: string): void;
}
"#;
    let workflow_source = r#"
import type { Notifier as Mailer } from "./notifier";
import { type Repository } from "./repository";
import { Archive } from "./archive";

export function run(mailer: Mailer, repo: Repository, archive: Archive): void {
    mailer.notifyEvent("ready");
    repo.save("ready"); archive.save("ready");
}

export function untyped(mailer): void {
    mailer.notifyEvent("loose");
}
"#;
    let missing_import_source = r#"
import type { Notifier as Mailer } from "./notifier";

export function run(mailer: Mailer): void {
    mailer.notifyEvent("ready");
}
"#;
    let duplicate_import_source = r#"
import type { Notifier } from "./notifier";
import type { Notifier } from "./other/notifier";

export function run(notifier: Notifier): void {
    notifier.notifyEvent("ready");
}
"#;
    let local_shadow_source = r#"
import type { Notifier as Mailer } from "./notifier";

interface Mailer {
    notifyEvent(value: string): void;
}

export function run(mailer: Mailer): void {
    mailer.notifyEvent("ready");
}
"#;
    let js_extension_import_source = r#"
import type { Notifier } from "./notifier.js";

export function run(notifier: Notifier): void {
    notifier.notifyEvent("ready");
}
"#;
    let namespace_import_source = r#"
import type * as mail from "./notifier";

export function run(notifier: mail.Notifier): void {
    notifier.notifyEvent("ready");
}
"#;

    let (nodes, edges) = index_files(&[
        ("notifier.ts", notifier_source),
        ("repository.ts", repository_source),
        ("archive.ts", archive_source),
        ("other/notifier.ts", other_notifier_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "typescript imported typed receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "notifier.ts",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "typescript imported typed receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Notifier",
            method_name: "notifyEvent",
            file_suffix: "notifier.ts",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript imported typed receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "other/notifier.ts",
    );
    assert_resolved_call_to_method_owner_in_file(
        "typescript imported typed receiver",
        &nodes,
        &edges,
        "run",
        "Repository",
        "save",
        "repository.ts",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "typescript imported typed receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Repository",
            method_name: "save",
            file_suffix: "repository.ts",
            expected_count: 1,
        },
    );
    assert_resolved_call_to_method_owner_in_file(
        "typescript same-line imported typed receiver",
        &nodes,
        &edges,
        "run",
        "Archive",
        "save",
        "archive.ts",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "typescript same-line imported typed receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Archive",
            method_name: "save",
            file_suffix: "archive.ts",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript same-line imported typed receiver",
        &nodes,
        &edges,
        "run",
        "Archive",
        "save",
        "repository.ts",
    );
    assert_no_resolved_call_to_method_owner(
        "typescript imported untyped receiver",
        &nodes,
        &edges,
        "untyped",
        "Notifier",
        "notifyEvent",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("other/notifier.ts", other_notifier_source),
        ("workflow.ts", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "typescript missing imported owner",
        &missing_nodes,
        &missing_edges,
        "run",
        "Notifier",
        "notifyEvent",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("notifier.ts", notifier_source),
        ("other/notifier.ts", other_notifier_source),
        ("workflow.ts", duplicate_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript duplicate imported owner",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "notifier.ts",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript duplicate imported owner",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "other/notifier.ts",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("notifier.ts", notifier_source),
        ("workflow.ts", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "typescript local shadowed imported owner",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Mailer",
        "notifyEvent",
        "workflow.ts",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript local shadowed imported owner",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "notifier.ts",
    );

    let (js_extension_nodes, js_extension_edges) = index_files(&[
        ("notifier.ts", notifier_source),
        ("workflow.ts", js_extension_import_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "typescript js-extension relative imported owner",
        &js_extension_nodes,
        &js_extension_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "notifier.ts",
    );

    let (namespace_nodes, namespace_edges) = index_files(&[
        ("notifier.ts", notifier_source),
        ("other/notifier.ts", other_notifier_source),
        ("workflow.ts", namespace_import_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "typescript namespace imported owner",
        &namespace_nodes,
        &namespace_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "notifier.ts",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "typescript namespace imported owner",
        &namespace_nodes,
        &namespace_edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Notifier",
            method_name: "notifyEvent",
            file_suffix: "notifier.ts",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript namespace imported owner",
        &namespace_nodes,
        &namespace_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "other/notifier.ts",
    );

    Ok(())
}

#[test]
fn test_typescript_imported_class_property_receiver_call_resolves_to_imported_owner_method()
-> anyhow::Result<()> {
    let notifier_source = r#"
export interface Notifier {
    notifyEvent(value: string): void;
}
"#;
    let repository_source = r#"
export interface Repository {
    save(value: string): void;
}
"#;
    let archive_source = r#"
export interface Archive {
    save(value: string): void;
}
"#;
    let other_repository_source = r#"
export interface Repository {
    save(value: string): void;
}
"#;
    let workflow_source = r#"
import type { Notifier as Mailer } from "./notifier";
import { type Repository } from "./repository";
import type * as archive from "./archive";

class Workflow {
    private mailer: Mailer;
    #privateMailer: Mailer;
    private repository: Repository;
    private archive: archive.Archive;

    run(value: string): void {
        this.mailer.notifyEvent(value);
        this.#privateMailer.notifyEvent(value);
        this.repository.save(value);
        this.archive.save(value);
    }
}
"#;
    let missing_import_source = r#"
import type { Repository } from "./repository";

class Workflow {
    private repository: Repository;

    run(value: string): void {
        this.repository.save(value);
    }
}
"#;
    let duplicate_import_source = r#"
import type { Repository } from "./repository";
import type { Repository } from "./other/repository";

class Workflow {
    private repository: Repository;

    run(value: string): void {
        this.repository.save(value);
    }
}
"#;
    let local_shadow_source = r#"
import type { Repository } from "./repository";

interface Repository {
    save(value: string): void;
}

class Workflow {
    private repository: Repository;

    run(value: string): void {
        this.repository.save(value);
    }
}
"#;
    let missing_namespace_source = r#"
class Workflow {
    private archive: archive.Archive;

    run(value: string): void {
        this.archive.save(value);
    }
}
"#;
    let local_namespace_shadow_source = r#"
import type * as archive from "./archive";

interface archive {
    save(value: string): void;
}

class Workflow {
    private archive: archive.Archive;

    run(value: string): void {
        this.archive.save(value);
    }
}
"#;
    let import_namespace_collision_source = r#"
import type { Archive as archive } from "./other/archive";
import type * as archive from "./archive";

class Workflow {
    private archive: archive.Archive;

    run(value: string): void {
        this.archive.save(value);
    }
}
"#;

    let (nodes, edges) = index_files(&[
        ("notifier.ts", notifier_source),
        ("repository.ts", repository_source),
        ("archive.ts", archive_source),
        ("other/repository.ts", other_repository_source),
        ("workflow.ts", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "typescript imported class property receiver",
        &nodes,
        &edges,
        "Workflow.run",
        "Notifier",
        "notifyEvent",
        "notifier.ts",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "typescript imported private class property receiver adds second notifier call",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "Workflow.run",
            owner_name: "Notifier",
            method_name: "notifyEvent",
            file_suffix: "notifier.ts",
            expected_count: 2,
        },
    );
    assert_resolved_call_to_method_owner_in_file(
        "typescript imported class property receiver",
        &nodes,
        &edges,
        "Workflow.run",
        "Repository",
        "save",
        "repository.ts",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "typescript imported class property receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "Workflow.run",
            owner_name: "Repository",
            method_name: "save",
            file_suffix: "repository.ts",
            expected_count: 1,
        },
    );
    assert_resolved_call_to_method_owner_in_file(
        "typescript namespace imported class property receiver",
        &nodes,
        &edges,
        "Workflow.run",
        "Archive",
        "save",
        "archive.ts",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript imported class property receiver avoids same-name owner",
        &nodes,
        &edges,
        "Workflow.run",
        "Repository",
        "save",
        "other/repository.ts",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("other/repository.ts", other_repository_source),
        ("workflow.ts", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript missing imported class property receiver",
        &missing_nodes,
        &missing_edges,
        "Workflow.run",
        "Repository",
        "save",
        "other/repository.ts",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("repository.ts", repository_source),
        ("other/repository.ts", other_repository_source),
        ("workflow.ts", duplicate_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript duplicate imported class property receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "Workflow.run",
        "Repository",
        "save",
        "repository.ts",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript duplicate imported class property receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "Workflow.run",
        "Repository",
        "save",
        "other/repository.ts",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("repository.ts", repository_source),
        ("workflow.ts", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "typescript local shadowed class property receiver",
        &shadow_nodes,
        &shadow_edges,
        "Workflow.run",
        "Repository",
        "save",
        "workflow.ts",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript local shadowed class property receiver",
        &shadow_nodes,
        &shadow_edges,
        "Workflow.run",
        "Repository",
        "save",
        "repository.ts",
    );

    let (missing_namespace_nodes, missing_namespace_edges) = index_files(&[
        ("archive.ts", archive_source),
        ("workflow.ts", missing_namespace_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript missing namespace class property receiver",
        &missing_namespace_nodes,
        &missing_namespace_edges,
        "Workflow.run",
        "Archive",
        "save",
        "archive.ts",
    );

    let (local_namespace_shadow_nodes, local_namespace_shadow_edges) = index_files(&[
        ("archive.ts", archive_source),
        ("workflow.ts", local_namespace_shadow_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript local namespace shadowed class property receiver",
        &local_namespace_shadow_nodes,
        &local_namespace_shadow_edges,
        "Workflow.run",
        "Archive",
        "save",
        "archive.ts",
    );

    let (import_namespace_collision_nodes, import_namespace_collision_edges) = index_files(&[
        ("archive.ts", archive_source),
        ("workflow.ts", import_namespace_collision_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "typescript import namespace collision class property receiver",
        &import_namespace_collision_nodes,
        &import_namespace_collision_edges,
        "Workflow.run",
        "Archive",
        "save",
        "archive.ts",
    );

    Ok(())
}

#[test]
fn test_dart_prefixed_import_receiver_call_resolves_to_imported_owner_method() -> anyhow::Result<()>
{
    let notifier_source = r#"
class Notifier {
  void notifyEvent(String value) {}
}
"#;
    let other_notifier_source = r#"
class Notifier {
  void notifyEvent(String value) {}
}
"#;
    let workflow_source = r#"
import './mail/notifier.dart' as mail;

void run(mail.Notifier notifier) {
  notifier.notifyEvent('ready');
}
"#;
    let missing_import_source = r#"
import './missing/notifier.dart' as mail;

void run(mail.Notifier notifier) {
  notifier.notifyEvent('ready');
}
"#;
    let duplicate_alias_source = r#"
import './mail/notifier.dart' as shared;
import './other/notifier.dart' as shared;

void run(shared.Notifier notifier) {
  notifier.notifyEvent('ready');
}
"#;
    let unprefixed_duplicate_source = r#"
import './mail/notifier.dart';
import './other/notifier.dart';

void run(Notifier notifier) {
  notifier.notifyEvent('ready');
}
"#;
    let no_import_source = r#"
void run(Notifier notifier) {
  notifier.notifyEvent('ready');
}
"#;
    let commented_import_source = r#"
/*
import './mail/notifier.dart';
*/
const importExample = """
import './mail/notifier.dart';
""";

void run(Notifier notifier) {
  notifier.notifyEvent('ready');
}
"#;

    let (nodes, edges) = index_files(&[
        ("lib/mail/notifier.dart", notifier_source),
        ("lib/other/notifier.dart", other_notifier_source),
        ("lib/workflow.dart", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "dart prefixed imported receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/mail/notifier.dart",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "dart prefixed imported receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Notifier",
            method_name: "notifyEvent",
            file_suffix: "lib/mail/notifier.dart",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "dart prefixed imported receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/other/notifier.dart",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("lib/mail/notifier.dart", notifier_source),
        ("lib/workflow.dart", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart missing prefixed imported receiver",
        &missing_nodes,
        &missing_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/mail/notifier.dart",
    );

    let (no_import_nodes, no_import_edges) = index_files(&[
        ("lib/mail/notifier.dart", notifier_source),
        ("lib/workflow.dart", no_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart unimported receiver type stays unresolved",
        &no_import_nodes,
        &no_import_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/mail/notifier.dart",
    );

    let (commented_import_nodes, commented_import_edges) = index_files(&[
        ("lib/mail/notifier.dart", notifier_source),
        ("lib/workflow.dart", commented_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart comment and string text cannot prove an import",
        &commented_import_nodes,
        &commented_import_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/mail/notifier.dart",
    );

    let (duplicate_alias_nodes, duplicate_alias_edges) = index_files(&[
        ("lib/mail/notifier.dart", notifier_source),
        ("lib/other/notifier.dart", other_notifier_source),
        ("lib/workflow.dart", duplicate_alias_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart duplicate prefixed import alias",
        &duplicate_alias_nodes,
        &duplicate_alias_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/mail/notifier.dart",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "dart duplicate prefixed import alias",
        &duplicate_alias_nodes,
        &duplicate_alias_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/other/notifier.dart",
    );

    let (unprefixed_nodes, unprefixed_edges) = index_files(&[
        ("lib/mail/notifier.dart", notifier_source),
        ("lib/other/notifier.dart", other_notifier_source),
        ("lib/workflow.dart", unprefixed_duplicate_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart unprefixed ambiguous imported receiver",
        &unprefixed_nodes,
        &unprefixed_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/mail/notifier.dart",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "dart unprefixed ambiguous imported receiver",
        &unprefixed_nodes,
        &unprefixed_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/other/notifier.dart",
    );

    Ok(())
}

#[test]
fn test_dart_same_file_constructor_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let workflow_source = r#"
class Workflow {
  const Workflow();

  void run(String value) {}
}

class OtherWorkflow {
  void run(String value) {}
}

Workflow makeWorkflow() {
  return Workflow();
}

void orchestrate() {
  final workflow = Workflow();
  workflow.run('ready');
}

void constOrchestrate() {
  final workflow = const Workflow();
  workflow.run('ready');
}

void factoryOnly() {
  final workflow = makeWorkflow();
  workflow.run('ready');
}

void directOnly() {
  makeWorkflow();
}

void erased(dynamic workflow) {
  workflow.run('ready');
}

void innerShadow(dynamic workflow, bool enabled) {
  if (enabled) {
    final workflow = Workflow();
    workflow.run('inner');
  }
  workflow.run('outer');
}

void orderAware() {
  workflow.run('early');
  final workflow = Workflow();
  workflow.run('late');
}
"#;
    let (nodes, edges) = index_files(&[("lib/workflow.dart", workflow_source)])?;
    assert_resolved_call_to_method_owner_in_file(
        "dart same-file constructor receiver",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "run",
        "lib/workflow.dart",
    );
    assert_resolved_call_to_method_owner_in_file(
        "dart const constructor receiver",
        &nodes,
        &edges,
        "constOrchestrate",
        "Workflow",
        "run",
        "lib/workflow.dart",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "dart same-file constructor receiver",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "orchestrate",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "lib/workflow.dart",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner(
        "dart same-file constructor receiver avoids other owner",
        &nodes,
        &edges,
        "orchestrate",
        "OtherWorkflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "dart factory receiver stays unresolved",
        &nodes,
        &edges,
        "factoryOnly",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "dart erased receiver stays unresolved",
        &nodes,
        &edges,
        "erased",
        "Workflow",
        "run",
    );
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let factory_call_resolved = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter_map(|edge| {
            let source = node_by_id.get(&edge.source)?;
            if !is_matching_name(&source.serialized_name, "directOnly") {
                return None;
            }
            let resolved_id = edge.resolved_target?;
            let resolved_node = node_by_id.get(&resolved_id)?;
            Some(resolved_node.serialized_name.as_str())
        })
        .any(|resolved_name| is_matching_name(resolved_name, "makeWorkflow"));
    assert!(
        factory_call_resolved,
        "Case `dart direct call remains resolved`: expected direct CALL from `directOnly` to resolve to `makeWorkflow`. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "dart inner constructor binding stays block-scoped",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "innerShadow",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "lib/workflow.dart",
            expected_count: 1,
        },
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "dart constructor binding only applies after declaration",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "orderAware",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "lib/workflow.dart",
            expected_count: 1,
        },
    );

    let imported_owner_source = r#"
class Workflow {
  void run(String value) {}
}
"#;
    let imported_caller_source = r#"
import './other/workflow.dart';

void orchestrate() {
  final workflow = Workflow();
  workflow.run('ready');
}
"#;
    let shadowed_parameter_source = r#"
import './other/workflow.dart' as other;

class OtherWorkflow {}

void shadowed(other.Workflow workflow, bool enabled) {
  if (enabled) {
    final workflow = OtherWorkflow();
    workflow.run('inner');
  }
}
"#;
    let (imported_nodes, imported_edges) = index_files(&[
        ("lib/other/workflow.dart", imported_owner_source),
        ("lib/use_workflow.dart", imported_caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart constructor receiver does not use imported cross-file owner",
        &imported_nodes,
        &imported_edges,
        "orchestrate",
        "Workflow",
        "run",
        "lib/other/workflow.dart",
    );

    let (shadowed_nodes, shadowed_edges) = index_files(&[
        ("lib/other/workflow.dart", imported_owner_source),
        ("lib/shadowed.dart", shadowed_parameter_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart local constructor shadows imported typed parameter",
        &shadowed_nodes,
        &shadowed_edges,
        "shadowed",
        "Workflow",
        "run",
        "lib/other/workflow.dart",
    );

    Ok(())
}

#[test]
fn test_dart_same_file_property_receiver_call_resolves_to_declared_owner_method()
-> anyhow::Result<()> {
    let source = r#"
class Notifier {
  void notifyEvent(String value) {}
}

class Repository {
  void save(String value) {}
}

class OtherRepository {
  void save(String value) {}
}

class Workflow {
  final Notifier notifier;
  final Repository repository;
  final dynamic erased;

  Workflow(this.notifier, this.repository, this.erased);

  void run(String value) {
    notifier.notifyEvent(value);
    this.repository.save(value);
    this.decorate(value);
  }

  void decorate(String value) {}

  void parameterShadowsField(Notifier repository, String value) {
    repository.save(value);
  }

  void thisPropertyDespiteParameter(Notifier repository, String value) {
    this.repository.save(value);
  }

  void localFactoryShadowsField(String value) {
    final notifier = makeNotifier();
    notifier.notifyEvent(value);
  }

  void localDynamicShadowsField(String value) {
    dynamic notifier = makeNotifier();
    notifier.notifyEvent(value);
  }

  void localUnknownShadowsField(String value) {
    var notifier = readNotifier();
    notifier.notifyEvent(value);
  }

  void erasedProperty(String value) {
    erased.notifyEvent(value);
  }
}

Notifier makeNotifier() {
  return Notifier();
}

Object readNotifier() {
  return Object();
}
"#;

    let (nodes, edges) = index_single_file("lib/workflow.dart", source)?;
    assert_resolved_call_to_method_owner_in_file(
        "dart bare field receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/workflow.dart",
    );
    assert_resolved_call_to_method_owner_in_file(
        "dart this field receiver",
        &nodes,
        &edges,
        "run",
        "Repository",
        "save",
        "lib/workflow.dart",
    );
    assert_resolved_call_to_method_owner_in_file(
        "dart this self receiver",
        &nodes,
        &edges,
        "run",
        "Workflow",
        "decorate",
        "lib/workflow.dart",
    );
    assert_no_resolved_call_to_method_owner(
        "dart field receiver avoids other owner",
        &nodes,
        &edges,
        "run",
        "OtherRepository",
        "save",
    );
    assert_no_resolved_call_to_method_owner(
        "dart parameter receiver shadows field receiver",
        &nodes,
        &edges,
        "parameterShadowsField",
        "Repository",
        "save",
    );
    assert_resolved_call_to_method_owner_in_file(
        "dart explicit this property ignores same-named parameter",
        &nodes,
        &edges,
        "thisPropertyDespiteParameter",
        "Repository",
        "save",
        "lib/workflow.dart",
    );
    assert_no_resolved_call_to_method_owner(
        "dart local factory shadow prevents property fallback",
        &nodes,
        &edges,
        "localFactoryShadowsField",
        "Notifier",
        "notifyEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "dart local dynamic shadow prevents property fallback",
        &nodes,
        &edges,
        "localDynamicShadowsField",
        "Notifier",
        "notifyEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "dart local unknown shadow prevents property fallback",
        &nodes,
        &edges,
        "localUnknownShadowsField",
        "Notifier",
        "notifyEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "dart erased property receiver stays unresolved",
        &nodes,
        &edges,
        "erasedProperty",
        "Notifier",
        "notifyEvent",
    );

    let owner_source = r#"
class Workflow {
  void run(String value) {}
}
"#;
    let caller_source = r#"
class Entry {
  final Workflow workflow;

  Entry(this.workflow);

  void call() {
    workflow.run('ready');
  }
}
"#;
    let (cross_file_nodes, cross_file_edges) = index_files(&[
        ("lib/workflow.dart", owner_source),
        ("lib/entry.dart", caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart property receiver does not use unimported cross-file owner",
        &cross_file_nodes,
        &cross_file_edges,
        "call",
        "Workflow",
        "run",
        "lib/workflow.dart",
    );

    Ok(())
}

#[test]
fn test_php_use_alias_property_receiver_call_resolves_to_exact_imported_owner_method()
-> anyhow::Result<()> {
    let mail_notifier_source = r#"
<?php

namespace Acme\Mail;

interface Notifier
{
    public function notifyEvent(string $value): void;
}
"#;
    let other_notifier_source = r#"
<?php

namespace Acme\Other;

interface Notifier
{
    public function notifyEvent(string $value): void;
}
"#;
    let workflow_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Mail\Notifier as Mailer;

class Workflow
{
    private Mailer $notifier;

    public function __construct(private Mailer $promoted)
    {
    }

    public function run(string $value): void
    {
        $this->notifier->notifyEvent($value);
        $this->promoted->notifyEvent($value);
    }
}
"#;
    let plain_use_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Mail\Notifier;

class Workflow
{
    private Notifier $notifier;

    public function __construct(private Notifier $promoted)
    {
    }

    public function run(string $value): void
    {
        $this->notifier->notifyEvent($value);
        $this->promoted->notifyEvent($value);
    }
}
"#;
    let missing_import_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Missing\Notifier as Mailer;

class Workflow
{
    private Mailer $notifier;

    public function run(string $value): void
    {
        $this->notifier->notifyEvent($value);
    }
}
"#;
    let duplicate_alias_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Mail\Notifier as Mailer;
use Acme\Other\Notifier as Mailer;

class Workflow
{
    private Mailer $notifier;

    public function run(string $value): void
    {
        $this->notifier->notifyEvent($value);
    }
}
"#;
    let local_shadow_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Mail\Notifier as Mailer;

interface Mailer
{
    public function notifyEvent(string $value): void;
}

class Workflow
{
    private Mailer $notifier;

    public function run(string $value): void
    {
        $this->notifier->notifyEvent($value);
    }
}
"#;
    let alias_constructor_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Remote\Workflow as RemoteWorkflow;

function construct_alias(): void
{
    $workflow = new RemoteWorkflow();
    $workflow->run('ready');
}

function direct_alias(): void
{
    (new RemoteWorkflow())->run('ready');
}
"#;
    let plain_constructor_source = r#"
<?php

namespace Acme\Workflow;

use Acme\Remote\Workflow;

function construct_plain(): void
{
    $workflow = new Workflow();
    $workflow->run('ready');
}

function direct_plain(): void
{
    (new Workflow())->run('ready');
}
"#;
    let remote_workflow_source = r#"
<?php

namespace Acme\Remote;

class Workflow
{
    public function run(string $value): void {}
}
"#;

    let (nodes, edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Other/Notifier.php", other_notifier_source),
        ("src/Acme/Workflow/workflow.php", workflow_source),
    ])?;
    assert_resolved_call_count_to_method_owner_in_file(
        "php use alias imported property receiver exact namespace",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Notifier",
            method_name: "notifyEvent",
            file_suffix: "src/Acme/Mail/Notifier.php",
            expected_count: 2,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php use alias imported property receiver avoids other namespace",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Other/Notifier.php",
    );

    let (plain_nodes, plain_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Other/Notifier.php", other_notifier_source),
        ("src/Acme/Workflow/workflow.php", plain_use_source),
    ])?;
    assert_resolved_call_count_to_method_owner_in_file(
        "php plain use imported property receiver exact namespace",
        &plain_nodes,
        &plain_edges,
        ResolvedCallCountInFile {
            caller_name: "run",
            owner_name: "Notifier",
            method_name: "notifyEvent",
            file_suffix: "src/Acme/Mail/Notifier.php",
            expected_count: 2,
        },
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php plain use imported property receiver avoids other namespace",
        &plain_nodes,
        &plain_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Other/Notifier.php",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Workflow/workflow.php", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "php missing use alias imported property receiver",
        &missing_nodes,
        &missing_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Other/Notifier.php", other_notifier_source),
        ("src/Acme/Workflow/workflow.php", duplicate_alias_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "php duplicate use alias imported property receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php duplicate use alias imported property receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Other/Notifier.php",
    );

    let (shadow_nodes, shadow_edges) = index_files(&[
        ("src/Acme/Mail/Notifier.php", mail_notifier_source),
        ("src/Acme/Workflow/workflow.php", local_shadow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "php local receiver shadows use alias property type",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Mailer",
        "notifyEvent",
        "src/Acme/Workflow/workflow.php",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "php local receiver shadows use alias property type",
        &shadow_nodes,
        &shadow_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "src/Acme/Mail/Notifier.php",
    );

    let (alias_constructor_nodes, alias_constructor_edges) = index_files(&[
        ("src/Acme/Remote/Workflow.php", remote_workflow_source),
        ("src/Acme/Workflow/workflow.php", alias_constructor_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "php use alias local constructor receiver resolves imported owner",
        &alias_constructor_nodes,
        &alias_constructor_edges,
        "construct_alias",
        "Workflow",
        "run",
        "src/Acme/Remote/Workflow.php",
    );
    assert_resolved_call_to_method_owner_in_file(
        "php use alias direct constructor receiver resolves imported owner",
        &alias_constructor_nodes,
        &alias_constructor_edges,
        "direct_alias",
        "Workflow",
        "run",
        "src/Acme/Remote/Workflow.php",
    );

    let (plain_constructor_nodes, plain_constructor_edges) = index_files(&[
        ("src/Acme/Remote/Workflow.php", remote_workflow_source),
        ("src/Acme/Workflow/workflow.php", plain_constructor_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "php plain use local constructor receiver resolves imported owner",
        &plain_constructor_nodes,
        &plain_constructor_edges,
        "construct_plain",
        "Workflow",
        "run",
        "src/Acme/Remote/Workflow.php",
    );
    assert_resolved_call_to_method_owner_in_file(
        "php plain use direct constructor receiver resolves imported owner",
        &plain_constructor_nodes,
        &plain_constructor_edges,
        "direct_plain",
        "Workflow",
        "run",
        "src/Acme/Remote/Workflow.php",
    );

    Ok(())
}

#[test]
fn test_dart_imported_property_receiver_call_resolves_to_prefixed_import_owner_method()
-> anyhow::Result<()> {
    let notifier_source = r#"
class Notifier {
  void notifyEvent(String value) {}
}
"#;
    let duplicate_notifier_source = r#"
class Notifier {
  void notifyEvent(String value) {}
}
"#;
    let other_notifier_source = r#"
class Notifier {
  void notifyEvent(String value) {}
}
"#;
    let workflow_source = r#"
import './mail/notifier.dart' as mail;

class Workflow {
  final mail.Notifier notifier;

  Workflow(this.notifier);

  void run() {
    notifier.notifyEvent('ready');
  }
}
"#;
    let missing_import_source = r#"
import './missing/notifier.dart' as mail;

class Workflow {
  final mail.Notifier notifier;

  Workflow(this.notifier);

  void run() {
    notifier.notifyEvent('ready');
  }
}
"#;
    let duplicate_alias_source = r#"
import './mail/notifier.dart' as shared;
import './other/notifier.dart' as shared;

class Workflow {
  final shared.Notifier notifier;

  Workflow(this.notifier);

  void run() {
    notifier.notifyEvent('ready');
  }
}
"#;

    let (nodes, edges) = index_files(&[
        ("lib/mail/notifier.dart", notifier_source),
        ("lib/other/notifier.dart", other_notifier_source),
        ("lib/workflow.dart", workflow_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "dart imported property receiver exact prefixed import",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/mail/notifier.dart",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "dart imported property receiver avoids other import",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/other/notifier.dart",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("lib/mail/notifier.dart", notifier_source),
        ("lib/workflow.dart", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart missing prefixed property receiver import",
        &missing_nodes,
        &missing_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/mail/notifier.dart",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("lib/mail/notifier.dart", notifier_source),
        ("lib/other/notifier.dart", other_notifier_source),
        (
            "lib/other/duplicate_notifier.dart",
            duplicate_notifier_source,
        ),
        ("lib/workflow.dart", duplicate_alias_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart duplicate prefixed property receiver alias",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/mail/notifier.dart",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "dart duplicate prefixed property receiver alias",
        &duplicate_nodes,
        &duplicate_edges,
        "run",
        "Notifier",
        "notifyEvent",
        "lib/other/notifier.dart",
    );

    Ok(())
}

#[test]
fn test_dart_imported_constructor_receiver_call_resolves_to_prefixed_import_owner_method()
-> anyhow::Result<()> {
    let workflow_source = r#"
class Workflow {
  const Workflow();

  void run(String value) {}
}
"#;
    let other_workflow_source = r#"
class Workflow {
  const Workflow();

  void run(String value) {}
}
"#;
    let caller_source = r#"
import './mail/workflow.dart' as mail;

void orchestrate() {
  final workflow = mail.Workflow();
  workflow.run('ready');
}

void constOrchestrate() {
  final workflow = const mail.Workflow();
  workflow.run('ready');
}

void newOrchestrate() {
  final workflow = new mail.Workflow();
  workflow.run('ready');
}
"#;
    let missing_import_source = r#"
import './missing/workflow.dart' as mail;

void orchestrate() {
  final workflow = mail.Workflow();
  workflow.run('ready');
}
"#;
    let duplicate_alias_source = r#"
import './mail/workflow.dart' as shared;
import './other/workflow.dart' as shared;

void orchestrate() {
  final workflow = shared.Workflow();
  workflow.run('ready');
}
"#;

    let (nodes, edges) = index_files(&[
        ("lib/mail/workflow.dart", workflow_source),
        ("lib/other/workflow.dart", other_workflow_source),
        ("lib/use_workflow.dart", caller_source),
    ])?;
    assert_resolved_call_to_method_owner_in_file(
        "dart prefixed imported constructor receiver",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "run",
        "lib/mail/workflow.dart",
    );
    assert_resolved_call_to_method_owner_in_file(
        "dart const prefixed imported constructor receiver",
        &nodes,
        &edges,
        "constOrchestrate",
        "Workflow",
        "run",
        "lib/mail/workflow.dart",
    );
    assert_resolved_call_to_method_owner_in_file(
        "dart new prefixed imported constructor receiver",
        &nodes,
        &edges,
        "newOrchestrate",
        "Workflow",
        "run",
        "lib/mail/workflow.dart",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "dart prefixed imported constructor avoids other owner",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "run",
        "lib/other/workflow.dart",
    );

    let (missing_nodes, missing_edges) = index_files(&[
        ("lib/mail/workflow.dart", workflow_source),
        ("lib/use_workflow.dart", missing_import_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart missing prefixed imported constructor receiver",
        &missing_nodes,
        &missing_edges,
        "orchestrate",
        "Workflow",
        "run",
        "lib/mail/workflow.dart",
    );

    let (duplicate_nodes, duplicate_edges) = index_files(&[
        ("lib/mail/workflow.dart", workflow_source),
        ("lib/other/workflow.dart", other_workflow_source),
        ("lib/use_workflow.dart", duplicate_alias_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "dart duplicate prefixed imported constructor receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "orchestrate",
        "Workflow",
        "run",
        "lib/mail/workflow.dart",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "dart duplicate prefixed imported constructor receiver",
        &duplicate_nodes,
        &duplicate_edges,
        "orchestrate",
        "Workflow",
        "run",
        "lib/other/workflow.dart",
    );

    Ok(())
}

#[test]
fn test_rust_parameter_receiver_call_resolves_to_declared_parameter_type() -> anyhow::Result<()> {
    let source = r#"
struct OtherStorage;
impl OtherStorage {
    fn projections(&mut self) {}
}

struct Storage;
impl Storage {
    fn projections(&mut self) {}
}

struct WorkspaceIndexer;
impl WorkspaceIndexer {
    fn flush_projection_batch(storage: &mut Storage) {
        storage.projections();
    }
}
"#;

    let (nodes, edges) = index_single_file("main.rs", source)?;
    assert_resolved_call_to_method_owner(
        "rust parameter receiver",
        &nodes,
        &edges,
        "WorkspaceIndexer::flush_projection_batch",
        "Storage",
        "projections",
    );

    Ok(())
}

#[test]
fn test_rust_constructor_inferred_receiver_call_resolves_to_owner_method() -> anyhow::Result<()> {
    let source = r#"
struct Error;

struct RuntimeContext;
impl RuntimeContext {
    fn new() -> Result<Self, Error> {
        Ok(Self)
    }

    fn new_inspect_only() -> Result<Self, Error> {
        Ok(Self)
    }

    fn ensure_open(&self) {
        self.ensure_open_from_summary();
    }

    fn ensure_open_from_summary(&self) {}
}

fn run(dry_run: bool) -> Result<(), Error> {
    let runtime = if dry_run {
        RuntimeContext::new_inspect_only()?
    } else {
        RuntimeContext::new()?
    };
    runtime.ensure_open();
    Ok(())
}
"#;

    let (nodes, edges) = index_single_file("main.rs", source)?;
    assert_resolved_call_to_method_owner(
        "rust constructor-inferred receiver",
        &nodes,
        &edges,
        "run",
        "RuntimeContext",
        "ensure_open",
    );
    assert_resolved_call_to_method_owner(
        "rust constructor-inferred receiver",
        &nodes,
        &edges,
        "RuntimeContext::ensure_open",
        "RuntimeContext",
        "ensure_open_from_summary",
    );

    Ok(())
}

#[test]
fn test_rust_chained_receiver_call_uses_method_return_type() -> anyhow::Result<()> {
    let source = r#"
struct Storage;
struct ProjectionStore;
struct SnapshotStore;

impl Storage {
    fn projections(&mut self) -> ProjectionStore {
        ProjectionStore
    }

    fn snapshots(&self) -> SnapshotStore {
        SnapshotStore
    }
}

impl ProjectionStore {
    fn flush_projection_batch(&mut self) {}
}

impl SnapshotStore {
    fn refresh_all_with_stats(&self) {}
}

struct WorkspaceIndexer;
impl WorkspaceIndexer {
    fn flush_projection_batch(storage: &mut Storage) {
        storage.projections().flush_projection_batch();
    }
}

fn refresh(storage: &Storage) {
    storage.snapshots().refresh_all_with_stats();
}
"#;

    let (nodes, edges) = index_single_file("main.rs", source)?;
    assert_resolved_call_to_method_owner(
        "rust chained receiver return",
        &nodes,
        &edges,
        "WorkspaceIndexer::flush_projection_batch",
        "ProjectionStore",
        "flush_projection_batch",
    );
    assert_resolved_call_to_method_owner(
        "rust chained receiver return",
        &nodes,
        &edges,
        "refresh",
        "SnapshotStore",
        "refresh_all_with_stats",
    );

    Ok(())
}

#[test]
fn test_rust_owner_alias_receiver_call_resolves_to_manifest_owner() -> anyhow::Result<()> {
    let source = r#"
struct WorkspaceManifest;
impl WorkspaceManifest {
    fn build_execution_plan(&self) {}
}

type Workspace = WorkspaceManifest;

fn run(workspace: &Workspace) {
    workspace.build_execution_plan();
}
"#;

    let (nodes, edges) = index_single_file("main.rs", source)?;
    assert_resolved_call_to_method_owner(
        "rust owner alias receiver call",
        &nodes,
        &edges,
        "run",
        "WorkspaceManifest",
        "build_execution_plan",
    );

    Ok(())
}

#[test]
fn test_rust_owner_prefix_without_known_alias_stays_unresolved() -> anyhow::Result<()> {
    let source = r#"
struct WorkspaceRunner;
impl WorkspaceRunner {
    fn build_execution_plan(&self) {}
}

fn run(workspace: &Workspace) {
    workspace.build_execution_plan();
}
"#;

    let (nodes, edges) = index_single_file("main.rs", source)?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let mut found = 0usize;
    for edge in edges.iter().filter(|edge| edge.kind == EdgeKind::CALL) {
        let Some(source) = node_by_id.get(&edge.source) else {
            continue;
        };
        let Some(target) = node_by_id.get(&edge.target) else {
            continue;
        };
        if is_matching_name(&source.serialized_name, "run")
            && target.serialized_name.contains("build_execution_plan")
        {
            found += 1;
            assert!(
                edge.resolved_target.is_none(),
                "unknown owner prefix should not resolve to unrelated owner. Calls: {:?}",
                describe_call_edges(&edges, &nodes)
            );
        }
    }
    assert!(
        found > 0,
        "expected unresolved build_execution_plan call. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );

    Ok(())
}

#[test]
fn test_rust_store_alias_receiver_call_resolves_to_storage_owner() -> anyhow::Result<()> {
    let source = r#"
struct Storage;
impl Storage {
    fn flush_projection_batch(&mut self) {}
}

type Store = Storage;

struct ProjectionStore<'a> {
    storage: &'a mut Store,
}
impl<'a> ProjectionStore<'a> {
    fn flush_projection_batch(&mut self) {
        self.storage.flush_projection_batch();
    }
}
"#;

    let (nodes, edges) = index_single_file("main.rs", source)?;
    assert_resolved_call_to_method_owner(
        "rust store alias receiver call",
        &nodes,
        &edges,
        "ProjectionStore::flush_projection_batch",
        "Storage",
        "flush_projection_batch",
    );

    Ok(())
}

#[test]
fn test_rust_cross_file_receiver_call_is_certain_owner_qualified_path() -> anyhow::Result<()> {
    let service_source = r#"
pub struct IndexService;
impl IndexService {
    pub fn run_indexing_blocking_without_runtime_refresh(&self) {}
}
"#;
    let runtime_source = r#"
use crate::service::IndexService;

struct RuntimeContext {
    index: IndexService,
}
impl RuntimeContext {
    fn ensure_open_from_summary(&self) {
        self.index.run_indexing_blocking_without_runtime_refresh();
    }
}
"#;

    let (nodes, edges) = index_files(&[
        ("service.rs", service_source),
        ("runtime.rs", runtime_source),
    ])?;
    assert_resolved_call_to_method_owner(
        "rust cross-file receiver",
        &nodes,
        &edges,
        "RuntimeContext::ensure_open_from_summary",
        "IndexService",
        "run_indexing_blocking_without_runtime_refresh",
    );

    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let caller_ids = nodes
        .iter()
        .filter(|node| {
            is_matching_owned_method(
                &node.serialized_name,
                "RuntimeContext",
                "ensure_open_from_summary",
            )
        })
        .map(|node| node.id)
        .collect::<Vec<_>>();
    let edge = edges
        .iter()
        .find(|edge| {
            edge.kind == EdgeKind::CALL
                && caller_ids.contains(&edge.source)
                && edge
                    .resolved_target
                    .and_then(|target_id| node_by_id.get(&target_id))
                    .is_some_and(|node| {
                        is_matching_owned_method(
                            &node.serialized_name,
                            "IndexService",
                            "run_indexing_blocking_without_runtime_refresh",
                        )
                    })
        })
        .ok_or_else(|| anyhow::anyhow!("expected cross-file receiver call edge"))?;
    assert_eq!(
        edge.certainty,
        Some(ResolutionCertainty::Certain),
        "cross-file owner-qualified receiver calls should survive hide-speculative trail filters"
    );

    Ok(())
}

#[test]
fn test_rust_untyped_common_receiver_call_remains_unresolved() -> anyhow::Result<()> {
    let source = r#"
struct NavigationHistory;
impl NavigationHistory {
    fn push(&mut self, _x: i32) {}
}

fn run() {
    let mut items = Vec::new();
    items.push(1);
}
"#;

    let (nodes, edges) = index_single_file("main.rs", source)?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let navigation_push_id = nodes
        .iter()
        .find(|node| is_matching_owned_method(&node.serialized_name, "NavigationHistory", "push"))
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected NavigationHistory::push symbol"))?;
    let run_ids = nodes
        .iter()
        .filter(|node| is_matching_name(&node.serialized_name, "run"))
        .map(|node| node.id)
        .collect::<Vec<_>>();

    let mut push_edges = 0usize;
    for edge in edges.iter().filter(|edge| edge.kind == EdgeKind::CALL) {
        if !run_ids.contains(&edge.source) {
            continue;
        }
        let Some(target_node) = node_by_id.get(&edge.target) else {
            continue;
        };
        if !is_matching_name(&target_node.serialized_name, "push") {
            continue;
        }
        push_edges += 1;
        assert_ne!(
            edge.resolved_target,
            Some(navigation_push_id),
            "untyped Vec::push receiver must not resolve to unrelated NavigationHistory::push"
        );
    }
    assert!(push_edges >= 1, "expected a push call edge from run");

    Ok(())
}

#[test]
fn test_cpp_parameter_receiver_call_resolves_to_declared_owner_method() -> anyhow::Result<()> {
    let source = r#"
class Notifier {
public:
    void save() {}
    void notifyEvent() {}
};

class Repository {
public:
    void save() {}
};

void run(const Notifier& notifier, Repository<std::string>& repository, Notifier* pointer) {
    notifier.save();
    repository.save();
    pointer->notifyEvent();
}
"#;

    let (nodes, edges) = index_single_file("main.cpp", source)?;
    assert_resolved_call_to_method_owner(
        "cpp parameter receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "save",
    );
    assert_resolved_call_to_method_owner(
        "cpp parameter receiver",
        &nodes,
        &edges,
        "run",
        "Repository",
        "save",
    );
    assert_resolved_call_to_method_owner(
        "cpp pointer parameter receiver",
        &nodes,
        &edges,
        "run",
        "Notifier",
        "notifyEvent",
    );

    let other_notifier_source = r#"
class Notifier {
public:
    void save() {}
};
"#;
    let caller_source = r#"
void run(Notifier& notifier) {
    notifier.save();
}
"#;
    let (ambiguous_nodes, ambiguous_edges) = index_files(&[
        ("one/Notifier.cpp", other_notifier_source),
        ("two/Notifier.cpp", other_notifier_source),
        ("workflow.cpp", caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "cpp cross-file ambiguous receiver",
        &ambiguous_nodes,
        &ambiguous_edges,
        "run",
        "Notifier",
        "save",
        "one/Notifier.cpp",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "cpp cross-file ambiguous receiver",
        &ambiguous_nodes,
        &ambiguous_edges,
        "run",
        "Notifier",
        "save",
        "two/Notifier.cpp",
    );

    let constructor_temporary_source = r#"
class Notifier {
public:
    void save() {}
};

class Repository {
public:
    void save() {}
};

void run() {
    Notifier().save();
}
"#;
    let (temporary_nodes, temporary_edges) =
        index_single_file("temporary.cpp", constructor_temporary_source)?;
    assert_no_resolved_call_to_method_owner(
        "cpp constructor temporary receiver",
        &temporary_nodes,
        &temporary_edges,
        "run",
        "Notifier",
        "save",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp constructor temporary receiver",
        &temporary_nodes,
        &temporary_edges,
        "run",
        "Repository",
        "save",
    );

    Ok(())
}

#[test]
fn test_cpp_same_file_local_receiver_call_resolves_to_declared_owner_method() -> anyhow::Result<()>
{
    let source = r#"
struct Event {};

class Workflow {
public:
    void run(Event) {}
};

class OtherWorkflow {
public:
    void run(Event) {}
};

namespace remote {
class QualifiedWorkflow {
public:
    void run(Event) {}
};
}

class NoRun {};

Workflow makeWorkflow() {
    return Workflow{};
}

void orchestrate() {
    Workflow workflow;
    workflow.run(Event{});
}

void bracedLocal() {
    Workflow workflow{};
    workflow.run(Event{});
}

void pointerLocal() {
    Workflow* workflow = nullptr;
    workflow->run(Event{});
}

void typedFactory() {
    Workflow workflow = makeWorkflow();
    workflow.run(Event{});
}

void autoBracedConstructor() {
    auto workflow = Workflow{};
    workflow.run(Event{});
}

void autoParenConstructor() {
    const auto workflow = Workflow();
    workflow.run(Event{});
}

void autoNewPointer() {
    auto workflow = new Workflow();
    workflow->run(Event{});
}

void autoFactory() {
    auto workflow = makeWorkflow();
    workflow.run(Event{});
}

void autoFactoryThenAssigned() {
    auto workflow = makeWorkflow();
    workflow = Workflow{};
    workflow.run(Event{});
}

void autoBareIdentifierInitializer() {
    auto workflow = Workflow;
    workflow.run(Event{});
}

void autoComposedBracedInitializer() {
    Workflow other;
    auto workflow = Workflow{} + other;
    workflow.run(Event{});
}

void autoChainedParenInitializer() {
    auto workflow = Workflow().unwrap();
    workflow.run(Event{});
}

void autoQualifiedConstructor() {
    auto workflow = remote::QualifiedWorkflow{};
    workflow.run(Event{});
}

void autoFactoryPointer() {
    auto workflow = std::make_unique<Workflow>();
    workflow->run(Event{});
}

void directOnly() {
    makeWorkflow();
}

void orderAware() {
    workflow.run(Event{});
    Workflow workflow;
    workflow.run(Event{});
}

void innerShadow(Workflow& workflow) {
    if (true) {
        auto workflow = makeWorkflow();
        workflow.run(Event{});
    }
}

void innerDirectAutoShadow(Workflow& workflow) {
    if (true) {
        auto workflow = Workflow{};
        workflow.run(Event{});
    }
}

void multiDeclaratorShadow(Workflow& workflow) {
    if (true) {
        auto workflow = makeWorkflow(), other = makeWorkflow();
        workflow.run(Event{});
    }
}

void directAutoMultiDeclaratorShadow(Workflow& workflow) {
    if (true) {
        auto workflow = Workflow{}, other = Workflow{};
        workflow.run(Event{});
    }
}

void outerParamStillWorks(Workflow& workflow) {
    if (true) {
        NoRun other;
        other.run(Event{});
    }
    workflow.run(Event{});
}
"#;

    let (nodes, edges) = index_single_file("workflow.cpp", source)?;
    assert_resolved_call_to_method_owner_in_file(
        "cpp same-file local receiver",
        &nodes,
        &edges,
        "orchestrate",
        "Workflow",
        "run",
        "workflow.cpp",
    );
    assert_resolved_call_to_method_owner_in_file(
        "cpp braced local receiver",
        &nodes,
        &edges,
        "bracedLocal",
        "Workflow",
        "run",
        "workflow.cpp",
    );
    assert_resolved_call_to_method_owner_in_file(
        "cpp pointer local receiver",
        &nodes,
        &edges,
        "pointerLocal",
        "Workflow",
        "run",
        "workflow.cpp",
    );
    assert_resolved_call_to_method_owner_in_file(
        "cpp typed factory local receiver",
        &nodes,
        &edges,
        "typedFactory",
        "Workflow",
        "run",
        "workflow.cpp",
    );
    assert_resolved_call_to_method_owner_in_file(
        "cpp auto braced constructor receiver",
        &nodes,
        &edges,
        "autoBracedConstructor",
        "Workflow",
        "run",
        "workflow.cpp",
    );
    assert_resolved_call_to_method_owner_in_file(
        "cpp auto paren constructor receiver",
        &nodes,
        &edges,
        "autoParenConstructor",
        "Workflow",
        "run",
        "workflow.cpp",
    );
    assert_resolved_call_to_method_owner_in_file(
        "cpp auto new pointer receiver",
        &nodes,
        &edges,
        "autoNewPointer",
        "Workflow",
        "run",
        "workflow.cpp",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp auto factory receiver stays unresolved",
        &nodes,
        &edges,
        "autoFactory",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp auto factory receiver assigned later stays unresolved",
        &nodes,
        &edges,
        "autoFactoryThenAssigned",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp auto bare identifier initializer stays unresolved",
        &nodes,
        &edges,
        "autoBareIdentifierInitializer",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp auto composed braced initializer stays unresolved",
        &nodes,
        &edges,
        "autoComposedBracedInitializer",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp auto chained paren initializer stays unresolved",
        &nodes,
        &edges,
        "autoChainedParenInitializer",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp auto qualified constructor receiver stays unresolved",
        &nodes,
        &edges,
        "autoQualifiedConstructor",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp auto smart-pointer factory receiver stays unresolved",
        &nodes,
        &edges,
        "autoFactoryPointer",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp local receiver avoids other owner",
        &nodes,
        &edges,
        "orchestrate",
        "OtherWorkflow",
        "run",
    );
    assert_resolved_call_to_name(
        "cpp direct call remains resolved",
        &nodes,
        &edges,
        "directOnly",
        "makeWorkflow",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "cpp local binding only applies after declaration",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "orderAware",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "workflow.cpp",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner(
        "cpp local auto declaration shadows typed parameter",
        &nodes,
        &edges,
        "innerShadow",
        "Workflow",
        "run",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "cpp direct auto local shadows typed parameter",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "innerDirectAutoShadow",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "workflow.cpp",
            expected_count: 1,
        },
    );
    assert_no_resolved_call_to_method_owner(
        "cpp multi-declarator local declaration shadows typed parameter",
        &nodes,
        &edges,
        "multiDeclaratorShadow",
        "Workflow",
        "run",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp direct auto multi-declarator shadows typed parameter",
        &nodes,
        &edges,
        "directAutoMultiDeclaratorShadow",
        "Workflow",
        "run",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "cpp outer typed parameter survives unrelated inner local",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "outerParamStillWorks",
            owner_name: "Workflow",
            method_name: "run",
            file_suffix: "workflow.cpp",
            expected_count: 1,
        },
    );

    let owner_source = r#"
struct Event {};

class Workflow {
public:
    void run(Event) {}
};
"#;
    let caller_source = r#"
struct Event {};

void orchestrate() {
    Workflow workflow;
    workflow.run(Event{});
}

void autoOrchestrate() {
    auto workflow = Workflow{};
    workflow.run(Event{});
}
"#;
    let (cross_nodes, cross_edges) = index_files(&[
        ("lib/workflow.cpp", owner_source),
        ("app/caller.cpp", caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "cpp local receiver does not use cross-file owner",
        &cross_nodes,
        &cross_edges,
        "orchestrate",
        "Workflow",
        "run",
        "lib/workflow.cpp",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "cpp auto local receiver does not use cross-file owner",
        &cross_nodes,
        &cross_edges,
        "autoOrchestrate",
        "Workflow",
        "run",
        "lib/workflow.cpp",
    );

    Ok(())
}

#[test]
fn test_cpp_same_file_field_receiver_call_resolves_to_declared_owner_method() -> anyhow::Result<()>
{
    let source = r#"
struct Event {};

class Notifier {
public:
    void notify(Event) {}
};

class Repository {
public:
    void save(Event) {}
};

class OtherRepository {
public:
    void save(Event) {}
};

class Workflow {
    Notifier notifier;
    Repository repository;
    Repository* pointer;
    Repository multi, other;

public:
    void fieldRun(Event event) {
        notifier.notify(event);
        this->repository.save(event);
        pointer->save(event);
        this->pointer->save(event);
        this->decorate(event);
    }

    void decorate(Event) {}

    void parameterShadowsField(Notifier repository, Event event) {
        repository.save(event);
    }

    void explicitThisFieldDespiteParameter(Notifier repository, Event event) {
        this->repository.save(event);
    }

    void localShadowsField(Event event) {
        Notifier repository;
        repository.save(event);
    }

    void multiDeclaratorField(Event event) {
        multi.save(event);
    }
};

class OtherWorkflow {
public:
    void decorate(Event) {}
};
"#;

    let (nodes, edges) = index_single_file("workflow.cpp", source)?;
    assert_resolved_call_to_method_owner_in_file(
        "cpp bare field receiver",
        &nodes,
        &edges,
        "fieldRun",
        "Notifier",
        "notify",
        "workflow.cpp",
    );
    assert_resolved_call_count_to_method_owner_in_file(
        "cpp this field and pointer field receivers",
        &nodes,
        &edges,
        ResolvedCallCountInFile {
            caller_name: "fieldRun",
            owner_name: "Repository",
            method_name: "save",
            file_suffix: "workflow.cpp",
            expected_count: 3,
        },
    );
    assert_resolved_call_to_method_owner_in_file(
        "cpp this self receiver",
        &nodes,
        &edges,
        "fieldRun",
        "Workflow",
        "decorate",
        "workflow.cpp",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp field receiver avoids other owner",
        &nodes,
        &edges,
        "fieldRun",
        "OtherRepository",
        "save",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp self receiver avoids other owner",
        &nodes,
        &edges,
        "fieldRun",
        "OtherWorkflow",
        "decorate",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp parameter receiver shadows field receiver",
        &nodes,
        &edges,
        "parameterShadowsField",
        "Repository",
        "save",
    );
    assert_resolved_call_to_method_owner_in_file(
        "cpp explicit this field ignores same-named parameter",
        &nodes,
        &edges,
        "explicitThisFieldDespiteParameter",
        "Repository",
        "save",
        "workflow.cpp",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp local receiver shadows field receiver",
        &nodes,
        &edges,
        "localShadowsField",
        "Repository",
        "save",
    );
    assert_no_resolved_call_to_method_owner(
        "cpp multi-declarator field stays unresolved",
        &nodes,
        &edges,
        "multiDeclaratorField",
        "Repository",
        "save",
    );

    let owner_source = r#"
struct Event {};

class Repository {
public:
    void save(Event) {}
};
"#;
    let caller_source = r#"
struct Event {};

class Entry {
    Repository repository;

public:
    void call(Event event) {
        repository.save(event);
    }
};
"#;
    let (cross_file_nodes, cross_file_edges) = index_files(&[
        ("lib/repository.cpp", owner_source),
        ("app/entry.cpp", caller_source),
    ])?;
    assert_no_resolved_call_to_method_owner_in_file(
        "cpp field receiver does not use cross-file owner",
        &cross_file_nodes,
        &cross_file_edges,
        "call",
        "Repository",
        "save",
        "lib/repository.cpp",
    );

    Ok(())
}

#[test]
fn test_cpp_receiver_call_does_not_resolve_to_unrelated_parser_method() -> anyhow::Result<()> {
    let cpp_source = r#"
class CxxParser {
public:
    void buildIndex() {}
};
"#;
    let java_parser_source = r#"
class JavaParser {
public:
    void buildIndex() {}
};
"#;
    let indexer_source = r#"
class ParserClient {};
class IndexerCommandJava {};
class IndexerStateInfo {};

class JavaParser {
public:
    JavaParser(ParserClient*, IndexerStateInfo*) {}
    void buildIndex(const IndexerCommandJava&);
};

class IndexerJava {
    IndexerStateInfo* state;
public:
    void doIndex(const IndexerCommandJava& indexerCommand) {
        ParserClient* parserClient = nullptr;
        JavaParser(parserClient, state).buildIndex(indexerCommand);
    }
};
"#;

    let (nodes, edges) = index_files(&[
        ("CxxParser.cpp", cpp_source),
        ("JavaParser.cpp", java_parser_source),
        ("IndexerJava.cpp", indexer_source),
    ])?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();

    let do_index_ids = nodes
        .iter()
        .filter(|node| is_matching_owned_method(&node.serialized_name, "IndexerJava", "doIndex"))
        .map(|node| node.id)
        .collect::<Vec<_>>();
    assert!(
        !do_index_ids.is_empty(),
        "expected IndexerJava::doIndex symbol to be indexed. Nodes: {:?}",
        nodes
            .iter()
            .map(|node| node.serialized_name.clone())
            .collect::<Vec<_>>()
    );

    let cxx_build_index_id = nodes
        .iter()
        .find(|node| is_matching_owned_method(&node.serialized_name, "CxxParser", "buildIndex"))
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected CxxParser::buildIndex symbol to be indexed"))?;

    let mut build_index_calls = 0usize;
    for edge in edges.iter().filter(|edge| edge.kind == EdgeKind::CALL) {
        if !do_index_ids.contains(&edge.source) {
            continue;
        }
        let Some(target_node) = node_by_id.get(&edge.target) else {
            continue;
        };
        if !is_matching_name(&target_node.serialized_name, "buildIndex") {
            continue;
        }

        build_index_calls += 1;
        assert_ne!(
            edge.resolved_target,
            Some(cxx_build_index_id),
            "Receiver call IndexerJava::doIndex -> JavaParser(...).buildIndex must not resolve to CxxParser::buildIndex. Calls: {:?}",
            describe_call_edges(&edges, &nodes)
        );
    }

    assert!(
        build_index_calls >= 1,
        "expected buildIndex receiver call from IndexerJava::doIndex. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );

    Ok(())
}

#[test]
fn test_same_line_duplicate_calls_keep_distinct_callsite_identities() -> anyhow::Result<()> {
    let source = "fn helper() {}\nfn run() { helper(); helper(); }\n";
    let (nodes, edges) = index_single_file("main.rs", source)?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();

    let run_id = nodes
        .iter()
        .find(|node| is_matching_name(&node.serialized_name, "run"))
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected run node"))?;

    let helper_calls = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == run_id)
        .filter(|edge| {
            node_by_id
                .get(&edge.effective_target())
                .or_else(|| node_by_id.get(&edge.target))
                .is_some_and(|node| is_matching_name(&node.serialized_name, "helper"))
        })
        .collect::<Vec<_>>();

    assert_eq!(
        helper_calls.len(),
        2,
        "expected both helper() invocations on the same line to survive as separate CALL edges"
    );

    let unique_edge_ids = helper_calls
        .iter()
        .map(|edge| edge.id)
        .collect::<HashSet<_>>();
    let unique_callsites = helper_calls
        .iter()
        .map(|edge| edge.callsite_identity.clone().unwrap_or_default())
        .collect::<HashSet<_>>();

    assert_eq!(
        unique_edge_ids.len(),
        2,
        "expected distinct edge ids per callsite"
    );
    assert_eq!(
        unique_callsites.len(),
        2,
        "expected distinct callsite identities per callsite"
    );
    assert!(
        helper_calls.iter().all(|edge| edge
            .callsite_identity
            .as_deref()
            .is_some_and(|identity| !identity.is_empty())),
        "expected every helper() call edge to carry a non-empty callsite identity"
    );

    Ok(())
}

#[test]
fn test_python_attribute_call_placeholder_span_tracks_terminal_identifier() -> anyhow::Result<()> {
    let source = "class Runner:\n    def run(self):\n        self.client.session.send()\n";
    let (nodes, edges) = index_single_file("main.py", source)?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();

    let run_id = nodes
        .iter()
        .find(|node| is_matching_name(&node.serialized_name, "run"))
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected run node"))?;

    let send_target = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == run_id)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .find(|node| is_matching_name(&node.serialized_name, "send"))
        .ok_or_else(|| anyhow::anyhow!("expected send placeholder node"))?;

    assert_eq!(send_target.start_line, Some(3));
    assert_eq!(send_target.end_line, Some(3));
    assert_eq!(
        snippet_for_node(source, send_target),
        Some("send"),
        "expected Python attribute-call placeholder to cover only the terminal identifier"
    );

    Ok(())
}

#[test]
fn test_cpp_qualified_call_placeholder_span_tracks_terminal_identifier() -> anyhow::Result<()> {
    let source = "namespace ns { int make() { return 1; } }\nint run() { return ns::make(); }\n";
    let (nodes, edges) = index_single_file("main.cpp", source)?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();

    let run_id = nodes
        .iter()
        .find(|node| is_matching_name(&node.serialized_name, "run"))
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected run node"))?;

    let make_target = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == run_id)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .find(|node| is_matching_name(&node.serialized_name, "make"))
        .ok_or_else(|| anyhow::anyhow!("expected make placeholder node"))?;

    assert_eq!(
        snippet_for_node(source, make_target),
        Some("make"),
        "expected C++ qualified-call placeholder to cover only the terminal identifier"
    );

    Ok(())
}

#[test]
fn test_swift_navigation_call_placeholder_span_tracks_terminal_identifier() -> anyhow::Result<()> {
    let source = r#"
func helper() {}

func run(notifier: Any) {
    notifier.notifyEvent("ready")
    helper()
}
"#;
    let (nodes, edges) = index_single_file("main.swift", source)?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();

    let run_id = nodes
        .iter()
        .find(|node| is_matching_name(&node.serialized_name, "run"))
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected run node"))?;

    let notify_target = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == run_id)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .find(|node| is_matching_name(&node.serialized_name, "notifyEvent"))
        .ok_or_else(|| anyhow::anyhow!("expected notifyEvent placeholder node"))?;

    assert_eq!(
        snippet_for_node(source, notify_target),
        Some("notifyEvent"),
        "expected Swift navigation-call placeholder to cover only the terminal identifier"
    );

    let helper_target = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == run_id)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .find(|node| is_matching_name(&node.serialized_name, "helper"))
        .ok_or_else(|| anyhow::anyhow!("expected helper direct-call placeholder node"))?;

    assert_eq!(
        snippet_for_node(source, helper_target),
        Some("helper"),
        "expected Swift direct-call placeholder to remain captured"
    );

    Ok(())
}

#[test]
fn test_java_annotation_usage_span_tracks_terminal_identifier() -> anyhow::Result<()> {
    let source = "@Deprecated\nclass Example {}\n";
    let (nodes, edges) = index_single_file("Main.java", source)?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();

    let deprecated_target = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::ANNOTATION_USAGE)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .find(|node| is_matching_name(&node.serialized_name, "Deprecated"))
        .ok_or_else(|| anyhow::anyhow!("expected Deprecated annotation usage node"))?;

    assert_eq!(
        snippet_for_node(source, deprecated_target),
        Some("Deprecated"),
        "expected Java annotation usage placeholder to cover only the annotation token"
    );

    Ok(())
}

#[test]
fn test_rust_impl_expr_span_tracks_terminal_identifier() -> anyhow::Result<()> {
    let source = "impl crate::api::Worker<T> {\n    fn run(&self) {}\n}\n";
    let (nodes, _edges) = index_single_file("main.rs", source)?;
    let worker_node = nodes
        .iter()
        .find(|node| is_matching_name(&node.serialized_name, "Worker"))
        .ok_or_else(|| anyhow::anyhow!("expected Worker impl anchor node"))?;

    assert_eq!(
        snippet_for_node(source, worker_node),
        Some("Worker"),
        "expected Rust impl surface to normalize to the terminal identifier span"
    );

    Ok(())
}

#[test]
fn test_java_annotation_usage_placeholder_span_tracks_terminal_identifier() -> anyhow::Result<()> {
    let source = "@Marker\nclass Example {}\n";
    let (nodes, edges) = index_single_file("Main.java", source)?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();

    let marker_target = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::ANNOTATION_USAGE)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .find(|node| is_matching_name(&node.serialized_name, "Marker"))
        .ok_or_else(|| anyhow::anyhow!("expected Marker annotation placeholder node"))?;

    assert_eq!(marker_target.start_line, Some(1));
    assert_eq!(marker_target.end_line, Some(1));
    assert_eq!(
        snippet_for_node(source, marker_target),
        Some("Marker"),
        "expected Java annotation placeholder to cover only the annotation token"
    );

    Ok(())
}

#[test]
fn test_java_annotation_usage_placeholder_span_tracks_annotation_token() -> anyhow::Result<()> {
    let source = "@Logged\nclass Example {}\n";
    let (nodes, edges) = index_single_file("Main.java", source)?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();

    let logged_target = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::ANNOTATION_USAGE)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .find(|node| is_matching_name(&node.serialized_name, "Logged"))
        .ok_or_else(|| anyhow::anyhow!("expected Logged annotation node"))?;

    assert_eq!(logged_target.start_line, Some(1));
    assert_eq!(logged_target.end_line, Some(1));
    assert_eq!(
        snippet_for_node(source, logged_target),
        Some("Logged"),
        "expected Java annotation usage placeholder to cover only the annotation token"
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

#[test]
fn test_java_argument_carrying_method_annotation_keeps_parameter_receiver_types()
-> anyhow::Result<()> {
    // The parameter surface used to be scanned from the first `(` in the
    // method's own text, and a leading annotation with an argument list puts
    // that `(` inside the annotation (CR-009). `@GetMapping("/dispatch")`
    // yielded `"/dispatch"` as the parameter list, so the receiver map came out
    // empty and `listener.handleEvent()` never resolved.
    let source = r#"
class Listener {
    void handleEvent() {}
}

class AuditListener {
    void handleEvent() {}
}

class Bus {
    @GetMapping("/dispatch")
    @Transactional(readOnly = true)
    void dispatchTo(Listener listener) {
        listener.handleEvent();
    }
}
"#;

    let (nodes, edges) = index_single_file("Bus.java", source)?;
    assert_resolved_call_to_method_owner(
        "java annotated method",
        &nodes,
        &edges,
        "dispatchTo",
        "Listener",
        "handleEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "java annotated method",
        &nodes,
        &edges,
        "dispatchTo",
        "AuditListener",
        "handleEvent",
    );
    Ok(())
}

#[test]
fn test_java_parameter_annotation_does_not_become_the_receiver_owner_type() -> anyhow::Result<()> {
    // `f(@RequestParam("id") Listener listener)` recorded the owner type of
    // `listener` as `@RequestParam`, because the annotation is just another
    // whitespace-separated token and the type surface truncates at its `(`.
    let source = r#"
class Listener {
    void handleEvent() {}
}

class AuditListener {
    void handleEvent() {}
}

class Bus {
    void dispatchTo(@RequestParam(value = "id") final Listener listener) {
        listener.handleEvent();
    }
}
"#;

    let (nodes, edges) = index_single_file("Bus.java", source)?;
    assert_resolved_call_to_method_owner(
        "java annotated parameter",
        &nodes,
        &edges,
        "dispatchTo",
        "Listener",
        "handleEvent",
    );
    assert_no_resolved_call_to_method_owner(
        "java annotated parameter",
        &nodes,
        &edges,
        "dispatchTo",
        "AuditListener",
        "handleEvent",
    );
    Ok(())
}

#[test]
fn test_kotlin_argument_carrying_annotations_keep_parameter_and_property_bindings()
-> anyhow::Result<()> {
    // Two separate CR-009 failures on one path: `@Throws(IOException::class)`
    // on a function turned `IOException::class` into the parameter surface, and
    // `@Table(name = "users")` on a data class swallowed the whole primary
    // constructor, so the class lost every property binding.
    let source = r#"
package app

class Repository {
    fun save() {}
}

class Audit {
    fun save() {}
}

@Table(name = "users")
data class User(private val repository: Repository) {
    fun persist() {
        repository.save()
    }
}

class Bus {
    @Throws(IllegalStateException::class)
    fun dispatchTo(repository: Repository) {
        repository.save()
    }
}
"#;

    let (nodes, edges) = index_single_file("App.kt", source)?;
    assert_resolved_call_to_method_owner(
        "kotlin annotated data class",
        &nodes,
        &edges,
        "persist",
        "Repository",
        "save",
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin annotated data class",
        &nodes,
        &edges,
        "persist",
        "Audit",
        "save",
    );
    assert_resolved_call_to_method_owner(
        "kotlin annotated function",
        &nodes,
        &edges,
        "dispatchTo",
        "Repository",
        "save",
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin annotated function",
        &nodes,
        &edges,
        "dispatchTo",
        "Audit",
        "save",
    );
    Ok(())
}

#[test]
fn test_kotlin_trailing_lambda_call_keeps_its_receiver() -> anyhow::Result<()> {
    // A trailing lambda has no argument parentheses, so cutting the callee at
    // the first `(` and splitting on the last `.` reached into the lambda body:
    // `repository.forEach { it.hashCode() }` produced the receiver
    // `repository.forEach { it` and the member `hashCode`, and the real call to
    // `forEach` was lost (CR-010).
    let source = r#"
package app

class Repository {
    fun forEach(block: (Int) -> Unit) {}
}

class Audit {
    fun forEach(block: (Int) -> Unit) {}
}

class Bus {
    fun run(repository: Repository) {
        repository.forEach { it.hashCode() }
    }
}
"#;

    let (nodes, edges) = index_single_file("Bus.kt", source)?;
    assert_resolved_call_to_method_owner(
        "kotlin trailing lambda",
        &nodes,
        &edges,
        "run",
        "Repository",
        "forEach",
    );
    assert_no_resolved_call_to_method_owner(
        "kotlin trailing lambda",
        &nodes,
        &edges,
        "run",
        "Audit",
        "forEach",
    );
    Ok(())
}

#[test]
fn test_projection_regression_rust_untyped_cross_file_method_stays_unresolved() -> anyhow::Result<()>
{
    let worker = r#"
pub struct Worker;
impl Worker {
    pub fn execute(&mut self) {}
}
"#;
    let factory = r#"
pub struct Factory;
impl Factory {
    pub fn make(&self) -> Result<Worker, ()> { unimplemented!() }
}
"#;
    let caller = r#"
fn run(factory: &Factory) -> Result<(), ()> {
    let mut worker = factory.make()?;
    worker.execute();
    Ok(())
}
"#;

    let (nodes, edges) = index_files(&[
        ("worker.rs", worker),
        ("factory.rs", factory),
        ("caller.rs", caller),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "rust cross-file return type remains fail-closed",
        &nodes,
        &edges,
        "run",
        "Worker",
        "execute",
    );
    Ok(())
}

#[test]
fn test_projection_regression_rust_trait_default_self_calls_keep_trait_owner() -> anyhow::Result<()>
{
    let source = r#"
trait Workflow {
    fn persist(&self);
    fn audit(&self) {}

    fn run(&self) {
        self.persist();
        self.audit();
    }
}

trait OtherWorkflow {
    fn persist(&self);
    fn audit(&self);
}

struct Other;
impl Other {
    fn inspect(&self) {}
}

trait ConstructorWorkflow {
    fn new() -> Other;
    fn inspect(&self) {}

    fn run_constructor(&self) {
        let value = Self::new();
        value.inspect();
    }
}
"#;

    let (nodes, edges) = index_single_file("workflow.rs", source)?;
    for method in ["persist", "audit"] {
        assert_resolved_call_to_method_owner(
            "rust trait default self call",
            &nodes,
            &edges,
            "run",
            "Workflow",
            method,
        );
        assert_no_resolved_call_to_method_owner(
            "rust trait default self call",
            &nodes,
            &edges,
            "run",
            "OtherWorkflow",
            method,
        );
    }
    assert_no_resolved_call_to_method_owner(
        "rust trait constructor return remains fail closed",
        &nodes,
        &edges,
        "run_constructor",
        "ConstructorWorkflow",
        "inspect",
    );
    Ok(())
}

#[test]
fn test_projection_regression_rust_local_generic_bound_requires_one_trait_method()
-> anyhow::Result<()> {
    let source = r#"
trait Handler {
    fn handle(&self);
}

trait Auditor {
    fn handle(&self);
}

fn dispatch<T: Handler>(target: &T) {
    target.handle();
}

fn ambiguous<T: Handler + Auditor>(target: &T) {
    target.handle();
}
"#;

    let (nodes, edges) = index_single_file("workflow.rs", source)?;
    assert_resolved_call_to_method_owner(
        "rust unique local generic trait bound",
        &nodes,
        &edges,
        "dispatch",
        "Handler",
        "handle",
    );
    assert_no_resolved_call_to_method_owner(
        "rust ambiguous local generic trait bound",
        &nodes,
        &edges,
        "ambiguous",
        "Handler",
        "handle",
    );
    assert_no_resolved_call_to_method_owner(
        "rust ambiguous local generic trait bound",
        &nodes,
        &edges,
        "ambiguous",
        "Auditor",
        "handle",
    );
    Ok(())
}

#[test]
fn test_projection_regression_rust_local_unit_struct_constructor_keeps_owner() -> anyhow::Result<()>
{
    let source = r#"
struct Runner;
impl Runner {
    fn execute(&self) {}
}

struct OtherRunner;
impl OtherRunner {
    fn execute(&self) {}
}

fn entrypoint() {
    let runner = Runner;
    runner.execute();
}
"#;

    let (nodes, edges) = index_single_file("runner.rs", source)?;
    assert_resolved_call_to_method_owner(
        "rust local unit struct constructor",
        &nodes,
        &edges,
        "entrypoint",
        "Runner",
        "execute",
    );
    assert_no_resolved_call_to_method_owner(
        "rust local unit struct constructor",
        &nodes,
        &edges,
        "entrypoint",
        "OtherRunner",
        "execute",
    );
    Ok(())
}

#[test]
fn test_projection_regression_go_cross_file_typed_receivers_resolve_without_guessing()
-> anyhow::Result<()> {
    let types = r#"
package router

type node struct{}
func (*node) addRoute() {}

type Context struct{}
func (*Context) Next() {}

type other struct{}
func (*other) addRoute() {}
"#;
    let callers = r#"
package router

func dispatch(c *Context) {
    c.Next()
}

func build() {
    var root *node
    root = new(node)
    root.addRoute()
}

func shadowed(new func(node) *other) {
    root := new(node)
    root.addRoute()
}
"#;

    let (nodes, edges) = index_files(&[("types.go", types), ("callers.go", callers)])?;
    assert_resolved_call_to_method_owner(
        "go cross-file typed parameter",
        &nodes,
        &edges,
        "dispatch",
        "Context",
        "Next",
    );
    assert_resolved_call_to_method_owner(
        "go builtin new binding",
        &nodes,
        &edges,
        "build",
        "node",
        "addRoute",
    );
    assert_no_resolved_call_to_method_owner(
        "go shadowed new remains fail-closed",
        &nodes,
        &edges,
        "shadowed",
        "node",
        "addRoute",
    );
    Ok(())
}

#[test]
fn test_projection_regression_go_file_scope_new_shadow_stays_unresolved() -> anyhow::Result<()> {
    let source = r#"
package router

type node struct{}
func (*node) addRoute() {}

type other struct{}
func (*other) addRoute() {}

var new = func(node) *other { return &other{} }

func build() {
    root := new(node)
    root.addRoute()
}
"#;

    let (nodes, edges) = index_files(&[("router.go", source)])?;
    assert_no_resolved_call_to_method_owner(
        "go file-scope new shadow remains fail-closed",
        &nodes,
        &edges,
        "build",
        "node",
        "addRoute",
    );
    Ok(())
}

#[test]
fn test_go_navigation_accepts_only_closed_type_and_constructor_ast_shapes() -> anyhow::Result<()> {
    let source = r#"
package proof

type A struct { B B }
type B struct{}
func (*A) Run() {}
func (*B) Run() {}

func nestedSelector() {
    x := A{B: B{}}.B
    x.Run()
}

func doublePointer(x **A) {
    x.Run()
}

func indexedComposite() {
    x := []A{{}}[0]
    x.Run()
}
"#;
    let (nodes, edges) = index_files(&[("main.go", source)])?;
    for caller in ["nestedSelector", "doublePointer", "indexedComposite"] {
        for owner in ["A", "B"] {
            assert_no_resolved_call_to_method_owner(
                "Go non-closed type/constructor AST",
                &nodes,
                &edges,
                caller,
                owner,
                "Run",
            );
        }
    }
    Ok(())
}

#[test]
fn test_projection_regression_dart_arrow_and_initializer_calls_are_typed() -> anyhow::Result<()> {
    let request = r#"
class BaseRequest {
  void finalize() {}
}
"#;
    let client = r#"
class BaseClient {
  Object get(Object url) => _sendUnstreamed(url);
  Object _sendUnstreamed(Object url) => url;
}
"#;
    let io_client = r#"
import 'base_request.dart';

class IOClient {
  void send(BaseRequest request) {
    var stream = request.finalize();
  }

  void untyped(dynamic request) {
    var stream = request.finalize();
  }
}
"#;

    let (nodes, edges) = index_files(&[
        ("lib/base_request.dart", request),
        ("lib/base_client.dart", client),
        ("lib/io_client.dart", io_client),
    ])?;
    assert_resolved_call_to_method_owner(
        "dart arrow body direct call",
        &nodes,
        &edges,
        "BaseClient.get",
        "BaseClient",
        "_sendUnstreamed",
    );
    assert_resolved_call_to_method_owner(
        "dart typed initializer receiver",
        &nodes,
        &edges,
        "IOClient.send",
        "BaseRequest",
        "finalize",
    );
    assert_no_resolved_call_to_method_owner(
        "dart untyped initializer remains fail-closed",
        &nodes,
        &edges,
        "IOClient.untyped",
        "BaseRequest",
        "finalize",
    );

    let worker = r#"
class Worker {
  void run() {}
}
"#;
    let unimported_entry = r#"
void call(Worker worker) {
  worker.run();
}
"#;
    let (unimported_nodes, unimported_edges) = index_files(&[
        ("lib/worker.dart", worker),
        ("lib/entry.dart", unimported_entry),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "dart unimported typed parameter remains fail-closed",
        &unimported_nodes,
        &unimported_edges,
        "call",
        "Worker",
        "run",
    );
    Ok(())
}

#[test]
fn test_projection_regression_dart_duplicate_local_owner_stays_unresolved() -> anyhow::Result<()> {
    let first_request = r#"
class Request {
  void finish() {}
}
"#;
    let second_request = r#"
class Request {
  void finish() {}
}
"#;
    let caller = r#"
import 'first_request.dart';

class Client {
  void send(Request request) {
    request.finish();
  }
}
"#;

    let (nodes, edges) = index_files(&[
        ("lib/first_request.dart", first_request),
        ("lib/second_request.dart", second_request),
        ("lib/client.dart", caller),
    ])?;
    assert_no_resolved_call_to_method_owner(
        "dart duplicate same-directory owner remains fail-closed",
        &nodes,
        &edges,
        "Client.send",
        "Request",
        "finish",
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// C# stage 3b: ManualTypeUsageSpec channel + constructor-body visibility (P2).
//
// TYPE_USAGE certainty is stamped at emit and only for types that resolved
// against the file's binding tables, so the negative tests come first: every
// hostile surface below must produce NO TYPE_USAGE edge at all.
// ---------------------------------------------------------------------------

fn type_usage_edges_from_source<'a>(
    nodes: &[Node],
    edges: &'a [Edge],
    source_name: &str,
) -> Vec<&'a Edge> {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::TYPE_USAGE)
        .filter(|edge| {
            node_by_id
                .get(&edge.source)
                .is_some_and(|source| is_matching_name(&source.serialized_name, source_name))
        })
        .collect()
}

fn describe_type_usage_edges(nodes: &[Node], edges: &[Edge]) -> Vec<String> {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::TYPE_USAGE)
        .map(|edge| {
            let label = |id: &NodeId| {
                node_by_id
                    .get(id)
                    .map(|n| format!("{:?}:{}", n.kind, n.serialized_name))
                    .unwrap_or_else(|| format!("<missing:{}>", id.0))
            };
            format!(
                "{} -> {} (line: {:?}, certainty: {:?})",
                label(&edge.source),
                label(&edge.target),
                edge.line,
                edge.certainty
            )
        })
        .collect()
}

struct ExpectedTypeUsage<'a> {
    source_name: &'a str,
    source_kind: codestory_contracts::graph::NodeKind,
    target_name: &'a str,
    /// The target node's qualified name, when the type resolved through an
    /// import table and the edge must land on a module-qualified reference
    /// node rather than on the alias surface.
    target_qualified: Option<&'a str>,
    /// File the EFFECTIVE target must live in — the declaration's own file
    /// for the same-root channel, which resolves cross-file at finalize.
    target_file_suffix: Option<&'a str>,
}

fn assert_certain_type_usage(
    case_name: &str,
    nodes: &[Node],
    edges: &[Edge],
    expected: ExpectedTypeUsage<'_>,
) {
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let found = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::TYPE_USAGE)
        .any(|edge| {
            let Some(source) = node_by_id.get(&edge.source) else {
                return false;
            };
            // Effective target: same-root edges carry the declaration in
            // `resolved_target`; emit-resolved edges carry it raw.
            let effective_target = edge.resolved_target.unwrap_or(edge.target);
            let Some(target) = node_by_id.get(&effective_target) else {
                return false;
            };
            is_matching_name(&source.serialized_name, expected.source_name)
                && source.kind == expected.source_kind
                && is_matching_name(&target.serialized_name, expected.target_name)
                && expected
                    .target_qualified
                    .is_none_or(|qualified| target.qualified_name.as_deref() == Some(qualified))
                && expected.target_file_suffix.is_none_or(|suffix| {
                    file_path_for_node(&node_by_id, target)
                        .map(|path| path.replace('\\', "/").ends_with(suffix))
                        .unwrap_or(false)
                })
                && edge.certainty == Some(ResolutionCertainty::Certain)
        });
    assert!(
        found,
        "Case `{case_name}`: expected a certain TYPE_USAGE from {:?} `{}` to `{}` (qualified {:?}). TYPE_USAGE edges: {:?}",
        expected.source_kind,
        expected.source_name,
        expected.target_name,
        expected.target_qualified,
        describe_type_usage_edges(nodes, edges)
    );
}

fn assert_no_type_usage_from(case_name: &str, nodes: &[Node], edges: &[Edge], source_name: &str) {
    let from_source = type_usage_edges_from_source(nodes, edges, source_name);
    assert!(
        from_source.is_empty(),
        "Case `{case_name}`: expected NO TYPE_USAGE edge from `{source_name}`. TYPE_USAGE edges: {:?}",
        describe_type_usage_edges(nodes, edges)
    );
}

fn assert_no_type_usage_self_edges(case_name: &str, edges: &[Edge]) {
    let self_edges = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::TYPE_USAGE && edge.source == edge.target)
        .count();
    assert_eq!(
        self_edges, 0,
        "Case `{case_name}`: TYPE_USAGE self-edges must never be emitted"
    );
}

/// Every hostile type surface fails closed: no TYPE_USAGE edge exists at all.
///
/// This is the guard on the `csharp_receiver_owner_from_type` fallthrough
/// (it returns `Some((owner_name, None))` for ANY unknown surface, which is
/// safe for CALL annotation but must never feed the certainty-stamped
/// TYPE_USAGE channel).
#[test]
fn test_csharp_type_usage_hostile_surfaces_emit_no_edges() -> anyhow::Result<()> {
    let hostile_source = r#"
using System;

namespace Acme.App;

class Known
{
    public void Announce() {}
}

class HostileOwner
{
    private readonly UnknownType missing;
    private readonly Acme.External.Widget external;
    // private readonly CommentType commented;
    private string note = "StringType inside a literal";

    public void Run()
    {
        var mystery = MakeSomething();
    }

    static object MakeSomething() => new object();
}
"#;
    let (nodes, edges) = index_files(&[("src/Acme/App/Hostile.cs", hostile_source)])?;

    // The unknown-type fallthrough, the qualified-but-unimported surface, the
    // comment and string mentions, and the unresolvable `var` all fail closed.
    assert_no_type_usage_from("csharp hostile type usage", &nodes, &edges, "HostileOwner");
    assert_no_type_usage_from("csharp hostile type usage", &nodes, &edges, "Run");
    assert_no_type_usage_from("csharp hostile type usage", &nodes, &edges, "MakeSomething");
    for absent in ["UnknownType", "Widget", "CommentType", "StringType"] {
        assert!(
            !has_node_name(&nodes, absent),
            "hostile type surface `{absent}` must not mint a node"
        );
    }
    assert_no_type_usage_self_edges("csharp hostile type usage", &edges);
    Ok(())
}

/// Generic type parameters never resolve, even when a single plain namespace
/// import would otherwise claim any bare name in the file.
#[test]
fn test_csharp_type_usage_generic_parameters_fail_closed_despite_namespace_import()
-> anyhow::Result<()> {
    let generic_source = r#"
using Acme.Events;

namespace Acme.App;

class GenericOwner<T>
{
    private readonly T item;
    private readonly Listener listener;

    public void Fill<TItem>(TItem seed) {}
}
"#;
    let (nodes, edges) = index_files(&[("src/Acme/App/GenericOwner.cs", generic_source)])?;

    // The leak guard: without the generic-parameter check, `T` would resolve
    // to `Acme.Events.T` through the single-namespace-import heuristic.
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let generic_targets = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::TYPE_USAGE)
        .filter(|edge| {
            node_by_id
                .get(&edge.target)
                .is_some_and(|target| matches!(target.serialized_name.as_str(), "T" | "TItem"))
        })
        .count();
    assert_eq!(
        generic_targets,
        0,
        "generic type parameters must never become TYPE_USAGE targets. TYPE_USAGE edges: {:?}",
        describe_type_usage_edges(&nodes, &edges)
    );
    assert_no_type_usage_from("csharp generic parameter", &nodes, &edges, "Fill");

    // Positive control in the same file: the namespace-import table itself
    // works — the field of imported type `Listener` produces the certain,
    // class-anchored, module-qualified edge.
    assert_certain_type_usage(
        "csharp namespace-imported field type",
        &nodes,
        &edges,
        ExpectedTypeUsage {
            source_name: "GenericOwner",
            source_kind: codestory_contracts::graph::NodeKind::CLASS,
            target_name: "Listener",
            target_qualified: Some("Acme.Events.Listener"),
            target_file_suffix: None,
        },
    );
    assert_no_type_usage_self_edges("csharp generic parameter", &edges);
    Ok(())
}

/// Same-file positives: field declarations and constructor-context facts are
/// CLASS-anchored, method parameters are METHOD-anchored, and the
/// constructor-body receiver call resolves exactly like the method path with
/// the enclosing class as its source anchor.
#[test]
fn test_csharp_field_and_constructor_type_usage_and_constructor_call_resolve_same_file()
-> anyhow::Result<()> {
    let source = r#"
namespace Acme.Mapper;

class ConfigType {}

class Builder
{
    public Builder(ConfigType cfg) {}

    public void Prepare() {}
}

class PlanOwner
{
    private readonly Builder builder;

    public PlanOwner(ConfigType cfg)
    {
        var b = new Builder(cfg);
        b.Prepare();
    }

    public void Drive(Builder builder)
    {
        builder.Prepare();
    }
}
"#;
    let (nodes, edges) = index_files(&[("src/Acme/Mapper/PlanOwner.cs", source)])?;
    let class_kind = codestory_contracts::graph::NodeKind::CLASS;
    let method_kind = codestory_contracts::graph::NodeKind::METHOD;

    // Field declaration: certain TYPE_USAGE from the CLASS node.
    assert_certain_type_usage(
        "csharp same-file field type usage",
        &nodes,
        &edges,
        ExpectedTypeUsage {
            source_name: "PlanOwner",
            source_kind: class_kind,
            target_name: "Builder",
            target_qualified: None,
            target_file_suffix: None,
        },
    );
    // Constructor parameter: class-anchored (A1's Builder -> ConfigType shape).
    assert_certain_type_usage(
        "csharp constructor parameter type usage",
        &nodes,
        &edges,
        ExpectedTypeUsage {
            source_name: "Builder",
            source_kind: class_kind,
            target_name: "ConfigType",
            target_qualified: None,
            target_file_suffix: None,
        },
    );
    assert_certain_type_usage(
        "csharp constructor parameter type usage",
        &nodes,
        &edges,
        ExpectedTypeUsage {
            source_name: "PlanOwner",
            source_kind: class_kind,
            target_name: "ConfigType",
            target_qualified: None,
            target_file_suffix: None,
        },
    );
    // Method parameter: METHOD-anchored.
    assert_certain_type_usage(
        "csharp method parameter type usage",
        &nodes,
        &edges,
        ExpectedTypeUsage {
            source_name: "Drive",
            source_kind: method_kind,
            target_name: "Builder",
            target_qualified: None,
            target_file_suffix: None,
        },
    );

    // Constructor body `var b = new Builder(cfg); b.Prepare();` resolves the
    // receiver call with the enclosing CLASS as the edge source.
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let constructor_call = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .find(|edge| {
            node_by_id.get(&edge.source).is_some_and(|source| {
                source.kind == class_kind && is_matching_name(&source.serialized_name, "PlanOwner")
            }) && edge
                .resolved_target
                .and_then(|id| node_by_id.get(&id))
                .is_some_and(|resolved| {
                    is_matching_owned_method(&resolved.serialized_name, "Builder", "Prepare")
                })
        });
    let constructor_call = constructor_call.unwrap_or_else(|| {
        panic!(
            "expected the constructor-body call to resolve class-anchored to Builder::Prepare. \
             Calls: {:?}",
            describe_call_edges(&edges, &nodes)
        )
    });
    assert_eq!(
        constructor_call.certainty,
        Some(ResolutionCertainty::Certain),
        "the class-anchored constructor-body call must be certain"
    );

    // The method_declaration path is untouched: the same call from a method
    // body still resolves METHOD-anchored.
    assert_resolved_call_to_method_owner_in_file(
        "csharp method receiver path unchanged",
        &nodes,
        &edges,
        "Drive",
        "Builder",
        "Prepare",
        "src/Acme/Mapper/PlanOwner.cs",
    );
    assert_no_call_self_edges("csharp constructor visibility", &edges);
    assert_no_type_usage_self_edges("csharp constructor visibility", &edges);
    Ok(())
}

/// Imported positives: a `using` alias resolves through the alias table for
/// fields, method-body creations, and constructor-body creations, and the
/// constructor-body receiver call on the aliased type resolves cross-file.
#[test]
fn test_csharp_aliased_type_usage_and_constructor_call_resolve_through_alias_table()
-> anyhow::Result<()> {
    let builder_source = r#"
namespace Acme.Builders;

public class Builder
{
    public void Prepare() {}
}
"#;
    let consumer_source = r#"
using B = Acme.Builders.Builder;

namespace Acme.App;

class FieldOwner
{
    private readonly B builder;
}

class MakerOwner
{
    public void Make()
    {
        var built = new B();
        built.Prepare();
    }
}

class CtorOwner
{
    public CtorOwner()
    {
        var built = new B();
        built.Prepare();
    }
}
"#;
    let (nodes, edges) = index_files(&[
        ("src/Acme/Builders/Builder.cs", builder_source),
        ("src/Acme/App/Consumer.cs", consumer_source),
    ])?;
    let class_kind = codestory_contracts::graph::NodeKind::CLASS;
    let method_kind = codestory_contracts::graph::NodeKind::METHOD;

    // Imported field variant: the edge target carries the alias-resolved
    // module-qualified identity, never the alias surface `B`.
    assert_certain_type_usage(
        "csharp aliased field type usage",
        &nodes,
        &edges,
        ExpectedTypeUsage {
            source_name: "FieldOwner",
            source_kind: class_kind,
            target_name: "Builder",
            target_qualified: Some("Acme.Builders.Builder"),
            target_file_suffix: None,
        },
    );
    // Aliased object creation in a method body: METHOD-anchored.
    assert_certain_type_usage(
        "csharp aliased object creation",
        &nodes,
        &edges,
        ExpectedTypeUsage {
            source_name: "Make",
            source_kind: method_kind,
            target_name: "Builder",
            target_qualified: Some("Acme.Builders.Builder"),
            target_file_suffix: None,
        },
    );
    // Aliased object creation in a constructor body: CLASS-anchored.
    assert_certain_type_usage(
        "csharp aliased constructor-body creation",
        &nodes,
        &edges,
        ExpectedTypeUsage {
            source_name: "CtorOwner",
            source_kind: class_kind,
            target_name: "Builder",
            target_qualified: Some("Acme.Builders.Builder"),
            target_file_suffix: None,
        },
    );
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let alias_targets = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::TYPE_USAGE)
        .filter(|edge| {
            node_by_id
                .get(&edge.target)
                .is_some_and(|target| target.serialized_name == "B")
        })
        .count();
    assert_eq!(
        alias_targets, 0,
        "the alias surface `B` must never be a TYPE_USAGE target"
    );

    // The aliased method-body receiver call resolves cross-file (pre-existing
    // behaviour), and the constructor-body call now resolves the same way,
    // class-anchored.
    assert_resolved_call_to_method_owner_in_file(
        "csharp aliased method receiver",
        &nodes,
        &edges,
        "Make",
        "Builder",
        "Prepare",
        "src/Acme/Builders/Builder.cs",
    );
    assert_resolved_call_to_method_owner_in_file(
        "csharp aliased constructor receiver",
        &nodes,
        &edges,
        "CtorOwner",
        "Builder",
        "Prepare",
        "src/Acme/Builders/Builder.cs",
    );
    let constructor_call = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .find(|edge| {
            node_by_id.get(&edge.source).is_some_and(|source| {
                source.kind == class_kind && is_matching_name(&source.serialized_name, "CtorOwner")
            }) && edge
                .resolved_target
                .and_then(|id| node_by_id.get(&id))
                .is_some_and(|resolved| {
                    is_matching_owned_method(&resolved.serialized_name, "Builder", "Prepare")
                })
        });
    assert!(
        constructor_call.is_some(),
        "the constructor-body call must resolve with the enclosing CLASS as its source. Calls: {:?}",
        describe_call_edges(&edges, &nodes)
    );
    assert_no_call_self_edges("csharp aliased constructor visibility", &edges);
    assert_no_type_usage_self_edges("csharp aliased constructor visibility", &edges);
    Ok(())
}

// ---------------------------------------------------------------------------
// C# stage 3b revision: primary constructors, chained creation receivers, and
// the same-root-namespace declaration lookup. Negative-first: every hostile
// shape fails closed before the positives prove the AutoMapper mirror.
// ---------------------------------------------------------------------------

fn assert_no_pending_type_usage_nodes(case_name: &str, nodes: &[Node]) {
    let leftover = nodes
        .iter()
        .filter(|node| {
            node.canonical_id
                .as_deref()
                .is_some_and(|canonical| canonical.starts_with("type_ref_pending:"))
        })
        .count();
    assert_eq!(
        leftover, 0,
        "Case `{case_name}`: failed same-root facts must clean their pending reference nodes"
    );
}

/// Same-root hostiles, all fail closed: a bare name whose only declaration
/// lives under a DIFFERENT root namespace (the vendor-shadow shape), and a
/// bare name with TWO declarations under the caller's root (ambiguity).
/// Neither the TYPE_USAGE channel nor the chained-creation CALL channel may
/// produce anything, and the failed pending facts must leave no residue.
#[test]
fn test_csharp_same_root_hostile_shapes_fail_closed() -> anyhow::Result<()> {
    let vendor_source = r#"
namespace Vendor.Ui;

public class Widget
{
    public void Render() {}
}
"#;
    let panel_a_source = r#"
namespace Acme.A;

public class Panel
{
    public void Draw() {}
}
"#;
    let panel_b_source = r#"
namespace Acme.B;

public class Panel
{
    public void Draw() {}
}
"#;
    let consumer_source = r#"
namespace Acme.App;

class ShadowOwner
{
    private Widget vendorShadow;

    public void Chain()
    {
        new Widget().Render();
    }
}

class PanelOwner
{
    private Panel ambiguous;

    public void Chain()
    {
        new Panel().Draw();
    }
}
"#;
    let (nodes, edges) = index_files(&[
        ("src/Vendor/Widget.cs", vendor_source),
        ("src/Acme/A/Panel.cs", panel_a_source),
        ("src/Acme/B/Panel.cs", panel_b_source),
        ("src/Acme/App/Consumer.cs", consumer_source),
    ])?;

    // Vendor shadow: Widget's only declaration is under root `Vendor`.
    assert_no_type_usage_from("csharp vendor shadow", &nodes, &edges, "ShadowOwner");
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp vendor shadow chained call",
        &nodes,
        &edges,
        "ShadowOwner.Chain",
        "Widget",
        "Render",
        "src/Vendor/Widget.cs",
    );

    // Ambiguity: two Panel declarations under root `Acme`.
    assert_no_type_usage_from("csharp same-root ambiguity", &nodes, &edges, "PanelOwner");
    for panel_file in ["src/Acme/A/Panel.cs", "src/Acme/B/Panel.cs"] {
        assert_no_resolved_call_to_method_owner_in_file(
            "csharp same-root ambiguity chained call",
            &nodes,
            &edges,
            "PanelOwner.Chain",
            "Panel",
            "Draw",
            panel_file,
        );
    }

    assert_no_type_usage_from("csharp same-root hostiles", &nodes, &edges, "Chain");
    assert_no_pending_type_usage_nodes("csharp same-root hostiles", &nodes);
    assert_no_call_self_edges("csharp same-root hostiles", &edges);
    assert_no_type_usage_self_edges("csharp same-root hostiles", &edges);
    Ok(())
}

/// Generic parameters of a PRIMARY constructor never resolve, even though
/// the parameter list hangs directly off the type declaration.
#[test]
fn test_csharp_primary_constructor_generic_parameter_fails_closed() -> anyhow::Result<()> {
    let source = r#"
namespace Acme.App;

class Box<T>(T seed)
{
    private readonly T item = seed;
}
"#;
    let (nodes, edges) = index_files(&[("src/Acme/App/Box.cs", source)])?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let generic_targets = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::TYPE_USAGE)
        .filter(|edge| {
            let effective_target = edge.resolved_target.unwrap_or(edge.target);
            node_by_id
                .get(&effective_target)
                .is_some_and(|target| target.serialized_name == "T")
        })
        .count();
    assert_eq!(
        generic_targets,
        0,
        "a primary-constructor type parameter must never become a TYPE_USAGE target. Edges: {:?}",
        describe_type_usage_edges(&nodes, &edges)
    );
    assert_no_pending_type_usage_nodes("csharp primary ctor generic", &nodes);
    Ok(())
}

/// The AutoMapper mirror (A1 + A3 shapes, all three review findings at once):
///
/// * `TypeMapPlanBuilder` is a C#12 primary-constructor REF STRUCT whose
///   parameter types are the A1 facts — type-anchored TYPE_USAGE, resolved
///   cross-file through the same-root rule (namespace `Acme.Execution` sees
///   `Acme`'s TypeMap with no using of any kind);
/// * `TypeMap.CreateMapperLambda` hands off through a member call whose
///   receiver IS the object creation, in an expression-bodied method, with a
///   deliberate name collision against the enclosing method;
/// * `MEMBER(TypeMapPlanBuilder -> CreateMapperLambda)` comes from the
///   member collector's struct arm.
#[test]
fn test_csharp_primary_ctor_type_usage_and_chained_creation_call_resolve_same_root()
-> anyhow::Result<()> {
    let type_map_source = r#"
namespace Acme;

public class TypeMap
{
    internal object CreateMapperLambda() =>
        new TypeMapPlanBuilder(this).CreateMapperLambda();
}

class Holder
{
    private object plan = new TypeMapPlanBuilder(null).CreateMapperLambda();
}
"#;
    let builder_source = r#"
namespace Acme.Execution;

public ref struct TypeMapPlanBuilder(TypeMap typeMap)
{
    public object CreateMapperLambda() => typeMap;
}
"#;
    let (nodes, edges) = index_files(&[
        ("src/Acme/TypeMap.cs", type_map_source),
        ("src/Acme/Execution/TypeMapPlanBuilder.cs", builder_source),
    ])?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let struct_kind = codestory_contracts::graph::NodeKind::STRUCT;
    let method_kind = codestory_contracts::graph::NodeKind::METHOD;

    // A1 mirror: primary-ctor parameter type, type-anchored on the ref
    // struct, resolved to the real declaration in the OTHER file.
    assert_certain_type_usage(
        "csharp primary ctor same-root type usage",
        &nodes,
        &edges,
        ExpectedTypeUsage {
            source_name: "TypeMapPlanBuilder",
            source_kind: struct_kind,
            target_name: "TypeMap",
            target_qualified: Some("Acme.TypeMap"),
            target_file_suffix: Some("src/Acme/TypeMap.cs"),
        },
    );
    // Reverse direction: the creation's type in TypeMap.cs resolves into the
    // Execution namespace the same way (the csproj-global-using shape).
    assert_certain_type_usage(
        "csharp chained creation type usage",
        &nodes,
        &edges,
        ExpectedTypeUsage {
            source_name: "TypeMap.CreateMapperLambda",
            source_kind: method_kind,
            target_name: "TypeMapPlanBuilder",
            target_qualified: Some("Acme.Execution.TypeMapPlanBuilder"),
            target_file_suffix: Some("src/Acme/Execution/TypeMapPlanBuilder.cs"),
        },
    );

    // A3 mirror: the chained-creation handoff resolves cross-file to the
    // struct's method — never to the same-named enclosing method.
    assert_resolved_call_to_method_owner_in_file(
        "csharp chained creation handoff",
        &nodes,
        &edges,
        "TypeMap.CreateMapperLambda",
        "TypeMapPlanBuilder",
        "CreateMapperLambda",
        "src/Acme/Execution/TypeMapPlanBuilder.cs",
    );
    assert_no_resolved_call_to_method_owner_in_file(
        "csharp chained creation must not self-resolve",
        &nodes,
        &edges,
        "TypeMap.CreateMapperLambda",
        "TypeMap",
        "CreateMapperLambda",
        "src/Acme/TypeMap.cs",
    );
    let handoff = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .find(|edge| {
            node_by_id.get(&edge.source).is_some_and(|source| {
                is_matching_name(&source.serialized_name, "TypeMap.CreateMapperLambda")
            }) && edge
                .resolved_target
                .and_then(|id| node_by_id.get(&id))
                .is_some_and(|resolved| {
                    is_matching_owned_method(
                        &resolved.serialized_name,
                        "TypeMapPlanBuilder",
                        "CreateMapperLambda",
                    )
                })
        })
        .unwrap_or_else(|| {
            panic!(
                "expected the chained-creation handoff edge. Calls: {:?}",
                describe_call_edges(&edges, &nodes)
            )
        });
    assert!(
        callsite_has_segment(handoff, "receiver-owner:TypeMapPlanBuilder"),
        "the handoff callsite must carry the syntactic owner annotation: {:?}",
        handoff.callsite_identity
    );
    assert_eq!(
        handoff.certainty,
        Some(ResolutionCertainty::Certain),
        "the same-root handoff resolution must be certain"
    );

    // Field-initializer chained call: no method or constructor encloses it,
    // so the spec anchors at the class and still resolves.
    assert_resolved_call_to_method_owner_in_file(
        "csharp field initializer chained call",
        &nodes,
        &edges,
        "Holder",
        "TypeMapPlanBuilder",
        "CreateMapperLambda",
        "src/Acme/Execution/TypeMapPlanBuilder.cs",
    );

    // MEMBER through the struct arm (the A3 join edge).
    let member_present = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::MEMBER)
        .any(|edge| {
            node_by_id.get(&edge.source).is_some_and(|source| {
                source.kind == struct_kind
                    && is_matching_name(&source.serialized_name, "TypeMapPlanBuilder")
            }) && node_by_id.get(&edge.target).is_some_and(|target| {
                is_matching_owned_method(
                    &target.serialized_name,
                    "TypeMapPlanBuilder",
                    "CreateMapperLambda",
                )
            })
        });
    assert!(
        member_present,
        "expected MEMBER(TypeMapPlanBuilder -> CreateMapperLambda) via the struct arm"
    );

    assert_no_call_self_edges("csharp automapper mirror", &edges);
    assert_no_type_usage_self_edges("csharp automapper mirror", &edges);
    Ok(())
}
