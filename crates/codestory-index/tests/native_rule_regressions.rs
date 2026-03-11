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
        mode: codestory_project::BuildMode::Incremental,
        files_to_index,
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
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
    nodes
        .iter()
        .find(|node| matches_name(&node.serialized_name, name))
}

fn has_node_kind(nodes: &[Node], name: &str, kind: NodeKind) -> bool {
    nodes
        .iter()
        .any(|node| matches_name(&node.serialized_name, name) && node.kind == kind)
}

fn edge_between(
    nodes: &[Node],
    edges: &[Edge],
    kind: EdgeKind,
    source: &str,
    target: &str,
) -> bool {
    let source_ids = nodes
        .iter()
        .filter(|node| matches_name(&node.serialized_name, source))
        .map(|node| node.id)
        .collect::<Vec<_>>();
    let target_ids = nodes
        .iter()
        .filter(|node| matches_name(&node.serialized_name, target))
        .map(|node| node.id)
        .collect::<Vec<_>>();
    !source_ids.is_empty()
        && !target_ids.is_empty()
        && edges.iter().any(|edge| {
            edge.kind == kind
                && source_ids.iter().any(|source_id| {
                    edge.source == *source_id || edge.resolved_source == Some(*source_id)
                })
                && target_ids.iter().any(|target_id| {
                    edge.target == *target_id || edge.resolved_target == Some(*target_id)
                })
        })
}

fn find_node_by_name_and_kind<'a>(
    nodes: &'a [Node],
    name: &str,
    kind: NodeKind,
) -> Option<&'a Node> {
    nodes
        .iter()
        .find(|node| matches_name(&node.serialized_name, name) && node.kind == kind)
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

#[test]
fn test_rust_grouped_use_and_scoped_generic_impls_are_indexed_without_override_loops()
-> anyhow::Result<()> {
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
        nodes
            .iter()
            .any(|node| node.serialized_name.contains("Thing")),
        "expected a type node for Thing"
    );
    assert!(
        edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::INHERITANCE)
            .any(|edge| {
                let source = nodes
                    .iter()
                    .find(|node| node.id == edge.source)
                    .map(|node| node.serialized_name.as_str())
                    .unwrap_or("");
                let target = nodes
                    .iter()
                    .find(|node| node.id == edge.target)
                    .map(|node| node.serialized_name.as_str())
                    .unwrap_or("");
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
fn test_rust_cross_file_inherent_impl_attaches_members_to_declared_type() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[
        ("main.rs", "mod a; mod b;\n"),
        ("a.rs", "pub struct Thing;\n"),
        (
            "b.rs",
            r#"
use crate::a::Thing;

impl Thing {
    pub fn run(&self) {}
}
"#,
        ),
    ])?;

    let thing_nodes = nodes
        .iter()
        .filter(|node| matches_name(&node.serialized_name, "Thing"))
        .collect::<Vec<_>>();
    assert_eq!(thing_nodes.len(), 1, "expected one logical Thing node");
    assert_eq!(thing_nodes[0].kind, NodeKind::STRUCT);
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Thing", "run"),
        "expected Thing -> run member edge"
    );

    Ok(())
}

#[test]
fn test_rust_same_name_impl_anchors_stay_separate_across_modules() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[
        ("main.rs", "mod a; mod b;\n"),
        (
            "a.rs",
            r#"
pub struct Thing;

impl Thing {
    pub fn from_a(&self) {}
}
"#,
        ),
        (
            "b.rs",
            r#"
pub struct Thing;

impl Thing {
    pub fn from_b(&self) {}
}
"#,
        ),
    ])?;

    let thing_nodes = nodes
        .iter()
        .filter(|node| {
            matches_name(&node.serialized_name, "Thing") && node.kind == NodeKind::STRUCT
        })
        .collect::<Vec<_>>();
    assert_eq!(
        thing_nodes.len(),
        2,
        "expected distinct Thing nodes for a::Thing and b::Thing"
    );

    let from_a_id = find_node(&nodes, "from_a")
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected from_a node"))?;
    let from_b_id = find_node(&nodes, "from_b")
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected from_b node"))?;

    let from_a_owner = edges
        .iter()
        .find(|edge| edge.kind == EdgeKind::MEMBER && edge.target == from_a_id)
        .map(|edge| edge.source)
        .ok_or_else(|| anyhow::anyhow!("expected member edge for from_a"))?;
    let from_b_owner = edges
        .iter()
        .find(|edge| edge.kind == EdgeKind::MEMBER && edge.target == from_b_id)
        .map(|edge| edge.source)
        .ok_or_else(|| anyhow::anyhow!("expected member edge for from_b"))?;

    assert_ne!(
        from_a_owner, from_b_owner,
        "same-name impl anchors from different modules must not merge"
    );
    assert!(
        thing_nodes.iter().any(|node| node.id == from_a_owner),
        "expected from_a to belong to one of the Thing anchors"
    );
    assert!(
        thing_nodes.iter().any(|node| node.id == from_b_owner),
        "expected from_b to belong to one of the Thing anchors"
    );

    Ok(())
}

