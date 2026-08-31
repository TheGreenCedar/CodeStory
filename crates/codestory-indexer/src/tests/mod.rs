//! Indexer tests, split out of `lib.rs` verbatim.
//!
//! The module is a third of the file, and `lib.rs` was a few commits from
//! crossing the oversized-source cap it enforces on other repositories
//! (#1801). Nothing here changed but its location.
//!
//! It lives under a `tests/` directory rather than as a flat `tests.rs`
//! because the retrieval-generalization lint excludes any path with a
//! `tests` segment from every pass, while its corpus pass deliberately
//! still reads out-of-line `#[cfg(test)]` module bodies. As a flat file
//! this module's Go fixtures leaked `mux` into the derived corpus names
//! and silently stopped the hostile-literal ban firing for it.

use super::*;
use rusqlite::types::Value;
use std::collections::HashSet;
use tempfile::tempdir;

fn measured_go_method_identity_qualification_work(method_count: usize) -> usize {
    let mut nodes = HashMap::new();
    let mut roles = HashMap::new();
    let mut specs = Vec::new();
    for index in 0..method_count {
        let id = NodeId(i64::try_from(index + 1).expect("method id"));
        let line = u32::try_from(index + 1).expect("method line");
        nodes.insert(
            id,
            Node {
                id,
                kind: NodeKind::METHOD,
                serialized_name: format!("Method{index}"),
                start_line: Some(line),
                start_col: Some(1),
                end_line: Some(line),
                end_col: Some(20),
                ..Default::default()
            },
        );
        roles.insert(id, CanonicalNodeRole::Definition);
        specs.push(ManualMemberEdgeSpec {
            source_name: format!("Owner{index}"),
            target_name: format!("Method{index}"),
            source_span: GraphNodeSpan {
                start_line: line,
                start_col: 1,
                end_line: line,
                end_col: 5,
            },
            target_span: GraphNodeSpan {
                start_line: line,
                start_col: 1,
                end_line: line,
                end_col: 20,
            },
            line: Some(line),
        });
    }

    reset_go_method_identity_work();
    apply_go_receiver_method_identities("go", &mut nodes, &specs, &HashSet::new(), &roles);
    assert!(
        nodes
            .values()
            .all(|node| node.serialized_name.starts_with("Owner"))
    );
    go_method_identity_work()
}

#[test]
fn go_method_identity_qualification_work_is_linear() {
    let baseline = measured_go_method_identity_qualification_work(128);
    let doubled = measured_go_method_identity_qualification_work(256);
    assert!(baseline >= 256, "Go identity work was not fully counted");
    assert!(
        doubled <= baseline * 2 + 16,
        "Go identity qualification grew superlinearly: {baseline} -> {doubled}"
    );
}

#[test]
fn go_builtin_new_package_and_local_shadowing_matrix_is_closed() -> Result<()> {
    struct Case {
        name: &'static str,
        declarations: &'static str,
        expect_receiver_resolution: bool,
    }

    let cases = [
        Case {
            name: "direct package var",
            declarations: "var new func(int)\n",
            expect_receiver_resolution: false,
        },
        Case {
            name: "grouped package var",
            declarations: "var (\n  new func(int)\n)\n",
            expect_receiver_resolution: false,
        },
        Case {
            name: "direct package const",
            declarations: "const new = 1\n",
            expect_receiver_resolution: false,
        },
        Case {
            name: "grouped package const",
            declarations: "const (\n  new = 1\n)\n",
            expect_receiver_resolution: false,
        },
        Case {
            name: "direct package type",
            declarations: "type new int\n",
            expect_receiver_resolution: false,
        },
        Case {
            name: "grouped package type",
            declarations: "type (\n  new int\n)\n",
            expect_receiver_resolution: false,
        },
        Case {
            name: "unrelated package names",
            declarations: "var otherVar func(int)\nconst otherConst = 1\ntype otherType int\n",
            expect_receiver_resolution: true,
        },
        Case {
            name: "local new in unrelated callable",
            declarations: r#"
func unrelated() {
  { var new func(int); _ = new }
  { const new = 1; _ = new }
  { type new int; var _ new }
}
"#,
            expect_receiver_resolution: true,
        },
        Case {
            name: "local new in caller",
            declarations: "",
            expect_receiver_resolution: false,
        },
    ];

    for case in cases {
        let caller_shadow = if case.name == "local new in caller" {
            "  var new func(int)\n"
        } else {
            ""
        };
        let source = format!(
            r#"package proof

type node struct{{}}
func (*node) addRoute() {{}}

{}
func build() {{
{}  root := new(node)
  root.addRoute()
}}
"#,
            case.declarations, caller_shadow
        );
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .expect("Go parser language");
        let tree = parser.parse(&source, None).expect("Go syntax tree");
        assert!(
            !tree.root_node().has_error(),
            "case `{}` must be syntactically valid",
            case.name
        );

        let has_receiver_spec = languages::go::receiver_call_specs(&tree, &source)
            .iter()
            .any(|spec| {
                spec.source_name == "build"
                    && spec.owner_name == "node"
                    && spec.method_name == "addRoute"
            });
        assert_eq!(
            has_receiver_spec, case.expect_receiver_resolution,
            "case `{}` receiver-spec decision",
            case.name
        );

        let language_config = get_language_for_ext("go").expect("Go language config");
        let result = index_file(Path::new("main.go"), &source, &language_config, None, None)?;
        let nodes_by_id = result
            .nodes
            .iter()
            .map(|node| (node.id, node))
            .collect::<HashMap<_, _>>();
        let has_resolved_method_edge = result.edges.iter().any(|edge| {
            edge.kind == EdgeKind::CALL
                && edge
                    .resolved_target
                    .and_then(|target| nodes_by_id.get(&target))
                    .is_some_and(|target| {
                        target.serialized_name == "node.addRoute"
                            || target.serialized_name.ends_with(".node.addRoute")
                    })
        });
        assert_eq!(
            has_resolved_method_edge, case.expect_receiver_resolution,
            "case `{}` resolved-edge decision",
            case.name
        );
    }

    Ok(())
}

fn measured_manual_receiver_index_work(owner_count: usize, lookup_count: usize) -> usize {
    let file_id = NodeId(1);
    let mut nodes = HashMap::new();
    let mut edges = Vec::new();
    for index in 0..owner_count {
        let owner_id = NodeId(i64::try_from(index * 2 + 2).expect("owner id"));
        let target_id = NodeId(i64::try_from(index * 2 + 3).expect("target id"));
        nodes.insert(
            owner_id,
            Node {
                id: owner_id,
                kind: NodeKind::CLASS,
                serialized_name: format!("Owner{index}"),
                qualified_name: Some(format!("module.Owner{index}")),
                file_node_id: Some(file_id),
                start_line: Some(u32::try_from(index + 1).expect("owner line")),
                end_line: Some(u32::try_from(index + 2).expect("owner end line")),
                ..Default::default()
            },
        );
        nodes.insert(
            target_id,
            Node {
                id: target_id,
                kind: NodeKind::METHOD,
                serialized_name: "run".to_owned(),
                qualified_name: Some(format!("module.Owner{index}.run")),
                file_node_id: Some(file_id),
                start_line: Some(u32::try_from(index + 2).expect("method line")),
                ..Default::default()
            },
        );
        edges.push(Edge {
            source: owner_id,
            target: target_id,
            kind: EdgeKind::MEMBER,
            ..Default::default()
        });
    }
    reset_manual_receiver_lookup_work();
    let prepared = PreparedMemberTargetIndex::prepare(&nodes, &edges);
    for index in 0..lookup_count {
        let owner_index = index % owner_count;
        assert_eq!(
            prepared.target(&format!("Owner{owner_index}"), "run", file_id, false, None,),
            Some(NodeId(
                i64::try_from(owner_index * 2 + 3).expect("target id")
            ))
        );
    }
    manual_receiver_lookup_work()
}

#[test]
fn prepared_manual_receiver_members_and_lookups_are_independently_linear() {
    let baseline = measured_manual_receiver_index_work(64, 64);
    let more_members = measured_manual_receiver_index_work(128, 64);
    let more_lookups = measured_manual_receiver_index_work(64, 128);
    let combined = measured_manual_receiver_index_work(128, 128);
    assert!(baseline >= 64, "manual receiver work was not counted");
    assert!(
        more_members <= baseline * 2 + 64,
        "member preparation: {baseline} -> {more_members}"
    );
    assert!(
        more_lookups <= baseline * 2 + 64,
        "member lookups: {baseline} -> {more_lookups}"
    );
    assert!(
        combined <= baseline * 2 + 128,
        "combined work: {baseline} -> {combined}"
    );
}

fn measured_python_local_owner_line_work(owner_count: usize, lookup_count: usize) -> usize {
    let mut source = String::new();
    for index in 0..owner_count {
        source.push_str(&format!(
            "def caller_{index}():\n    class Owner{index}:\n        pass\n\n"
        ));
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("Python parser language");
    let tree = parser.parse(&source, None).expect("Python syntax tree");
    reset_manual_receiver_lookup_work();
    let prepared = PythonLocalOwnerLineIndex::prepare(&tree, &source);
    for index in 0..lookup_count {
        let owner_index = index % owner_count;
        assert!(
            prepared
                .unique_line(&format!("caller_{owner_index}.Owner{owner_index}"))
                .is_some()
        );
    }
    manual_receiver_lookup_work()
}

#[test]
fn python_local_owner_lines_are_prepared_once_and_looked_up_linearly() {
    let baseline = measured_python_local_owner_line_work(64, 64);
    let more_owners = measured_python_local_owner_line_work(128, 64);
    let more_lookups = measured_python_local_owner_line_work(64, 128);
    let combined = measured_python_local_owner_line_work(128, 128);
    assert!(baseline >= 64, "Python owner-line work was not counted");
    assert!(
        more_owners <= baseline * 2 + 128,
        "owner preparation: {baseline} -> {more_owners}"
    );
    assert!(
        more_lookups <= baseline * 2 + 64,
        "owner lookups: {baseline} -> {more_lookups}"
    );
    assert!(
        combined <= baseline * 2 + 192,
        "combined work: {baseline} -> {combined}"
    );
}

/// A file whose only structural node is a position-derived one: a shape
/// several collectors produce today, because `structural_node_id` mixes the
/// declaration's line and column into the id.
fn projection_for_positioned_structural_node(
    structural_id: i64,
    structural_line: u32,
    callable_start: u32,
) -> Vec<CallableProjectionState> {
    let file_id = NodeId(1);
    let nodes = vec![
        Node {
            id: file_id,
            kind: NodeKind::FILE,
            serialized_name: "app.sql".to_string(),
            qualified_name: Some("app.sql".to_string()),
            start_line: Some(1),
            start_col: Some(1),
            end_line: Some(40),
            end_col: Some(1),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "run".to_string(),
            qualified_name: Some("run".to_string()),
            file_node_id: Some(file_id),
            start_line: Some(callable_start),
            start_col: Some(1),
            end_line: Some(callable_start + 4),
            end_col: Some(1),
            ..Default::default()
        },
        Node {
            id: NodeId(structural_id),
            kind: NodeKind::CLASS,
            serialized_name: "public.users".to_string(),
            qualified_name: Some("public.users".to_string()),
            file_node_id: Some(file_id),
            start_line: Some(structural_line),
            start_col: Some(1),
            end_line: Some(structural_line),
            end_col: Some(12),
            ..Default::default()
        },
    ];
    // The structural node's only occurrence sits inside the callable, so
    // the occurrence fence cannot see it move; node identity is the sole
    // remaining signal that the row was replaced rather than repositioned.
    let occurrences = vec![Occurrence {
        element_id: structural_id,
        kind: OccurrenceKind::DEFINITION,
        location: SourceLocation {
            file_node_id: file_id,
            start_line: callable_start + 1,
            start_col: 1,
            end_line: callable_start + 1,
            end_col: 12,
        },
    }];
    build_callable_projection_states(&nodes, &[], &occurrences)
}

#[test]
fn a_replaced_node_identity_forces_a_full_replacement() {
    let before = projection_for_positioned_structural_node(30, 3, 10);
    let unchanged = projection_for_positioned_structural_node(30, 3, 10);
    assert!(
        matches!(
            classify_projection_update(&before, &unchanged),
            ProjectionUpdateMode::NoChanges
        ),
        "an unchanged file must not be reprojected"
    );

    // Same kind, same qualified name, same everything a caller-scoped
    // repair would rewrite — only the id differs, which is what a
    // position-derived collector emits after a shift. The old row is
    // keyed by the old id and nothing would ever delete it.
    let reidentified = projection_for_positioned_structural_node(31, 3, 10);
    assert!(
        matches!(
            classify_projection_update(&before, &reidentified),
            ProjectionUpdateMode::FullReplace
        ),
        "a node that changed identity must not be repaired incrementally: {:?}",
        classify_projection_update(&before, &reidentified)
    );
}

/// A callable that projects nothing: no outgoing edges, no body
/// occurrences. Only its own extent distinguishes one position from
/// another, and the stored row's `start_line`/`end_line` are what the
/// occurrence cleanup later reads to decide what a delta may delete.
fn projection_for_empty_stub(start_line: u32) -> Vec<CallableProjectionState> {
    let file_id = NodeId(1);
    let nodes = vec![
        Node {
            id: file_id,
            kind: NodeKind::FILE,
            serialized_name: "app.rs".to_string(),
            qualified_name: Some("app.rs".to_string()),
            start_line: Some(1),
            start_col: Some(1),
            end_line: Some(40),
            end_col: Some(1),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "stub".to_string(),
            qualified_name: Some("stub".to_string()),
            file_node_id: Some(file_id),
            start_line: Some(start_line),
            start_col: Some(1),
            end_line: Some(start_line + 1),
            end_col: Some(1),
            ..Default::default()
        },
    ];
    build_callable_projection_states(&nodes, &[], &[])
}

#[test]
fn a_moved_stub_is_still_reprojected() {
    let before = projection_for_empty_stub(10);
    assert!(
        matches!(
            classify_projection_update(&before, &projection_for_empty_stub(10)),
            ProjectionUpdateMode::NoChanges
        ),
        "an unchanged stub must not be reprojected"
    );
    let mode = classify_projection_update(&before, &projection_for_empty_stub(12));
    assert!(
        matches!(mode, ProjectionUpdateMode::Delta { ref changed_callers }
                if changed_callers == &vec![NodeId(2)]),
        "a stub that moved must still have its stored extent rewritten, got {mode:?}"
    );
}

#[test]
fn a_callable_that_only_moved_is_repaired_in_place() {
    let before = projection_for_positioned_structural_node(30, 3, 10);
    let moved = projection_for_positioned_structural_node(30, 3, 12);
    let mode = classify_projection_update(&before, &moved);
    assert!(
        matches!(mode, ProjectionUpdateMode::Delta { ref changed_callers }
                if changed_callers == &vec![NodeId(2)]),
        "a callable that only moved must be repaired caller-scoped, got {mode:?}"
    );
}

fn projection_snapshot(storage: &Storage) -> Result<Vec<(String, Vec<Vec<Value>>)>> {
    const QUERIES: [(&str, &str); 8] = [
        ("file", "SELECT * FROM file ORDER BY id"),
        ("node", "SELECT * FROM node ORDER BY id"),
        ("edge", "SELECT * FROM edge ORDER BY id"),
        (
            "occurrence",
            "SELECT * FROM occurrence ORDER BY element_id, kind, file_node_id, start_line, start_col, end_line, end_col",
        ),
        (
            "component_access",
            "SELECT * FROM component_access ORDER BY node_id, type",
        ),
        ("error", "SELECT * FROM error ORDER BY id"),
        (
            "callable_projection_state",
            "SELECT * FROM callable_projection_state ORDER BY file_id, symbol_key",
        ),
        (
            "index_artifact_cache",
            "SELECT file_path, cache_key FROM index_artifact_cache ORDER BY file_path",
        ),
    ];
    let mut snapshot = Vec::with_capacity(QUERIES.len());
    for (name, query) in QUERIES {
        let mut statement = storage.get_connection().prepare(query)?;
        let column_count = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..column_count)
                    .map(|column| row.get::<_, Value>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        snapshot.push((name.to_string(), rows));
    }
    Ok(snapshot)
}

fn assert_projection_snapshots_equal(
    expected: &Storage,
    actual: &Storage,
    actual_label: &str,
) -> Result<()> {
    for ((expected_table, expected_rows), (actual_table, actual_rows)) in
        projection_snapshot(expected)?
            .into_iter()
            .zip(projection_snapshot(actual)?)
    {
        assert_eq!(expected_table, actual_table);
        assert_eq!(
            expected_rows, actual_rows,
            "serial and {actual_label} {expected_table} projections differ"
        );
    }
    Ok(())
}

fn overwrite_preserving_mtime(path: &Path, source: &str) -> Result<()> {
    let modified = std::fs::metadata(path)?.modified()?;
    std::fs::write(path, source)?;
    std::fs::File::options()
        .write(true)
        .open(path)?
        .set_times(std::fs::FileTimes::new().set_modified(modified))?;
    assert_eq!(std::fs::metadata(path)?.modified()?, modified);
    Ok(())
}

#[derive(Debug)]
struct RawGraphContract {
    nodes: HashSet<(String, String)>,
    edges: HashSet<(String, String, String)>,
    call_counts: HashMap<(String, Option<String>), usize>,
    has_parse_error: bool,
}

fn execute_raw_graph_contract(
    path: &Path,
    source: &str,
    language_config: &LanguageConfig,
) -> Result<RawGraphContract> {
    let mut parser = Parser::new();
    parser
        .set_language(&language_config.language)
        .map_err(|e| anyhow!("parser language error: {e}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("parser did not produce a tree"))?;
    let has_parse_error = tree.root_node().has_error();
    let variables = Variables::new();
    let functions = Functions::stdlib();
    let config = ExecutionConfig::new(&functions, &variables)
        .lazy(index_feature_flags().lazy_graph_execution);
    let graph = language_config
        .compiled_rules()?
        .graph_file
        .execute(&tree, source, &config, &NoCancellation)
        .map_err(|e| anyhow!("Graph execution error: {:?}", e))?;

    let mut node_names = HashMap::new();
    let mut nodes = HashSet::new();
    for node_id in graph.iter_nodes() {
        let node_data = &graph[node_id];
        let mut kind = None;
        let mut name = None;
        for (attr, val) in node_data.attributes.iter() {
            match attr.as_str() {
                "kind" => kind = val.as_str().ok().map(str::to_string),
                "name" => name = val.as_str().ok().map(str::to_string),
                _ => {}
            }
        }
        let (Some(kind), Some(name)) = (kind, name) else {
            continue;
        };
        node_names.insert(node_id, name.clone());
        nodes.insert((kind, name));
    }

    let mut edges = HashSet::new();
    let mut call_counts = HashMap::new();
    for source_ref in graph.iter_nodes() {
        let Some(source_name) = node_names.get(&source_ref).cloned() else {
            continue;
        };
        let graph_node = &graph[source_ref];
        for (target_ref, edge) in graph_node.iter_edges() {
            let Some(target_name) = node_names.get(&target_ref).cloned() else {
                continue;
            };
            let mut kind = None;
            let mut call_syntax = None;
            for (attr, val) in edge.attributes.iter() {
                match attr.as_str() {
                    "kind" => kind = val.as_str().ok().map(str::to_string),
                    "call_syntax" => {
                        call_syntax = val.as_str().ok().map(str::to_string);
                    }
                    _ => {}
                }
            }
            let Some(kind) = kind else {
                continue;
            };
            if kind == "CALL" {
                *call_counts
                    .entry((target_name.clone(), call_syntax))
                    .or_insert(0) += 1;
            }
            edges.insert((source_name.clone(), target_name, kind));
        }
    }

    let _ = path;
    Ok(RawGraphContract {
        nodes,
        edges,
        call_counts,
        has_parse_error,
    })
}

fn parser_node_kinds(language: Language) -> HashSet<String> {
    (0..language.node_kind_count())
        .filter_map(|id| language.node_kind_for_id(id as u16))
        .map(str::to_string)
        .collect()
}

#[test]
fn test_index_python_semantics() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    let python_code = r#"
class Parent:
    pass

class MyClass(Parent):
    def my_method(self):
        pass
"#;
    let language_config = get_language_for_ext("py").unwrap();

    let result = index_file(
        Path::new("test.py"),
        python_code,
        &language_config,
        None,
        None,
    )?;

    assert!(
        result.edges.iter().any(|e| e.kind == EdgeKind::MEMBER),
        "MEMBER edge not found"
    );
    assert!(
        result.edges.iter().any(|e| {
            e.kind == EdgeKind::INHERITANCE && e.certainty == Some(ResolutionCertainty::Certain)
        }),
        "certain INHERITANCE edge not found"
    );
    assert!(!result.occurrences.is_empty(), "No occurrences found");

    Ok(())
}

#[test]
fn test_index_java_semantics() -> Result<()> {
    let java_code = r#"
class Parent {}
class MyClass extends Parent {
    void myMethod() {}
}
"#;
    let language_config = get_language_for_ext("java").unwrap();

    let result = index_file(
        Path::new("Test.java"),
        java_code,
        &language_config,
        None,
        None,
    )?;

    assert!(
        result.edges.iter().any(|e| e.kind == EdgeKind::MEMBER),
        "MEMBER edge not found"
    );
    assert!(
        result.edges.iter().any(|e| {
            e.kind == EdgeKind::INHERITANCE && e.certainty == Some(ResolutionCertainty::Certain)
        }),
        "certain INHERITANCE edge not found"
    );
    Ok(())
}

#[test]
fn test_index_rust_semantics() -> Result<()> {
    let rust_code = r#"
struct MyStruct { field: i32 }
impl MyStruct {
    fn my_fn(&self) {}
}
"#;
    let language_config = get_language_for_ext("rs").unwrap();

    let result = index_file(
        Path::new("main.rs"),
        rust_code,
        &language_config,
        None,
        None,
    )?;

    assert!(
        result.edges.iter().any(|e| e.kind == EdgeKind::MEMBER),
        "MEMBER edge not found"
    );
    Ok(())
}

#[test]
fn test_rust_type_anchor_prefers_declaration_over_impl_anchor() -> Result<()> {
    let rust_code = r#"
pub struct AppController;

impl Default for AppController {
    fn default() -> Self {
        Self
    }
}

impl AppController {
    fn open_project(&self) {}
}
"#;
    let language_config = get_language_for_ext("rs").unwrap();

    let result = index_file(
        Path::new("main.rs"),
        rust_code,
        &language_config,
        None,
        None,
    )?;

    let matching = result
        .nodes
        .iter()
        .filter(|node| node.serialized_name == "AppController")
        .collect::<Vec<_>>();
    assert_eq!(
        matching.len(),
        1,
        "expected one canonical AppController node"
    );

    let type_node = matching[0];
    assert_eq!(type_node.kind, NodeKind::STRUCT);
    assert_eq!(type_node.start_line, Some(2));

    let open_project = result
        .nodes
        .iter()
        .find(|node| node.serialized_name.ends_with("open_project"))
        .expect("open_project method");
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::MEMBER
            && edge.source == type_node.id
            && edge.target == open_project.id
            && edge.certainty == Some(ResolutionCertainty::Certain)
    }));

    Ok(())
}

#[test]
fn test_rust_impl_queries_normalize_plain_scoped_and_generic_type_expressions() -> Result<()> {
    let rust_code = r#"
mod api {
    pub trait Runner {}
}

struct Plain;
struct Generic<T>(T);

mod nested {
    pub struct Scoped;
    pub struct ScopedGeneric<T>(pub T);
}

impl Plain {
    fn plain(&self) {}
}

impl<T> Generic<T> {
    fn generic(&self) {}
}

impl nested::Scoped {
    fn scoped(&self) {}
}

impl<T> nested::ScopedGeneric<T> {
    fn scoped_generic(&self) {}
}

impl api::Runner for nested::ScopedGeneric<String> {}
"#;
    let language_config = get_language_for_ext("rs").unwrap();

    let result = index_file(
        Path::new("main.rs"),
        rust_code,
        &language_config,
        None,
        None,
    )?;

    for (type_name, method_name) in [
        ("Plain", "plain"),
        ("Generic", "generic"),
        ("Scoped", "scoped"),
        ("ScopedGeneric", "scoped_generic"),
    ] {
        let matching = result
            .nodes
            .iter()
            .filter(|node| {
                short_member_name(&node.serialized_name) == type_name
                    && node.kind == NodeKind::STRUCT
            })
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 1, "expected one canonical {type_name} node");

        let method = result
            .nodes
            .iter()
            .find(|node| short_member_name(&node.serialized_name) == method_name)
            .expect("expected impl method node");
        assert!(result.edges.iter().any(|edge| {
            edge.kind == EdgeKind::MEMBER
                && edge.source == matching[0].id
                && edge.target == method.id
                && edge.certainty == Some(ResolutionCertainty::Certain)
        }));
    }

    let runner = result
        .nodes
        .iter()
        .find(|node| node.serialized_name.ends_with("Runner"))
        .expect("expected Runner node");
    let scoped_generic = result
        .nodes
        .iter()
        .find(|node| {
            node.serialized_name.ends_with("ScopedGeneric") && node.kind == NodeKind::STRUCT
        })
        .expect("expected ScopedGeneric node");
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::INHERITANCE
            && edge.source == scoped_generic.id
            && edge.target == runner.id
            && edge.certainty == Some(ResolutionCertainty::Certain)
    }));

    Ok(())
}

#[test]
fn test_runtime_import_edges_bind_to_the_exact_shadowed_binding() -> Result<()> {
    let js_code = r#"
const pkg = "outer";

function load() {
    const pkg = require("./pkg.js");
    return pkg;
}
"#;
    let language_config = get_language_for_ext("js").unwrap();
    let result = index_file(Path::new("main.js"), js_code, &language_config, None, None)?;

    let pkg_module = result
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::MODULE && node.serialized_name == "\"./pkg.js\"")
        .expect("pkg module node");
    let shadowed_pkg = result
        .nodes
        .iter()
        .find(|node| {
            node.kind == NodeKind::UNKNOWN
                && node.serialized_name == "pkg"
                && node.start_line == Some(5)
        })
        .expect("shadowed runtime import binding");

    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::IMPORT
            && edge.source == shadowed_pkg.id
            && edge.target == pkg_module.id
    }));
    assert!(!result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::IMPORT
            && edge.target == pkg_module.id
            && edge.source != shadowed_pkg.id
    }));

    Ok(())
}

#[test]
fn test_rust_impl_query_simplification_keeps_terminal_type_names() -> Result<()> {
    let rust_code = r#"
mod outer {
    pub struct Thing<T>(pub T);
}

trait Runner {
    fn run(&self);
}

impl Runner for outer::Thing<String> {
    fn run(&self) {}
}

impl outer::Thing<String> {
    fn open(&self) {}
}
"#;
    let language_config = get_language_for_ext("rs").unwrap();

    let result = index_file(
        Path::new("main.rs"),
        rust_code,
        &language_config,
        None,
        None,
    )?;

    let thing_nodes = result
        .nodes
        .iter()
        .filter(|node| node.serialized_name.ends_with("Thing") && node.kind == NodeKind::STRUCT)
        .collect::<Vec<_>>();
    assert_eq!(
        thing_nodes.len(),
        1,
        "expected impl captures to normalize scoped generic type expressions to Thing"
    );
    assert_eq!(thing_nodes[0].kind, NodeKind::STRUCT);

    let runner = result
        .nodes
        .iter()
        .find(|node| node.serialized_name.ends_with("Runner"))
        .expect("Runner trait");
    let open = result
        .nodes
        .iter()
        .find(|node| node.serialized_name.ends_with("open"))
        .expect("open method");

    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::INHERITANCE
            && edge.source == thing_nodes[0].id
            && edge.target == runner.id
    }));
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::MEMBER && edge.source == thing_nodes[0].id && edge.target == open.id
    }));

    Ok(())
}

