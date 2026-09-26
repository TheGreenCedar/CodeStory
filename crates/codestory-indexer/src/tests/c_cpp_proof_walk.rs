use super::*;
use std::process::Command;
use tree_sitter::Parser;

const DEEP_WALK_CHILD_ENV: &str = "CODESTORY_TEST_CPP_PROOF_WALK_DEEP";
const DEEP_DECLARATOR_CHILD_ENV: &str = "CODESTORY_TEST_CPP_DECLARATOR_DEEP";
const DEEP_COMPOUND_DEPTH: usize = 24_000;
const DEEP_POINTER_DECLARATOR_LEVELS: usize = 16_000;

fn parse(language: &str, source: &str) -> Tree {
    let mut parser = Parser::new();
    let grammar = match language {
        "c" => tree_sitter_c::LANGUAGE.into(),
        "cpp" => tree_sitter_cpp::LANGUAGE.into(),
        other => panic!("unsupported proof-walk language {other}"),
    };
    parser
        .set_language(&grammar)
        .expect("C/C++ parser language");
    let tree = parser.parse(source, None).expect("C/C++ syntax tree");
    assert!(
        !tree.root_node().has_error(),
        "{language} fixture must be syntactically valid"
    );
    tree
}

fn index<'tree>(
    language: &'tree str,
    tree: &'tree Tree,
    source: &str,
    nodes: &[Node],
) -> CCppResolutionIndex<'tree> {
    CCppResolutionIndex::build(
        tree,
        source,
        Path::new(if language == "c" {
            "fixture.c"
        } else {
            "fixture.cpp"
        }),
        language,
        NodeId(1),
        nodes,
    )
}

fn class_nodes(tree: &Tree, source: &str) -> Vec<Node> {
    let file_id = NodeId(1);
    let mut nodes = Vec::new();
    let mut next_id = 2_i64;
    super::walk_nodes(tree.root_node(), &mut |node| {
        if !matches!(node.kind(), "class_specifier" | "struct_specifier") {
            return;
        }
        let Some(name) = node
            .child_by_field_name("name")
            .and_then(|name| node_text(name, source))
        else {
            return;
        };
        nodes.push(Node {
            id: NodeId(next_id),
            kind: NodeKind::CLASS,
            serialized_name: name.to_string(),
            file_node_id: Some(file_id),
            start_line: Some(node.start_position().row as u32 + 1),
            ..Node::default()
        });
        next_id += 1;
    });
    nodes
}

fn call_named<'a>(index: &'a CCppResolutionIndex<'_>, name: &str) -> Vec<&'a IndexedCCppCall<'a>> {
    index
        .calls
        .iter()
        .filter(|call| call.raw_target == name)
        .collect()
}

fn receiver_owner(receiver: &CCppCallReceiver) -> String {
    match receiver {
        CCppCallReceiver::ExactType {
            owner_name,
            constructor: false,
            ..
        } => owner_name.clone(),
        CCppCallReceiver::Blocked => "blocked".to_string(),
        CCppCallReceiver::None => "none".to_string(),
        CCppCallReceiver::Implicit => "implicit".to_string(),
        CCppCallReceiver::Qualified(_) => "qualified".to_string(),
        CCppCallReceiver::ExactType {
            constructor: true, ..
        } => "constructor".to_string(),
    }
}

fn deep_compound_source(depth: usize) -> String {
    let mut source = String::from("void f(void)\n{\n");
    source.reserve(depth * 2 + 64);
    source.extend(std::iter::repeat_n('{', depth));
    source.push_str("int after; target();");
    source.extend(std::iter::repeat_n('}', depth));
    source.push_str("\nafter();\n}\n");
    source
}

fn compound_ancestor_depth(node: TsNode<'_>) -> usize {
    let mut depth = 0usize;
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "compound_statement" {
            depth += 1;
        }
        current = parent;
    }
    depth
}