#[test]
fn test_rust_impl_variants_attach_to_terminal_type_identifier() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.rs",
        r#"
mod api {
    pub trait Action<T> {
        fn run(&self);
    }
}

mod types {
    pub struct Plain;
    pub struct Generic<T>(pub T);
    pub struct ScopedGeneric<T>(pub T);
}

impl types::Plain {
    pub fn scoped_plain(&self) {}
}

impl<T> types::Generic<T> {
    pub fn scoped_generic(&self) {}
}

impl<T> types::ScopedGeneric<T> {
    pub fn scoped_generic_type(&self) {}
}

impl<T> api::Action<T> for types::ScopedGeneric<T> {
    fn run(&self) {}
}
"#,
    )])?;

    let plain_nodes = nodes
        .iter()
        .filter(|node| {
            matches_name(&node.serialized_name, "Plain") && node.kind == NodeKind::STRUCT
        })
        .collect::<Vec<_>>();
    let generic_nodes = nodes
        .iter()
        .filter(|node| {
            matches_name(&node.serialized_name, "Generic") && node.kind == NodeKind::STRUCT
        })
        .collect::<Vec<_>>();
    let scoped_generic_nodes = nodes
        .iter()
        .filter(|node| {
            matches_name(&node.serialized_name, "ScopedGeneric") && node.kind == NodeKind::STRUCT
        })
        .collect::<Vec<_>>();

    assert_eq!(plain_nodes.len(), 1, "expected one canonical Plain node");
    assert_eq!(
        generic_nodes.len(),
        1,
        "expected one canonical Generic node"
    );
    assert_eq!(
        scoped_generic_nodes.len(),
        1,
        "expected one canonical ScopedGeneric node"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Plain", "scoped_plain"),
        "expected scoped inherent impl to attach members to Plain"
    );
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::MEMBER,
            "Generic",
            "scoped_generic"
        ),
        "expected generic impl to attach members to Generic"
    );
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::MEMBER,
            "ScopedGeneric",
            "scoped_generic_type"
        ),
        "expected scoped generic impl to attach members to ScopedGeneric"
    );
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::INHERITANCE,
            "ScopedGeneric",
            "Action"
        ),
        "expected trait impl to attach inheritance to ScopedGeneric -> Action"
    );

    Ok(())
}

#[test]
fn test_rust_impl_query_simplification_keeps_terminal_type_names_and_members() -> anyhow::Result<()>
{
    let (nodes, edges) = index_project(&[(
        "main.rs",
        r#"
mod api {
    pub trait Runner<T> {
        fn run(&self);
    }

    pub struct Worker;
    pub struct Wrapper<T>(pub T);
}

impl api::Worker {
    pub fn scoped(&self) {}
}

impl api::Wrapper<u32> {
    pub fn scoped_generic(&self) {}
}

impl api::Runner<u32> for api::Wrapper<u32> {
    fn run(&self) {}
}
"#,
    )])?;

    assert!(has_node_kind(&nodes, "Worker", NodeKind::STRUCT));
    assert!(has_node_kind(&nodes, "Wrapper", NodeKind::STRUCT));
    assert!(has_node_kind(&nodes, "Runner", NodeKind::INTERFACE));
    assert!(
        !nodes.iter().any(|node| {
            matches!(
                node.kind,
                NodeKind::STRUCT | NodeKind::CLASS | NodeKind::INTERFACE
            ) && (node.serialized_name.contains("::Worker")
                || node.serialized_name.contains("Wrapper<u32>")
                || node.serialized_name.contains("Runner<u32>"))
        }),
        "expected Rust impl surfaces to normalize to terminal type identifiers"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Worker", "scoped"),
        "expected scoped impl method to attach to Worker"
    );
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::MEMBER,
            "Wrapper",
            "scoped_generic"
        ),
        "expected scoped generic impl method to attach to Wrapper"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::INHERITANCE, "Wrapper", "Runner"),
        "expected scoped generic trait impl to inherit from Runner"
    );

    Ok(())
}