#[test]
fn test_normalize_graph_capture_for_rust_impl_expr_uses_terminal_identifier_span() {
    let source = "impl crate::api::Worker<T> {\n    fn run(&self) {}\n}\n";
    let raw = "crate::api::Worker<T>";
    let raw_start = source.find(raw).expect("raw impl type span");
    let raw_end = raw_start + raw.len();
    let worker_start = source.find("Worker").expect("terminal identifier start");
    let worker_end = worker_start + "Worker".len();
    let (start_line, start_col) =
        byte_offset_to_line_col(source, raw_start).expect("raw start location");
    let (end_line, end_col) = byte_offset_to_line_col(source, raw_end).expect("raw end location");
    let (worker_line, worker_col) =
        byte_offset_to_line_col(source, worker_start).expect("worker start location");
    let (worker_end_line, worker_end_col) =
        byte_offset_to_line_col(source, worker_end).expect("worker end location");

    let normalized = normalize_graph_capture(&GraphCaptureNormalizationInput {
        language_name: "rust",
        kind: NodeKind::CLASS,
        canonical_role: CanonicalNodeRole::ImplAnchor,
        rust_impl_expr: true,
        name: raw,
        graph_span: GraphNodeSpan {
            start_line,
            start_col,
            end_line,
            end_col,
        },
        source,
        has_token_surface_edge: false,
    })
    .expect("normalized Rust impl expression");

    assert_eq!(normalized.0, "Worker");
    assert_eq!(normalized.1, worker_line);
    assert_eq!(normalized.2, worker_col);
    assert_eq!(normalized.3, worker_end_line);
    assert_eq!(normalized.4, worker_end_col);
}

#[test]
fn test_index_cpp_semantics() -> Result<()> {
    let cpp_code = r#"
class MyClass {
    void myMethod() {}
};
"#;
    let language_config = get_language_for_ext("cpp").unwrap();

    let result = index_file(
        Path::new("test.cpp"),
        cpp_code,
        &language_config,
        None,
        None,
    )?;

    assert!(
        result.edges.iter().any(|e| e.kind == EdgeKind::MEMBER),
        "MEMBER edge not found"
    );
    Ok(())
}

#[test]
fn test_index_typescript_semantics() -> Result<()> {
    let ts_code = r#"
class MyClass {
    myMethod() {}
}
function globalFunc() {}
export const Posts = {
    slug: "posts",
    access: {
        read: () => true,
    },
    fields: [],
    hooks: {},
};
export const contentBlocks = [];
export default buildConfig({
    collections: [Posts],
});
"#;
    let language_config = get_language_for_ext("ts").unwrap();

    let result = index_file(Path::new("test.ts"), ts_code, &language_config, None, None)?;

    // Find MyClass
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.serialized_name == "MyClass" && n.kind == NodeKind::CLASS)
    );
    // Find globalFunc
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.serialized_name == "globalFunc" && n.kind == NodeKind::FUNCTION)
    );
    assert!(
        result
            .nodes
            .iter()
            .any(|n| { n.serialized_name == "Posts" && n.kind == NodeKind::GLOBAL_VARIABLE }),
        "exported object config should be indexed as a global variable"
    );
    for field_name in ["Posts.slug", "Posts.access", "Posts.fields", "Posts.hooks"] {
        assert!(
            result.nodes.iter().any(|node| {
                node.qualified_name.as_deref() == Some(field_name) && node.kind == NodeKind::FIELD
            }),
            "exported object config should index top-level field {field_name}"
        );
    }
    let posts_id = result
        .nodes
        .iter()
        .find(|node| node.serialized_name == "Posts" && node.kind == NodeKind::GLOBAL_VARIABLE)
        .expect("posts node")
        .id;
    let field_ids = result
        .nodes
        .iter()
        .filter(|node| {
            node.kind == NodeKind::FIELD
                && node
                    .qualified_name
                    .as_deref()
                    .is_some_and(|name| name.starts_with("Posts."))
        })
        .map(|node| node.id)
        .collect::<HashSet<_>>();
    assert!(
        result.edges.iter().any(|edge| {
            edge.kind == EdgeKind::MEMBER
                && edge.source == posts_id
                && field_ids.contains(&edge.target)
                && edge.certainty == Some(ResolutionCertainty::Certain)
        }),
        "exported object config fields should be connected to their owner"
    );
    assert!(
        result.nodes.iter().any(|n| {
            n.serialized_name == "contentBlocks" && n.kind == NodeKind::GLOBAL_VARIABLE
        }),
        "exported array config should be indexed as a global variable"
    );
    assert!(
        result
            .nodes
            .iter()
            .any(|n| { n.serialized_name == "buildConfig" && n.kind == NodeKind::GLOBAL_VARIABLE }),
        "default-exported config factory calls should be indexed as global variables"
    );

    // Assert Edge Creation (MEMBER)
    // Note: The original query for TS likely failed to match class name which is type_identifier
    assert!(
        result.edges.iter().any(|e| e.kind == EdgeKind::MEMBER),
        "MEMBER edge not found in TypeScript index result"
    );

    Ok(())
}

#[test]
fn test_header_language_defaults_to_c_and_can_upgrade_to_cpp_from_compile_info() {
    let default_config = get_language_for_ext("h").expect("header extension should resolve");
    assert_eq!(default_config.language_name, "c");

    let cpp_info = compilation_database::CompilationInfo {
        standard: Some(compilation_database::CxxStandard::Cxx20),
        ..Default::default()
    };
    let config = get_language_config_for_path(Path::new("widget.h"), Some(&cpp_info))
        .expect("path-based header config should resolve");
    assert_eq!(config.language_name, "cpp");
}

#[test]
fn test_header_source_signals_can_upgrade_c_header_to_cpp() {
    let c_header = r#"
#ifndef COUNT_H
#define COUNT_H
typedef struct Counter Counter;
void counter_increment(Counter* counter);
#endif
"#;
    assert!(!header_source_has_cpp_signals(c_header));

    let cpp_header = r#"
#ifndef STORAGE_ACCESS_H
#define STORAGE_ACCESS_H
class Graph;
class StorageAccess {
public:
    virtual std::shared_ptr<Graph> getGraphForAll() const = 0;
};
#endif
"#;
    assert!(header_source_has_cpp_signals(cpp_header));

    let base_config = get_language_for_ext("h").expect("header extension should resolve");
    let upgraded = maybe_upgrade_header_language_from_source(
        Path::new("StorageAccess.h"),
        cpp_header,
        &base_config,
    )
    .expect("C++ header signals should upgrade parser");
    assert_eq!(upgraded.language_name, "cpp");
}

#[test]
fn test_file_completeness_tracks_parse_errors() -> Result<()> {
    let language_config = get_language_for_ext("rs").unwrap();
    let result = index_file(
        Path::new("broken.rs"),
        "fn broken( {",
        &language_config,
        None,
        None,
    )?;

    assert_eq!(result.files.len(), 1);
    assert!(
        !result.files[0].complete,
        "malformed Rust source should be incomplete"
    );
    Ok(())
}

#[test]
fn test_file_scoped_errors_share_projection_transaction() -> Result<()> {
    use codestory_store::Store as Storage;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let broken = dir.path().join("broken.rs");
    std::fs::write(&broken, "fn broken( {\n")?;
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![broken],
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::new_in_memory()?;
    let stats = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_file_byte_cap(8)
        .run(&mut storage, &plan, &EventBus::new(), None)?;

    let errors = storage.get_errors(None)?;
    assert!(!errors.is_empty());
    assert!(errors.iter().all(|error| error.file_id.is_some()));
    assert_eq!(stats.projection_batch_transactions, 1);
    assert_eq!(stats.projection_persistence.transactions, 1);
    assert_eq!(
        stats
            .projection_persistence
            .file_errors
            .statement_executions,
        1 + errors.len() as u64
    );
    assert_eq!(
        stats.projection_persistence.file_errors.row_attempts,
        1 + errors.len() as u64
    );
    Ok(())
}

#[test]
fn test_cached_projection_failures_without_file_rows_remain_file_outcomes() -> Result<()> {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

    let dir = tempdir()?;
    let relative_path = PathBuf::from("cached.rs");
    let full_path = dir.path().join(&relative_path);
    std::fs::write(&full_path, "pub fn cached_projection() {}\n")?;
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![full_path.clone()],
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let mut storage = Storage::open_build(dir.path().join("cached.sqlite"))?;
    indexer.run(&mut storage, &plan, &EventBus::new(), None)?;
    let file_id = storage
        .get_file_by_path(&full_path)?
        .expect("cached file row")
        .id;
    let existing_projection_file_ids = HashSet::from([file_id]);

    for (failure, expected_message) in [
        ("metadata", "Failed to refresh cached file metadata"),
        ("error-clear", "Failed to replace cached file errors"),
    ] {
        let deny_metadata = failure == "metadata";
        storage
            .get_connection()
            .authorizer(Some(move |context: AuthContext<'_>| {
                let denied = match context.action {
                    AuthAction::Update { table_name, .. } => deny_metadata && table_name == "file",
                    AuthAction::Delete { table_name } => !deny_metadata && table_name == "error",
                    _ => false,
                };
                if denied {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }))?;
        let error_only_storage = {
            let mut stats = IncrementalIndexingStats::default();
            let symbol_table = Arc::new(SymbolTable::new());
            let mut cache_access =
                ArtifactCacheAccess::storage(&mut storage, ArtifactCachePolicies::default());
            match indexer.prepare_index_work(
                &mut cache_access,
                &relative_path,
                dir.path(),
                Some(file_id),
                &symbol_table,
                &mut stats,
            ) {
                Err(storage) => storage,
                Ok(_) => panic!("injected cached projection write failure was ignored"),
            }
        };
        storage
            .get_connection()
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;

        assert!(error_only_storage.files.is_empty(), "{failure}");
        assert_eq!(error_only_storage.errors.len(), 1, "{failure}");
        assert_eq!(
            error_only_storage.errors[0].file_id,
            Some(NodeId(file_id)),
            "{failure}"
        );
        assert!(
            error_only_storage.errors[0]
                .message
                .contains(expected_message),
            "{failure}: {}",
            error_only_storage.errors[0].message
        );

        let mut writer = ProjectionWriter::new(
            &mut storage,
            codestory_workspace::BuildMode::Incremental,
            IncrementalIndexingConfig::default(),
            &existing_projection_file_ids,
            false,
        );
        writer.accept_storage(error_only_storage)?;
        let output = writer.finish()?;
        assert!(output.all_errors.is_empty(), "{failure}");
        assert_eq!(output.stats.projection_batch_transactions, 0, "{failure}");

        let errors = storage.get_errors(None)?;
        assert_eq!(errors.len(), 1, "{failure}");
        assert_eq!(errors[0].file_id, Some(NodeId(file_id)), "{failure}");
        assert!(
            errors[0].message.contains(expected_message),
            "{failure}: {}",
            errors[0].message
        );
        assert!(
            storage
                .get_nodes()?
                .iter()
                .any(|node| node.serialized_name == "cached_projection"),
            "{failure} must preserve the existing projection"
        );
    }
    Ok(())
}

#[test]
fn test_rust_2024_constructs_are_complete() -> Result<()> {
    let language_config = get_language_for_ext("rs").unwrap();
    let result = index_file(
        Path::new("rust_2024.rs"),
        r#"
unsafe extern "C" {
    fn foreign(value: i32) -> i32;
}

fn checked_foreign(value: Option<i32>) -> Option<i32> {
    let Some(value) = value else {
        return None;
    };
    if let Some(next) = value.checked_add(1)
        && next > 0
    {
        Some(unsafe { foreign(next) })
    } else {
        None
    }
}
"#,
        &language_config,
        None,
        None,
    )?;

    assert_eq!(result.files.len(), 1);
    assert!(
        result.files[0].complete,
        "valid Rust 2024 source should be parser-complete"
    );
    Ok(())
}

#[test]
fn test_incremental_indexing() -> Result<()> {
    use codestory_store::Store as Storage;
    use codestory_workspace::RefreshInfo;
    use std::fs;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    let dir = tempdir()?;
    let f1 = dir.path().join("main.rs");
    let source = r#"
            struct Foo { x: i32 }
            fn bar() {}
        "#;
    fs::write(&f1, source)?;

    let mut storage = Storage::new_in_memory().unwrap();
    let bus = EventBus::new();
    let rx = bus.receiver();
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());

    // Create RefreshInfo manually
    let refresh_info = RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![f1.clone()],
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    let first_stats = indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;
    storage.put_resolution_support_snapshot(7_001, b"sealed graph support")?;
    let reused_stats = indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;

    assert_eq!(first_stats.parser_artifact_cache.misses, 1);
    assert!(first_stats.graph_projection_changed);
    assert_eq!(
        reused_stats.parser_artifact_cache.policy,
        ArtifactCachePolicy::ReadThrough
    );
    assert_eq!(reused_stats.parser_artifact_cache.logical_lookups, 1);
    assert_eq!(reused_stats.parser_artifact_cache.physical_queries, 1);
    assert_eq!(reused_stats.parser_artifact_cache.hits, 1);
    assert_eq!(reused_stats.parser_artifact_cache.misses, 0);
    assert_eq!(reused_stats.parser_artifact_cache.reader_opens, 0);
    assert!(!reused_stats.graph_projection_changed);
    assert_eq!(reused_stats.source_identity_only_files, 1);
    assert!(
        !reused_stats.resolution_ran,
        "an unchanged graph cannot require a global resolution pass"
    );
    assert_eq!(reused_stats.flush_nodes_ms, 0);
    assert_eq!(reused_stats.flush_edges_ms, 0);
    assert_eq!(
        storage.get_resolution_support_snapshot(7_001)?,
        Some(b"sealed graph support".to_vec()),
        "source-identity-only persistence must retain graph-derived support"
    );

    let file_id = WorkspaceIndexer::canonical_file_node_id_for_path(&f1);
    assert_eq!(
        storage.get_file_content_hash(file_id)?.as_deref(),
        Some(source_content_hash(source.as_bytes()).as_str())
    );
    assert_eq!(reused_stats.artifact_cache_hits, 1);

    let graph_before_lf = (storage.get_nodes()?.len(), storage.get_edges()?.len());
    let source_with_appended_lf = format!("{source}\n");
    fs::write(&f1, &source_with_appended_lf)?;
    let appended_lf_stats = indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;
    assert!(!appended_lf_stats.graph_projection_changed);
    assert_eq!(appended_lf_stats.source_identity_only_files, 1);
    assert!(
        !appended_lf_stats.resolution_ran,
        "source-identity-only refresh must retain resolved edges as-is"
    );
    assert_eq!(
        (storage.get_nodes()?.len(), storage.get_edges()?.len()),
        graph_before_lf,
        "an appended line feed cannot rewrite an equivalent graph"
    );
    assert_eq!(
        storage.get_file_content_hash(file_id)?.as_deref(),
        Some(source_content_hash(source_with_appended_lf.as_bytes()).as_str())
    );

    // Check verification
    let nodes = storage.get_nodes().unwrap();
    assert!(
        nodes
            .iter()
            .any(|n| n.serialized_name == "Foo" && n.kind == NodeKind::STRUCT)
    );
    assert!(
        nodes
            .iter()
            .any(|n| n.serialized_name == "bar" && n.kind == NodeKind::FUNCTION)
    );

    // Check progress events with a short timeout to avoid race with async fan-out thread.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut saw_started = false;
    let mut saw_complete = false;
    while Instant::now() < deadline && progress_events_still_pending(saw_started, saw_complete) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::IndexingStarted { .. }) => saw_started = true,
            Ok(Event::IndexingComplete { .. }) => saw_complete = true,
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(saw_started, "expected IndexingStarted event");
    assert!(saw_complete, "expected IndexingComplete event");

    Ok(())
}

#[test]
fn incremental_incomplete_result_preserves_previous_projection() -> Result<()> {
    use codestory_workspace::RefreshInfo;

    let dir = tempdir()?;
    let path = dir.path().join("preserved.rs");
    std::fs::write(&path, "pub fn preserved_symbol() -> i32 { 7 }\n")?;
    let refresh = RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![path.clone()],
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::new_in_memory()?;
    let bus = EventBus::new();

    WorkspaceIndexer::new(dir.path().to_path_buf()).run_incremental(
        &mut storage,
        &refresh,
        &bus,
        None,
    )?;
    let preserved_id = storage
        .get_nodes()?
        .into_iter()
        .find(|node| node.serialized_name == "preserved_symbol")
        .map(|node| node.id)
        .expect("initial verified projection");

    std::fs::write(
        &path,
        "pub fn preserved_symbol() -> i32 { 8 }\n// force an oversized retry\n",
    )?;
    WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_file_byte_cap(1)
        .run_incremental(&mut storage, &refresh, &bus, None)?;

    assert!(
        storage
            .get_nodes()?
            .iter()
            .any(|node| node.id == preserved_id),
        "an incomplete retry must retain the last verified graph projection"
    );
    let file = storage
        .get_file_by_path(&path)?
        .expect("incomplete file metadata");
    assert!(
        !file.complete,
        "the retained projection must still request retry"
    );
    assert!(storage.get_errors(None)?.iter().any(|error| {
        error.file_id == Some(NodeId(file.id))
            && error.message.contains("Skipped oversized source file")
            && error.coverage_reason == Some(FileCoverageReason::Oversized)
    }));
    Ok(())
}

#[test]
fn parser_partial_append_lf_reuses_the_equivalent_graph_projection() -> Result<()> {
    use codestory_workspace::RefreshInfo;

    let dir = tempdir()?;
    let path = dir.path().join("partial.c");
    let source = "int helper(void) { return 1; }\nint broken( { return helper(); }\n";
    std::fs::write(&path, source)?;
    let refresh = RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![path.clone()],
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    indexer.run_incremental(&mut storage, &refresh, &EventBus::new(), None)?;
    assert!(!storage.get_file_by_path(&path)?.unwrap().complete);
    let graph_before = (storage.get_nodes()?.len(), storage.get_edges()?.len());

    std::fs::write(&path, format!("{source}\n"))?;
    let stats = indexer.run_incremental(&mut storage, &refresh, &EventBus::new(), None)?;
    assert!(!stats.graph_projection_changed);
    assert_eq!(stats.source_identity_only_files, 1);
    assert!(!stats.resolution_ran);
    assert_eq!(
        (storage.get_nodes()?.len(), storage.get_edges()?.len()),
        graph_before
    );
    Ok(())
}

/// `LineOffsets` replaced `source.lines().nth()` in the visibility classifier,
/// so any disagreement with `str::lines()` silently changes the projected
/// access of a member — no panic, no error row, just a different graph. This
/// pins every boundary case rather than trusting the two to agree.
#[test]
fn line_offsets_agree_with_str_lines_on_every_boundary_shape() {
    for source in [
        "",
        "\n",
        "\n\n",
        "a",
        "a\n",
        "a\nb",
        "a\nb\n",
        "a\r\nb\r\n",
        "a\r\nb",
        "\r\n",
        "pub fn a() {}\n\nprivate:\n  int x;\n",
        "trailing spaces   \nand a tab\t\n",
        "unicode: héllo wörld\nsecond ünicode line\n",
        "no trailing newline at all",
    ] {
        let offsets = super::LineOffsets::new(source);
        let expected: Vec<&str> = source.lines().collect();
        for (index, want) in expected.iter().enumerate() {
            let line = u32::try_from(index + 1).expect("line number");
            assert_eq!(
                offsets.line(source, line),
                Some(*want),
                "line {line} of {source:?}"
            );
        }
        assert_eq!(
            offsets.line(source, 0),
            None,
            "line numbers are 1-based; 0 must not resolve for {source:?}"
        );
        let past_end = u32::try_from(expected.len() + 1).expect("line number");
        assert_eq!(
            offsets.line(source, past_end),
            None,
            "reading past the last line of {source:?} must be None, not empty"
        );
    }
}

#[test]
fn a_structural_source_over_the_structural_bound_is_refused_below_the_parser_headroom() -> Result<()>
{
    // The structural bound only does work when it sits *below* the parser
    // headroom, which is exactly the shipped configuration and exactly what no
    // existing test reached: every other test sets both caps to the same tiny
    // value, so the generic guard refuses the file first and the structural
    // branch is never entered. Disabling the structural bound left the whole
    // suite green.
    //
    // Two guards enforce this and they are deliberately redundant — a metadata
    // check before the read, and the collector's own bound on the decoded
    // source. This asserts the observable outcome rather than either branch,
    // so it survives one being refactored away and fails when both are gone.
    use codestory_workspace::RefreshInfo;

    let dir = tempdir()?;
    let path = dir.path().join("schema.sql");
    std::fs::write(
        &path,
        "CREATE TABLE wide (id INTEGER, name TEXT, note TEXT);\n",
    )?;
    let observed = std::fs::metadata(&path)?.len();

    let refresh = RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![path.clone()],
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::new_in_memory()?;
    let bus = EventBus::new();

    WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_index_policy(SourceIndexPolicy {
            structural_byte_cap: observed - 1,
            ..SourceIndexPolicy::oversized(observed * 16)
        })
        .run_incremental(&mut storage, &refresh, &bus, None)?;

    let file = storage
        .get_file_by_path(&path)?
        .expect("the refused file still gets a metadata row");
    assert!(
        storage.get_errors(None)?.iter().any(|error| {
            error.file_id == Some(NodeId(file.id))
                && error.coverage_reason == Some(FileCoverageReason::Oversized)
                && error.message.contains(&format!("{}", observed - 1))
        }),
        "the refusal must name the structural bound that produced it, not the \
         parser headroom the file is comfortably inside"
    );
    Ok(())
}

#[test]
fn parser_result_changed_with_restored_mtime_is_incomplete_and_not_cached() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("changed.rs");
    let original = "fn original() {}\n";
    std::fs::write(&path, original)?;
    let prepared = PreparedIndexInput {
        full_path: path.clone(),
        artifact_cache_path: Some(path.with_extension("artifact")),
        source: original.to_string(),
        source_utf8_exact: true,
        compilation_info: None,
        language_config: get_language_for_ext("rs").expect("rust config"),
        artifact_cache_key: Some("old-source".to_string()),
        content_hash: source_content_hash(original.as_bytes()),
    };
    overwrite_preserving_mtime(&path, "fn replaced() {}\n")?;

    let result = WorkspaceIndexer::new(dir.path().to_path_buf())
        .execute_prepared_index(&prepared, &Arc::new(SymbolTable::new()));

    assert!(result.cache_write.is_none());
    assert!(result.local_storage.file_content_hashes.is_empty());
    assert_eq!(result.local_storage.files.len(), 1);
    assert!(!result.local_storage.files[0].complete);
    assert!(result.local_storage.errors.iter().any(|error| {
        error.message.contains("Source changed while indexing")
            && error.message.contains("retry required")
            && error.coverage_reason == Some(FileCoverageReason::SourceChanged)
    }));
    assert!(
        result
            .local_storage
            .nodes
            .iter()
            .all(|node| node.serialized_name != "original")
    );
    Ok(())
}

#[test]
fn artifact_cache_result_changed_with_restored_mtime_is_rejected() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("cached.rs");
    let original = "fn cached_original() {}\n";
    std::fs::write(&path, original)?;
    let config = get_language_for_ext("rs").expect("rust config");
    let mut artifact =
        CachedIndexArtifact::from_index_result(index_file(&path, original, &config, None, None)?);
    let content_hash = source_content_hash(original.as_bytes());
    overwrite_preserving_mtime(&path, "fn cached_replaced() {}\n")?;

    let rejected =
        verify_cached_artifact_source(&mut artifact, &path, config.language_name, &content_hash)
            .expect_err("changed cached source must be rejected");

    assert!(rejected.file_content_hashes.is_empty());
    assert_eq!(rejected.files.len(), 1);
    assert!(!rejected.files[0].complete);
    assert!(rejected.errors[0].message.contains("retry required"));
    assert_eq!(
        rejected.errors[0].coverage_reason,
        Some(FileCoverageReason::SourceChanged)
    );
    Ok(())
}

fn progress_events_still_pending(saw_started: bool, saw_complete: bool) -> bool {
    !saw_started || !saw_complete
}

#[test]
fn test_full_refresh_batches_artifact_cache_writes_per_file_chunk() -> Result<()> {
    use codestory_store::Store as Storage;
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut files = Vec::new();
    for index in 0..12 {
        let path = dir.path().join(format!("module_{index}.rs"));
        fs::write(&path, format!("struct File_{index} {{}}\n"))?;
        files.push(path);
    }

    let mut storage = Storage::new_in_memory().unwrap();
    let bus = EventBus::new();
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf()).with_batch_config(
        IncrementalIndexingConfig {
            file_batch_size: 3,
            node_batch_size: 4,
            edge_batch_size: 4,
            occurrence_batch_size: 8,
            error_batch_size: 128,
        },
    );

    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: files,
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    let stats = indexer.run(&mut storage, &plan, &bus, None)?;

    assert_eq!(stats.artifact_cache_writes, 12);
    assert_eq!(stats.artifact_cache_write_transactions, 4);

    // Each file should contribute at least one file node and one symbol node.
    let nodes = storage.get_nodes()?;
    assert!(nodes.len() >= 24);

    Ok(())
}

#[test]
fn test_file_backed_full_refresh_uses_bounded_projection_pipeline() -> Result<()> {
    use codestory_store::Store as Storage;
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut files = Vec::new();
    for index in 0..12 {
        let path = dir.path().join(format!("pipeline_{index}.rs"));
        fs::write(&path, format!("struct Pipeline_{index} {{}}\n"))?;
        files.push(path);
    }

    let database_path = dir.path().join("staged.sqlite");
    let mut storage = Storage::open_build(&database_path)?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_artifact_cache_policies(ArtifactCachePolicies {
            parser: ArtifactCachePolicy::KnownEmpty,
            structural: ArtifactCachePolicy::ReadThrough,
        })
        .with_batch_config(IncrementalIndexingConfig {
            file_batch_size: 3,
            node_batch_size: 4,
            edge_batch_size: 4,
            occurrence_batch_size: 8,
            error_batch_size: 128,
        });
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: files,
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    let stats = indexer.run(&mut storage, &plan, &EventBus::new(), None)?;

    assert_eq!(stats.full_refresh_chunks_produced, 4);
    assert_eq!(stats.full_refresh_chunks_persisted, 4);
    assert_eq!(stats.full_refresh_queue_capacity, 1);
    assert_eq!(stats.full_refresh_queue_high_water, 1);
    assert_eq!(stats.artifact_cache_writes, 12);
    assert_eq!(stats.artifact_cache_write_transactions, 4);
    assert_eq!(
        stats.parser_artifact_cache.policy,
        ArtifactCachePolicy::KnownEmpty
    );
    assert_eq!(stats.parser_artifact_cache.logical_lookups, 12);
    assert_eq!(stats.parser_artifact_cache.physical_queries, 0);
    assert_eq!(stats.parser_artifact_cache.hits, 0);
    assert_eq!(stats.parser_artifact_cache.misses, 12);
    assert_eq!(stats.parser_artifact_cache.reader_opens, 0);
    assert_eq!(stats.parser_artifact_cache.lookup_wall_ns, 0);
    assert_eq!(
        stats.structural_artifact_cache.policy,
        ArtifactCachePolicy::ReadThrough
    );
    assert_eq!(stats.structural_artifact_cache.logical_lookups, 0);
    assert_eq!(stats.structural_artifact_cache.physical_queries, 0);
    assert_eq!(stats.structural_artifact_cache.reader_opens, 0);
    assert!(storage.get_nodes()?.len() >= 24);
    Ok(())
}