#[test]
fn c_cpp_proof_walk_keeps_named_child_preorder_and_restores_scope() {
    let source = r#"
struct Owner { void run(); };
struct Other { void run(); };
void caller(Owner parameter) {
    Owner keep;
    first();
    {
        Other hidden;
        hidden.run();
        int target;
        target();
    }
    nested_done();
    keep.run();
    hidden.run();
    {
        Other other;
        other.run();
        keep.run();
    }
    second();
    parameter.run();
}
void sibling(void) {
    parameter.run();
    keep.run();
}
"#;
    let tree = parse("cpp", source);
    let indexed = index("cpp", &tree, source, &[]);
    let observed = indexed
        .calls
        .iter()
        .map(|call| {
            (
                call.raw_target.as_str(),
                receiver_owner(&call.receiver),
                call.identifier_shadowed,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        [
            ("first", "none".to_string(), false),
            ("run", "Other".to_string(), false),
            ("target", "none".to_string(), true),
            ("nested_done", "none".to_string(), false),
            ("run", "Owner".to_string(), false),
            ("run", "blocked".to_string(), false),
            ("run", "Other".to_string(), false),
            ("run", "Owner".to_string(), false),
            ("second", "none".to_string(), false),
            ("run", "Owner".to_string(), false),
            ("run", "blocked".to_string(), false),
            ("run", "blocked".to_string(), false),
        ]
    );
}

#[test]
fn c_cpp_proof_walk_restores_sibling_namespace_callable_and_unsupported_context() {
    let source = r#"
namespace left {
void namespaced() { target(); }
}
namespace right {
void namespaced() { target(); }
}
template<typename T>
void templated() { target(); }
void plain() { target(); }
struct Box {
  void method() { target(); }
};
void Box::qualified() { target(); }
void after() { target(); }
"#;
    let tree = parse("cpp", source);
    let nodes = class_nodes(&tree, source);
    let indexed = index("cpp", &tree, source, &nodes);
    let calls = call_named(&indexed, "target");
    assert_eq!(
        calls.len(),
        7,
        "every sibling callable must record target()"
    );

    assert_eq!(calls[0].namespace_path, ["left"]);
    assert_eq!(calls[1].namespace_path, ["right"]);
    assert!(
        calls[2].unsupported,
        "template context stays on its own call"
    );
    assert!(
        !calls[3].unsupported,
        "plain sibling must not inherit template"
    );
    assert!(calls[3].namespace_path.is_empty());
    assert!(calls[3].owner_index.is_none());

    let method_owner = calls[4].owner_index.expect("method stays inside Box");
    let qualified_owner = calls[5]
        .owner_index
        .expect("qualified definition binds Box");
    assert_eq!(method_owner, qualified_owner);
    assert_ne!(calls[4].callable_id, calls[5].callable_id);
    assert!(calls[6].owner_index.is_none());
    assert!(!calls[6].unsupported);
    assert!(calls[6].namespace_path.is_empty());
    assert_ne!(calls[6].callable_id, calls[4].callable_id);
    assert_ne!(calls[6].callable_id, calls[5].callable_id);

    let definitions = indexed
        .callable_records
        .iter()
        .filter(|record| record.defined)
        .map(|record| record.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        definitions,
        [
            "namespaced",
            "namespaced",
            "templated",
            "plain",
            "method",
            "qualified",
            "after",
        ]
    );
}

#[test]
fn c_cpp_proof_walk_survives_deep_tree_child() {
    if std::env::var_os(DEEP_WALK_CHILD_ENV).is_none() {
        return;
    }
    let source = deep_compound_source(DEEP_COMPOUND_DEPTH);
    // Ordinary Rust worker stack, not an increase. The isolated process
    // keeps an abort here from taking down the parent test runner.
    let source_for_worker = source.clone();
    std::thread::spawn(move || {
        for language in ["c", "cpp"] {
            let tree = parse(language, &source_for_worker);
            let mut declaration = None;
            crate::walk_tree_nodes(tree.root_node(), &mut |node| {
                if declaration.is_none() && node.kind() == "declaration" {
                    declaration = Some(node);
                }
            });
            let declaration = declaration.expect("inner declaration");
            assert!(
                compound_ancestor_depth(declaration) > DEEP_COMPOUND_DEPTH,
                "{language} fixture did not nest {DEEP_COMPOUND_DEPTH} compounds"
            );
            let indexed = index(language, &tree, &source_for_worker, &[]);
            let targets = indexed
                .calls
                .iter()
                .map(|call| {
                    (
                        call.raw_target.clone(),
                        call.identifier_shadowed,
                        call.unsupported,
                        call.namespace_path.clone(),
                        call.owner_index,
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                targets,
                vec![
                    ("target".to_string(), false, false, Vec::new(), None),
                    ("after".to_string(), false, false, Vec::new(), None),
                ],
                "{language} deep walk must restore the inner binding before the sibling call"
            );
        }
    })
    .join()
    .expect("deep C/C++ proof walk on the worker stack");
}

#[test]
fn c_cpp_proof_walk_survives_deep_tree_in_isolated_subprocess() {
    let status = Command::new(std::env::current_exe().expect("resolve indexer test executable"))
        .arg("--exact")
        .arg("proof_resolution::c_cpp_proof_walk_tests::c_cpp_proof_walk_survives_deep_tree_child")
        .arg("--nocapture")
        .env(DEEP_WALK_CHILD_ENV, "1")
        .status()
        .expect("run deep C/C++ proof walk child");
    assert!(
        status.success(),
        "deep C/C++ proof walk aborted the isolated child: {status}"
    );
}

#[test]
fn c_cpp_declarator_bound_names_keeps_work_and_rejection() {
    let source = "void caller() { int **kept = 0; int &ref = kept; auto [z, a] = pair; }\n";
    let tree = parse("cpp", source);
    let declarators = declaration_declarators(&tree);
    assert_eq!(
        declarators.len(),
        3,
        "expected pointer, reference, and binding"
    );

    reset_c_cpp_resolution_work();
    assert_eq!(
        c_cpp_declarator_bound_names(declarators[0], source),
        Some(vec!["kept".to_string()])
    );
    // init_declarator, two pointer_declarators, identifier.
    assert_eq!(
        c_cpp_resolution_work(),
        declarator_chain_kinds(declarators[0]).len()
    );
    assert_eq!(
        declarator_chain_kinds(declarators[0]),
        [
            "init_declarator",
            "pointer_declarator",
            "pointer_declarator",
            "identifier",
        ]
    );

    reset_c_cpp_resolution_work();
    assert_eq!(
        c_cpp_declarator_bound_names(declarators[1], source),
        Some(vec!["ref".to_string()])
    );
    assert_eq!(
        c_cpp_resolution_work(),
        declarator_chain_kinds(declarators[1]).len()
    );
    assert_eq!(
        declarator_chain_kinds(declarators[1]),
        ["init_declarator", "reference_declarator", "identifier"]
    );

    reset_c_cpp_resolution_work();
    assert_eq!(
        c_cpp_declarator_bound_names(declarators[2], source),
        Some(vec!["a".to_string(), "z".to_string()])
    );
    // Entered nodes: the init wrapper, the structured binding, and each
    // identifier. Identifiers are collected as z then a, then sorted.
    assert_eq!(c_cpp_resolution_work(), 4);
    assert_eq!(
        declarator_chain_kinds(declarators[2]),
        ["init_declarator", "structured_binding_declarator"]
    );

    // The grammar only admits identifier bindings. Recovery of a
    // qualified name inserts an ERROR child, which must reject the
    // declarator without entering those children.
    let rejected_source = "void caller() { auto [a, ns::b] = pair; }\n";
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .expect("C++ parser language");
    let rejected = parser
        .parse(rejected_source, None)
        .expect("recovered structured binding");
    let rejected_declarators = declaration_declarators(&rejected);
    assert_eq!(rejected_declarators.len(), 1);
    reset_c_cpp_resolution_work();
    assert_eq!(
        c_cpp_declarator_bound_names(rejected_declarators[0], rejected_source),
        None,
        "a non-identifier binding must fail the whole declarator"
    );
    // init_declarator and the structured binding are entered. The ERROR
    // child and the identifier siblings are not.
    assert_eq!(c_cpp_resolution_work(), 2);
}

#[test]
fn c_cpp_deep_pointer_declarator_indexes_child() {
    if std::env::var_os(DEEP_DECLARATOR_CHILD_ENV).is_none() {
        return;
    }
    let source = deep_pointer_declarator_source(DEEP_POINTER_DECLARATOR_LEVELS);
    let tree = parse("cpp", &source);
    let declarator = first_declaration_declarator(&tree);
    let depth = pointer_declarator_depth(declarator);
    assert!(
        depth >= DEEP_POINTER_DECLARATOR_LEVELS,
        "pointer declarator nested only {depth}"
    );
    reset_c_cpp_resolution_work();
    assert_eq!(
        c_cpp_declarator_bound_names(declarator, &source),
        Some(vec!["x".to_string()])
    );
    // One count per pointer wrapper plus the identifier the recursion entered.
    assert_eq!(c_cpp_resolution_work(), depth + 1);

    let config = crate::get_language_for_ext("cpp").expect("cpp config");
    crate::index_file(Path::new("deep-pointer.cpp"), &source, &config, None, None)
        .expect("deep pointer declarator must index");
    let indexed = index("cpp", &tree, &source, &[]);
    let calls = call_named(&indexed, "x");
    assert_eq!(calls.len(), 1, "deep declarator must still bind x");
    assert!(
        calls[0].identifier_shadowed,
        "collected declarator name must shadow the following call"
    );
}

#[test]
fn c_cpp_deep_pointer_declarator_indexes_in_isolated_child() {
    let output = Command::new(std::env::current_exe().expect("resolve indexer test executable"))
        .arg("--exact")
        .arg(
            "proof_resolution::c_cpp_proof_walk_tests::c_cpp_deep_pointer_declarator_indexes_child",
        )
        .arg("--nocapture")
        .env(DEEP_DECLARATOR_CHILD_ENV, "1")
        .output()
        .expect("run deep pointer declarator child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "deep pointer declarator aborted the isolated child: status={} stdout={stdout} stderr={stderr}",
        output.status
    );
}

fn declaration_declarators(tree: &Tree) -> Vec<TsNode<'_>> {
    let mut declarators = Vec::new();
    let mut pending = vec![tree.root_node()];
    while let Some(node) = pending.pop() {
        if node.kind() == "declaration"
            && let Some(declarator) = node.child_by_field_name("declarator")
        {
            declarators.push(declarator);
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    declarators.sort_by_key(|node| node.start_byte());
    declarators
}

fn first_declaration_declarator(tree: &Tree) -> TsNode<'_> {
    let mut pending = vec![tree.root_node()];
    while let Some(node) = pending.pop() {
        if node.kind() == "declaration"
            && let Some(declarator) = node.child_by_field_name("declarator")
        {
            return declarator;
        }
        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        pending.extend(children.into_iter().rev());
    }
    panic!("declaration declarator");
}

fn declarator_chain_kinds(node: TsNode<'_>) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    let mut current = Some(node);
    while let Some(node) = current {
        kinds.push(node.kind());
        current = node.child_by_field_name("declarator").or_else(|| {
            let mut cursor = node.walk();
            let children = node.named_children(&mut cursor).collect::<Vec<_>>();
            match children.as_slice() {
                [only] => Some(*only),
                _ => None,
            }
        });
    }
    kinds
}

fn pointer_declarator_depth(node: TsNode<'_>) -> usize {
    let mut depth = 0usize;
    let mut current = Some(node);
    while let Some(node) = current {
        if node.kind() == "pointer_declarator" {
            depth += 1;
        }
        current = node.child_by_field_name("declarator").or_else(|| {
            let mut cursor = node.walk();
            node.named_children(&mut cursor).next()
        });
    }
    depth
}

fn deep_pointer_declarator_source(levels: usize) -> String {
    let mut source = String::from("void caller() { int ");
    source.reserve(levels + 16);
    source.extend(std::iter::repeat_n('*', levels));
    source.push_str("x; x(); }\n");
    source
}
