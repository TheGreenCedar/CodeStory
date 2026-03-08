use codestory_core::{Edge, EdgeKind, Node, NodeKind};
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_storage::Storage;
use std::fs;
use tempfile::tempdir;

fn index_project(files: &[(&str, &str)]) -> anyhow::Result<(Vec<Node>, Vec<Edge>)> {
    let dir = tempdir()?;
    let root = dir.path();
    let mut files_to_index = Vec::with_capacity(files.len());
    for (relative_path, contents) in files {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, contents)?;
        files_to_index.push(path);
    }

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();
    let refresh_info = codestory_project::RefreshInfo {
        files_to_index,
        files_to_remove: vec![],
    };
    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    let errors = storage.get_errors(None)?;
    anyhow::ensure!(errors.is_empty(), "indexing errors: {errors:?}");
    Ok((storage.get_nodes()?, storage.get_edges()?))
}

fn matches_name(actual: &str, wanted: &str) -> bool {
    actual == wanted
        || actual.ends_with(&format!(".{wanted}"))
        || actual.ends_with(&format!("::{wanted}"))
        || actual.ends_with(&format!(" {wanted}"))
}

fn find_node<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Node> {
    nodes.iter().find(|node| matches_name(&node.serialized_name, name))
}

fn has_node_kind(nodes: &[Node], name: &str, kind: NodeKind) -> bool {
    nodes.iter()
        .any(|node| matches_name(&node.serialized_name, name) && node.kind == kind)
}

fn edge_between(nodes: &[Node], edges: &[Edge], kind: EdgeKind, source: &str, target: &str) -> bool {
    let source_id = find_node(nodes, source).map(|node| node.id);
    let target_id = find_node(nodes, target).map(|node| node.id);
    match (source_id, target_id) {
        (Some(source_id), Some(target_id)) => edges.iter().any(|edge| {
            edge.kind == kind
                && (edge.source == source_id || edge.resolved_source == Some(source_id))
                && (edge.target == target_id || edge.resolved_target == Some(target_id))
        }),
        _ => false,
    }
}

#[test]
fn test_rust_grouped_use_and_scoped_generic_impls_are_indexed_without_override_loops() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.rs",
        r#"
pub trait Action<T> {
    fn run(&self);
}

pub struct Thing;

impl<T> Action<T> for Thing {
    fn run(&self) {}
}

use crate::{Action, Thing};
"#,
    )])?;

    assert!(
        nodes.iter().any(|node| node.kind == NodeKind::INTERFACE && node.serialized_name.contains("Action")),
        "expected a trait/interface node for Action"
    );
    assert!(
        nodes.iter().any(|node| node.serialized_name.contains("Thing")),
        "expected a type node for Thing"
    );
    assert!(
        edges.iter().filter(|edge| edge.kind == EdgeKind::INHERITANCE).any(|edge| {
            let source = nodes.iter().find(|node| node.id == edge.source).map(|node| node.serialized_name.as_str()).unwrap_or("");
            let target = nodes.iter().find(|node| node.id == edge.target).map(|node| node.serialized_name.as_str()).unwrap_or("");
            source.contains("Thing") && target.contains("Action")
        }),
        "expected generic impl to yield a Thing -> Action inheritance edge"
    );
    assert!(
        edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::OVERRIDE)
            .all(|edge| edge.source != edge.target),
        "Rust trait impls should not create reflexive OVERRIDE self-loops"
    );

    Ok(())
}

#[test]
fn test_c_typedef_struct_members_cover_pointer_and_function_pointer_declarators() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.c",
        r#"
typedef struct {
    int *p;
    int (*cb)(void);
} Item;
"#,
    )])?;

    assert!(find_node(&nodes, "Item").is_some(), "expected Item typedef node");
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Item", "p"),
        "expected Item -> p member edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Item", "cb"),
        "expected Item -> cb member edge"
    );

    Ok(())
}

#[test]
fn test_cpp_template_structs_keep_inheritance_and_members() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.cpp",
        r#"
struct Base {};

struct Child : Base {
    void run() {}
    int value;
};
"#,
    )])?;

    assert!(has_node_kind(&nodes, "Base", NodeKind::CLASS));
    assert!(has_node_kind(&nodes, "Child", NodeKind::CLASS));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::INHERITANCE, "Child", "Base"),
        "expected Child -> Base inheritance edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Child", "run"),
        "expected Child -> run member edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Child", "value"),
        "expected Child -> value member edge"
    );

    Ok(())
}

#[test]
fn test_rust_generic_calls_with_multiple_type_arguments_do_not_duplicate_nodes() -> anyhow::Result<()> {
    let (_nodes, edges) = index_project(&[(
        "main.rs",
        r#"
struct Left;
struct Right;

fn pair<T, U>() {}

fn run() {
    pair::<Left, Right>();
}
"#,
    )])?;

    assert!(
        edges.iter().any(|edge| edge.kind == EdgeKind::CALL),
        "expected generic call to retain a CALL edge"
    );

    Ok(())
}

#[test]
fn test_cpp_template_types_with_multiple_arguments_do_not_duplicate_nodes() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.cpp",
        r#"
struct Key {};
struct Value {};

template <typename T, typename U>
struct PairStore {};

struct Holder {
    PairStore<Key, Value> store;
};
"#,
    )])?;

    assert!(has_node_kind(&nodes, "Holder", NodeKind::CLASS));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Holder", "store"),
        "expected Holder -> store member edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::TYPE_ARGUMENT, "PairStore", "Key"),
        "expected template type to retain at least the first type argument"
    );

    Ok(())
}

#[test]
fn test_cpp_template_base_class_with_type_argument_does_not_duplicate_nodes() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.cpp",
        r#"
struct MatcherUntypedBase {};

template <typename ObjectT>
struct MatcherMethod {};

template <typename T>
struct MatcherBase : MatcherUntypedBase, MatcherMethod<T> {};
"#,
    )])?;

    assert!(has_node_kind(&nodes, "MatcherBase", NodeKind::CLASS));
    assert!(has_node_kind(&nodes, "MatcherMethod", NodeKind::CLASS));
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::INHERITANCE,
            "MatcherBase",
            "MatcherMethod"
        ),
        "expected MatcherBase -> MatcherMethod inheritance edge"
    );
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::TYPE_ARGUMENT,
            "MatcherMethod",
            "T"
        ),
        "expected MatcherMethod<T> to retain its type argument"
    );

    Ok(())
}

#[test]
fn test_cpp_template_specialization_name_does_not_duplicate_nodes() -> anyhow::Result<()> {
    let (nodes, _edges) = index_project(&[(
        "main.cpp",
        r#"
template <typename StringT>
class StringTraits {};

template <>
class StringTraits<int> {};
"#,
    )])?;

    assert!(has_node_kind(&nodes, "StringTraits", NodeKind::CLASS));
    assert!(
        has_node_kind(&nodes, "StringTraits<int>", NodeKind::CLASS),
        "expected specialized class declaration to index without duplicate-node errors"
    );

    Ok(())
}