fn assert_mixed_full_refresh_reader_owner(
    files_to_index: Vec<PathBuf>,
    expected_owner: ArtifactCacheFamily,
) -> Result<()> {
    use codestory_store::Store as Storage;
    use tempfile::tempdir;

    let dir = tempdir()?;
    std::fs::write(dir.path().join("lib.rs"), "pub fn parser_source() {}\n")?;
    std::fs::write(
        dir.path().join("config.json"),
        "{\"service\":{\"name\":\"api\"}}\n",
    )?;
    let mut storage = Storage::open_build(dir.path().join("mixed.sqlite"))?;
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index,
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };

    let stats = WorkspaceIndexer::new(dir.path().to_path_buf()).run(
        &mut storage,
        &plan,
        &EventBus::new(),
        None,
    )?;

    assert_eq!(stats.parser_artifact_cache.logical_lookups, 1);
    assert_eq!(stats.parser_artifact_cache.physical_queries, 1);
    assert_eq!(stats.structural_artifact_cache.logical_lookups, 1);
    assert_eq!(stats.structural_artifact_cache.physical_queries, 1);
    assert_eq!(
        stats
            .parser_artifact_cache
            .reader_opens
            .saturating_add(stats.structural_artifact_cache.reader_opens),
        1,
        "one shared reader open must be attributed exactly once"
    );
    match expected_owner {
        ArtifactCacheFamily::Parser => {
            assert_eq!(stats.parser_artifact_cache.reader_opens, 1);
            assert_eq!(stats.structural_artifact_cache.reader_opens, 0);
        }
        ArtifactCacheFamily::Structural => {
            assert_eq!(stats.parser_artifact_cache.reader_opens, 0);
            assert_eq!(stats.structural_artifact_cache.reader_opens, 1);
        }
    }
    Ok(())
}

#[test]
fn mixed_full_refresh_attributes_reader_open_to_structural_when_scheduled_first() -> Result<()> {
    assert_mixed_full_refresh_reader_owner(
        vec![PathBuf::from("config.json"), PathBuf::from("lib.rs")],
        ArtifactCacheFamily::Structural,
    )
}

#[test]
fn mixed_full_refresh_attributes_reader_open_to_parser_when_scheduled_first() -> Result<()> {
    assert_mixed_full_refresh_reader_owner(
        vec![PathBuf::from("lib.rs"), PathBuf::from("config.json")],
        ArtifactCacheFamily::Parser,
    )
}

#[test]
fn test_file_backed_full_refresh_pipeline_reuses_copied_artifact_cache() -> Result<()> {
    use codestory_store::Store as Storage;
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut files = Vec::new();
    for index in 0..6 {
        let path = dir.path().join(format!("cached_pipeline_{index}.rs"));
        fs::write(&path, format!("fn cached_pipeline_{index}() {{}}\n"))?;
        files.push(path);
    }
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: files,
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf()).with_batch_config(
        IncrementalIndexingConfig {
            file_batch_size: 2,
            node_batch_size: 8,
            edge_batch_size: 8,
            occurrence_batch_size: 8,
            error_batch_size: 128,
        },
    );

    let source_path = dir.path().join("cache-source.sqlite");
    let mut source = Storage::open_build(&source_path)?;
    let source_stats = indexer.run(&mut source, &plan, &EventBus::new(), None)?;
    assert_eq!(source_stats.artifact_cache_misses, 6);
    assert_eq!(source_stats.artifact_cache_writes, 6);
    drop(source);

    let mut target = Storage::open_build(dir.path().join("cache-target.sqlite"))?;
    assert_eq!(target.copy_index_artifact_cache_from(&source_path)?, 6);
    let cached_stats = indexer.run(&mut target, &plan, &EventBus::new(), None)?;

    assert_eq!(cached_stats.artifact_cache_hits, 6);
    assert_eq!(cached_stats.artifact_cache_misses, 0);
    assert_eq!(cached_stats.artifact_cache_writes, 0);
    assert_eq!(cached_stats.artifact_cache_write_transactions, 0);
    assert_eq!(
        cached_stats.parser_artifact_cache.policy,
        ArtifactCachePolicy::ReadThrough
    );
    assert_eq!(cached_stats.parser_artifact_cache.logical_lookups, 6);
    assert_eq!(cached_stats.parser_artifact_cache.physical_queries, 6);
    assert_eq!(cached_stats.parser_artifact_cache.hits, 6);
    assert_eq!(cached_stats.parser_artifact_cache.misses, 0);
    assert_eq!(cached_stats.parser_artifact_cache.reader_opens, 1);
    assert_eq!(cached_stats.full_refresh_chunks_produced, 3);
    assert_eq!(cached_stats.full_refresh_chunks_persisted, 3);
    assert!(target.get_nodes()?.len() >= 12);
    Ok(())
}

#[test]
fn structural_full_refresh_reuses_only_the_verified_structural_cache() -> Result<()> {
    use codestory_store::Store as Storage;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let workflow_dir = dir.path().join(".github/workflows");
    std::fs::create_dir_all(&workflow_dir)?;
    let workflow = workflow_dir.join("ci.yml");
    std::fs::write(
        &workflow,
        "name: CI\non:\n  push:\njobs:\n  build:\n    runs-on: ubuntu-latest\n",
    )?;
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![workflow],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let source_indexer = WorkspaceIndexer::new(dir.path().to_path_buf());

    let source_path = dir.path().join("structural-cache-source.sqlite");
    let mut source = Storage::open_build(&source_path)?;
    let source_stats = source_indexer.run(&mut source, &plan, &EventBus::new(), None)?;
    assert_eq!(source_stats.artifact_cache_misses, 1);
    assert!(
        !source
            .get_structural_text_units_for_nodes(
                &source
                    .get_nodes()?
                    .into_iter()
                    .map(|node| node.id)
                    .collect::<Vec<_>>()
            )?
            .is_empty()
    );
    drop(source);

    let mut target = Storage::open_build(dir.path().join("structural-cache-target.sqlite"))?;
    assert_eq!(target.copy_index_artifact_cache_from(&source_path)?, 0);
    assert_eq!(
        target.copy_structural_text_artifact_cache_from(&source_path)?,
        1
    );
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf()).with_artifact_cache_policies(
        ArtifactCachePolicies {
            parser: ArtifactCachePolicy::KnownEmpty,
            structural: ArtifactCachePolicy::ReadThrough,
        },
    );
    let cached_stats = indexer.run(&mut target, &plan, &EventBus::new(), None)?;

    assert_eq!(cached_stats.artifact_cache_hits, 1);
    assert_eq!(cached_stats.artifact_cache_misses, 0);
    assert_eq!(
        cached_stats.parser_artifact_cache.policy,
        ArtifactCachePolicy::KnownEmpty
    );
    assert_eq!(cached_stats.parser_artifact_cache.logical_lookups, 0);
    assert_eq!(cached_stats.parser_artifact_cache.physical_queries, 0);
    assert_eq!(cached_stats.parser_artifact_cache.reader_opens, 0);
    assert_eq!(
        cached_stats.structural_artifact_cache.policy,
        ArtifactCachePolicy::ReadThrough
    );
    assert_eq!(cached_stats.structural_artifact_cache.logical_lookups, 1);
    assert_eq!(cached_stats.structural_artifact_cache.physical_queries, 1);
    assert_eq!(cached_stats.structural_artifact_cache.hits, 1);
    assert_eq!(cached_stats.structural_artifact_cache.misses, 0);
    assert_eq!(cached_stats.structural_artifact_cache.reader_opens, 1);
    assert!(
        !target
            .get_structural_text_units_for_nodes(
                &target
                    .get_nodes()?
                    .into_iter()
                    .map(|node| node.id)
                    .collect::<Vec<_>>()
            )?
            .is_empty()
    );
    Ok(())
}

#[test]
fn parser_and_structural_cache_read_failures_recollect_as_physical_misses() -> Result<()> {
    use tempfile::tempdir;

    let dir = tempdir()?;
    let parser_path = dir.path().join("lib.rs");
    let structural_path = dir.path().join("config.json");
    std::fs::write(&parser_path, "pub fn cached_value() -> i32 { 1 }\n")?;
    std::fs::write(&structural_path, "{\"service\":{\"name\":\"api\"}}\n")?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let symbol_table = Arc::new(SymbolTable::new());

    let mut parser_stats = IncrementalIndexingStats {
        parser_artifact_cache: ArtifactCacheFamilyStats::new(ArtifactCachePolicy::ReadThrough),
        structural_artifact_cache: ArtifactCacheFamilyStats::new(ArtifactCachePolicy::ReadThrough),
        ..IncrementalIndexingStats::default()
    };
    let parser_work = {
        let mut access = ArtifactCacheAccess::failing(ArtifactCachePolicies::default());
        indexer.prepare_index_work(
            &mut access,
            &PathBuf::from("lib.rs"),
            dir.path(),
            None,
            &symbol_table,
            &mut parser_stats,
        )
    };
    assert!(matches!(parser_work, Ok(PreparedIndexWork::Parse(_))));
    assert_eq!(parser_stats.parser_artifact_cache.logical_lookups, 1);
    assert_eq!(parser_stats.parser_artifact_cache.physical_queries, 1);
    assert_eq!(parser_stats.parser_artifact_cache.hits, 0);
    assert_eq!(parser_stats.parser_artifact_cache.misses, 1);

    let mut structural_stats = IncrementalIndexingStats {
        parser_artifact_cache: ArtifactCacheFamilyStats::new(ArtifactCachePolicy::ReadThrough),
        structural_artifact_cache: ArtifactCacheFamilyStats::new(ArtifactCachePolicy::ReadThrough),
        ..IncrementalIndexingStats::default()
    };
    let structural_work = {
        let mut access = ArtifactCacheAccess::failing(ArtifactCachePolicies::default());
        indexer.prepare_index_work(
            &mut access,
            &PathBuf::from("config.json"),
            dir.path(),
            None,
            &symbol_table,
            &mut structural_stats,
        )
    };
    assert!(matches!(
        structural_work,
        Ok(PreparedIndexWork::Structural(_))
    ));
    assert_eq!(
        structural_stats.structural_artifact_cache.logical_lookups,
        1
    );
    assert_eq!(
        structural_stats.structural_artifact_cache.physical_queries,
        1
    );
    assert_eq!(structural_stats.structural_artifact_cache.hits, 0);
    assert_eq!(structural_stats.structural_artifact_cache.misses, 1);
    Ok(())
}

#[test]
fn corrupt_or_incompatible_structural_cache_recollects_and_changed_bytes_replace_only_that_path()
-> Result<()> {
    use codestory_store::Store as Storage;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let workflow_dir = dir.path().join(".github/workflows");
    std::fs::create_dir_all(&workflow_dir)?;
    let workflow = workflow_dir.join("ci.yml");
    std::fs::write(
        &workflow,
        "name: CI\non:\n  push:\njobs:\n  build:\n    runs-on: ubuntu-latest\n",
    )?;
    let mut plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![workflow.clone()],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let mut storage = Storage::open_build(dir.path().join("structural-cache.sqlite"))?;
    indexer.run(&mut storage, &plan, &EventBus::new(), None)?;
    plan.mode = codestory_workspace::BuildMode::Incremental;

    storage.get_connection().execute(
        "UPDATE structural_text_artifact_cache
             SET artifact_blob = ?1, artifact_digest = ?2
             WHERE file_path = ?3",
        (
            b"not-json".as_slice(),
            source_content_hash(b"not-json"),
            ".github/workflows/ci.yml",
        ),
    )?;
    let corrupt_stats = indexer.run(&mut storage, &plan, &EventBus::new(), None)?;
    assert_eq!(corrupt_stats.artifact_cache_invalid_entries, 1);
    assert_eq!(corrupt_stats.artifact_cache_misses, 1);

    let (cache_key, blob): (String, Vec<u8>) = storage.get_connection().query_row(
        "SELECT cache_key, artifact_blob
             FROM structural_text_artifact_cache
             WHERE file_path = ?1",
        [".github/workflows/ci.yml"],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let mut graph_corrupt: serde_json::Value = serde_json::from_slice(&blob)?;
    let graph_nodes = graph_corrupt["nodes"]
        .as_array_mut()
        .ok_or_else(|| anyhow!("structural cache nodes are missing"))?;
    let build_node = graph_nodes
        .iter_mut()
        .find(|node| node["serialized_name"] == "build")
        .ok_or_else(|| anyhow!("cached build node is missing"))?;
    build_node["serialized_name"] = serde_json::json!("poisoned-build");
    storage.get_connection().execute(
        "UPDATE structural_text_artifact_cache
             SET artifact_blob = ?1
             WHERE file_path = ?2 AND cache_key = ?3",
        (
            serde_json::to_vec(&graph_corrupt)?,
            ".github/workflows/ci.yml",
            &cache_key,
        ),
    )?;
    storage.get_connection().execute(
        "UPDATE node SET serialized_name = 'stale-live-build'
             WHERE serialized_name = 'build'",
        [],
    )?;
    let graph_corrupt_stats = indexer.run(&mut storage, &plan, &EventBus::new(), None)?;
    assert_eq!(graph_corrupt_stats.artifact_cache_hits, 0);
    assert_eq!(graph_corrupt_stats.artifact_cache_invalid_entries, 0);
    assert_eq!(graph_corrupt_stats.artifact_cache_misses, 1);
    let graph_names = storage
        .get_nodes()?
        .into_iter()
        .map(|node| node.serialized_name)
        .collect::<HashSet<_>>();
    assert!(graph_names.contains("build"));
    assert!(!graph_names.contains("poisoned-build"));
    assert!(!graph_names.contains("stale-live-build"));

    let (cache_key, blob): (String, Vec<u8>) = storage.get_connection().query_row(
        "SELECT cache_key, artifact_blob
             FROM structural_text_artifact_cache
             WHERE file_path = ?1",
        [".github/workflows/ci.yml"],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let mut incompatible: serde_json::Value = serde_json::from_slice(&blob)?;
    incompatible["descriptor_version"] = serde_json::json!(999);
    let incompatible = serde_json::to_vec(&incompatible)?;
    storage.get_connection().execute(
        "UPDATE structural_text_artifact_cache
             SET artifact_blob = ?1, artifact_digest = ?2
             WHERE file_path = ?3 AND cache_key = ?4",
        (
            &incompatible,
            source_content_hash(&incompatible),
            ".github/workflows/ci.yml",
            cache_key,
        ),
    )?;
    let incompatible_stats = indexer.run(&mut storage, &plan, &EventBus::new(), None)?;
    assert_eq!(incompatible_stats.artifact_cache_invalid_entries, 1);
    assert_eq!(incompatible_stats.artifact_cache_misses, 1);

    overwrite_preserving_mtime(
        &workflow,
        "name: CI\non:\n  push:\njobs:\n  verify:\n    runs-on: ubuntu-latest\n",
    )?;
    let changed_stats = indexer.run(&mut storage, &plan, &EventBus::new(), None)?;
    assert_eq!(changed_stats.artifact_cache_hits, 0);
    assert_eq!(changed_stats.artifact_cache_misses, 1);
    assert!(
        storage
            .get_nodes()?
            .iter()
            .any(|node| node.serialized_name == "verify")
    );
    assert!(
        storage
            .get_nodes()?
            .iter()
            .all(|node| node.serialized_name != "build")
    );
    Ok(())
}

#[test]
fn structural_source_drift_discards_units_and_cache_write_even_with_restored_mtime() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("schema.sql");
    let original = "CREATE TABLE original (id INTEGER);\n";
    std::fs::write(&path, original)?;
    let content_hash = source_content_hash(original.as_bytes());
    let prepared = PreparedStructuralInput {
        full_path: path.clone(),
        role_classification_path: PathBuf::from("schema.sql"),
        artifact_cache_path: Some(PathBuf::from("schema.sql")),
        artifact_cache_key: Some("v1:original".to_string()),
        source: original.to_string(),
        content_hash,
    };
    overwrite_preserving_mtime(&path, "CREATE TABLE changed (id INTEGER);\n")?;

    let result = WorkspaceIndexer::new(dir.path().to_path_buf())
        .execute_prepared_structural_index(&prepared);

    assert!(result.local_storage.structural_text_units.is_empty());
    assert!(result.local_storage.structural_text_projections.is_empty());
    assert!(result.local_storage.structural_text_cache_writes.is_empty());
    assert!(!result.local_storage.files[0].complete);
    assert_eq!(
        result.local_storage.errors[0].coverage_reason,
        Some(FileCoverageReason::SourceChanged)
    );
    Ok(())
}

#[test]
fn excluded_structural_paths_return_before_metadata_or_content_reads() -> Result<()> {
    let dir = tempdir()?;
    std::fs::create_dir_all(dir.path().join("vendor/unreadable.json"))?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let mut storage = Storage::new_in_memory()?;
    let symbol_table = Arc::new(SymbolTable::new());
    let mut stats = IncrementalIndexingStats::default();

    for relative in ["vendor/unreadable.json", "secrets/missing.json"] {
        let work = {
            let mut cache_access =
                ArtifactCacheAccess::storage(&mut storage, ArtifactCachePolicies::default());
            indexer.prepare_index_work(
                &mut cache_access,
                &PathBuf::from(relative),
                dir.path(),
                None,
                &symbol_table,
                &mut stats,
            )
        };
        match work {
            Ok(PreparedIndexWork::Immediate(local)) => {
                assert!(local.files.is_empty(), "{relative}");
                assert!(local.nodes.is_empty(), "{relative}");
                assert!(local.structural_text_units.is_empty(), "{relative}");
                assert!(local.structural_text_cache_writes.is_empty(), "{relative}");
                assert!(local.errors.is_empty(), "{relative}");
            }
            Ok(_) => panic!("excluded path was scheduled: {relative}"),
            Err(_) => panic!("excluded path reached metadata or content reads: {relative}"),
        }
    }

    assert!(storage.get_nodes()?.is_empty());
    assert!(storage.get_errors(None)?.is_empty());
    assert!(
        storage
            .get_structural_text_projection_file_ids()?
            .is_empty()
    );
    let cache_rows: i64 = storage.get_connection().query_row(
        "SELECT COUNT(*) FROM structural_text_artifact_cache",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(cache_rows, 0);
    assert_eq!(stats.artifact_cache_hits, 0);
    assert_eq!(stats.artifact_cache_misses, 0);
    Ok(())
}

#[test]
fn incremental_policy_upgrade_removes_pre_policy_structural_publication_and_cache() -> Result<()> {
    let dir = tempdir()?;
    let excluded = dir.path().join("vendor/config.json");
    std::fs::create_dir_all(excluded.parent().expect("excluded parent"))?;
    std::fs::write(&excluded, "{\"legacy\":true}\n")?;
    let source = std::fs::read(&excluded)?;
    let producer = structural::structural_producer(&excluded).expect("JSON producer");
    let cache_path = PathBuf::from("vendor/config.json");
    let cache_key =
        build_structural_artifact_cache_key(&cache_path, &source, producer).expect("cache key");
    let artifact =
        CachedStructuralArtifact::from_storage(structural::index_structural_file(&excluded)?);
    let artifact_blob = serde_json::to_vec(&artifact)?;
    let projected = artifact.into_intermediate_storage();
    let cache_write = codestory_store::StructuralTextArtifactCacheWrite {
        path: &cache_path,
        file_id: projected.files[0].id,
        cache_key: &cache_key,
        artifact_blob: &artifact_blob,
    };
    let mut storage = Storage::new_in_memory()?;
    storage
        .projections()
        .flush_projection_batch(codestory_store::ProjectionBatch {
            files: &projected.files,
            file_content_hashes: &projected.file_content_hashes,
            nodes: &projected.nodes,
            structural_text_units: &projected.structural_text_units,
            structural_text_projections: &projected.structural_text_projections,
            structural_text_cache_writes: std::slice::from_ref(&cache_write),
            edges: &projected.edges,
            occurrences: &projected.occurrences,
            component_access: &projected.component_access,
            callable_projection_states: &projected.callable_projection_states,
            file_errors: &[],
        })?;
    let publication = codestory_store::IndexPublicationRecord {
        generation: 1,
        generation_id: "pre-policy-generation".to_string(),
        run_id: "pre-policy-run".to_string(),
        mode: codestory_store::IndexPublicationMode::Full,
        published_at_epoch_ms: 1,
    };
    storage.publish_structural_text_unit_generation(&publication)?;
    storage.validate_structural_text_unit_publication(&publication)?;
    assert_eq!(storage.get_structural_text_projection_file_ids()?.len(), 1);

    let manifest = codestory_workspace::WorkspaceManifest::open(dir.path().to_path_buf())?;
    let outcome = manifest.build_execution_outcome(&codestory_workspace::RefreshInputs {
        stored_files: storage.files().inventory()?,
        policy_exclusions: Vec::new(),
        inventory: codestory_workspace::WorkspaceInventory::default(),
    })?;
    assert_eq!(outcome.plan.files_to_remove, vec![projected.files[0].id]);
    assert!(outcome.plan.files_to_index.is_empty());
    WorkspaceIndexer::new(dir.path().to_path_buf()).run(
        &mut storage,
        &outcome.plan,
        &EventBus::new(),
        None,
    )?;

    assert!(storage.get_files()?.is_empty());
    assert!(
        storage
            .get_structural_text_projection_file_ids()?
            .is_empty()
    );
    for table in [
        "structural_text_unit",
        "structural_text_artifact_cache",
        "structural_text_unit_publication",
    ] {
        let count: i64 = storage.get_connection().query_row(
            &format!("SELECT COUNT(*) FROM {table}"),
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 0, "{table} copied forward excluded data");
    }
    Ok(())
}

#[test]
fn prepare_path_preserves_specialized_structural_and_openapi_routing() -> Result<()> {
    let dir = tempdir()?;
    let fixtures = [
        (
            ".github/workflows/ci.yml",
            "name: CI\njobs:\n  build:\n    runs-on: ubuntu-latest\n",
            "structural_github_actions_workflow_collector",
        ),
        (
            "docker-compose.yaml",
            "services:\n  web:\n    image: nginx\n",
            "structural_docker_compose_collector",
        ),
        (
            "crates/app/Cargo.toml",
            "[package]\nname = \"app\"\n",
            "structural_cargo_manifest_collector",
        ),
        (
            "tsconfig.json",
            "{\"openapi\":\"3.1.0\",\"paths\":{\"/health\":{\"get\":{}}},\"compilerOptions\":{\"strict\":true}}",
            "structural_typescript_config_jsonc_collector",
        ),
    ];
    for (relative, source, _expected_producer) in fixtures {
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture parent"))?;
        std::fs::write(&path, source)?;
    }
    for (relative, source) in [
        (
            "openapi.json",
            "{\"openapi\":\"3.1.0\",\"paths\":{\"/health\":{\"get\":{}}}}",
        ),
        (
            "openapi.yaml",
            "openapi: 3.1.0\npaths:\n  /health:\n    get:\n      responses: {}\n",
        ),
    ] {
        std::fs::write(dir.path().join(relative), source)?;
    }
    std::fs::create_dir_all(dir.path().join("scripts"))?;
    std::fs::write(dir.path().join("scripts/run.sh"), "run() { echo ok; }\n")?;

    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let mut storage = Storage::new_in_memory()?;
    let symbol_table = Arc::new(SymbolTable::new());
    let mut stats = IncrementalIndexingStats::default();

    for (relative, _, expected_producer) in fixtures {
        let prepared = {
            let mut cache_access =
                ArtifactCacheAccess::storage(&mut storage, ArtifactCachePolicies::default());
            indexer.prepare_index_work(
                &mut cache_access,
                &PathBuf::from(relative),
                dir.path(),
                None,
                &symbol_table,
                &mut stats,
            )
        };
        let input = match prepared {
            Ok(PreparedIndexWork::Structural(input)) => input,
            Ok(_) => panic!("specialized structural route was bypassed: {relative}"),
            Err(_) => panic!("specialized structural route failed: {relative}"),
        };
        let projected = indexer.execute_prepared_structural_index(&input);
        assert!(projected.local_storage.errors.is_empty(), "{relative}");
        assert_eq!(
            projected.local_storage.structural_text_projections[0].producer, expected_producer,
            "{relative}"
        );
    }

    for relative in ["openapi.json", "openapi.yaml"] {
        let prepared = {
            let mut cache_access =
                ArtifactCacheAccess::storage(&mut storage, ArtifactCachePolicies::default());
            indexer.prepare_index_work(
                &mut cache_access,
                &PathBuf::from(relative),
                dir.path(),
                None,
                &symbol_table,
                &mut stats,
            )
        };
        let projected = match prepared {
            Ok(PreparedIndexWork::Immediate(projected)) => projected,
            Ok(_) => panic!("OpenAPI source entered generic structural routing: {relative}"),
            Err(_) => panic!("OpenAPI source preparation failed: {relative}"),
        };
        assert_eq!(projected.files[0].language, "openapi", "{relative}");
        assert_eq!(
            projected.file_content_hashes.len(),
            1,
            "{relative} must retain its verified source identity"
        );
        assert_eq!(
            projected.file_content_hashes[0].content_hash.len(),
            64,
            "{relative}"
        );
        assert!(projected.structural_text_units.is_empty(), "{relative}");
        assert!(projected.nodes.iter().any(|node| {
            node.canonical_id
                .as_deref()
                .is_some_and(|value| value == "openapi:endpoint:GET /health")
        }));
    }

    let bash = {
        let mut cache_access =
            ArtifactCacheAccess::storage(&mut storage, ArtifactCachePolicies::default());
        indexer.prepare_index_work(
            &mut cache_access,
            &PathBuf::from("scripts/run.sh"),
            dir.path(),
            None,
            &symbol_table,
            &mut stats,
        )
    };
    match bash {
        Ok(PreparedIndexWork::Parse(input)) => {
            assert_eq!(input.language_config.language_name, "bash")
        }
        Ok(_) => panic!("parser-backed .sh entered structural fallback"),
        Err(_) => panic!("parser-backed .sh preparation failed"),
    }
    Ok(())
}

#[test]
fn structural_zero_byte_role_uses_the_workspace_relative_path() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path().join("target/workspace");
    let relative = PathBuf::from("src/__tests__/fixtures/empty.json");
    let full_path = root.join(&relative);
    std::fs::create_dir_all(full_path.parent().expect("fixture parent"))?;
    std::fs::write(&full_path, [])?;

    let indexer = WorkspaceIndexer::new(root.clone());
    let mut storage = Storage::new_in_memory()?;
    let symbol_table = Arc::new(SymbolTable::new());
    let mut stats = IncrementalIndexingStats::default();
    let prepared_result = {
        let mut cache_access =
            ArtifactCacheAccess::storage(&mut storage, ArtifactCachePolicies::default());
        indexer.prepare_index_work(
            &mut cache_access,
            &relative,
            &root,
            None,
            &symbol_table,
            &mut stats,
        )
    };
    let prepared = match prepared_result {
        Ok(prepared) => prepared,
        Err(_) => panic!("zero-byte test JSON preparation failed"),
    };
    let input = match prepared {
        PreparedIndexWork::Structural(input) => input,
        _ => panic!("zero-byte test JSON must enter structural collection"),
    };
    let projected = indexer.execute_prepared_structural_index(&input);
    assert!(projected.local_storage.errors.is_empty());
    assert_eq!(
        projected.local_storage.files[0].file_role,
        codestory_store::FileRole::Test
    );
    assert!(projected.local_storage.structural_text_units.is_empty());
    Ok(())
}

#[test]
fn structural_unit_bound_failure_writes_no_partial_projection_or_cache() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("many.json");
    let mut source = String::from("{");
    for index in 0..=structural::MAX_STRUCTURAL_UNITS_PER_FILE {
        if index > 0 {
            source.push(',');
        }
        source.push_str(&format!("\"key{index}\":{index}"));
    }
    source.push('}');
    std::fs::write(&path, source)?;

    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![path],
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let mut storage = Storage::new_in_memory()?;
    let stats = indexer.run(&mut storage, &plan, &EventBus::new(), None)?;

    assert!(
        storage
            .get_structural_text_projection_file_ids()?
            .is_empty()
    );
    let unit_rows: i64 = storage.get_connection().query_row(
        "SELECT COUNT(*) FROM structural_text_unit",
        [],
        |row| row.get(0),
    )?;
    let cache_rows: i64 = storage.get_connection().query_row(
        "SELECT COUNT(*) FROM structural_text_artifact_cache",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(unit_rows, 0);
    assert_eq!(cache_rows, 0);
    assert_eq!(stats.artifact_cache_writes, 0);
    assert!(storage.get_errors(None)?.iter().any(|error| {
        error.coverage_reason == Some(FileCoverageReason::Oversized)
            && error.message.contains("unit collector limit")
    }));
    Ok(())
}

#[test]
fn structural_unit_bound_can_become_a_verified_policy_exclusion() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("evidence-generated.json");
    let mut source = String::from("{");
    for index in 0..=structural::MAX_STRUCTURAL_UNITS_PER_FILE {
        if index > 0 {
            source.push(',');
        }
        source.push_str(&format!("\"key{index}\":{index}"));
    }
    source.push('}');
    std::fs::write(&path, &source)?;
    assert!(source.len() as u64 <= SourceIndexPolicy::default().byte_cap);

    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![path],
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };
    let policy = SourceIndexPolicy::default();
    let indexer =
        WorkspaceIndexer::new(dir.path().to_path_buf()).with_source_index_policy(policy.clone());
    let mut storage = Storage::new_in_memory()?;
    let outcome =
        indexer.run_with_policy_exclusions(&mut storage, &plan, &EventBus::new(), None)?;

    assert!(storage.get_files()?.is_empty());
    assert!(storage.get_errors(None)?.is_empty());
    assert!(
        storage
            .get_structural_text_projection_file_ids()?
            .is_empty()
    );
    assert_eq!(outcome.policy_exclusions.len(), 1);
    let exclusion = &outcome.policy_exclusions[0];
    assert_eq!(exclusion.normalized_path, "evidence-generated.json");
    assert_eq!(exclusion.observed_size, source.len() as u64);
    assert_eq!(
        exclusion.observed_unit_count,
        structural::MAX_STRUCTURAL_UNITS_PER_FILE as u64 + 1
    );
    assert_eq!(exclusion.policy_version, policy.policy_version);
    assert_eq!(exclusion.structural_unit_cap, policy.structural_unit_cap);
    Ok(())
}

