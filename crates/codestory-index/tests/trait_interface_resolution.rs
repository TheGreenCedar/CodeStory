use codestory_core::{Edge, EdgeKind, Node};
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_storage::Storage;
use std::collections::HashMap;
use std::fs;
use tempfile::tempdir;

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
    assert!(
        errors.is_empty(),
        "Indexing errors for `{filename}`: {errors:?}"
    );

    Ok((storage.get_nodes()?, storage.get_edges()?))
}

fn is_matching_name(serialized_name: &str, wanted_name: &str) -> bool {
    serialized_name == wanted_name
        || serialized_name.ends_with(&format!(".{wanted_name}"))
        || serialized_name.ends_with(&format!("::{wanted_name}"))
}

fn is_matching_owned_method(serialized_name: &str, owner: &str, method: &str) -> bool {
    serialized_name == format!("{owner}.{method}")
        || serialized_name == format!("{owner}::{method}")
        || serialized_name.ends_with(&format!(".{owner}.{method}"))
        || serialized_name.ends_with(&format!("::{owner}::{method}"))
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
fn test_trait_or_interface_call_resolves_to_declared_method_owner() -> anyhow::Result<()> {
    let cases = [
        (
            "main.rs",
            r#"
trait EventListener {
    fn handle_event(&mut self);
}

struct EventBus;
impl EventBus {
    fn dispatch_to<L: EventListener>(&self, listener: &mut L) {
        while true {
            listener.handle_event();
            break;
        }
    }
}
"#,
            "dispatch_to",
            "EventListener",
            "handle_event",
        ),
        (
            "Test.java",
            r#"
interface EventListener {
    void handleEvent();
}

class EventBus {
    void dispatchTo(EventListener listener) {
        listener.handleEvent();
    }
}
"#,
            "dispatchTo",
            "EventListener",
            "handleEvent",
        ),
        (
            "main.ts",
            r#"
interface EventListener {
    handleEvent(): void;
}

class EventBus {
    dispatchTo(listener: EventListener) {
        listener.handleEvent();
    }
}
"#,
            "dispatchTo",
            "EventListener",
            "handleEvent",
        ),
        (
            "main.cpp",
            r#"
class EventListener {
public:
    virtual void handleEvent() = 0;
};

class EventBus {
public:
    void dispatchTo(EventListener& listener);
};

void EventBus::dispatchTo(EventListener& listener) {
    listener.handleEvent();
}
"#,
            "dispatchTo",
            "EventListener",
            "handleEvent",
        ),
    ];

    for (filename, source, caller_name, owner_name, method_name) in cases {
        let (nodes, edges) = index_single_file(filename, source)?;
        assert_resolved_call_to_method_owner(
            filename,
            &nodes,
            &edges,
            caller_name,
            owner_name,
            method_name,
        );
    }

    Ok(())
}

#[test]
fn test_simple_call_resolution_across_all_supported_languages() -> anyhow::Result<()> {
    let cases = [
        (
            "main.py",
            r#"
def callee():
    return 1

def caller():
    callee()
    return 1
"#,
            "caller",
            "callee",
        ),
        (
            "Test.java",
            r#"
class Test {
    int callee() { return 1; }
    int caller() { callee(); return 1; }
}
"#,
            "caller",
            "callee",
        ),
        (
            "main.rs",
            r#"
fn callee() -> i32 { 1 }
fn caller() -> i32 { callee(); 1 }
"#,
            "caller",
            "callee",
        ),
        (
            "main.js",
            r#"
function callee() { return 1; }
function caller() { callee(); return 1; }
"#,
            "caller",
            "callee",
        ),
        (
            "main.ts",
            r#"
function callee(): number { return 1; }
function caller(): number { callee(); return 1; }
"#,
            "caller",
            "callee",
        ),
        (
            "main.cpp",
            r#"
int callee() { return 1; }
int caller() { callee(); return 1; }
"#,
            "caller",
            "callee",
        ),
        (
            "main.c",
            r#"
int callee() { return 1; }
int caller() { callee(); return 1; }
"#,
            "caller",
            "callee",
        ),
    ];

    for (filename, source, caller_name, callee_name) in cases {
        let (nodes, edges) = index_single_file(filename, source)?;
        assert_resolved_call_to_name(filename, &nodes, &edges, caller_name, callee_name);
    }

    Ok(())
}