#[test]
fn test_rust_macros_inside_expression_contexts_emit_call_edges() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.rs",
        r#"
macro_rules! emit {
    () => {
        1
    };
}

fn run() -> i32 {
    Some(emit!()).unwrap_or_default() + emit!()
}
"#,
    )])?;

    assert!(has_node_kind(&nodes, "emit", NodeKind::MACRO));
    let run_id = find_node(&nodes, "run")
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected run node"))?;
    let emit_id = find_node(&nodes, "emit")
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected emit macro node"))?;

    let macro_calls = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == run_id)
        .filter(|edge| edge.target == emit_id || edge.resolved_target == Some(emit_id))
        .collect::<Vec<_>>();

    assert!(
        macro_calls.len() >= 2,
        "expected emit! macro calls in expression contexts to surface as CALL edges"
    );
    assert!(
        macro_calls.iter().all(|edge| edge.source != edge.target),
        "macro CALL edges should not become reflexive self-loops"
    );

    Ok(())
}

#[test]
fn test_c_typedef_struct_members_cover_pointer_and_function_pointer_declarators()
-> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.c",
        r#"
typedef struct {
    int *p;
    int (*cb)(void);
} Item;
"#,
    )])?;

    assert!(
        find_node(&nodes, "Item").is_some(),
        "expected Item typedef node"
    );
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
fn test_rust_generic_calls_with_multiple_type_arguments_do_not_duplicate_nodes()
-> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.rs",
        r#"
struct Left;
struct Middle;
struct Right;

fn pair<T, U, V>() {}

fn run() {
    pair::<Left, Middle, Right>();
}
"#,
    )])?;

    assert!(
        edges.iter().any(|edge| edge.kind == EdgeKind::CALL),
        "expected generic call to retain a CALL edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::TYPE_ARGUMENT, "pair", "Left"),
        "expected pair::<Left, Middle, Right> to retain the first type argument"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::TYPE_ARGUMENT, "pair", "Middle"),
        "expected pair::<Left, Middle, Right> to retain the middle type argument"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::TYPE_ARGUMENT, "pair", "Right"),
        "expected pair::<Left, Middle, Right> to retain the third type argument"
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
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::TYPE_ARGUMENT,
            "PairStore",
            "Value"
        ),
        "expected template type to retain the second type argument"
    );

    Ok(())
}

#[test]
fn test_cpp_template_types_with_multiline_and_nested_arguments_stay_ast_driven()
-> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.cpp",
        r#"
struct Key {};
struct Value {};

template <typename T>
struct Wrapper {};

template <typename T, typename U>
struct PairStore {};

struct Holder {
    PairStore<
        Key,
        Wrapper<Value> // nested template with comment
    > store;
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
        "expected multiline template type to retain the first type argument"
    );
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::TYPE_ARGUMENT,
            "PairStore",
            "Wrapper"
        ),
        "expected multiline template type to retain the nested template owner"
    );

    Ok(())
}

#[test]
fn test_cpp_template_aliases_with_multiple_arguments_keep_all_arguments() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.cpp",
        r#"
struct Key {};
struct Value {};

template <typename T, typename U>
struct PairStore {};

using StoreAlias = PairStore<Key, Value>;
"#,
    )])?;

    assert!(has_node_kind(&nodes, "PairStore", NodeKind::CLASS));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::TYPE_ARGUMENT, "PairStore", "Key"),
        "expected alias template type to retain the first type argument"
    );
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::TYPE_ARGUMENT,
            "PairStore",
            "Value"
        ),
        "expected alias template type to retain the second type argument"
    );

    Ok(())
}

#[test]
fn test_typescript_override_resolves_to_base_method() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.ts",
        r#"
class Base {
    greet() {}
}