#[test]
fn structural_unit_exclusion_uses_the_caller_owned_policy_cap() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("evidence.json");
    std::fs::write(&path, "{\"one\":1,\"two\":2,\"three\":3}")?;
    let policy = SourceIndexPolicy {
        policy_version: codestory_contracts::workspace::OVERSIZED_SOURCE_POLICY_VERSION.to_string(),
        byte_cap: codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP,
        structural_byte_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP,
        structural_unit_cap: 2,
    };
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![path],
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::new_in_memory()?;
    let outcome = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_index_policy(policy.clone())
        .run_with_policy_exclusions(&mut storage, &plan, &EventBus::new(), None)?;

    assert_eq!(outcome.policy_exclusions.len(), 1);
    assert_eq!(outcome.policy_exclusions[0].observed_unit_count, 3);
    assert_eq!(outcome.policy_exclusions[0].structural_unit_cap, 2);
    assert!(storage.get_files()?.is_empty());
    Ok(())
}

#[test]
fn pre_limit_v1_cache_is_ineligible_and_matches_fresh_unit_bound_failure() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("many.json");
    let mut source = String::from("{");
    for index in 0..=structural::MAX_STRUCTURAL_UNITS_PER_FILE {
        if index > 0 {
            source.push(',');
        }
        source.push_str(&format!("\"key{index}\":{index}"));
    }
    source.push('}');
    std::fs::write(&path, &source)?;
    let source_hash = source_content_hash(source.as_bytes());
    let file_id = WorkspaceIndexer::canonical_file_node_id_for_path(&path);
    let legacy_artifact = CachedStructuralArtifact {
        descriptor_version: codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
        files: vec![codestory_store::FileInfo {
            id: file_id,
            path: path.clone(),
            language: "json".to_string(),
            modification_time: file_modification_time(&path),
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: codestory_store::FileRole::Source,
        }],
        file_content_hashes: vec![codestory_store::FileContentHash {
            file_id,
            content_hash: source_hash.clone(),
        }],
        nodes: Vec::new(),
        structural_unit_node_ids: vec![NodeId(1); structural::MAX_STRUCTURAL_UNITS_PER_FILE + 1],
        structural_text_units: Vec::new(),
        structural_text_projections: Vec::new(),
        edges: Vec::new(),
        occurrences: Vec::new(),
        component_access: Vec::new(),
        callable_projection_states: Vec::new(),
    };
    let blob = serde_json::to_vec(&legacy_artifact)?;
    let mut legacy_cache = Storage::new_in_memory()?;
    legacy_cache.get_connection().execute(
        "INSERT INTO structural_text_artifact_cache (
                file_path, file_id, cache_key, source_content_hash,
                descriptor_version, producer, artifact_digest, artifact_blob,
                updated_at_epoch_ms
             ) VALUES ('many.json', ?1, 'v1:pre-limit', ?2, ?3,
                       'structural_json_collector', ?4, ?5, 1)",
        rusqlite::params![
            file_id,
            source_hash,
            codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION as i64,
            format!("{:x}", Sha256::digest(&blob)),
            blob,
        ],
    )?;
    let current_key = build_structural_artifact_cache_key(
        Path::new("many.json"),
        source.as_bytes(),
        "structural_json_collector",
    )
    .expect("current structural cache key");
    assert!(current_key.starts_with(&format!(
        "v{}:",
        crate::cache::STRUCTURAL_ARTIFACT_CACHE_VERSION
    )));

    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let symbol_table = Arc::new(SymbolTable::new());
    let mut legacy_stats = IncrementalIndexingStats::default();
    let legacy_prepared = {
        let mut access =
            ArtifactCacheAccess::storage(&mut legacy_cache, ArtifactCachePolicies::default());
        indexer.prepare_index_work(
            &mut access,
            &PathBuf::from("many.json"),
            dir.path(),
            None,
            &symbol_table,
            &mut legacy_stats,
        )
    };
    let Ok(PreparedIndexWork::Structural(legacy_input)) = legacy_prepared else {
        panic!("a superseded cache version must not satisfy the current lookup");
    };
    let legacy_result = indexer.execute_prepared_structural_index(&legacy_input);
    assert_eq!(legacy_stats.artifact_cache_hits, 0);
    assert_eq!(legacy_stats.artifact_cache_misses, 1);

    legacy_cache.get_connection().execute(
        "UPDATE structural_text_artifact_cache SET cache_key = ?1",
        [&current_key],
    )?;
    let mut over_limit_hit_stats = IncrementalIndexingStats::default();
    let over_limit_hit_prepared = {
        let mut access =
            ArtifactCacheAccess::storage(&mut legacy_cache, ArtifactCachePolicies::default());
        indexer.prepare_index_work(
            &mut access,
            &PathBuf::from("many.json"),
            dir.path(),
            None,
            &symbol_table,
            &mut over_limit_hit_stats,
        )
    };
    let Ok(PreparedIndexWork::Structural(over_limit_hit_input)) = over_limit_hit_prepared else {
        panic!("over-limit current cache artifact must be recollected");
    };
    let over_limit_hit_result = indexer.execute_prepared_structural_index(&over_limit_hit_input);
    assert_eq!(over_limit_hit_stats.artifact_cache_hits, 0);
    assert_eq!(over_limit_hit_stats.artifact_cache_invalid_entries, 1);

    let mut fresh_cache = Storage::new_in_memory()?;
    let mut fresh_stats = IncrementalIndexingStats::default();
    let fresh_prepared = {
        let mut access =
            ArtifactCacheAccess::storage(&mut fresh_cache, ArtifactCachePolicies::default());
        indexer.prepare_index_work(
            &mut access,
            &PathBuf::from("many.json"),
            dir.path(),
            None,
            &symbol_table,
            &mut fresh_stats,
        )
    };
    let Ok(PreparedIndexWork::Structural(fresh_input)) = fresh_prepared else {
        panic!("fresh over-limit structural source must be collected");
    };
    let fresh_result = indexer.execute_prepared_structural_index(&fresh_input);
    assert_eq!(
        legacy_result.local_storage.errors[0].coverage_reason,
        Some(FileCoverageReason::Oversized)
    );
    assert_eq!(
        fresh_result.local_storage.errors[0].coverage_reason,
        legacy_result.local_storage.errors[0].coverage_reason
    );
    assert_eq!(
        over_limit_hit_result.local_storage.errors[0].coverage_reason,
        legacy_result.local_storage.errors[0].coverage_reason
    );
    assert!(legacy_result.cache_write.is_none());
    assert!(over_limit_hit_result.cache_write.is_none());
    assert!(fresh_result.cache_write.is_none());
    Ok(())
}

#[test]
fn test_adaptive_full_refresh_planner_tracks_dense_and_sparse_node_output() -> Result<()> {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut files = Vec::new();
    for index in 0..8 {
        let path = dir.path().join(format!("planned_{index}.rs"));
        fs::write(&path, vec![b'x'; 25])?;
        files.push(path);
    }
    let mut planner = AdaptiveFullRefreshChunkPlanner::new(FullRefreshChunkBudget {
        source_bytes: 100,
        projected_nodes: 100,
        file_ceiling: 10,
    });

    let initial = planner
        .next_chunk(&files, dir.path(), 0, None)
        .expect("initial chunk");
    assert_eq!((initial.start, initial.end), (0, 4));
    assert_eq!(initial.source_bytes, 100);

    planner.observe(initial.source_bytes, 400);
    let dense = planner
        .next_chunk(&files, dir.path(), initial.end, None)
        .expect("dense projection chunk");
    assert_eq!((dense.start, dense.end), (4, 5));
    assert_eq!(dense.projected_nodes, 100);

    planner.observe(dense.source_bytes, 1);
    let sparse = planner
        .next_chunk(&files, dir.path(), dense.end, None)
        .expect("sparse projection chunk");
    assert_eq!((sparse.start, sparse.end), (5, 8));
    assert_eq!(sparse.source_bytes, 75);
    Ok(())
}

#[test]
fn test_full_refresh_adaptive_budget_grows_beyond_legacy_file_window() -> Result<()> {
    use codestory_store::Store as Storage;
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut files = Vec::new();
    for index in 0..40 {
        let path = dir.path().join(format!("tiny_{index}.rs"));
        fs::write(&path, format!("fn tiny_{index}() {{}}\n"))?;
        files.push(path);
    }
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: files,
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::open_build(dir.path().join("staged.sqlite"))?;

    let stats = WorkspaceIndexer::new(dir.path().to_path_buf()).run(
        &mut storage,
        &plan,
        &EventBus::new(),
        None,
    )?;

    assert_eq!(stats.full_refresh_chunks_produced, 1);
    assert_eq!(stats.full_refresh_chunks_persisted, 1);
    assert_eq!(stats.full_refresh_chunk_target_bytes, 8 * 1024 * 1024);
    assert_eq!(stats.full_refresh_chunk_target_nodes, 120_000);
    assert_eq!(stats.full_refresh_chunk_file_ceiling, 512);
    assert_eq!(stats.full_refresh_chunk_max_files, 40);
    assert!(stats.full_refresh_chunk_max_files > 24);
    assert!(stats.full_refresh_chunk_max_planned_bytes < 8 * 1024 * 1024);
    assert!(stats.full_refresh_chunk_max_nodes < 120_000);
    assert_eq!(stats.full_refresh_chunk_budget_overruns, 0);
    Ok(())
}

#[test]
fn test_empty_full_refresh_reports_adaptive_chunk_config() -> Result<()> {
    use codestory_store::Store as Storage;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::open_build(dir.path().join("staged.sqlite"))?;

    let stats = WorkspaceIndexer::new(dir.path().to_path_buf()).run(
        &mut storage,
        &plan,
        &EventBus::new(),
        None,
    )?;

    assert_eq!(stats.full_refresh_chunk_target_bytes, 8 * 1024 * 1024);
    assert_eq!(stats.full_refresh_chunk_target_nodes, 120_000);
    assert_eq!(stats.full_refresh_chunk_file_ceiling, 512);
    assert_eq!(stats.full_refresh_chunk_max_files, 0);
    assert_eq!(stats.full_refresh_chunk_max_planned_bytes, 0);
    assert_eq!(stats.full_refresh_chunk_max_nodes, 0);
    assert_eq!(stats.full_refresh_chunk_budget_overruns, 0);
    assert_eq!(stats.projection_batch_transactions, 0);
    assert_eq!(stats.projection_batch_wall_ms, 0);
    Ok(())
}

#[test]
fn test_full_refresh_adaptive_budget_advances_one_over_budget_file() -> Result<()> {
    use codestory_store::Store as Storage;
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let large = dir.path().join("large.rs");
    let small = dir.path().join("small.rs");
    fs::write(&large, format!("fn large() {{}}\n{}", "x".repeat(64)))?;
    fs::write(&small, "fn small() {}\n")?;
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![large, small],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::open_build(dir.path().join("staged.sqlite"))?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_file_byte_cap(256)
        .with_full_refresh_chunk_budget(FullRefreshChunkBudget {
            source_bytes: 32,
            projected_nodes: 100,
            file_ceiling: 10,
        });

    let stats = indexer.run(&mut storage, &plan, &EventBus::new(), None)?;

    assert_eq!(stats.full_refresh_chunks_produced, 2);
    assert_eq!(stats.full_refresh_chunks_persisted, 2);
    assert_eq!(stats.full_refresh_chunk_max_files, 1);
    assert!(stats.full_refresh_chunk_max_planned_bytes > 32);
    assert_eq!(stats.full_refresh_chunk_budget_overruns, 1);
    assert!(
        storage
            .get_nodes()?
            .iter()
            .any(|node| node.serialized_name == "large")
    );
    assert!(
        storage
            .get_nodes()?
            .iter()
            .any(|node| node.serialized_name == "small")
    );
    Ok(())
}

#[test]
fn test_file_backed_full_refresh_duplicate_paths_keep_serial_cache_semantics() -> Result<()> {
    use codestory_store::Store as Storage;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let path = dir.path().join("duplicate.rs");
    std::fs::write(&path, "fn duplicate() {}\n")?;
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![path.clone(), path],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf()).with_batch_config(
        IncrementalIndexingConfig {
            file_batch_size: 1,
            ..IncrementalIndexingConfig::default()
        },
    );
    let mut storage = Storage::open_build(dir.path().join("duplicate.sqlite"))?;

    let stats = indexer.run(&mut storage, &plan, &EventBus::new(), None)?;

    assert_eq!(stats.full_refresh_queue_capacity, 0);
    assert_eq!(stats.full_refresh_chunks_produced, 0);
    assert_eq!(stats.artifact_cache_misses, 1);
    assert_eq!(stats.artifact_cache_hits, 1);
    assert_eq!(stats.artifact_cache_write_transactions, 1);
    assert_eq!(stats.source_prepare_ms, stats.artifact_cache_lookup_ms);
    assert_eq!(stats.projection_batch_transactions, 1);
    assert!(stats.projection_batch_wall_ms >= stats.projection_flush_ms);
    assert_eq!(
        storage
            .get_nodes()?
            .into_iter()
            .filter(|node| node.serialized_name == "duplicate")
            .count(),
        1
    );
    Ok(())
}

#[test]
fn test_duplicate_structural_paths_publish_one_coherent_projection() -> Result<()> {
    use codestory_store::Store as Storage;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let workflow_dir = dir.path().join(".github/workflows");
    std::fs::create_dir_all(&workflow_dir)?;
    let workflow = workflow_dir.join("ci.yml");
    std::fs::write(
        &workflow,
        "name: CI\non:\n  push:\njobs:\n  build:\n    runs-on: ubuntu-latest\n",
    )?;
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![workflow.clone(), workflow],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf()).with_batch_config(
        IncrementalIndexingConfig {
            file_batch_size: 1,
            ..IncrementalIndexingConfig::default()
        },
    );
    let mut storage = Storage::open_build(dir.path().join("duplicate-structural.sqlite"))?;

    let stats = indexer.run(&mut storage, &plan, &EventBus::new(), None)?;

    assert_eq!(stats.projection_batch_transactions, 1);
    assert_eq!(storage.get_structural_text_projection_file_ids()?.len(), 1);
    assert!(
        !storage
            .get_structural_text_units_for_nodes(
                &storage
                    .get_nodes()?
                    .into_iter()
                    .map(|node| node.id)
                    .collect::<Vec<_>>()
            )?
            .is_empty()
    );
    Ok(())
}

#[test]
fn test_full_refresh_pipeline_matches_serial_projection_snapshot() -> Result<()> {
    use codestory_store::Store as Storage;
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let workflow_dir = dir.path().join(".github/workflows");
    fs::create_dir_all(&workflow_dir)?;
    let files = vec![
            (
                dir.path().join("first.rs"),
                "fn first() { second(); }\n".to_string(),
            ),
            (
                dir.path().join("package.json"),
                "{\"scripts\":{\"build\":\"vite build\"}}\n".to_string(),
            ),
            (
                dir.path().join("second.ts"),
                "export function second(): number { return 2; }\n".to_string(),
            ),
            (
                workflow_dir.join("build.yml"),
                "name: build\non:\n  push:\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n".to_string(),
            ),
            (
                dir.path().join("oversized.rs"),
                format!("{}\nfn too_large() {{}}\n", "// padding".repeat(40)),
            ),
        ];
    let mut paths = Vec::new();
    for (path, source) in &files {
        fs::write(path, source)?;
        paths.push(path.clone());
    }
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: paths,
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let config = IncrementalIndexingConfig {
        file_batch_size: 2,
        node_batch_size: 5,
        edge_batch_size: 5,
        occurrence_batch_size: 5,
        error_batch_size: 2,
    };
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_file_byte_cap(256)
        .with_batch_config(config);

    let mut serial = Storage::new_in_memory()?;
    let serial_stats = indexer.run(&mut serial, &plan, &EventBus::new(), None)?;
    let pipeline_path = dir.path().join("pipeline.sqlite");
    let mut pipelined = Storage::open_build(&pipeline_path)?;
    let pipeline_stats = indexer.run(&mut pipelined, &plan, &EventBus::new(), None)?;

    assert_projection_snapshots_equal(&serial, &pipelined, "pipelined")?;

    let mut replay = Storage::open_build(dir.path().join("cache-replay.sqlite"))?;
    assert!(replay.copy_index_artifact_cache_from(&pipeline_path)? > 0);
    let replay_stats = indexer.run(&mut replay, &plan, &EventBus::new(), None)?;
    assert!(replay_stats.artifact_cache_hits > 0);
    assert_projection_snapshots_equal(&serial, &replay, "cache-replayed")?;
    assert_eq!(serial_stats.full_refresh_queue_capacity, 0);
    assert_eq!(pipeline_stats.full_refresh_queue_capacity, 1);
    assert_eq!(
        serial_stats.source_prepare_ms,
        serial_stats.artifact_cache_lookup_ms
    );
    assert_eq!(
        pipeline_stats.source_prepare_ms,
        pipeline_stats.artifact_cache_lookup_ms
    );
    assert_eq!(
        serial_stats.artifact_cache_write_transactions,
        pipeline_stats.artifact_cache_write_transactions
    );
    assert_eq!(
        serial_stats.projection_batch_transactions,
        pipeline_stats.projection_batch_transactions
    );
    assert!(serial_stats.projection_batch_transactions > 0);
    assert_eq!(
        serial_stats.projection_persistence.transactions,
        serial_stats.projection_batch_transactions as u64
    );
    assert_eq!(
        pipeline_stats.projection_persistence.transactions,
        pipeline_stats.projection_batch_transactions as u64
    );
    assert_eq!(
        serial_stats.projection_persistence.row_attempts(),
        pipeline_stats.projection_persistence.row_attempts()
    );
    assert_eq!(
        serial_stats.projection_persistence.bound_bytes(),
        pipeline_stats.projection_persistence.bound_bytes()
    );
    assert_eq!(
        serial_stats.projection_persistence.statement_executions(),
        pipeline_stats.projection_persistence.statement_executions()
    );
    assert!(
        serial_stats
            .projection_persistence
            .file_errors
            .statement_executions
            > 0
    );
    assert!(serial_stats.projection_batch_wall_ms >= serial_stats.projection_flush_ms);
    assert!(pipeline_stats.projection_batch_wall_ms >= pipeline_stats.projection_flush_ms);
    Ok(())
}

#[test]
fn test_full_refresh_parses_next_chunk_while_writer_owns_previous_chunk() -> Result<()> {
    use codestory_store::Store as Storage;
    use std::fs;
    use std::sync::Barrier;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut files = Vec::new();
    for index in 0..4 {
        let path = dir.path().join(format!("overlap_{index}.rs"));
        fs::write(&path, format!("fn overlap_{index}() {{}}\n"))?;
        files.push(path);
    }

    let writer_has_chunk = Arc::new(Barrier::new(2));
    let release_writer = Arc::new(Barrier::new(2));
    let next_parse_started = Arc::new(AtomicBool::new(false));
    let parse_barrier_entered = Arc::new(AtomicBool::new(false));
    let parse_writer_has_chunk = writer_has_chunk.clone();
    let parse_release_writer = release_writer.clone();
    let parse_next_parse_started = next_parse_started.clone();
    let parse_barrier_entered_hook = parse_barrier_entered.clone();
    let writer_writer_has_chunk = writer_has_chunk.clone();
    let writer_release_writer = release_writer.clone();
    let hooks = FullRefreshPipelineTestHooks {
        before_plan_file: None,
        before_prepare_chunk: None,
        before_parse_job: Some(Arc::new(move |chunk_index| {
            if chunk_index == 1 && !parse_barrier_entered_hook.swap(true, Ordering::SeqCst) {
                parse_writer_has_chunk.wait();
                parse_next_parse_started.store(true, Ordering::SeqCst);
                parse_release_writer.wait();
            }
        })),
        before_writer_chunk: Some(Arc::new(move |chunk_index| {
            if chunk_index == 0 {
                writer_writer_has_chunk.wait();
                writer_release_writer.wait();
            }
        })),
        after_send_chunk: None,
        on_send_timeout: None,
    };

    let database_path = dir.path().join("staged.sqlite");
    let mut storage = Storage::open_build(&database_path)?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_batch_config(IncrementalIndexingConfig {
            file_batch_size: 2,
            node_batch_size: usize::MAX,
            edge_batch_size: usize::MAX,
            occurrence_batch_size: usize::MAX,
            error_batch_size: usize::MAX,
        })
        .with_pipeline_test_hooks(hooks);
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: files,
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };

    let stats = indexer.run(&mut storage, &plan, &EventBus::new(), None)?;

    assert!(next_parse_started.load(Ordering::SeqCst));
    assert_eq!(stats.full_refresh_chunks_produced, 2);
    assert_eq!(stats.full_refresh_chunks_persisted, 2);
    Ok(())
}

#[test]
fn test_full_refresh_cancellation_while_queue_is_full_drains_only_accepted_chunks() -> Result<()> {
    use codestory_store::Store as Storage;
    use std::fs;
    use std::sync::Barrier;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut files = Vec::new();
    for index in 0..6 {
        let path = dir.path().join(format!("cancel_{index}.rs"));
        fs::write(&path, format!("fn cancel_{index}() {{}}\n"))?;
        files.push(path);
    }

    let writer_has_chunk = Arc::new(Barrier::new(2));
    let release_writer = Arc::new(Barrier::new(2));
    let prepare_writer_has_chunk = writer_has_chunk.clone();
    let writer_writer_has_chunk = writer_has_chunk.clone();
    let writer_release_writer = release_writer.clone();
    let timeout_release_writer = release_writer.clone();
    let cancel_token = CancellationToken::new();
    let timeout_cancel_token = cancel_token.clone();
    let hooks = FullRefreshPipelineTestHooks {
        before_plan_file: None,
        before_prepare_chunk: Some(Arc::new(move |chunk_index| {
            if chunk_index == 1 {
                prepare_writer_has_chunk.wait();
            }
        })),
        before_parse_job: None,
        before_writer_chunk: Some(Arc::new(move |chunk_index| {
            if chunk_index == 0 {
                writer_writer_has_chunk.wait();
                writer_release_writer.wait();
            }
        })),
        after_send_chunk: None,
        on_send_timeout: Some(Arc::new(move |chunk_index| {
            if chunk_index == 2 {
                timeout_cancel_token.cancel();
                timeout_release_writer.wait();
            }
        })),
    };

    let database_path = dir.path().join("staged.sqlite");
    let mut storage = Storage::open_build(&database_path)?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_batch_config(IncrementalIndexingConfig {
            file_batch_size: 2,
            node_batch_size: usize::MAX,
            edge_batch_size: usize::MAX,
            occurrence_batch_size: usize::MAX,
            error_batch_size: usize::MAX,
        })
        .with_pipeline_test_hooks(hooks);
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: files,
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };

    let stats = indexer.run(&mut storage, &plan, &EventBus::new(), Some(&cancel_token))?;

    assert!(cancel_token.is_cancelled());
    assert_eq!(stats.full_refresh_chunks_produced, 2);
    assert_eq!(stats.full_refresh_chunks_persisted, 2);
    assert!(
        stats.full_refresh_producer_blocked_ms >= 20,
        "queue saturation should record bounded producer backpressure"
    );
    let names = storage
        .get_nodes()?
        .into_iter()
        .map(|node| node.serialized_name)
        .collect::<HashSet<_>>();
    assert!(names.contains("cancel_0"));
    assert!(names.contains("cancel_3"));
    assert!(!names.contains("cancel_4"));
    assert!(!names.contains("cancel_5"));
    Ok(())
}

#[test]
fn test_full_refresh_cancellation_before_dispatch_writes_nothing() -> Result<()> {
    use codestory_store::Store as Storage;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let path = dir.path().join("cancelled.rs");
    std::fs::write(&path, "fn cancelled() {}\n")?;
    let mut storage = Storage::open_build(dir.path().join("staged.sqlite"))?;
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: vec![path],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let cancel_token = CancellationToken::new();
    cancel_token.cancel();

    let stats = WorkspaceIndexer::new(dir.path().to_path_buf()).run(
        &mut storage,
        &plan,
        &EventBus::new(),
        Some(&cancel_token),
    )?;

    assert_eq!(stats.full_refresh_chunks_produced, 0);
    assert!(storage.get_nodes()?.is_empty());
    Ok(())
}

#[test]
fn test_full_refresh_cancellation_during_planning_drops_partial_chunk() -> Result<()> {
    use codestory_store::Store as Storage;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut paths = Vec::new();
    for index in 0..40 {
        let path = dir.path().join(format!("plan_cancel_{index}.rs"));
        std::fs::write(&path, format!("fn plan_cancel_{index}() {{}}\n"))?;
        paths.push(path);
    }
    let cancel_token = CancellationToken::new();
    let planning_cancel_token = cancel_token.clone();
    let planned_files = Arc::new(AtomicUsize::new(0));
    let planned_files_from_hook = planned_files.clone();
    let hooks = FullRefreshPipelineTestHooks {
        before_plan_file: Some(Arc::new(move |file_index| {
            planned_files_from_hook.store(file_index.saturating_add(1), Ordering::SeqCst);
            if file_index == 5 {
                planning_cancel_token.cancel();
            }
        })),
        before_prepare_chunk: None,
        before_parse_job: None,
        before_writer_chunk: None,
        after_send_chunk: None,
        on_send_timeout: None,
    };
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf()).with_pipeline_test_hooks(hooks);
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: paths,
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::open_build(dir.path().join("staged.sqlite"))?;

    let stats = indexer.run(&mut storage, &plan, &EventBus::new(), Some(&cancel_token))?;

    assert!(cancel_token.is_cancelled());
    assert_eq!(planned_files.load(Ordering::SeqCst), 6);
    assert_eq!(stats.full_refresh_chunks_produced, 0);
    assert_eq!(stats.full_refresh_chunks_persisted, 0);
    assert!(storage.get_nodes()?.is_empty());
    Ok(())
}

#[test]
fn test_full_refresh_cancellation_during_parse_drops_unaccepted_chunk() -> Result<()> {
    use codestory_store::Store as Storage;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut paths = Vec::new();
    for index in 0..4 {
        let path = dir.path().join(format!("parse_cancel_{index}.rs"));
        std::fs::write(&path, format!("fn parse_cancel_{index}() {{}}\n"))?;
        paths.push(path);
    }
    let cancel_token = CancellationToken::new();
    let parse_cancel_token = cancel_token.clone();
    let hooks = FullRefreshPipelineTestHooks {
        before_plan_file: None,
        before_prepare_chunk: None,
        before_parse_job: Some(Arc::new(move |_| parse_cancel_token.cancel())),
        before_writer_chunk: None,
        after_send_chunk: None,
        on_send_timeout: None,
    };
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_batch_config(IncrementalIndexingConfig {
            file_batch_size: 2,
            ..IncrementalIndexingConfig::default()
        })
        .with_pipeline_test_hooks(hooks);
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: paths,
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::open_build(dir.path().join("staged.sqlite"))?;

    let stats = indexer.run(&mut storage, &plan, &EventBus::new(), Some(&cancel_token))?;

    assert!(cancel_token.is_cancelled());
    assert_eq!(stats.full_refresh_chunks_produced, 0);
    assert_eq!(stats.full_refresh_chunks_persisted, 0);
    assert!(storage.get_nodes()?.is_empty());
    Ok(())
}

#[test]
fn test_full_refresh_cancellation_after_writer_acceptance_drains_that_chunk() -> Result<()> {
    use codestory_store::Store as Storage;
    use std::sync::Barrier;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut paths = Vec::new();
    for index in 0..4 {
        let path = dir.path().join(format!("accepted_{index}.rs"));
        std::fs::write(&path, format!("fn accepted_{index}() {{}}\n"))?;
        paths.push(path);
    }
    let accepted = Arc::new(Barrier::new(2));
    let producer_accepted = accepted.clone();
    let writer_accepted = accepted.clone();
    let cancel_token = CancellationToken::new();
    let writer_cancel_token = cancel_token.clone();
    let hooks = FullRefreshPipelineTestHooks {
        before_plan_file: None,
        before_prepare_chunk: None,
        before_parse_job: None,
        before_writer_chunk: Some(Arc::new(move |chunk_index| {
            if chunk_index == 0 {
                writer_cancel_token.cancel();
                writer_accepted.wait();
            }
        })),
        after_send_chunk: Some(Arc::new(move |chunk_index| {
            if chunk_index == 0 {
                producer_accepted.wait();
            }
        })),
        on_send_timeout: None,
    };
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_batch_config(IncrementalIndexingConfig {
            file_batch_size: 2,
            ..IncrementalIndexingConfig::default()
        })
        .with_pipeline_test_hooks(hooks);
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: paths,
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::open_build(dir.path().join("staged.sqlite"))?;

    let stats = indexer.run(&mut storage, &plan, &EventBus::new(), Some(&cancel_token))?;

    assert!(cancel_token.is_cancelled());
    assert_eq!(stats.full_refresh_chunks_produced, 1);
    assert_eq!(stats.full_refresh_chunks_persisted, 1);
    let names = storage
        .get_nodes()?
        .into_iter()
        .map(|node| node.serialized_name)
        .collect::<HashSet<_>>();
    assert!(names.contains("accepted_0"));
    assert!(names.contains("accepted_1"));
    assert!(!names.contains("accepted_2"));
    Ok(())
}

#[test]
fn test_full_refresh_writer_failure_disconnects_producer_without_deadlock() -> Result<()> {
    use codestory_store::Store as Storage;
    use std::fs;
    use std::sync::mpsc;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut files = Vec::new();
    for index in 0..8 {
        let path = dir.path().join(format!("failure_{index}.rs"));
        fs::write(&path, format!("fn failure_{index}() {{}}\n"))?;
        files.push(path);
    }

    let database_path = dir.path().join("staged.sqlite");
    let mut storage = Storage::open_build(&database_path)?;
    storage.get_connection().execute_batch(
        "CREATE TRIGGER reject_pipeline_cache_write
             BEFORE INSERT ON index_artifact_cache
             BEGIN
               SELECT RAISE(ABORT, 'forced pipeline cache failure');
             END;",
    )?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf()).with_batch_config(
        IncrementalIndexingConfig {
            file_batch_size: 2,
            node_batch_size: usize::MAX,
            edge_batch_size: usize::MAX,
            occurrence_batch_size: usize::MAX,
            error_batch_size: usize::MAX,
        },
    );
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::FullRefresh,
        files_to_index: files,
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let (result_tx, result_rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let result = indexer
            .run(&mut storage, &plan, &EventBus::new(), None)
            .map(|_| ())
            .map_err(|error| error.to_string());
        result_tx
            .send(result)
            .expect("result receiver must remain open");
    });

    let error = result_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("pipeline failure must not deadlock")
        .expect_err("injected writer failure must propagate");
    handle.join().expect("indexing thread must not panic");
    assert!(error.contains("forced pipeline cache failure"), "{error}");
    Ok(())
}

#[test]
fn companion_source_without_evidence_producer_persists_inventory_identity_only() -> Result<()> {
    use codestory_store::Store as Storage;
    use codestory_workspace::{RefreshInfo, RefreshInputs, WorkspaceManifest};

    let dir = tempdir()?;
    let companion = dir.path().join("maintenance.lua");
    std::fs::write(&companion, "return { enabled = true }\n")?;

    let mut storage = Storage::new_in_memory()?;
    WorkspaceIndexer::new(dir.path().to_path_buf()).run_incremental(
        &mut storage,
        &RefreshInfo {
            mode: codestory_workspace::BuildMode::Incremental,
            files_to_index: vec![companion.clone()],
            files_to_remove: Vec::new(),
            existing_file_ids: HashMap::new(),
        },
        &EventBus::new(),
        None,
    )?;

    let file = storage
        .get_file_by_path(&companion)?
        .expect("companion inventory row");
    assert_eq!(file.language, "lua");
    assert!(file.indexed);
    assert!(
        !file.complete,
        "inventory-only rows cannot prove graph absence"
    );
    assert!(storage.get_file_content_hash(file.id)?.is_some());
    assert!(
        storage
            .get_errors(None)?
            .iter()
            .all(|error| error.file_id != Some(NodeId(file.id))),
        "inventory-only coverage is stable, not a retryable collector failure"
    );
    let nodes = storage.get_nodes()?;
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node.file_node_id == Some(NodeId(file.id)))
            .count(),
        0,
        "inventory-only sources cannot emit non-file graph nodes"
    );
    assert!(storage.get_edges()?.is_empty());

    let manifest = WorkspaceManifest::open(dir.path().to_path_buf())?;
    let inventory = storage.files().inventory()?;
    let clean = manifest.build_execution_plan(&RefreshInputs {
        stored_files: inventory.clone(),
        policy_exclusions: Vec::new(),
        inventory: codestory_workspace::WorkspaceInventory::default(),
    })?;
    assert!(clean.files_to_index.is_empty());

    std::fs::write(&companion, "return { enabled = false }\n")?;
    let changed = manifest.build_execution_plan(&RefreshInputs {
        stored_files: inventory,
        policy_exclusions: Vec::new(),
        inventory: codestory_workspace::WorkspaceInventory::default(),
    })?;
    assert_eq!(changed.files_to_index, vec![companion]);
    Ok(())
}

#[test]
fn test_oversized_parser_file_is_skipped_before_indexing_read() -> Result<()> {
    use codestory_store::Store as Storage;
    use codestory_workspace::RefreshInfo;
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let oversized = dir.path().join("oversized.rs");
    let normal = dir.path().join("normal.rs");
    fs::write(
        &oversized,
        format!("{}\nfn too_large() {{}}\n", "// padded".repeat(16)),
    )?;
    fs::write(&normal, "fn small() {}\n")?;

    let mut storage = Storage::new_in_memory().unwrap();
    let bus = EventBus::new();
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_file_byte_cap(64)
        .with_batch_config(IncrementalIndexingConfig {
            file_batch_size: 2,
            node_batch_size: 128,
            edge_batch_size: 128,
            occurrence_batch_size: 128,
            error_batch_size: 128,
        });
    let refresh_info = RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![oversized.clone(), normal.clone()],
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;

    let files = storage.get_files()?;
    let oversized_file = files
        .iter()
        .find(|file| file.path == oversized)
        .expect("oversized file row should be persisted");
    assert!(
        !oversized_file.complete,
        "oversized file should be marked incomplete"
    );
    let normal_file = files
        .iter()
        .find(|file| file.path == normal)
        .expect("normal file row should be persisted");
    assert!(normal_file.complete, "normal file should remain complete");

    let errors = storage.get_errors(None)?;
    assert_eq!(errors.len(), 1);
    let error = &errors[0];
    assert_eq!(error.file_id, Some(NodeId(oversized_file.id)));
    assert!(!error.is_fatal, "oversized skip should be nonfatal");
    assert_eq!(error.coverage_reason, Some(FileCoverageReason::Oversized));
    assert!(
        error.message.contains("Skipped oversized source file"),
        "unexpected oversized error: {}",
        error.message
    );

    let nodes = storage.get_nodes()?;
    assert!(
        nodes
            .iter()
            .any(|node| node.serialized_name == "small" && node.kind == NodeKind::FUNCTION),
        "normal files should still be indexed"
    );
    assert!(
        !nodes.iter().any(|node| node.serialized_name == "too_large"),
        "oversized parser-backed source should not be read and parsed"
    );

    Ok(())
}

#[test]
fn test_source_byte_cap_precedes_special_collector_reads() -> Result<()> {
    use codestory_store::Store as Storage;
    use codestory_workspace::RefreshInfo;
    use std::fs;
    use tempfile::tempdir;

    const CAP: usize = 512;
    let dir = tempdir()?;
    let cases = [
        (
            "openapi",
            "oversized-openapi.json",
            "small-openapi.json",
            r#"{
  "openapi": "3.1.0",
  "paths": {
    "/small": {
      "get": { "operationId": "getSmall" }
    }
  }
}"#,
        ),
        (
            "svelte",
            "Oversized.svelte",
            "Small.svelte",
            r#"<script>
  export function smallTemplate() { return 1; }
</script>
<h1>Small</h1>"#,
        ),
        (
            "docker_compose",
            "docker-compose.override.yml",
            "compose.yaml",
            "services:\n  web:\n    image: example/web:latest\n",
        ),
        (
            "csharp",
            "oversized.cshtml",
            "small.cshtml",
            "[HttpGet(\"/small\")]\n",
        ),
    ];

    let mut files_to_index = Vec::new();
    let mut oversized_paths = Vec::new();
    let mut control_paths = Vec::new();
    for (_, oversized_name, control_name, source) in cases {
        assert!(source.len() <= CAP, "control fixture must remain below cap");
        let oversized_path = dir.path().join(oversized_name);
        let mut oversized_source = source.as_bytes().to_vec();
        oversized_source.resize(CAP + 1, b' ');
        fs::write(&oversized_path, oversized_source)?;
        files_to_index.push(oversized_path.clone());
        oversized_paths.push(oversized_path);

        let control_path = dir.path().join(control_name);
        fs::write(&control_path, source)?;
        files_to_index.push(control_path.clone());
        control_paths.push(control_path);
    }
    let unsupported_path = dir.path().join("oversized.bin");
    fs::write(&unsupported_path, vec![b'x'; CAP + 1])?;
    files_to_index.push(unsupported_path.clone());

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_file_byte_cap(CAP as u64)
        .with_batch_config(IncrementalIndexingConfig {
            file_batch_size: files_to_index.len(),
            node_batch_size: 256,
            edge_batch_size: 256,
            occurrence_batch_size: 256,
            error_batch_size: 256,
        });
    indexer.run_incremental(
        &mut storage,
        &RefreshInfo {
            mode: codestory_workspace::BuildMode::Incremental,
            files_to_index,
            files_to_remove: Vec::new(),
            existing_file_ids: HashMap::new(),
        },
        &EventBus::new(),
        None,
    )?;

    let files = storage.get_files()?;
    let nodes = storage.get_nodes()?;
    let edges = storage.get_edges()?;
    let errors = storage.get_errors(None)?;
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.coverage_reason == Some(FileCoverageReason::Oversized))
            .count(),
        oversized_paths.len()
    );

    for ((expected_language, _, _, _), path) in cases.iter().zip(&oversized_paths) {
        let file = files
            .iter()
            .find(|file| file.path == *path)
            .expect("oversized collector candidate must retain a diagnostic file row");
        assert!(!file.complete);
        assert_eq!(&file.language, expected_language);
        assert!(errors.iter().any(|error| {
            error.file_id == Some(NodeId(file.id))
                && error.coverage_reason == Some(FileCoverageReason::Oversized)
        }));
        assert_eq!(storage.get_file_content_hash(file.id)?, None);
        assert!(
            storage
                .get_callable_projection_states_for_file(file.id)?
                .is_empty(),
            "oversized collector candidate cannot retain callable projection state"
        );
        assert!(
            nodes
                .iter()
                .filter(|node| node.id != NodeId(file.id))
                .all(|node| node.file_node_id != Some(NodeId(file.id))),
            "oversized collector candidate cannot emit non-file graph nodes"
        );
        assert!(
            edges
                .iter()
                .all(|edge| edge.file_node_id != Some(NodeId(file.id))),
            "oversized collector candidate cannot emit graph edges"
        );
    }

    for path in control_paths {
        let file = files
            .iter()
            .find(|file| file.path == path)
            .expect("below-cap collector control must retain a file row");
        assert!(
            file.complete,
            "below-cap collector control must remain usable"
        );
        assert!(
            nodes.iter().any(|node| {
                node.kind != NodeKind::FILE && node.file_node_id == Some(NodeId(file.id))
            }),
            "below-cap collector control must still emit collector evidence"
        );
    }
    assert!(
        files.iter().all(|file| file.path != unsupported_path),
        "ordinary unsupported paths must remain ignored"
    );

    Ok(())
}

#[test]
fn test_incremental_indexing_cancel_after_flush_skips_resolution() -> Result<()> {
    use codestory_store::Store as Storage;
    use codestory_workspace::RefreshInfo;
    use std::fs;
    use std::time::Duration;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let mut files = Vec::new();
    for index in 0..64 {
        let path = dir.path().join(format!("module_{index}.rs"));
        fs::write(
            &path,
            format!("fn caller_{index}() {{ callee_{index}(); }}\nfn callee_{index}() {{}}\n"),
        )?;
        files.push(path);
    }

    let mut storage = Storage::new_in_memory().unwrap();
    let bus = EventBus::new();
    let rx = bus.receiver();
    let cancel_token = CancellationToken::new();
    let cancel_from_progress = cancel_token.clone();
    let canceller = std::thread::spawn(move || {
        while let Ok(event) = rx.recv_timeout(Duration::from_secs(2)) {
            if let Event::IndexingProgress { current, total } = event
                && current == total
            {
                cancel_from_progress.cancel();
                return;
            }
        }
    });
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf()).with_batch_config(
        IncrementalIndexingConfig {
            file_batch_size: 64,
            node_batch_size: usize::MAX,
            edge_batch_size: usize::MAX,
            occurrence_batch_size: usize::MAX,
            error_batch_size: 128,
        },
    );

    let refresh_info = RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: files,
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    let stats = indexer.run_incremental(&mut storage, &refresh_info, &bus, Some(&cancel_token))?;
    canceller.join().expect("progress canceller should finish");

    assert!(
        cancel_token.is_cancelled(),
        "expected progress to cancel token"
    );
    assert!(
        !stats.resolution_ran,
        "cancellation after indexing flush should skip resolution"
    );
    assert_eq!(stats.artifact_cache_writes, 64);
    assert_eq!(stats.artifact_cache_write_transactions, 1);
    assert!(
        !storage.get_edges()?.is_empty(),
        "indexing should flush edges"
    );

    Ok(())
}

#[test]
fn test_resolution_cancellation_after_outer_check_returns_completed_indexing_work() -> Result<()> {
    use codestory_store::Store as Storage;
    use codestory_workspace::RefreshInfo;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let path = dir.path().join("module.rs");
    std::fs::write(&path, "fn caller() { callee(); }\nfn callee() {}\n")?;

    let mut storage = Storage::new_in_memory()?;
    let cancel_token = CancellationToken::new();
    let cancel_before_resolution = cancel_token.clone();
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_batch_config(IncrementalIndexingConfig {
            file_batch_size: 1,
            node_batch_size: usize::MAX,
            edge_batch_size: usize::MAX,
            occurrence_batch_size: usize::MAX,
            error_batch_size: usize::MAX,
        })
        .with_before_resolution_test_hook(Arc::new(move || {
            cancel_before_resolution.cancel();
        }));
    let refresh_info = RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![path],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };

    let stats = indexer.run_incremental(
        &mut storage,
        &refresh_info,
        &EventBus::new(),
        Some(&cancel_token),
    )?;

    assert!(cancel_token.is_cancelled());
    assert!(
        !stats.resolution_ran,
        "rolled-back resolution is not completed work"
    );
    assert_eq!(stats.artifact_cache_writes, 1);
    assert_eq!(stats.artifact_cache_write_transactions, 1);
    let edges = storage.get_edges()?;
    assert!(!edges.is_empty(), "completed indexing work should remain");
    let call_edges = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .collect::<Vec<_>>();
    assert!(
        !call_edges.is_empty(),
        "fixture must produce a resolvable call"
    );
    assert!(
        call_edges
            .into_iter()
            .all(|edge| edge.resolved_target.is_none()),
        "cancelled resolution must not publish partial target updates"
    );

    Ok(())
}

#[test]
fn test_incremental_immediate_progress_precedes_parse_and_preserves_cancellation_boundary()
-> Result<()> {
    use codestory_store::Store as Storage;
    use std::fs;
    use std::time::Duration;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let oversized = dir.path().join("oversized.rs");
    let parsed = dir.path().join("parsed.rs");
    fs::write(
        &oversized,
        format!("fn oversized() {{}}\n{}", "x".repeat(64)),
    )?;
    fs::write(&parsed, "fn parsed() {}\n")?;

    let bus = EventBus::new();
    let progress_rx = bus.receiver();
    let cancel_token = CancellationToken::new();
    let parse_cancel_token = cancel_token.clone();
    let parse_hook_ran = Arc::new(AtomicBool::new(false));
    let parse_hook_ran_from_hook = parse_hook_ran.clone();
    let hooks = FullRefreshPipelineTestHooks {
        before_plan_file: None,
        before_prepare_chunk: None,
        before_parse_job: Some(Arc::new(move |_| {
            let (current, total) = loop {
                match progress_rx.recv_timeout(Duration::from_secs(2)) {
                    Ok(Event::IndexingProgress { current, total }) => break (current, total),
                    Ok(_) => {}
                    Err(error) => {
                        panic!("immediate progress must be observable before parser work: {error}")
                    }
                }
            };
            assert_eq!((current, total), (1, 2));
            parse_hook_ran_from_hook.store(true, Ordering::SeqCst);
            parse_cancel_token.cancel();
        })),
        before_writer_chunk: None,
        after_send_chunk: None,
        on_send_timeout: None,
    };
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_file_byte_cap(32)
        .with_batch_config(IncrementalIndexingConfig {
            file_batch_size: 2,
            ..IncrementalIndexingConfig::default()
        })
        .with_pipeline_test_hooks(hooks);
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![oversized.clone(), parsed.clone()],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    let mut storage = Storage::new_in_memory()?;

    let stats = indexer.run(&mut storage, &plan, &bus, Some(&cancel_token))?;

    assert!(parse_hook_ran.load(Ordering::SeqCst));
    assert!(cancel_token.is_cancelled());
    assert_eq!(stats.artifact_cache_writes, 0);
    let files = storage.get_files()?;
    assert!(files.iter().any(|file| file.path == oversized));
    assert!(files.iter().all(|file| file.path != parsed));
    Ok(())
}

#[test]
fn cancelled_run_skips_file_identity_lookups_and_projection_writes() -> Result<()> {
    use tempfile::tempdir;

    let dir = tempdir()?;
    let path = dir.path().join("cancelled.rs");
    std::fs::write(&path, "fn must_not_publish() {}\n")?;
    let mut storage = Storage::new_in_memory()?;
    storage.get_connection().execute("DROP TABLE node", [])?;
    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![path],
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };
    let cancel_token = CancellationToken::new();
    cancel_token.cancel();

    let stats = WorkspaceIndexer::new(dir.path().to_path_buf()).run(
        &mut storage,
        &plan,
        &EventBus::new(),
        Some(&cancel_token),
    )?;

    assert_eq!(stats.setup_existing_projection_ids_ms, 0);
    assert_eq!(stats.setup_seed_symbol_table_ms, 0);
    assert!(!stats.resolution_ran);
    assert!(storage.get_files()?.is_empty());
    Ok(())
}

#[test]
fn file_identity_lookup_errors_retain_indexer_stage_context_without_writes() -> Result<()> {
    use tempfile::tempdir;

    let dir = tempdir()?;
    let path = dir.path().join("lookup.rs");
    let storage = Storage::new_in_memory()?;
    storage.get_connection().execute("DROP TABLE node", [])?;

    let identity_error = WorkspaceIndexer::existing_projection_file_ids(
        &storage,
        dir.path(),
        std::slice::from_ref(&path),
        &HashMap::new(),
    )
    .expect_err("missing node storage must fail identity discovery");
    assert!(
        identity_error
            .to_string()
            .contains("Storage file identity lookup error"),
        "unexpected identity error: {identity_error:#}"
    );

    let symbol_error = WorkspaceIndexer::seed_symbol_table(
        &storage,
        &SymbolTable::new(),
        codestory_workspace::BuildMode::Incremental,
        &HashSet::from([1]),
    )
    .expect_err("missing node storage must fail symbol seeding");
    assert!(
        symbol_error
            .to_string()
            .contains("Storage symbol seed error"),
        "unexpected symbol error: {symbol_error:#}"
    );
    assert!(storage.get_files()?.is_empty());
    Ok(())
}

#[test]
fn test_run_incremental_helper_calls_are_indexed() -> Result<()> {
    use codestory_store::Store as Storage;
    use codestory_workspace::RefreshInfo;
    use std::collections::HashSet;
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let f1 = dir.path().join("indexer.rs");
    fs::write(
        &f1,
        r#"
            struct WorkspaceIndexer;
            impl WorkspaceIndexer {
                fn run_incremental(&self) {
                    Self::seed_symbol_table();
                    Self::flush_projection_batch();
                    Self::flush_errors();
                }
                fn seed_symbol_table() {}
                fn flush_projection_batch() {}
                fn flush_errors() {}
            }
        "#,
    )?;

    let mut storage = Storage::new_in_memory().unwrap();
    let bus = EventBus::new();
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let refresh_info = RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![f1.clone()],
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;

    let run_node_ids: HashSet<_> = storage
        .get_nodes()?
        .into_iter()
        .filter(|node| node.serialized_name.ends_with("run_incremental"))
        .map(|node| node.id)
        .collect();
    assert!(!run_node_ids.is_empty(), "run_incremental node not found");

    let edges = storage.get_edges()?;
    let mut callees = HashSet::new();
    for edge in edges {
        if edge.kind != EdgeKind::CALL || !run_node_ids.contains(&edge.source) {
            continue;
        }
        if let Some(callsite_identity) = edge.callsite_identity.as_ref()
            && !callsite_identity.is_empty()
        {
            callees.insert(callsite_identity.clone());
        }
        if let Some(target) = storage.get_node(edge.target)? {
            callees.insert(target.serialized_name);
        }
    }

    assert!(
        callees
            .iter()
            .any(|name| name.contains("seed_symbol_table")),
        "missing seed_symbol_table call edge; found: {:?}",
        callees
    );
    assert!(
        callees
            .iter()
            .any(|name| name.contains("flush_projection_batch")),
        "missing flush_projection_batch call edge; found: {:?}",
        callees
    );
    assert!(
        callees.iter().any(|name| name.contains("flush_errors")),
        "missing flush_errors call edge; found: {:?}",
        callees
    );

    Ok(())
}

#[test]
fn test_index_cpp_advanced() -> Result<()> {
    let code = r#"
class Base {};
class Derived : public Base {
    int x;
    void foo() {}
};
"#;
    let language_config = get_language_for_ext("cpp").unwrap();
    let result = index_file(Path::new("test.cpp"), code, &language_config, None, None)?;

    // Verify Membership
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.serialized_name == "Base" && n.kind == NodeKind::CLASS)
    );
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.serialized_name == "Derived" && n.kind == NodeKind::CLASS)
    );
    // Verify Membership
    assert!(result.edges.iter().any(|e| {
        e.kind == EdgeKind::MEMBER && e.certainty == Some(ResolutionCertainty::Certain)
    }));
    // Verify Inheritance (TODO: Fix structural matching for inheritance in single-pass TS queries)
    // assert!(result.edges.iter().any(|e| e.kind == EdgeKind::INHERITANCE));
    Ok(())
}

#[test]
fn test_index_python_advanced() -> Result<()> {
    let code = r#"
from os import path
@decorator
class MyClass:
    x = 1
"#;
    let language_config = get_language_for_ext("py").unwrap();
    let result = index_file(Path::new("test.py"), code, &language_config, None, None)?;

    // Verify Assignment Node
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.serialized_name == "x" && n.kind == NodeKind::VARIABLE)
    );
    // Verify IMPORT for import statement
    assert!(result.edges.iter().any(|e| {
        e.kind == EdgeKind::IMPORT && e.certainty == Some(ResolutionCertainty::Certain)
    }));
    // Verify CALL for decorator
    assert!(result.edges.iter().any(|e| e.kind == EdgeKind::CALL));
    Ok(())
}

#[test]
fn test_index_rust_advanced() -> Result<()> {
    let code = r#"
trait MyTrait {}
struct MyStruct;
impl MyTrait for MyStruct {}
fn main() {
    println!("Hello");
}
"#;
    let language_config = get_language_for_ext("rs").unwrap();
    let result = index_file(Path::new("main.rs"), code, &language_config, None, None)?;

    // Verify Trait Node
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.serialized_name == "MyTrait" && n.kind == NodeKind::INTERFACE)
    );
    // Verify Impl Inheritance
    assert!(result.edges.iter().any(|e| {
        e.kind == EdgeKind::INHERITANCE && e.certainty == Some(ResolutionCertainty::Certain)
    }));
    // Verify macro CALL
    assert!(result.edges.iter().any(|e| e.kind == EdgeKind::CALL));
    Ok(())
}

#[test]
fn test_index_rust_trait_impl_for_generic_type() -> Result<()> {
    let code = r#"
trait Listener {
    fn on_event(&mut self);
}

struct Wrapper<T> {
    inner: T,
}

impl<T> Listener for Wrapper<T> {
    fn on_event(&mut self) {}
}
"#;
    let language_config = get_language_for_ext("rs").unwrap();
    let result = index_file(Path::new("main.rs"), code, &language_config, None, None)?;

    let listener = result
        .nodes
        .iter()
        .find(|n| n.serialized_name == "Listener" && n.kind == NodeKind::INTERFACE)
        .expect("Listener interface not found");
    let wrapper = result
        .nodes
        .iter()
        .find(|n| n.serialized_name == "Wrapper" && n.kind == NodeKind::STRUCT)
        .unwrap_or_else(|| {
            panic!(
                "Wrapper type not found; nodes={:?}",
                result
                    .nodes
                    .iter()
                    .map(|n| (&n.serialized_name, &n.kind))
                    .collect::<Vec<_>>()
            )
        });

    assert!(
        result.edges.iter().any(|e| e.kind == EdgeKind::INHERITANCE
            && e.source == wrapper.id
            && e.target == listener.id),
        "INHERITANCE edge from Wrapper to Listener not found"
    );

    Ok(())
}