class Child extends Base {
    override greet() {}
}
"#,
    )])?;
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::OVERRIDE,
            "Child.greet",
            "Base.greet"
        ),
        "expected override edge to resolve Child.greet -> Base.greet"
    );
    assert!(
        edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::OVERRIDE)
            .all(|edge| edge.source != edge.target),
        "override edges should not remain reflexive after placeholder rewrite"
    );

    Ok(())
}

#[test]
fn test_tsx_override_render_keeps_jsx_usage_edges() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.tsx",
        r#"
type Props = { label: string };

function Badge(props: Props) {
    return <span>{props.label}</span>;
}

class BaseView {
    render() {
        return <div />;
    }
}

class View extends BaseView {
    override render() {
        return (
            <>
                <Badge label="hello" />
            </>
        );
    }
}
"#,
    )])?;

    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::OVERRIDE,
            "View.render",
            "BaseView.render"
        ),
        "expected TSX override edge to resolve View.render -> BaseView.render"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::CALL, "View.render", "Badge"),
        "expected View.render to retain Badge JSX call"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::USAGE, "View.render", "label"),
        "expected View.render to retain label prop usage"
    );

    Ok(())
}

#[test]
fn test_tsx_nested_fragments_and_parenthesized_usage_keep_component_edges() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.tsx",
        r#"
type Props = { label: string; tone: string };

function Badge(props: Props) {
    return <span>{props.label}{props.tone}</span>;
}

class View {
    render() {
        return (
            <>
                <section>
                    <>
                        <Badge label="hello" tone="warm" />
                    </>
                </section>
            </>
        );
    }
}
"#,
    )])?;

    assert!(
        edge_between(&nodes, &edges, EdgeKind::CALL, "View.render", "Badge"),
        "expected nested TSX usage to retain View.render -> Badge CALL"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::USAGE, "View.render", "label"),
        "expected nested TSX usage to retain View.render -> label"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::USAGE, "View.render", "tone"),
        "expected nested TSX usage to retain View.render -> tone"
    );

    Ok(())
}

#[test]
fn test_cpp_template_base_class_with_type_argument_does_not_duplicate_nodes() -> anyhow::Result<()>
{
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

#[test]
fn test_rust_impl_variants_normalize_to_terminal_type_identifiers() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.rs",
        r#"
struct Plain;

mod api {
    pub struct Scoped;
    pub struct Generic<T>(pub T);
    pub trait Runner<T> {
        fn run(&self);
    }
}

impl Plain {
    fn plain(&self) {}
}

impl crate::api::Scoped {
    fn scoped(&self) {}
}

impl api::Generic<u8> {
    fn generic(&self) {}
}

impl crate::api::Generic<u16> {
    fn scoped_generic(&self) {}
}

impl api::Runner<u32> for crate::api::Generic<u32> {
    fn run(&self) {}
}
"#,
    )])?;

    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::MEMBER,
        "Plain",
        "plain"
    ));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::MEMBER,
        "Scoped",
        "scoped"
    ));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::MEMBER,
        "Generic",
        "generic"
    ));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::MEMBER,
        "Generic",
        "scoped_generic"
    ));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::INHERITANCE,
        "Generic",
        "Runner"
    ));
    assert!(
        !nodes.iter().any(|node| {
            node.serialized_name.contains("crate::api::Scoped")
                || node.serialized_name.contains("crate::api::Generic<u16>")
                || node.serialized_name.contains("api::Runner<u32>")
        }),
        "expected impl surface names to normalize to terminal type identifiers"
    );

    Ok(())
}

#[test]
fn test_c_union_enum_constants_and_forward_function_declarations_surface() -> anyhow::Result<()> {
    let source = r#"
union Payload {
    int id;
};

enum State {
    Idle,
    Busy,
};

int forward(int value);
"#;
    let (nodes, edges) = index_project(&[("main.c", source)])?;

    assert!(has_node_kind(&nodes, "Payload", NodeKind::UNION));
    assert!(has_node_kind(&nodes, "State", NodeKind::ENUM));
    assert!(has_node_kind(&nodes, "Idle", NodeKind::ENUM_CONSTANT));
    assert!(has_node_kind(&nodes, "Busy", NodeKind::ENUM_CONSTANT));
    assert!(has_node_kind(&nodes, "forward", NodeKind::FUNCTION));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::MEMBER,
        "State",
        "Idle"
    ));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::MEMBER,
        "State",
        "Busy"
    ));

    let forward = find_node_by_name_and_kind(&nodes, "forward", NodeKind::FUNCTION)
        .ok_or_else(|| anyhow::anyhow!("expected forward declaration node"))?;
    assert_eq!(
        snippet_for_node(source, forward),
        Some("int forward(int value);"),
        "expected forward declaration node span to cover the declaration"
    );

    Ok(())
}