#[test]
fn test_rust_impl_anchor_normalization_handles_plain_scoped_and_generic_forms() -> Result<()> {
    let code = r#"
mod inner {
    pub trait Paint {}
    pub trait Label<T> {}
}

struct Widget;
struct Wrapper<T>(T);

impl Widget {
    fn plain(&self) {}
}

impl inner::Paint for Widget {}

impl Wrapper<Widget> {
    fn wrapped(&self) {}
}

impl inner::Label<Widget> for crate::Wrapper<Widget> {}
"#;
    let language_config = get_language_for_ext("rs").unwrap();
    let result = index_file(Path::new("main.rs"), code, &language_config, None, None)?;

    let widgets = result
        .nodes
        .iter()
        .filter(|node| node.serialized_name == "Widget" && node.kind == NodeKind::STRUCT)
        .collect::<Vec<_>>();
    assert_eq!(
        widgets.len(),
        1,
        "expected one canonical Widget struct node"
    );

    let wrappers = result
        .nodes
        .iter()
        .filter(|node| node.serialized_name == "Wrapper" && node.kind == NodeKind::STRUCT)
        .collect::<Vec<_>>();
    assert_eq!(
        wrappers.len(),
        1,
        "expected one canonical Wrapper struct node"
    );

    let paints = result
        .nodes
        .iter()
        .filter(|node| node.serialized_name == "Paint" && node.kind == NodeKind::INTERFACE)
        .collect::<Vec<_>>();
    assert_eq!(paints.len(), 1, "expected one canonical Paint trait node");

    let labels = result
        .nodes
        .iter()
        .filter(|node| node.serialized_name == "Label" && node.kind == NodeKind::INTERFACE)
        .collect::<Vec<_>>();
    assert_eq!(labels.len(), 1, "expected one canonical Label trait node");

    let plain = result
        .nodes
        .iter()
        .find(|node| node.serialized_name.ends_with("plain"))
        .expect("plain method");
    let wrapped = result
        .nodes
        .iter()
        .find(|node| node.serialized_name.ends_with("wrapped"))
        .expect("wrapped method");

    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::MEMBER && edge.source == widgets[0].id && edge.target == plain.id
    }));
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::MEMBER && edge.source == wrappers[0].id && edge.target == wrapped.id
    }));
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::INHERITANCE
            && edge.source == widgets[0].id
            && edge.target == paints[0].id
    }));
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::INHERITANCE
            && edge.source == wrappers[0].id
            && edge.target == labels[0].id
    }));

    Ok(())
}

#[test]
fn test_index_rust_local_binding_and_closure_assignment_distinguish_variable_and_function()
-> Result<()> {
    let code = r#"
fn sample(value: i32) -> i32 {
    let local = value + 1;
    let helper = |input: i32| input + local;
    helper(value)
}
"#;
    let language_config = get_language_for_ext("rs").unwrap();
    let result = index_file(Path::new("main.rs"), code, &language_config, None, None)?;

    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.serialized_name == "local" && n.kind == NodeKind::VARIABLE),
        "plain let binding should be indexed as VARIABLE"
    );
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.serialized_name == "helper" && n.kind == NodeKind::FUNCTION),
        "closure-backed let binding should be indexed as FUNCTION"
    );

    Ok(())
}

#[test]
fn test_call_edges_from_graph() -> Result<()> {
    let java_code = r#"
class Test {
    void caller() {
        callee();
    }
    void callee() {}
}
"#;
    let language_config = get_language_for_ext("java").unwrap();
    let result = index_file(
        Path::new("Test.java"),
        java_code,
        &language_config,
        None,
        None,
    )?;

    assert!(
        result.nodes.iter().any(
            |n| short_member_name(&n.serialized_name) == "caller" && n.kind == NodeKind::METHOD
        ),
        "Caller node not found"
    );
    assert!(
        result.nodes.iter().any(
            |n| short_member_name(&n.serialized_name) == "callee" && n.kind == NodeKind::METHOD
        ),
        "Callee node not found"
    );
    assert!(
        result.edges.iter().any(|e| e.kind == EdgeKind::CALL),
        "CALL edge not found"
    );

    Ok(())
}

#[test]
fn test_call_attribution_line_range() -> Result<()> {
    let java_code = r#"
class Test {
    void first() {}
    void second() {
        first();
    }
}
"#;
    let language_config = get_language_for_ext("java").unwrap();
    let result = index_file(
        Path::new("Test.java"),
        java_code,
        &language_config,
        None,
        None,
    )?;

    let caller = result
        .nodes
        .iter()
        .find(|n| short_member_name(&n.serialized_name) == "second")
        .expect("second() node not found");

    let call_edge = result
        .edges
        .iter()
        .find(|e| e.kind == EdgeKind::CALL)
        .expect("CALL edge not found");

    assert_eq!(call_edge.source, caller.id);
    Ok(())
}

#[test]
fn test_call_edges_same_line_preserve_distinct_callsites() {
    use std::collections::{HashMap, HashSet};

    let flags = IndexFeatureFlags {
        legacy_edge_identity: false,
        lazy_graph_execution: false,
    };
    let file_id = NodeId(1);
    let mut edges = vec![
        Edge {
            id: EdgeId(0),
            source: NodeId(10),
            target: NodeId(20),
            kind: EdgeKind::CALL,
            file_node_id: Some(file_id),
            line: Some(42),
            ..Default::default()
        },
        Edge {
            id: EdgeId(0),
            source: NodeId(10),
            target: NodeId(20),
            kind: EdgeKind::CALL,
            file_node_id: Some(file_id),
            line: Some(42),
            ..Default::default()
        },
    ];

    let mut callsite_ordinals: HashMap<(NodeId, Option<u32>), u32> = HashMap::new();
    for edge in &mut edges {
        let key = (edge.target, edge.line);
        let next = callsite_ordinals.entry(key).or_insert(0);
        *next = next.saturating_add(1);
        ensure_callsite_identity(edge, Some(*next));
        edge.id = EdgeId(generate_edge_id_for_edge(edge, flags));
    }

    let mut dedup = HashSet::new();
    let deduped = edges
        .into_iter()
        .filter(|edge| dedup.insert(edge_dedup_key(edge, flags)))
        .collect::<Vec<_>>();

    assert_eq!(deduped.len(), 2, "expected one edge per callsite");
    let identities = deduped
        .iter()
        .map(|edge| edge.callsite_identity.clone().unwrap_or_default())
        .collect::<HashSet<_>>();
    assert_eq!(
        identities.len(),
        2,
        "callsites should have unique identities"
    );
    let edge_ids = deduped.iter().map(|edge| edge.id).collect::<HashSet<_>>();
    assert_eq!(edge_ids.len(), 2, "callsites should have unique edge ids");
}

#[test]
fn test_runtime_import_call_suppression_uses_callsite_column() {
    let file_id = NodeId(1);
    let require_id = NodeId(20);
    let module_id = NodeId(30);
    let nodes = vec![
        Node {
            id: require_id,
            kind: NodeKind::UNKNOWN,
            serialized_name: "require".to_string(),
            start_line: Some(42),
            start_col: Some(1),
            ..Default::default()
        },
        Node {
            id: module_id,
            kind: NodeKind::MODULE,
            serialized_name: "\"./workflow\"".to_string(),
            start_line: Some(42),
            start_col: Some(9),
            ..Default::default()
        },
    ];
    let mut edges = vec![
        Edge {
            id: EdgeId(1),
            source: NodeId(10),
            target: require_id,
            kind: EdgeKind::CALL,
            file_node_id: Some(file_id),
            line: Some(42),
            callsite_identity: canonical_callsite_identity(
                Some(file_id),
                Some(42),
                Some(1),
                require_id,
            ),
            ..Default::default()
        },
        Edge {
            id: EdgeId(2),
            source: NodeId(11),
            target: require_id,
            kind: EdgeKind::CALL,
            file_node_id: Some(file_id),
            line: Some(42),
            callsite_identity: canonical_callsite_identity(
                Some(file_id),
                Some(42),
                Some(23),
                require_id,
            ),
            ..Default::default()
        },
    ];
    let specs = vec![RuntimeImportSpec {
        binding_node_id: None,
        module_node_id: module_id,
        line: 42,
        suppress_line: 42,
        suppress_start_col: 1,
        suppress_callee_name: "require".to_string(),
        exact_bare_call_target_spans: Vec::new(),
    }];

    suppress_runtime_import_call_edges(&nodes, &mut edges, &specs);

    assert_eq!(
        edges.len(),
        1,
        "only the exact import call should be suppressed"
    );
    assert_eq!(
        edges[0].callsite_identity,
        canonical_callsite_identity(Some(file_id), Some(42), Some(23), require_id),
        "same-line non-import callsite should remain"
    );
}

#[test]
fn test_legacy_edge_identity_dedup_ignores_callsite_identity() {
    let edge_a = Edge {
        id: EdgeId(1),
        source: NodeId(10),
        target: NodeId(20),
        kind: EdgeKind::CALL,
        line: Some(42),
        callsite_identity: Some("10:42:1:20".to_string()),
        ..Default::default()
    };
    let edge_b = Edge {
        id: EdgeId(2),
        source: NodeId(10),
        target: NodeId(20),
        kind: EdgeKind::CALL,
        line: Some(42),
        callsite_identity: Some("10:42:2:20".to_string()),
        ..Default::default()
    };

    let modern_flags = IndexFeatureFlags {
        legacy_edge_identity: false,
        lazy_graph_execution: false,
    };
    let legacy_flags = IndexFeatureFlags {
        legacy_edge_identity: true,
        lazy_graph_execution: false,
    };
    assert_ne!(
        edge_dedup_key(&edge_a, modern_flags),
        edge_dedup_key(&edge_b, modern_flags),
        "modern identity should differentiate callsites"
    );
    assert_eq!(
        edge_dedup_key(&edge_a, legacy_flags),
        edge_dedup_key(&edge_b, legacy_flags),
        "legacy identity should collapse callsites"
    );
}

#[test]
fn test_run_incremental_emits_compile_db_warning_on_load_failure() -> Result<()> {
    use codestory_store::Store as Storage;
    use codestory_workspace::RefreshInfo;
    use std::fs;
    use std::time::Duration;
    use tempfile::tempdir;

    let dir = tempdir()?;
    fs::write(
        dir.path().join("compile_commands.json"),
        "{ this is not valid json ",
    )?;
    let file = dir.path().join("main.rs");
    fs::write(&file, "fn main() {}")?;

    let mut storage = Storage::new_in_memory().unwrap();
    let bus = EventBus::new();
    let rx = bus.receiver();
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let refresh_info = RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![file],
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;

    let mut saw_warning = false;
    for _ in 0..32 {
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(Event::ShowWarning { message }) => {
                if message.contains("compile_commands.json") {
                    saw_warning = true;
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    assert!(
        saw_warning,
        "expected compile_commands warning event when loading fails"
    );
    Ok(())
}

#[test]
fn test_node_kind_mapping_preserves_method_and_field() {
    assert_eq!(node_kind_from_graph_kind("METHOD"), NodeKind::METHOD);
    assert_eq!(node_kind_from_graph_kind("FIELD"), NodeKind::FIELD);
    assert_eq!(node_kind_from_graph_kind("INTERFACE"), NodeKind::INTERFACE);
}

#[test]
fn test_header_language_defaults_to_c_without_compilation_metadata() {
    let config = get_language_for_ext("h").expect("header extension should resolve");
    assert_eq!(config.language_name, "c");
}

#[test]
fn test_header_language_uses_cpp_when_compilation_standard_is_cxx() {
    let info = compilation_database::CompilationInfo {
        standard: Some(compilation_database::CxxStandard::Cxx20),
        ..Default::default()
    };
    let config = get_language_config_for_path(Path::new("widget.h"), Some(&info)).expect("config");
    assert_eq!(config.language_name, "cpp");
}

#[test]
fn test_live_rule_registry_uses_split_rule_assets() {
    // Rust's split rule assets moved into `languages::rust`; the config must
    // still come back through the same extension lookup, and it must still
    // be the only graph rule file that pairs with a tags query.
    let rust = get_language_for_ext("rs").expect("rust config");
    let rust_row = languages::extraction_for_ext("rs").expect("rust registry row");
    assert_eq!(rust.language_name, "rust");
    assert_eq!(rust.graph_query, rust_row.graph_query);
    assert_eq!(rust.tags_query, rust_row.tags_query);
    assert!(rust.tags_query.is_some());
    assert_ne!(rust.graph_query, rust.tags_query.expect("rust tags query"));

    // TypeScript's rule files moved into `languages::typescript`; the
    // config must still come back through the same extension lookup, and
    // TSX must still reuse the tags query it always shared.
    let ts = get_language_for_ext("ts").expect("ts config");
    let ts_row = languages::extraction_for_ext("ts").expect("typescript registry row");
    assert_eq!(ts.language_name, "typescript");
    assert_eq!(ts.graph_query, ts_row.graph_query);
    assert_eq!(ts.tags_query, ts_row.tags_query);
    assert!(ts.graph_query.contains("ts_member"));
    assert!(ts.tags_query.is_some());

    // TSX's rule files moved into `languages::tsx`; the config must still
    // come back through the same extension lookup, still on the TSX
    // grammar, and still sharing TypeScript's tags query.
    let tsx = get_language_for_ext("tsx").expect("tsx config");
    assert_eq!(tsx.language_name, "typescript");
    assert_eq!(
        tsx.graph_query,
        languages::extraction_for_ext("tsx")
            .expect("tsx registry row")
            .graph_query
    );
    assert_ne!(tsx.graph_query, ts.graph_query);
    assert_eq!(tsx.tags_query, Some(languages::typescript::TAGS_QUERY));
    assert_eq!(tsx.tags_query, ts.tags_query);
    assert_ne!(tsx.graph_query, ts.graph_query);

    // Kotlin's rule file moved into `languages::kotlin`; the config must
    // still come back through the same extension lookup.
    let kotlin = get_language_for_ext("kt").expect("kotlin config");
    assert_eq!(kotlin.language_name, "kotlin");
    assert_eq!(
        kotlin.graph_query,
        languages::extraction_for_ext("kt")
            .expect("kotlin registry row")
            .graph_query
    );
    assert!(kotlin.graph_query.contains("kotlin_member"));
    assert!(kotlin.tags_query.is_none());
    let kotlin_script = get_language_for_ext("kts").expect("kotlin script config");
    assert_eq!(kotlin_script.graph_query, kotlin.graph_query);

    // Swift's rule file moved into `languages::swift`; the config must
    // still come back through the same extension lookup.
    let swift = get_language_for_ext("swift").expect("swift config");
    assert_eq!(swift.language_name, "swift");
    assert_eq!(
        swift.graph_query,
        languages::extraction_for_ext("swift")
            .expect("swift registry row")
            .graph_query
    );
    assert!(swift.graph_query.contains("swift_member"));
    assert!(swift.tags_query.is_none());

    // Dart's rule file moved into `languages::dart`; the config must still
    // come back through the same extension lookup.
    let dart = get_language_for_ext("dart").expect("dart config");
    assert_eq!(dart.language_name, "dart");
    assert_eq!(
        dart.graph_query,
        languages::extraction_for_ext("dart")
            .expect("dart registry row")
            .graph_query
    );
    assert!(dart.graph_query.contains("dart_member"));
    assert!(dart.tags_query.is_none());

    // Bash moved into `languages::bash`; the config it hands back must
    // still come back through the same extension lookup.
    let bash = get_language_for_ext("sh").expect("bash config");
    assert_eq!(bash.language_name, "bash");
    assert_eq!(
        bash.graph_query,
        languages::extraction_for_ext("sh")
            .expect("bash registry row")
            .graph_query
    );
    assert!(bash.tags_query.is_none());
    let bash_extension = get_language_for_ext("bash").expect("bash extension config");
    assert_eq!(bash_extension.graph_query, bash.graph_query);
}

#[test]
fn test_language_support_profiles_separate_runtime_claims() {
    let rust = language_support_profile_for_ext("rs").expect("rust profile");
    assert_eq!(rust.support_mode, LanguageSupportMode::ParserBackedGraph);
    assert_eq!(rust.evidence_tier, LanguageEvidenceTier::GraphFidelity);
    assert_eq!(rust.claim_label, "parser-backed graph, fidelity-gated");

    let go = language_support_profile_for_ext("go").expect("go profile");
    assert_eq!(go.support_mode, LanguageSupportMode::ParserBackedGraph);
    assert_eq!(go.evidence_tier, LanguageEvidenceTier::GraphFidelity);
    assert_eq!(go.claim_label, "parser-backed graph, fidelity-gated");

    let structural = language_support_profile_for_ext("html").expect("html profile");
    assert_eq!(
        structural.support_mode,
        LanguageSupportMode::StructuralCollector
    );
    assert_eq!(
        structural.evidence_tier,
        LanguageEvidenceTier::StructuralOnly
    );
    assert!(
        language_support_profile_for_ext("cshtml").is_none(),
        ".cshtml stays compatibility-only until Razor support has a public profile"
    );
    assert!(
        get_language_for_ext("cshtml").is_none(),
        ".cshtml must not route into parser-backed indexing without a public profile"
    );

    for profile in codestory_contracts::language_support::LANGUAGE_SUPPORT_PROFILES {
        if profile.support_mode == LanguageSupportMode::ParserBackedGraph {
            for ext in profile.extensions {
                assert_eq!(profile.evidence_tier, LanguageEvidenceTier::GraphFidelity);
                assert!(
                    get_language_for_ext(ext).is_some(),
                    "parser-backed language {} extension {} must route into live indexing",
                    profile.language_name,
                    ext
                );
            }
        }
    }
}

#[test]
fn test_compiled_rules_cache_reuses_compiled_artifacts() -> Result<()> {
    let config = get_language_for_ext("tsx").expect("tsx config");
    let first = config.compiled_rules()? as *const CompiledLanguageRules;
    let second = config.compiled_rules()? as *const CompiledLanguageRules;
    assert_eq!(
        first, second,
        "compiled rules should be cached per language"
    );
    Ok(())
}

#[test]
fn test_dart_graph_query_tracks_grammar_0_4_call_shapes_without_duplicates() -> Result<()> {
    let config = get_language_for_ext("dart").expect("dart config");
    let direct = execute_raw_graph_contract(
        Path::new("direct.dart"),
        r#"
void bareHelper() {}
void genericHelper<T>() {}
void repeatedHelper() {}

void calls() {
  bareHelper();
  genericHelper<int>();
  repeatedHelper(); repeatedHelper();
}
"#,
        &config,
    )?;
    assert!(
        !direct.has_parse_error,
        "direct-call fixture must parse cleanly"
    );
    for (target, expected, shape) in [
        ("bareHelper", 1, "bare"),
        ("genericHelper", 1, "generic"),
        ("repeatedHelper", 2, "repeated same-line"),
    ] {
        assert_eq!(
            direct.call_counts.get(&(target.to_string(), None)).copied(),
            Some(expected),
            "{shape} calls should each emit exactly one direct placeholder"
        );
    }

    let member = execute_raw_graph_contract(
        Path::new("member.dart"),
        r#"
class Worker {
  void runPlain() {}
  void runGeneric<T>() {}
}

void calls(Worker worker) {
  worker.runPlain();
  worker.runGeneric<int>();
}
"#,
        &config,
    )?;
    assert!(
        !member.has_parse_error,
        "member-call fixture must parse cleanly"
    );
    for (target, shape) in [("runPlain", "plain"), ("runGeneric", "generic")] {
        assert_eq!(
            member
                .call_counts
                .get(&(target.to_string(), Some("dart_member".to_string())))
                .copied(),
            Some(1),
            "{shape} selector-based member call should stay on the member path"
        );
        assert_eq!(
            member
                .call_counts
                .get(&(target.to_string(), None))
                .copied()
                .unwrap_or_default(),
            0,
            "{shape} member call must not also emit a direct placeholder"
        );
    }

    let complex_receivers = execute_raw_graph_contract(
        Path::new("complex_receivers.dart"),
        r#"
class Worker {
  int run() => 1;
  int save() => 2;
}

void calls(Worker worker) {
  final values = [worker.run(), worker.save()];
  final matched = worker.run() == worker.save();
  final selected = true ? worker.run() : worker.save();
}
"#,
        &config,
    )?;
    assert!(
        !complex_receivers.has_parse_error,
        "complex receiver fixture must parse cleanly"
    );
    for target in ["run", "save"] {
        assert_eq!(
            complex_receivers
                .call_counts
                .get(&(target.to_string(), Some("dart_member".to_string())))
                .copied(),
            Some(3),
            "multiple member calls inside one expression must bind distinct graph nodes"
        );
        assert_eq!(
            complex_receivers
                .call_counts
                .get(&(target.to_string(), None))
                .copied()
                .unwrap_or_default(),
            0,
            "complex member calls must not also emit direct placeholders"
        );
    }

    let unsupported_selectors = execute_raw_graph_contract(
        Path::new("selectors.dart"),
        r#"
class Worker {
  void run() {}
  void save() {}
}

void calls(Worker? worker) {
  worker?.run();
  worker?..run()..save();
}
"#,
        &config,
    )?;
    assert!(
        !unsupported_selectors.has_parse_error,
        "null-aware and cascade fixture must parse cleanly"
    );
    for target in ["run", "save"] {
        assert_eq!(
            unsupported_selectors
                .call_counts
                .get(&(target.to_string(), None))
                .copied()
                .unwrap_or_default(),
            0,
            "null-aware and cascade selectors must never be misclassified as direct calls"
        );
        assert_eq!(
            unsupported_selectors
                .call_counts
                .get(&(target.to_string(), Some("dart_member".to_string()),))
                .copied()
                .unwrap_or_default(),
            0,
            "the graph query makes no null-aware or cascade member-call claim"
        );
    }

    let chained = execute_raw_graph_contract(
        Path::new("chained.dart"),
        r#"
class Worker {
  void run() {}
}

Worker factory() => Worker();

void calls() {
  factory().run();
}
"#,
        &config,
    )?;
    assert!(
        !chained.has_parse_error,
        "chained-call fixture must parse cleanly"
    );
    assert_eq!(
        chained
            .call_counts
            .get(&("factory".to_string(), None))
            .copied(),
        Some(1),
        "the inner bare factory call should remain visible"
    );
    assert_eq!(
        chained
            .call_counts
            .get(&("run".to_string(), None))
            .copied()
            .unwrap_or_default(),
        0,
        "the chained receiver method must not be stolen by the direct-call rule"
    );
    assert_eq!(
        chained
            .call_counts
            .get(&("run".to_string(), Some("dart_member".to_string())))
            .copied()
            .unwrap_or_default(),
        0,
        "the graph query makes no chained-receiver member-call claim"
    );

    Ok(())
}

#[test]
fn test_raw_graph_contracts_cover_supported_languages() -> Result<()> {
    let python = execute_raw_graph_contract(
        Path::new("sample.py"),
        r#"
from app.helpers import tool

class Worker:
    def run(self):
        tool()
"#,
        &get_language_for_ext("py").expect("python config"),
    )?;
    assert!(
        python
            .nodes
            .contains(&("CLASS".to_string(), "Worker".to_string()))
    );
    assert!(python.edges.contains(&(
        "Worker".to_string(),
        "run".to_string(),
        "MEMBER".to_string()
    )));

    let java = execute_raw_graph_contract(
        Path::new("Sample.java"),
        r#"
class Base {}
class Child extends Base {
    void run() {}
}
"#,
        &get_language_for_ext("java").expect("java config"),
    )?;
    assert!(java.edges.contains(&(
        "Child".to_string(),
        "Base".to_string(),
        "INHERITANCE".to_string()
    )));

    let rust = execute_raw_graph_contract(
        Path::new("main.rs"),
        r#"
use crate::helpers::tool;

struct Worker;

impl Worker {
    fn run(&self) {
        tool::<u32>();
    }
}
"#,
        &get_language_for_ext("rs").expect("rust config"),
    )?;
    assert!(
        rust.nodes
            .contains(&("STRUCT".to_string(), "Worker".to_string()))
    );
    assert!(rust.edges.contains(&(
        "crate::helpers::tool".to_string(),
        "crate::helpers::tool".to_string(),
        "IMPORT".to_string()
    )));

    let javascript = execute_raw_graph_contract(
        Path::new("main.js"),
        r#"
import thing from "./dep";

function run() {
    thing();
}
"#,
        &get_language_for_ext("js").expect("javascript config"),
    )?;
    assert!(javascript.edges.contains(&(
        "\"./dep\"".to_string(),
        "\"./dep\"".to_string(),
        "IMPORT".to_string()
    )));
    assert!(javascript.edges.contains(&(
        "thing".to_string(),
        "thing".to_string(),
        "CALL".to_string()
    )));

    let typescript = execute_raw_graph_contract(
        Path::new("main.ts"),
        r#"
interface Base {}
interface Child extends Base {}
"#,
        &get_language_for_ext("ts").expect("typescript config"),
    )?;
    assert!(typescript.edges.contains(&(
        "Child".to_string(),
        "Base".to_string(),
        "INHERITANCE".to_string()
    )));

    let tsx = execute_raw_graph_contract(
        Path::new("main.tsx"),
        r#"
type Props = { label: string };

function Badge(props: Props) {
    return <span>{props.label}</span>;
}

class View {
    render() {
        return <Badge label="hi" />;
    }
}
"#,
        &get_language_for_ext("tsx").expect("tsx config"),
    )?;
    assert!(tsx.edges.contains(&(
        "render".to_string(),
        "Badge".to_string(),
        "CALL".to_string()
    )));
    assert!(tsx.edges.contains(&(
        "render".to_string(),
        "label".to_string(),
        "USAGE".to_string()
    )));

    let cpp = execute_raw_graph_contract(
        Path::new("main.cpp"),
        r#"
struct Base {};

template <typename T>
struct Wrapper {};

struct Child : Base {
    Wrapper<int> value;
};
"#,
        &get_language_for_ext("cpp").expect("cpp config"),
    )?;
    assert!(cpp.edges.contains(&(
        "Child".to_string(),
        "Base".to_string(),
        "INHERITANCE".to_string()
    )));

    let c = execute_raw_graph_contract(
        Path::new("main.h"),
        r#"
typedef struct Worker {
    int value;
} Worker;
"#,
        &get_language_for_ext("h").expect("c config"),
    )?;
    assert!(c.edges.contains(&(
        "Worker".to_string(),
        "value".to_string(),
        "MEMBER".to_string()
    )));

    let kotlin = execute_raw_graph_contract(
        Path::new("Main.kt"),
        r#"
package demo.game

import demo.tools.Helper

open class Base

class Worker : Base() {
    fun run() {
        helper()
    }
}

fun helper() {}
typealias Alias = Worker
"#,
        &get_language_for_ext("kt").expect("kotlin config"),
    )?;
    assert!(
        kotlin
            .nodes
            .contains(&("CLASS".to_string(), "Worker".to_string()))
    );
    assert!(
        kotlin
            .nodes
            .contains(&("FUNCTION".to_string(), "helper".to_string()))
    );
    assert!(kotlin.edges.contains(&(
        "Worker".to_string(),
        "run".to_string(),
        "MEMBER".to_string()
    )));
    assert!(
        kotlin.edges.contains(&(
            "Worker".to_string(),
            "Base".to_string(),
            "INHERITANCE".to_string()
        )),
        "kotlin raw graph nodes: {:?}; edges: {:?}",
        kotlin.nodes,
        kotlin.edges
    );
    assert!(kotlin.edges.contains(&(
        "helper".to_string(),
        "helper".to_string(),
        "CALL".to_string()
    )));
    assert!(kotlin.edges.contains(&(
        "demo.tools.Helper".to_string(),
        "demo.tools.Helper".to_string(),
        "IMPORT".to_string()
    )));

    let swift = execute_raw_graph_contract(
        Path::new("Main.swift"),
        r#"
import Foundation

protocol Runnable {
    func run()
}

class Base {}

class Worker: Base, Runnable {
    func run() {
        helper()
    }
}

func helper() {}
typealias Alias = Worker
"#,
        &get_language_for_ext("swift").expect("swift config"),
    )?;
    assert!(
        swift
            .nodes
            .contains(&("CLASS".to_string(), "Worker".to_string()))
    );
    assert!(
        swift
            .nodes
            .contains(&("INTERFACE".to_string(), "Runnable".to_string()))
    );
    assert!(
        swift
            .nodes
            .contains(&("FUNCTION".to_string(), "helper".to_string()))
    );
    assert!(swift.edges.contains(&(
        "Worker".to_string(),
        "run".to_string(),
        "MEMBER".to_string()
    )));
    assert!(swift.edges.contains(&(
        "Worker".to_string(),
        "Base".to_string(),
        "INHERITANCE".to_string()
    )));
    assert!(swift.edges.contains(&(
        "helper".to_string(),
        "helper".to_string(),
        "CALL".to_string()
    )));
    assert!(swift.edges.contains(&(
        "Foundation".to_string(),
        "Foundation".to_string(),
        "IMPORT".to_string()
    )));

    let dart = execute_raw_graph_contract(
        Path::new("main.dart"),
        r#"
import 'dart:math';

class Base {}

class Worker extends Base {
  void run() {
    helper();
  }
}

void helper() {}
"#,
        &get_language_for_ext("dart").expect("dart config"),
    )?;
    assert!(
        dart.nodes
            .contains(&("CLASS".to_string(), "Worker".to_string()))
    );
    assert!(
        dart.nodes
            .contains(&("FUNCTION".to_string(), "helper".to_string()))
    );
    assert!(dart.edges.contains(&(
        "Worker".to_string(),
        "run".to_string(),
        "MEMBER".to_string()
    )));
    assert!(dart.edges.contains(&(
        "Worker".to_string(),
        "Base".to_string(),
        "INHERITANCE".to_string()
    )));
    assert!(dart.edges.contains(&(
        "helper".to_string(),
        "helper".to_string(),
        "CALL".to_string()
    )));
    assert!(dart.edges.contains(&(
        "'dart:math'".to_string(),
        "'dart:math'".to_string(),
        "IMPORT".to_string()
    )));

    let bash = execute_raw_graph_contract(
        Path::new("main.sh"),
        r#"
NAME=world

helper() {
  echo "$NAME"
}

main() {
  helper
}

main
"#,
        &get_language_for_ext("sh").expect("bash config"),
    )?;
    assert!(
        bash.nodes
            .contains(&("FUNCTION".to_string(), "helper".to_string()))
    );
    assert!(
        bash.nodes
            .contains(&("VARIABLE".to_string(), "NAME".to_string()))
    );
    assert!(bash.edges.contains(&(
        "helper".to_string(),
        "helper".to_string(),
        "CALL".to_string()
    )));
    assert!(
        bash.edges
            .contains(&("main".to_string(), "main".to_string(), "CALL".to_string()))
    );

    Ok(())
}

#[test]
fn test_live_rule_parsers_expose_key_node_kinds() {
    let python_kinds = parser_node_kinds(tree_sitter_python::LANGUAGE.into());
    for kind in ["class_definition", "function_definition", "call"] {
        assert!(
            python_kinds.contains(kind),
            "python grammar should expose {kind}"
        );
    }

    let java_kinds = parser_node_kinds(tree_sitter_java::LANGUAGE.into());
    for kind in [
        "class_declaration",
        "method_declaration",
        "method_invocation",
    ] {
        assert!(
            java_kinds.contains(kind),
            "java grammar should expose {kind}"
        );
    }

    let rust_kinds = parser_node_kinds(tree_sitter_rust::LANGUAGE.into());
    for kind in [
        "struct_item",
        "impl_item",
        "call_expression",
        "use_declaration",
    ] {
        assert!(
            rust_kinds.contains(kind),
            "rust grammar should expose {kind}"
        );
    }

    let js_kinds = parser_node_kinds(tree_sitter_javascript::LANGUAGE.into());
    for kind in [
        "function_declaration",
        "call_expression",
        "import_statement",
    ] {
        assert!(
            js_kinds.contains(kind),
            "javascript grammar should expose {kind}"
        );
    }

    let ts_kinds = parser_node_kinds(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into());
    for kind in [
        "interface_declaration",
        "class_declaration",
        "method_definition",
        "generic_type",
    ] {
        assert!(
            ts_kinds.contains(kind),
            "typescript grammar should expose {kind}"
        );
    }

    let tsx_kinds = parser_node_kinds(tree_sitter_typescript::LANGUAGE_TSX.into());
    for kind in [
        "jsx_element",
        "jsx_self_closing_element",
        "jsx_expression",
        "jsx_attribute",
    ] {
        assert!(tsx_kinds.contains(kind), "tsx grammar should expose {kind}");
    }

    let cpp_kinds = parser_node_kinds(tree_sitter_cpp::LANGUAGE.into());
    for kind in ["template_type", "field_declaration", "class_specifier"] {
        assert!(cpp_kinds.contains(kind), "cpp grammar should expose {kind}");
    }

    let c_kinds = parser_node_kinds(tree_sitter_c::LANGUAGE.into());
    for kind in ["struct_specifier", "field_declaration", "type_definition"] {
        assert!(c_kinds.contains(kind), "c grammar should expose {kind}");
    }

    let kotlin_kinds = parser_node_kinds(tree_sitter_kotlin_ng::LANGUAGE.into());
    for kind in [
        "class_declaration",
        "function_declaration",
        "call_expression",
        "import",
        "delegation_specifier",
    ] {
        assert!(
            kotlin_kinds.contains(kind),
            "kotlin grammar should expose {kind}"
        );
    }

    let swift_kinds = parser_node_kinds(tree_sitter_swift::LANGUAGE.into());
    for kind in [
        "class_declaration",
        "protocol_declaration",
        "function_declaration",
        "call_expression",
        "import_declaration",
    ] {
        assert!(
            swift_kinds.contains(kind),
            "swift grammar should expose {kind}"
        );
    }

    let dart_kinds = parser_node_kinds(tree_sitter_dart_orchard::LANGUAGE.into());
    for kind in [
        "class_definition",
        "function_signature",
        "method_signature",
        "selector",
        "argument_part",
        "import_specification",
    ] {
        assert!(
            dart_kinds.contains(kind),
            "dart grammar should expose {kind}"
        );
    }

    let bash_kinds = parser_node_kinds(tree_sitter_bash::LANGUAGE.into());
    for kind in [
        "function_definition",
        "command",
        "command_name",
        "variable_assignment",
    ] {
        assert!(
            bash_kinds.contains(kind),
            "bash grammar should expose {kind}"
        );
    }
}

#[test]
fn test_cpp_template_type_arguments_support_multiline_and_nested_templates() -> Result<()> {
    let cpp_code = r#"
struct Key {};
struct Value {};

template <typename T>
struct Wrapper {};

template <typename T, typename U>
struct PairStore {};

struct Holder {
    PairStore<
        Key,
        Wrapper<Value> // keep nested templates and comments parse-driven
    > store;
};
"#;
    let language_config = get_language_for_ext("cpp").expect("cpp config");
    let result = index_file(
        Path::new("holder.cpp"),
        cpp_code,
        &language_config,
        None,
        None,
    )?;

    let node_by_id = result
        .nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let has_type_argument = |source_suffix: &str, target_suffix: &str| {
        result.edges.iter().any(|edge| {
            edge.kind == EdgeKind::TYPE_ARGUMENT
                && node_by_id
                    .get(&edge.source)
                    .is_some_and(|node| node.serialized_name.ends_with(source_suffix))
                && node_by_id
                    .get(&edge.target)
                    .is_some_and(|node| node.serialized_name.ends_with(target_suffix))
        })
    };

    assert!(
        has_type_argument("PairStore", "Key"),
        "expected PairStore -> Key type argument edge"
    );
    assert!(
        has_type_argument("PairStore", "Wrapper"),
        "expected PairStore -> Wrapper type argument edge"
    );

    Ok(())
}

#[test]
fn test_incomplete_parse_marks_file_incomplete() -> Result<()> {
    let code = "fn broken( {\n";
    let language_config = get_language_for_ext("rs").unwrap();
    let result = index_file(Path::new("broken.rs"), code, &language_config, None, None)?;
    assert_eq!(result.files.len(), 1);
    assert!(
        !result.files[0].complete,
        "malformed syntax should mark the file incomplete"
    );
    Ok(())
}

#[test]
fn test_jsx_component_and_prop_usage_recovery_matches_tsx_behavior() -> Result<()> {
    let code = r#"
function Badge(props) {
    return <span>{props.label}</span>;
}

function render() {
    return <Badge label="hi" />;
}
"#;
    let language_config = get_language_for_ext("jsx").expect("jsx config");
    let result = index_file(Path::new("App.jsx"), code, &language_config, None, None)?;
    let node_by_id = result
        .nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();

    assert!(
        result.edges.iter().any(|edge| {
            edge.kind == EdgeKind::CALL
                && node_by_id
                    .get(&edge.source)
                    .is_some_and(|node| node.serialized_name == "render")
                && node_by_id
                    .get(&edge.target)
                    .is_some_and(|node| node.serialized_name == "Badge")
        }),
        "expected JSX component call recovery to link render() to Badge"
    );
    assert!(
        result.edges.iter().any(|edge| {
            edge.kind == EdgeKind::USAGE
                && node_by_id
                    .get(&edge.source)
                    .is_some_and(|node| node.serialized_name == "render")
                && node_by_id
                    .get(&edge.target)
                    .is_some_and(|node| node.serialized_name == "label")
        }),
        "expected JSX prop usage recovery to link render() to label"
    );
    Ok(())
}

#[test]
fn test_openapi_schema_indexes_endpoint_symbols() -> Result<()> {
    let schema = r#"{
  "openapi": "3.1.0",
  "paths": {
    "/api/users": {
      "get": {
        "operationId": "listUsers"
      }
    }
  }
}"#;
    let storage = index_openapi_schema_file(Path::new("openapi.json"), schema)?
        .expect("schema should be indexed");
    let endpoint_id = schema_endpoint_node_id("GET", "/api/users");
    assert!(storage.nodes.iter().any(|node| {
        node.id == endpoint_id
            && node.kind == NodeKind::FUNCTION
            && node.serialized_name == "GET /api/users"
    }));
    assert!(storage.edges.iter().any(|edge| {
        edge.kind == EdgeKind::MEMBER
            && edge.target == endpoint_id
            && edge.certainty == Some(ResolutionCertainty::Certain)
    }));
    Ok(())
}

#[test]
fn workspace_openapi_routing_precedes_generic_json_structural_collection() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("openapi.json");
    std::fs::write(
        &path,
        r#"{
  "openapi": "3.1.0",
  "paths": {
    "/health": {
      "get": {}
    }
  }
}"#,
    )?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let storage = match indexer.prepare_openapi_schema_work(&path) {
        Ok(Some(storage)) => storage,
        Ok(None) => panic!("expected dedicated OpenAPI projection"),
        Err(_) => panic!("OpenAPI preparation failed"),
    };
    assert_eq!(storage.files[0].language, "openapi");
    assert!(storage.structural_text_units.is_empty());
    assert!(storage.nodes.iter().any(|node| {
        node.canonical_id
            .as_deref()
            .is_some_and(|value| value == "openapi:endpoint:GET /health")
    }));
    Ok(())
}

#[test]
fn openapi_components_only_schema_emits_no_endpoint_anchors() -> Result<()> {
    let schema = r#"{
  "openapi": "3.1.0",
  "components": {
    "schemas": {
      "User": {
        "type": "object"
      }
    }
  }
}"#;

    let storage = index_openapi_schema_file(Path::new("openapi.json"), schema)?;

    assert!(storage.is_none());
    Ok(())
}

#[test]
fn generic_yaml_with_paths_key_is_not_openapi() -> Result<()> {
    let yaml = r#"name: build
paths:
  cache: target
"#;

    let storage = index_openapi_schema_file(Path::new("config.yml"), yaml)?;

    assert!(storage.is_none());
    Ok(())
}

#[test]
fn github_actions_workflow_with_openapi_keys_stays_structural() -> Result<()> {
    let temp = tempdir()?;
    let workflow_dir = temp.path().join(".github").join("workflows");
    std::fs::create_dir_all(&workflow_dir)?;
    let workflow = workflow_dir.join("api.yml");
    std::fs::write(
        &workflow,
        r#"name: API
on:
  push:
openapi: 3.1.0
paths:
  /api/users:
    get:
      operationId: listUsers
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
"#,
    )?;

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(temp.path().to_path_buf());
    let bus = EventBus::new();
    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![workflow.clone()],
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;

    let files = storage.get_files()?;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].language, "github_actions_workflow");
    let nodes = storage.get_nodes()?;
    assert!(
        nodes.iter().any(|node| {
            node.kind == NodeKind::FUNCTION
                && node.serialized_name == "build"
                && node
                    .canonical_id
                    .as_deref()
                    .is_some_and(|value| value.contains("github-actions:job:"))
        }),
        "workflow job anchor should be indexed"
    );
    assert!(
        nodes.iter().all(|node| !node
            .canonical_id
            .as_deref()
            .unwrap_or_default()
            .starts_with("openapi:")),
        "workflow must not be indexed as OpenAPI"
    );
    Ok(())
}

#[test]
fn test_text_only_svelte_file_records_file_inventory() -> Result<()> {
    let temp = tempdir()?;
    let path = temp.path().join("App.svelte");
    std::fs::write(
        &path,
        "<script>\n  import { invoke } from '@tauri-apps/api/core';\n</script>\n",
    )?;

    let storage = index_text_only_file(&path)?;

    assert_eq!(storage.files.len(), 1);
    assert_eq!(storage.files[0].language, "svelte");
    assert_eq!(storage.files[0].path, path);
    assert!(storage.nodes.iter().any(|node| node.kind == NodeKind::FILE));
    assert!(storage.edges.is_empty());
    assert_eq!(storage.file_content_hashes.len(), 1);
    assert_eq!(
        storage.file_content_hashes[0].content_hash,
        source_content_hash(
            b"<script>\n  import { invoke } from '@tauri-apps/api/core';\n</script>\n"
        )
    );
    Ok(())
}

#[test]
fn template_collector_records_verified_source_hash() -> Result<()> {
    let source = "<script>export const answer = 42</script>\n";
    let storage = index_template_file(
        Path::new("src/Answer.svelte"),
        template_pipeline::TemplateKind::Svelte,
        source,
    )?;

    assert_eq!(storage.file_content_hashes.len(), 1);
    assert_eq!(storage.file_content_hashes[0].file_id, storage.files[0].id);
    assert_eq!(
        storage.file_content_hashes[0].content_hash,
        source_content_hash(source.as_bytes())
    );
    Ok(())
}

#[test]
fn same_basename_templates_keep_distinct_symbol_identities() -> Result<()> {
    let source = r#"<script lang="ts">
export interface Props { title: string }
</script>
"#;
    let left = index_template_file(
        Path::new("src/left/index.vue"),
        template_pipeline::TemplateKind::Vue,
        source,
    )?;
    let right = index_template_file(
        Path::new("src/right/index.vue"),
        template_pipeline::TemplateKind::Vue,
        source,
    )?;
    let props_id = |storage: &IntermediateStorage| {
        storage
            .nodes
            .iter()
            .find(|node| node.serialized_name == "Props")
            .map(|node| node.id)
            .expect("Props symbol")
    };

    assert_ne!(props_id(&left), props_id(&right));
    Ok(())
}

#[test]
fn test_text_only_sveltekit_page_indexes_file_convention_route() -> Result<()> {
    let temp = tempdir()?;
    let routes = temp.path().join("src/routes/users/[id]");
    std::fs::create_dir_all(&routes)?;
    let path = routes.join("+page.svelte");
    std::fs::write(&path, "<h1>User</h1>\n")?;

    let storage = index_text_only_file(&path)?;

    assert!(storage.nodes.iter().any(|node| {
        node.serialized_name == "GET /users/:id (sveltekit route; confidence=file_convention)"
    }));
    let route = storage
        .nodes
        .iter()
        .find(|node| {
            node.serialized_name == "GET /users/:id (sveltekit route; confidence=file_convention)"
        })
        .expect("sveltekit route node");
    let canonical_id = route.canonical_id.as_deref().expect("route canonical id");
    assert!(canonical_id.contains(r#""extraction_provenance":"text_only""#));
    assert!(canonical_id.contains(r#""extraction:text_only""#));
    assert!(storage.edges.iter().any(|edge| {
        edge.kind == EdgeKind::MEMBER && edge.certainty == Some(ResolutionCertainty::Certain)
    }));
    Ok(())
}

#[test]
fn test_text_only_svelte_tauri_invoke_indexes_uncertain_command_edge() -> Result<()> {
    let temp = tempdir()?;
    let path = temp.path().join("App.svelte");
    std::fs::write(
        &path,
        r#"
<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  export async function refresh() {
    await invoke("get_snapshot");
  }
</script>
"#,
    )?;

    let storage = index_text_only_file(&path)?;
    let command = storage
        .nodes
        .iter()
        .find(|node| node.canonical_id.as_deref() == Some("tauri:command:get_snapshot"))
        .expect("tauri command node");

    assert_eq!(command.kind, NodeKind::FUNCTION);
    assert!(command.serialized_name.contains("get_snapshot"));
    assert!(storage.edges.iter().any(|edge| {
        edge.kind == EdgeKind::CALL
            && edge.target == command.id
            && edge.certainty == Some(ResolutionCertainty::Uncertain)
            && edge.confidence == Some(0.45)
    }));
    assert!(storage.occurrences.iter().any(|occurrence| {
        occurrence.element_id == command.id.0 && occurrence.kind == OccurrenceKind::REFERENCE
    }));
    Ok(())
}

#[test]
fn test_template_svelte_tauri_invoke_survives_projection_flush() -> Result<()> {
    let source = r#"
<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  export async function refresh() {
    await invoke("get_snapshot");
  }
</script>
"#;
    let local = index_template_file(
        Path::new("src/App.svelte"),
        template_pipeline::TemplateKind::Svelte,
        source,
    )?;
    let command = local
        .nodes
        .iter()
        .find(|node| node.canonical_id.as_deref() == Some("tauri:command:get_snapshot"))
        .expect("tauri command node");
    let file_id = local.files[0].id;
    assert!(local.edges.iter().any(|edge| {
        edge.kind == EdgeKind::CALL
            && edge.target == command.id
            && edge.certainty == Some(ResolutionCertainty::Uncertain)
    }));

    let mut storage = Storage::new_in_memory()?;
    storage
        .projections()
        .flush_projection_batch(codestory_store::ProjectionBatch {
            files: &local.files,
            file_content_hashes: &local.file_content_hashes,
            nodes: &local.nodes,
            structural_text_units: &local.structural_text_units,
            structural_text_projections: &local.structural_text_projections,
            structural_text_cache_writes: &[],
            edges: &local.edges,
            occurrences: &local.occurrences,
            component_access: &local.component_access,
            callable_projection_states: &local.callable_projection_states,
            file_errors: &[],
        })?;

    let edges = storage.get_edges()?;
    assert_eq!(
        storage.get_file_content_hash(file_id)?.as_deref(),
        Some(source_content_hash(source.as_bytes()).as_str())
    );
    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::CALL
                && edge.target == command.id
                && edge.certainty == Some(ResolutionCertainty::Uncertain)
        }),
        "flush should preserve uncertain tauri invoke edge: {edges:?}"
    );
    Ok(())
}

#[test]
fn test_workspace_svelte_tauri_invoke_indexes_uncertain_command_edge() -> Result<()> {
    use codestory_contracts::events::EventBus;
    use codestory_workspace::RefreshInfo;
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir()?;
    let root = dir.path();
    let svelte = root.join("src/App.svelte");
    fs::create_dir_all(svelte.parent().expect("parent"))?;
    fs::write(
        &svelte,
        r#"
<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  export async function refresh() {
    await invoke("get_snapshot");
  }
</script>
"#,
    )?;
    let rust = root.join("src-tauri/src/lib.rs");
    fs::create_dir_all(rust.parent().expect("parent"))?;
    fs::write(
        &rust,
        r#"
#[tauri::command]
fn get_snapshot() -> String {
    String::new()
}

pub fn build() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_snapshot]);
}
"#,
    )?;

    let mut storage = Storage::new_in_memory()?;
    let bus = EventBus::new();
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let refresh_info = RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![svelte, rust],
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };
    indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;

    let nodes = storage.get_nodes()?;
    let edges = storage.get_edges()?;
    let command = nodes
        .iter()
        .find(|node| node.canonical_id.as_deref() == Some("tauri:command:get_snapshot"))
        .expect("tauri command node");
    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::CALL
                && edge.target == command.id
                && edge.certainty == Some(ResolutionCertainty::Uncertain)
        }),
        "expected uncertain invoke edge to command {:?}, got {:?}",
        command.id,
        edges
            .iter()
            .filter(|edge| edge.target == command.id)
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_template_svelte_tauri_invoke_indexes_uncertain_command_edge() -> Result<()> {
    let source = r#"
<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  export async function refresh() {
    await invoke("get_snapshot");
  }
</script>
"#;
    let storage = index_template_file(
        Path::new("src/App.svelte"),
        template_pipeline::TemplateKind::Svelte,
        source,
    )?;
    let command = storage
        .nodes
        .iter()
        .find(|node| node.canonical_id.as_deref() == Some("tauri:command:get_snapshot"))
        .expect("tauri command node");

    assert!(storage.edges.iter().any(|edge| {
        edge.kind == EdgeKind::CALL
            && edge.target == command.id
            && edge.certainty == Some(ResolutionCertainty::Uncertain)
    }));
    Ok(())
}

#[test]
fn test_text_only_svelte_plain_invoke_does_not_index_tauri_command() -> Result<()> {
    let temp = tempdir()?;
    let path = temp.path().join("App.svelte");
    std::fs::write(
        &path,
        r#"
<script lang="ts">
  import { invoke } from "./local-rpc";
  export async function refresh() {
    await invoke("get_snapshot");
  }
</script>
"#,
    )?;

    let storage = index_text_only_file(&path)?;
    assert!(
        storage.nodes.iter().all(|node| !node
            .canonical_id
            .as_deref()
            .is_some_and(|id| id.starts_with("tauri:command:"))),
        "non-Tauri Svelte invoke() should not synthesize tauri command nodes"
    );
    assert!(
        storage.edges.iter().all(|edge| edge.kind != EdgeKind::CALL),
        "non-Tauri Svelte invoke() should not synthesize tauri call edges"
    );
    Ok(())
}

#[test]
fn test_text_only_go_file_indexes_functions_types_and_methods() -> Result<()> {
    let temp = tempdir()?;
    let path = temp.path().join("mux.go");
    std::fs::write(
        &path,
        r#"
package mux

type Router struct {}
type RouteMatch struct {}
type MiddlewareFunc func(http.Handler) http.Handler

func NewRouter() *Router { return &Router{} }
func (r *Router) Match(req *http.Request, match *RouteMatch) bool { return false }
func (r *Router) StrictSlash(value bool) *Router { return r }
"#,
    )?;

    let storage = index_text_only_file(&path)?;
    let node_names = storage
        .nodes
        .iter()
        .map(|node| node.serialized_name.as_str())
        .collect::<HashSet<_>>();

    for expected in [
        "Router",
        "RouteMatch",
        "MiddlewareFunc",
        "NewRouter",
        "Router.Match",
        "Router.StrictSlash",
    ] {
        assert!(
            node_names.contains(expected),
            "expected Go text-only symbol {expected}; got {node_names:?}"
        );
    }
    assert!(storage.edges.iter().any(|edge| {
        edge.kind == EdgeKind::MEMBER
            && storage
                .nodes
                .iter()
                .any(|node| node.id == edge.target && node.serialized_name == "Router.Match")
    }));
    assert!(storage.occurrences.iter().any(|occurrence| {
        occurrence.kind == OccurrenceKind::DEFINITION
            && storage.nodes.iter().any(|node| {
                node.id.0 == occurrence.element_id && node.serialized_name == "NewRouter"
            })
    }));
    Ok(())
}

#[test]
fn test_svelte_tauri_invoke_variants_are_bounded_to_first_argument() -> Result<()> {
    let temp = tempdir()?;
    let path = temp.path().join("App.svelte");
    std::fs::write(
        &path,
        r#"
<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  export async function refresh() {
    await invoke<Snapshot>(
      "get_snapshot",
      { label: "not_a_command" }
    );
    await window.__TAURI__.core.invoke("save_config", { name: "also_not_a_command" });
    await invoke(commandName, { label: "dynamic_arg_not_command" });
    // invoke("commented_out")
  }
</script>
"#,
    )?;

    let storage = index_text_only_file(&path)?;
    let canonical_ids = storage
        .nodes
        .iter()
        .filter_map(|node| node.canonical_id.as_deref())
        .collect::<Vec<_>>();

    assert!(canonical_ids.contains(&"tauri:command:get_snapshot"));
    assert!(canonical_ids.contains(&"tauri:command:save_config"));
    assert!(!canonical_ids.contains(&"tauri:command:not_a_command"));
    assert!(!canonical_ids.contains(&"tauri:command:also_not_a_command"));
    assert!(!canonical_ids.contains(&"tauri:command:dynamic_arg_not_command"));
    assert!(!canonical_ids.contains(&"tauri:command:commented_out"));
    Ok(())
}

#[test]
fn test_rust_tauri_command_registration_indexes_command_symbol_and_boundary() -> Result<()> {
    let code = r#"
#[tauri::command]
async fn get_snapshot() -> String {
    String::new()
}

pub fn build() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_snapshot]);
}
"#;
    let language_config = get_language_for_ext("rs").expect("rust config");
    let result = index_file(
        Path::new("src-tauri/src/lib.rs"),
        code,
        &language_config,
        None,
        None,
    )?;
    let command = result
        .nodes
        .iter()
        .find(|node| node.canonical_id.as_deref() == Some("tauri:command:get_snapshot"))
        .expect("tauri command node");
    let function = result
        .nodes
        .iter()
        .find(|node| node.serialized_name == "get_snapshot" && node.kind == NodeKind::FUNCTION)
        .expect("rust command function");

    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::MEMBER
            && edge.target == command.id
            && edge.certainty == Some(ResolutionCertainty::Certain)
    }));
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::CALL
            && edge.source == command.id
            && edge.target == function.id
            && edge.certainty == Some(ResolutionCertainty::Probable)
    }));
    Ok(())
}

#[test]
fn test_tauri_generate_handler_parses_multiline_modules_and_ignores_comments() {
    let registrations = collect_tauri_command_registrations(
        r#"
pub fn build() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            save_config,
            // commented_out,
        ]);
}
"#,
    );
    let commands = registrations
        .iter()
        .map(|registration| registration.command.as_str())
        .collect::<Vec<_>>();

    assert_eq!(commands, vec!["get_snapshot", "save_config"]);
}

#[test]
fn test_typescript_framework_routes_index_express_react_and_sveltekit() -> Result<()> {
    let code = r#"
import express from "express";
import { Route } from "react-router-dom";
const app = express();
app.get("/users", listUsers);
export function listUsers() {
    return [];
}
export function Screen() {
    return <Route path="/dashboard" element={<Dashboard />} />;
}
"#;
    let language_config = get_language_for_ext("tsx").expect("tsx config");
    let result = index_file(
        Path::new("src/routes/+server.tsx"),
        code,
        &language_config,
        None,
        None,
    )?;
    let node_by_id = result
        .nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();

    let express_route = result
        .nodes
        .iter()
        .find(|node| node.serialized_name == "GET /users (express route; confidence=heuristic)")
        .expect("express route node");
    let express_canonical_id = express_route
        .canonical_id
        .as_deref()
        .expect("express route canonical id");
    assert!(express_canonical_id.contains(r#""extraction_provenance":"tree_sitter_query""#));
    assert!(express_canonical_id.contains(r#""claim_tier":"parser_backed""#));
    let handler = result
        .nodes
        .iter()
        .find(|node| node.serialized_name == "listUsers")
        .expect("handler node");
    assert!(result.nodes.iter().any(|node| {
        node.serialized_name == "GET /dashboard (react-router route; confidence=heuristic)"
    }));
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::CALL
            && edge.source == express_route.id
            && edge.target == handler.id
            && edge.certainty == Some(ResolutionCertainty::Probable)
    }));
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::MEMBER
            && edge.target == express_route.id
            && node_by_id
                .get(&edge.source)
                .is_some_and(|node| node.kind == NodeKind::FILE)
    }));
    Ok(())
}