#[test]
fn test_cpp_using_directive_and_qualified_call_placeholders_are_precise() -> anyhow::Result<()> {
    let source = r#"
namespace ns {
    int make();
}

using namespace ns;

int run() {
    return ns::make();
}
"#;
    let (nodes, edges) = index_project(&[("main.cpp", source)])?;

    assert!(has_node_kind(&nodes, "ns", NodeKind::MODULE));
    assert!(edge_between(&nodes, &edges, EdgeKind::IMPORT, "ns", "ns"));

    let run_id = find_node_by_name_and_kind(&nodes, "run", NodeKind::FUNCTION)
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected run() node"))?;
    let make_placeholder = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == run_id)
        .filter_map(|edge| nodes.iter().find(|node| node.id == edge.target))
        .find(|node| matches_name(&node.serialized_name, "make"))
        .ok_or_else(|| anyhow::anyhow!("expected qualified CALL placeholder for make"))?;

    assert_eq!(
        snippet_for_node(source, make_placeholder),
        Some("make"),
        "expected qualified call placeholder span to cover only the terminal token"
    );

    Ok(())
}

#[test]
fn test_java_annotations_constructors_inner_types_and_enum_constants_surface() -> anyhow::Result<()>
{
    let source = r#"
@interface Audit {}

class Outer {
    @Audit
    Outer() {}

    class Inner {}

    enum Kind {
        FIRST
    }
}

record Holder(int value) {
    Holder {}
}
"#;
    let (nodes, edges) = index_project(&[("Main.java", source)])?;

    assert!(has_node_kind(&nodes, "Audit", NodeKind::ANNOTATION));
    assert!(has_node_kind(&nodes, "Outer", NodeKind::CLASS));
    assert!(has_node_kind(&nodes, "Outer", NodeKind::METHOD));
    assert!(has_node_kind(&nodes, "Inner", NodeKind::CLASS));
    assert!(has_node_kind(&nodes, "Kind", NodeKind::ENUM));
    assert!(has_node_kind(&nodes, "FIRST", NodeKind::ENUM_CONSTANT));
    assert!(has_node_kind(&nodes, "Holder", NodeKind::METHOD));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::MEMBER,
        "Outer",
        "Inner"
    ));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::MEMBER,
        "Kind",
        "FIRST"
    ));
    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::ANNOTATION_USAGE
                && nodes
                    .iter()
                    .find(|node| node.id == edge.target)
                    .is_some_and(|node| matches_name(&node.serialized_name, "Audit"))
        }),
        "expected annotation usage edge targeting Audit"
    );

    let outer_class = find_node_by_name_and_kind(&nodes, "Outer", NodeKind::CLASS)
        .ok_or_else(|| anyhow::anyhow!("expected Outer class node"))?;
    let outer_ctor = find_node_by_name_and_kind(&nodes, "Outer", NodeKind::METHOD)
        .ok_or_else(|| anyhow::anyhow!("expected Outer constructor node"))?;
    let audit_usage = nodes
        .iter()
        .find(|node| {
            node.kind == NodeKind::ANNOTATION
                && matches_name(&node.serialized_name, "Audit")
                && snippet_for_node(source, node) == Some("Audit")
        })
        .ok_or_else(|| anyhow::anyhow!("expected Audit annotation usage node"))?;

    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::MEMBER
                && edge.source == outer_class.id
                && edge.target == outer_ctor.id
        }),
        "expected Outer -> Outer constructor membership edge"
    );

    assert_eq!(
        snippet_for_node(source, outer_class),
        Some(
            "class Outer {\n    @Audit\n    Outer() {}\n\n    class Inner {}\n\n    enum Kind {\n        FIRST\n    }\n}"
        ),
        "expected Java declaration node span to cover the full class declaration"
    );
    assert_eq!(
        snippet_for_node(source, audit_usage),
        Some("Audit"),
        "expected Java annotation placeholder span to cover only the annotation token"
    );

    Ok(())
}