#[test]
fn test_framework_routes_ignore_comment_only_declarations_and_non_router_path_configs() {
    let routes = collect_framework_routes(
        Path::new("src/config.ts"),
        "typescript",
        r#"
const screen = { path: "/not-a-route", label: "Settings" };
// app.get("/debug", debugHandler);
/* app.post("/debug-block", debugHandler); */
// <Route path="/shadow" element={<Shadow />} />;
"#,
    );

    assert!(
        routes.is_empty(),
        "comment-only routes and arbitrary path configs should not emit routes: {routes:?}"
    );
}

#[test]
fn test_react_router_object_routes_require_router_context() {
    let router_routes = collect_framework_routes(
        Path::new("src/router.tsx"),
        "typescript",
        r#"
import { createBrowserRouter } from "react-router-dom";
export const router = createBrowserRouter([
  { path: "/dashboard", element: <Dashboard /> },
]);
"#,
    );
    assert!(router_routes.iter().any(|route| {
        route.framework == "react-router" && route.method == "GET" && route.path == "/dashboard"
    }));

    let config_routes = collect_framework_routes(
        Path::new("src/theme.ts"),
        "typescript",
        r#"
// TODO migrate this config to react-router later.
export const item = { path: "/dashboard", label: "Dashboard" };
"#,
    );
    assert!(
        config_routes.is_empty(),
        "bare object path config should not be treated as react-router: {config_routes:?}"
    );
}

#[test]
fn test_remix_file_routes_require_remix_evidence() {
    let generic_routes = collect_framework_routes(
        Path::new("src/routes/accounts.tsx"),
        "typescript",
        r#"
export default function Accounts() {
  return null;
}
"#,
    );
    assert!(
        generic_routes
            .iter()
            .all(|route| route.framework != "remix"),
        "generic src/routes files should not be treated as Remix routes: {generic_routes:?}"
    );

    let remix_routes = collect_framework_routes(
        Path::new("app/routes/accounts.tsx"),
        "typescript",
        r#"
export default function Accounts() {
  return null;
}
"#,
    );
    assert!(remix_routes.iter().any(|route| {
        route.framework == "remix" && route.method == "GET" && route.path == "/accounts"
    }));
}

#[test]
fn test_react_router_context_ignores_unrelated_path_objects() {
    let routes = collect_framework_routes(
        Path::new("src/router.tsx"),
        "typescript",
        r#"
import { createBrowserRouter } from "react-router-dom";
const buildConfig = { path: "/tmp/cache", label: "Cache dir" };
export const router = createBrowserRouter([
  { path: "/dashboard", element: <Dashboard /> },
]);
"#,
    );

    assert!(
        routes.iter().any(|route| {
            route.framework == "react-router" && route.method == "GET" && route.path == "/dashboard"
        }),
        "actual react-router route should still be indexed: {routes:?}"
    );
    assert!(
        routes
            .iter()
            .all(|route| route.framework != "react-router" || route.path != "/tmp/cache"),
        "unrelated object path inside a router file should not emit a route: {routes:?}"
    );
}

#[test]
fn test_nextjs_layout_and_template_files_do_not_emit_endpoint_routes() {
    for path in [
        Path::new("app/dashboard/layout.tsx"),
        Path::new("app/dashboard/template.tsx"),
    ] {
        let routes = collect_framework_routes(
            path,
            "typescript",
            r#"export default function Wrapper({ children }) { return children; }"#,
        );
        assert!(
            routes
                .iter()
                .all(|route| route.framework != "nextjs" || route.method != "GET"),
            "{path:?} should not be indexed as a Next.js endpoint route: {routes:?}"
        );
    }
}

#[test]
fn test_framework_route_handler_resolution_prefers_same_file_nearest_match() {
    let file_id = NodeId(1);
    let other_file_id = NodeId(2);
    let mut nodes = HashMap::new();
    nodes.insert(
        NodeId(20),
        Node {
            id: NodeId(20),
            kind: NodeKind::FUNCTION,
            serialized_name: "handler".to_string(),
            file_node_id: Some(other_file_id),
            start_line: Some(5),
            end_line: Some(5),
            ..Default::default()
        },
    );
    nodes.insert(
        NodeId(10),
        Node {
            id: NodeId(10),
            kind: NodeKind::FUNCTION,
            serialized_name: "handler".to_string(),
            file_node_id: Some(file_id),
            start_line: Some(12),
            end_line: Some(12),
            ..Default::default()
        },
    );

    assert_eq!(
        find_framework_route_handler(&nodes, "handler", file_id, 10),
        Some(NodeId(10))
    );
}

#[test]
fn test_framework_route_handler_resolution_skips_ambiguous_best_match() {
    let file_id = NodeId(1);
    let mut nodes = HashMap::new();
    for id in [NodeId(10), NodeId(11)] {
        nodes.insert(
            id,
            Node {
                id,
                kind: NodeKind::FUNCTION,
                serialized_name: "handler".to_string(),
                file_node_id: Some(file_id),
                start_line: Some(12),
                end_line: Some(12),
                ..Default::default()
            },
        );
    }

    assert_eq!(
        find_framework_route_handler(&nodes, "handler", file_id, 10),
        None
    );
}

#[test]
fn test_framework_route_handler_resolution_skips_multiple_off_file_matches() {
    let route_file_id = NodeId(1);
    let mut nodes = HashMap::new();
    for (id, file_id, line) in [(NodeId(10), NodeId(2), 5), (NodeId(11), NodeId(3), 20)] {
        nodes.insert(
            id,
            Node {
                id,
                kind: NodeKind::FUNCTION,
                serialized_name: "handler".to_string(),
                file_node_id: Some(file_id),
                start_line: Some(line),
                end_line: Some(line),
                ..Default::default()
            },
        );
    }

    assert_eq!(
        find_framework_route_handler(&nodes, "handler", route_file_id, 10),
        None
    );
}

#[test]
fn test_framework_route_extractors_cover_requested_web_stacks() {
    let cases = [
        (
            "typescript",
            Path::new("app/users/[id]/page.tsx"),
            r#"export default function UserPage() { return null; }"#,
            vec!["nextjs"],
        ),
        (
            "typescript",
            Path::new("app/api/users/[id]/route.ts"),
            r#"export async function POST() { return Response.json({}); }"#,
            vec!["nextjs"],
        ),
        (
            "typescript",
            Path::new("app/routes/accounts.$accountId.tsx"),
            r#"
export const loader = async () => null;
export const action = async () => null;
"#,
            vec!["remix"],
        ),
        (
            "astro",
            Path::new("src/pages/blog/[slug].astro"),
            r#"<h1>Post</h1>"#,
            vec!["astro"],
        ),
        (
            "typescript",
            Path::new("server/api/users/[id].post.ts"),
            r#"export default defineEventHandler(() => ({}));"#,
            vec!["nuxt"],
        ),
        (
            "vue",
            Path::new("pages/users/[id].vue"),
            r#"<template><div /></template>"#,
            vec!["nuxt"],
        ),
        (
            "typescript",
            Path::new("routes.ts"),
            r#"
import fastify from "fastify";
const server = fastify();
server.get("/fastify/:id", listFastify);
"#,
            vec!["fastify"],
        ),
        (
            "typescript",
            Path::new("routes.ts"),
            r#"
import Router from "@koa/router";
const router = new Router();
router.post("/koa/:id", createKoa);
"#,
            vec!["koa"],
        ),
        (
            "typescript",
            Path::new("routes.ts"),
            r#"
import { Hono } from "hono";
const app = new Hono();
app.get("/hono/:id", getHono);
"#,
            vec!["hono"],
        ),
        (
            "typescript",
            Path::new("users.controller.ts"),
            r#"
@Controller("users")
export class UsersController {
  @Get(":id")
  show() {}
}
"#,
            vec!["nestjs"],
        ),
        (
            "go",
            Path::new("routes.go"),
            r#"
import "github.com/gin-gonic/gin"
func routes(r *gin.Engine) {
  r.GET("/gin/:id", showGin)
}
"#,
            vec!["gin"],
        ),
        (
            "go",
            Path::new("routes.go"),
            r#"
import "github.com/go-chi/chi/v5"
func routes(r chi.Router) {
  r.Get("/chi/{id}", showChi)
}
"#,
            vec!["chi"],
        ),
        (
            "go",
            Path::new("routes.go"),
            r#"
import "github.com/labstack/echo/v4"
func routes(e *echo.Echo) {
  e.GET("/echo/:id", showEcho)
}
"#,
            vec!["echo"],
        ),
        (
            "go",
            Path::new("routes.go"),
            r#"
import "github.com/gofiber/fiber/v2"
func routes(app *fiber.App) {
  app.Get("/fiber/:id", showFiber)
}
"#,
            vec!["fiber"],
        ),
        (
            "python",
            Path::new("urls.py"),
            r#"
@app.route("/flask", methods=["POST"])
def flask_handler(): pass
@app.get("/fastapi")
async def fastapi_handler(): pass
path("django/", views.home)
"#,
            vec!["flask", "fastapi", "django"],
        ),
        (
            "ruby",
            Path::new("config/routes.rb"),
            r#"get "/rails", to: "home#index""#,
            vec!["rails"],
        ),
        (
            "php",
            Path::new("routes/web.php"),
            r#"Route::post("/laravel", [UserController::class, "store"]);"#,
            vec!["laravel"],
        ),
        (
            "java",
            Path::new("Controller.java"),
            r#"@GetMapping("/spring")"#,
            vec!["spring"],
        ),
        (
            "csharp",
            Path::new("Controller.cs"),
            r#"[HttpGet("/aspnet")]"#,
            vec!["aspnet"],
        ),
        (
            "rust",
            Path::new("routes.rs"),
            r#"
Router::new().route("/axum", get(handler));
web::resource("/actix").route(web::get().to(handler));
#[post("/rocket")]
"#,
            vec!["axum", "actix", "rocket"],
        ),
        (
            "vue",
            Path::new("router.vue"),
            r#"{ path: "/vue", name: "VueHome" }"#,
            vec!["vue-router"],
        ),
        (
            "kotlin",
            Path::new("Routing.kt"),
            r#"
import io.ktor.server.routing.*
fun Application.module() {
  routing {
    get("/ktor/users") { }
    post("/ktor/users") { }
  }
}
"#,
            vec!["ktor"],
        ),
        (
            "swift",
            Path::new("routes.swift"),
            r#"
import Vapor
func routes(_ app: Application) throws {
  app.get("vapor/users", use: UserController.index)
}
"#,
            vec!["vapor"],
        ),
        (
            "dart",
            Path::new("routes.dart"),
            r#"
import 'package:shelf_router/shelf_router.dart';
final router = Router();
router.get('/shelf/users', usersHandler);
"#,
            vec!["shelf"],
        ),
    ];

    for (language, path, source, expected_frameworks) in cases {
        let routes = collect_framework_routes(path, language, source);
        let frameworks = routes
            .iter()
            .map(|route| route.framework)
            .collect::<HashSet<_>>();
        for expected in expected_frameworks {
            assert!(
                frameworks.contains(expected),
                "expected {expected} route in {language}; got {routes:?}"
            );
        }
    }

    let mux_library_routes = collect_framework_routes(
        Path::new("route.go"),
        "go",
        r#"
package mux

func (r *Route) Get(name string) interface{} { return r.namedRoutes[name] }
"#,
    );
    assert!(
        mux_library_routes.is_empty(),
        "plain mux library methods should not be indexed as framework routes: {mux_library_routes:?}"
    );
}

/// Kotlin route scanning keeps stripping C-style comments after the
/// language's comment style moved into the extraction registry.
///
/// `route_comments_are_c_style` is now a registry field rather than a name
/// in a `matches!` roster. Asserting the field's value alone would be a
/// tautology, so this drives the behaviour it controls: a commented-out
/// ktor route must not become a product route claim.
#[test]
fn kotlin_route_scanning_still_strips_c_style_comments() {
    let source = r#"
import io.ktor.server.routing.*
fun Application.module() {
  routing {
    get("/ktor/live") { }
    // get("/ktor/line-commented") { }
    /*
    post("/ktor/block-commented") { }
    */
  }
}
"#;
    let routes = collect_framework_routes(Path::new("Routing.kt"), "kotlin", source);
    let paths = routes
        .iter()
        .map(|route| route.path.as_str())
        .collect::<HashSet<_>>();
    assert!(paths.contains("/ktor/live"), "{routes:?}");
    assert!(!paths.contains("/ktor/line-commented"), "{routes:?}");
    assert!(!paths.contains("/ktor/block-commented"), "{routes:?}");
}

#[test]
fn test_nextjs_file_route_metadata_preserves_raw_path_params_and_convention() {
    let routes = collect_framework_routes(
        Path::new("app/api/users/[id]/route.ts"),
        "typescript",
        r#"export async function GET() { return Response.json({}); }"#,
    );
    let route = routes
        .iter()
        .find(|route| route.framework == "nextjs" && route.method == "GET")
        .expect("nextjs route");

    assert_eq!(route.path, "/api/users/:id");
    assert_eq!(route.raw_path, "/api/users/[id]");
    assert_eq!(route_params(&route.path), vec!["id"]);
    assert_eq!(route.confidence, "file_convention");
    assert_eq!(route.source_convention, "file_convention");

    let canonical_id = framework_route_canonical_id(route);
    assert!(canonical_id.starts_with("route_endpoint:"));
    assert!(canonical_id.contains(r#""framework":"nextjs""#));
    assert!(canonical_id.contains(r#""raw_path":"/api/users/[id]""#));
    assert!(canonical_id.contains(r#""params":["id"]"#));
    assert!(canonical_id.contains(r#""source_convention":"file_convention""#));
    assert!(canonical_id.contains(r#""extraction_provenance":"line_scan""#));
    assert!(canonical_id.contains(r#""extraction:line_scan""#));
}

#[test]
fn test_next_payload_collection_registration_and_page_usage_surface() -> Result<()> {
    let code = r#"
import type { CollectionConfig } from "payload";

export const Posts: CollectionConfig = {
  slug: "posts",
};

export async function loadWriting(payload: any) {
  return payload.find({ collection: "posts", limit: 10 });
}

async function getCommentAuth() {
  return { user: null };
}

async function getElsewhereFeed() {
  return [];
}

function ElsewhereFeed(_props: { entries: unknown[] }) {
  return null;
}

export default async function Page() {
  const auth = await getCommentAuth();
  const entries = await getElsewhereFeed();
  return <ElsewhereFeed entries={entries} auth={auth} />;
}
"#;
    let language_config = get_language_for_ext("tsx").expect("tsx config");
    let result = index_file(
        Path::new("app/writing/[slug]/page.tsx"),
        code,
        &language_config,
        None,
        None,
    )?;
    let route = result
        .nodes
        .iter()
        .find(|node| {
            node.serialized_name == "GET /writing/:slug (nextjs route; confidence=file_convention)"
        })
        .expect("next page route node");
    let page = result
        .nodes
        .iter()
        .find(|node| node.serialized_name == "Page" && node.kind == NodeKind::FUNCTION)
        .expect("Page function node");
    let collection = result
        .nodes
        .iter()
        .find(|node| node.canonical_id.as_deref() == Some("payload:collection:posts"))
        .expect("payload collection node");
    let loader = result
        .nodes
        .iter()
        .find(|node| node.serialized_name == "loadWriting" && node.kind == NodeKind::FUNCTION)
        .expect("loadWriting function node");

    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::CALL
            && edge.source == route.id
            && edge.target == page.id
            && edge.certainty == Some(ResolutionCertainty::Probable)
    }));
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::USAGE
            && edge.source == loader.id
            && edge.target == collection.id
            && edge.certainty == Some(ResolutionCertainty::Probable)
            && edge
                .callsite_identity
                .as_deref()
                .is_some_and(|identity| identity.starts_with("payload:find:posts:"))
    }));
    assert!(result.occurrences.iter().any(|occurrence| {
        occurrence.element_id == collection.id.0 && occurrence.kind == OccurrenceKind::DEFINITION
    }));
    Ok(())
}

#[test]
fn test_payload_collection_extraction_handles_multiline_and_ignores_noise() {
    let code = r#"
import type { CollectionConfig } from "payload";

const UiMetadata = {
  slug: "not-a-collection",
};

export const Posts = {
  slug:
    "posts",
  fields: []
} satisfies CollectionConfig;

export const Comments: CollectionConfig =
{
  slug: "comments",
  fields: []
};

export async function loadWriting(payload: any) {
  const props = { collection: "not_a_payload_call" };
  await payload.find({
    collection:
      "posts",
    where: { title: { equals: "not_a_collection" } },
  });
  await payload.find({ collection: "articles", where: { slug: { equals: "welcome" } } });
  return req.payload.create({ collection: "comments", data: {} });
}
"#;
    let registrations = collect_payload_collection_registrations(code);
    let registered = registrations
        .iter()
        .map(|registration| registration.slug.as_str())
        .collect::<Vec<_>>();
    assert_eq!(registered, vec!["posts", "comments"]);

    let usages = collect_payload_collection_usages(code);
    let used = usages
        .iter()
        .map(|usage| (usage.slug.as_str(), usage.operation.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        used,
        vec![
            ("posts", "find"),
            ("articles", "find"),
            ("comments", "create")
        ]
    );
}

#[test]
fn test_typescript_api_literal_creates_schema_endpoint_call_edge() -> Result<()> {
    let code = r#"
export async function loadUsers() {
    return fetch("/api/users");
}

export async function createUser() {
    return apiClient.post("/api/users", {});
}
"#;
    let language_config = get_language_for_ext("ts").expect("typescript config");
    let result = index_file(Path::new("client.ts"), code, &language_config, None, None)?;
    let get_endpoint = schema_endpoint_node_id("GET", "/api/users");
    let post_endpoint = schema_endpoint_node_id("POST", "/api/users");
    let node_by_id = result
        .nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();

    assert!(
        node_by_id
            .get(&get_endpoint)
            .is_some_and(|node| node.serialized_name == "GET /api/users")
    );
    assert!(
        node_by_id
            .get(&post_endpoint)
            .is_some_and(|node| node.serialized_name == "POST /api/users")
    );
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::CALL
            && edge.target == get_endpoint
            && edge.certainty == Some(ResolutionCertainty::Uncertain)
    }));
    assert!(result.edges.iter().any(|edge| {
        edge.kind == EdgeKind::CALL
            && edge.target == post_endpoint
            && edge.certainty == Some(ResolutionCertainty::Uncertain)
    }));
    Ok(())
}

#[test]
fn test_typescript_api_path_literals_without_client_calls_do_not_create_edges() -> Result<()> {
    let code = r#"
const docsPath = "/api/users";
export const routeMap = {
    users: "/api/users",
};
app.get("/api/users", handler);
export function handler() {}
"#;
    let language_config = get_language_for_ext("ts").expect("typescript config");
    let result = index_file(Path::new("routes.ts"), code, &language_config, None, None)?;
    let endpoint = schema_endpoint_node_id("GET", "/api/users");

    assert!(
        result.nodes.iter().all(|node| node.id != endpoint),
        "plain path literals and route declarations should not create endpoint nodes"
    );
    assert!(
        result
            .edges
            .iter()
            .all(|edge| edge.kind != EdgeKind::CALL || edge.target != endpoint),
        "plain path literals and route declarations should not create endpoint call edges"
    );
    Ok(())
}

#[test]
fn test_typescript_api_path_literals_in_trailing_comments_do_not_create_edges() -> Result<()> {
    let code = r#"
export function handler() {
    const ready = true; // fetch("/api/users")
}
"#;
    let language_config = get_language_for_ext("ts").expect("typescript config");
    let result = index_file(Path::new("client.ts"), code, &language_config, None, None)?;
    let endpoint = schema_endpoint_node_id("GET", "/api/users");

    assert!(
        result.nodes.iter().all(|node| node.id != endpoint),
        "trailing comments should not create endpoint nodes"
    );
    assert!(
        result
            .edges
            .iter()
            .all(|edge| edge.kind != EdgeKind::CALL || edge.target != endpoint),
        "trailing comments should not create endpoint call edges"
    );
    Ok(())
}

/// A graph shaped like a prior incremental run left it: one caller resolved
/// into a preferred definition, an equally-named fallback definition in a
/// third file, and an unrelated file whose own call never resolved.
struct RemovalScopeFixture {
    caller_file_id: i64,
    preferred_file_id: i64,
    fallback_definition_id: NodeId,
    caller_edge_id: EdgeId,
    untouched_edge_id: EdgeId,
    untouched_path: PathBuf,
}

fn seed_removal_scope_fixture(storage: &mut Storage, root: &Path) -> Result<RemovalScopeFixture> {
    let caller_path = root.join("caller.rs");
    let preferred_path = root.join("preferred.rs");
    let fallback_path = root.join("fallback.rs");
    let untouched_path = root.join("untouched.rs");

    let file_ids = [
        &caller_path,
        &preferred_path,
        &fallback_path,
        &untouched_path,
    ]
    .map(|path| WorkspaceIndexer::canonical_file_node_id_for_path(path));
    let [
        caller_file_id,
        preferred_file_id,
        fallback_file_id,
        untouched_file_id,
    ] = file_ids;

    let mut nodes = Vec::new();
    for (file_id, path) in file_ids.iter().zip([
        &caller_path,
        &preferred_path,
        &fallback_path,
        &untouched_path,
    ]) {
        storage.insert_file(&codestory_store::FileInfo {
            id: *file_id,
            path: path.clone(),
            language: "rust".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 4,
            file_role: codestory_store::FileRole::Source,
        })?;
        nodes.push(Node {
            id: NodeId(*file_id),
            kind: NodeKind::FILE,
            serialized_name: path.to_string_lossy().to_string(),
            ..Default::default()
        });
    }

    let caller_id = NodeId(910_001);
    let call_placeholder_id = NodeId(910_002);
    let preferred_definition_id = NodeId(920_001);
    let fallback_definition_id = NodeId(930_001);
    let untouched_caller_id = NodeId(940_001);
    let untouched_placeholder_id = NodeId(940_002);
    nodes.extend([
        Node {
            id: caller_id,
            kind: NodeKind::FUNCTION,
            serialized_name: "use_shared_target".to_string(),
            qualified_name: Some("use_shared_target".to_string()),
            file_node_id: Some(NodeId(caller_file_id)),
            start_line: Some(1),
            ..Default::default()
        },
        Node {
            id: call_placeholder_id,
            kind: NodeKind::UNKNOWN,
            serialized_name: "shared_target".to_string(),
            start_line: Some(2),
            ..Default::default()
        },
        Node {
            id: preferred_definition_id,
            kind: NodeKind::FUNCTION,
            serialized_name: "shared_target".to_string(),
            qualified_name: Some("shared_target".to_string()),
            file_node_id: Some(NodeId(preferred_file_id)),
            start_line: Some(1),
            ..Default::default()
        },
        Node {
            id: fallback_definition_id,
            kind: NodeKind::FUNCTION,
            serialized_name: "shared_target".to_string(),
            qualified_name: Some("shared_target".to_string()),
            file_node_id: Some(NodeId(fallback_file_id)),
            start_line: Some(1),
            ..Default::default()
        },
        Node {
            id: untouched_caller_id,
            kind: NodeKind::FUNCTION,
            serialized_name: "lonely_caller".to_string(),
            qualified_name: Some("lonely_caller".to_string()),
            file_node_id: Some(NodeId(untouched_file_id)),
            start_line: Some(1),
            ..Default::default()
        },
        Node {
            id: untouched_placeholder_id,
            kind: NodeKind::UNKNOWN,
            serialized_name: "shared_target".to_string(),
            start_line: Some(2),
            ..Default::default()
        },
    ]);
    storage.insert_nodes_batch(&nodes)?;

    let caller_edge_id = EdgeId(950_001);
    let untouched_edge_id = EdgeId(950_002);
    storage.insert_edges_batch(&[
        Edge {
            id: caller_edge_id,
            source: caller_id,
            target: call_placeholder_id,
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(caller_file_id)),
            resolved_target: Some(preferred_definition_id),
            confidence: Some(0.95),
            certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
            candidate_targets: vec![preferred_definition_id],
            ..Default::default()
        },
        Edge {
            id: untouched_edge_id,
            source: untouched_caller_id,
            target: untouched_placeholder_id,
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(untouched_file_id)),
            ..Default::default()
        },
    ])?;

    Ok(RemovalScopeFixture {
        caller_file_id,
        preferred_file_id,
        fallback_definition_id,
        caller_edge_id,
        untouched_edge_id,
        untouched_path,
    })
}

fn resolved_target_of(storage: &Storage, edge_id: EdgeId) -> Result<Option<NodeId>> {
    Ok(storage
        .get_edges()?
        .into_iter()
        .find(|edge| edge.id == edge_id)
        .unwrap_or_else(|| panic!("edge {edge_id:?} must survive the run"))
        .resolved_target)
}

#[test]
fn removing_a_preferred_definition_re_resolves_its_callers_to_a_fallback() -> Result<()> {
    let dir = tempdir()?;
    let mut storage = Storage::new_in_memory()?;
    let fixture = seed_removal_scope_fixture(&mut storage, dir.path())?;

    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: Vec::new(),
        files_to_remove: vec![fixture.preferred_file_id],
        existing_file_ids: HashMap::new(),
    };
    let stats = WorkspaceIndexer::new(dir.path().to_path_buf()).run(
        &mut storage,
        &plan,
        &EventBus::new(),
        None,
    )?;

    assert!(
        stats.resolution_ran,
        "a removal-only plan still has callers to repair"
    );
    assert_eq!(
        resolved_target_of(&storage, fixture.caller_edge_id)?,
        Some(fixture.fallback_definition_id),
        "the caller of the removed definition must re-resolve to the surviving one"
    );
    assert_eq!(
        resolved_target_of(&storage, fixture.untouched_edge_id)?,
        None,
        "a caller outside the removal's blast radius must stay out of scope"
    );
    assert_eq!(
        stats.unresolved_calls_start, 1,
        "the scope must be the removal's affected callers, not the whole graph"
    );
    Ok(())
}

#[test]
fn removal_affected_callers_join_an_already_scoped_incremental_run() -> Result<()> {
    let dir = tempdir()?;
    let mut storage = Storage::new_in_memory()?;
    let fixture = seed_removal_scope_fixture(&mut storage, dir.path())?;
    // A real file is scheduled for indexing, so the run is scoped to it and
    // would otherwise never look at the removal's caller file.
    let scheduled = dir.path().join("scheduled.rs");
    std::fs::write(
        &scheduled,
        "pub fn scheduled_symbol() { shared_target(); }\n",
    )?;

    let plan = codestory_workspace::RefreshExecutionPlan {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![scheduled.clone()],
        files_to_remove: vec![fixture.preferred_file_id],
        existing_file_ids: HashMap::new(),
    };
    WorkspaceIndexer::new(dir.path().to_path_buf()).run(
        &mut storage,
        &plan,
        &EventBus::new(),
        None,
    )?;

    assert_eq!(
        resolved_target_of(&storage, fixture.caller_edge_id)?,
        Some(fixture.fallback_definition_id),
        "a scoped incremental run must still repair the removal's callers"
    );
    assert_eq!(
        resolved_target_of(&storage, fixture.untouched_edge_id)?,
        None,
        "the scope must not silently widen to files nothing touched"
    );
    assert!(
        storage.get_file_by_path(&fixture.untouched_path)?.is_some(),
        "the untouched file must remain in the store"
    );
    assert!(
        storage.get_node(NodeId(fixture.caller_file_id))?.is_some(),
        "the caller file must survive the removal"
    );
    Ok(())
}
