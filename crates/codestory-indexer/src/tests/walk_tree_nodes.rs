use super::*;
use std::process::Command;
use tree_sitter::Node;

const DEEP_WALK_CHILD_ENV: &str = "CODESTORY_TEST_WALK_TREE_NODES_DEEP";
const DEEP_COMPOUND_DEPTH: usize = 24_000;

fn parse_c(source: &str) -> Tree {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c::LANGUAGE.into())
        .expect("C parser language");
    let tree = parser.parse(source, None).expect("C syntax tree");
    assert!(
        !tree.root_node().has_error(),
        "fixture must be syntactically valid C"
    );
    tree
}

fn recursive_named_preorder<'tree, F>(node: Node<'tree>, visit: &mut F)
where
    F: FnMut(Node<'tree>),
{
    visit(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        recursive_named_preorder(child, visit);
    }
}

fn walk_ids(node: Node<'_>) -> Vec<usize> {
    let mut ids = Vec::new();
    walk_tree_nodes(node, &mut |current| ids.push(current.id()));
    ids
}

fn recursive_ids(node: Node<'_>) -> Vec<usize> {
    let mut ids = Vec::new();
    recursive_named_preorder(node, &mut |current| ids.push(current.id()));
    ids
}

fn first_named<'tree>(node: Node<'tree>, kind: &str) -> Node<'tree> {
    let mut found = None;
    walk_tree_nodes(node, &mut |current| {
        if found.is_none() && current.kind() == kind {
            found = Some(current);
        }
    });
    found.unwrap_or_else(|| panic!("missing named `{kind}` node"))
}

fn deep_compound_source(depth: usize) -> String {
    let mut source = String::from("void f(void)\n");
    source.reserve(depth * 2 + 16);
    source.extend(std::iter::repeat_n('{', depth));
    source.push_str("int x;");
    source.extend(std::iter::repeat_n('}', depth));
    source.push('\n');
    source
}

#[test]
fn walk_tree_nodes_matches_recursive_named_preorder_on_shallow_c_trees() {
    let balanced = parse_c(
        r#"
int balanced(int n) {
    if (n) {
        if (n) {
            return n;
        } else {
            return n;
        }
    } else {
        if (n) {
            return n;
        } else {
            return n;
        }
    }
}
"#,
    );
    let wide = parse_c(
        r#"
void wide(void) {
    int a0; int a1; int a2; int a3; int a4;
    int a5; int a6; int a7; int a8; int a9;
}
"#,
    );
    let mixed = parse_c(
        r#"
int mixed(int n) {
    int x = (((n)));
    return x;
}
"#,
    );

    for tree in [&balanced, &wide, &mixed] {
        let root = tree.root_node();
        assert_eq!(walk_ids(root), recursive_ids(root), "root preorder");

        let function = first_named(root, "function_definition");
        assert_eq!(
            walk_ids(function),
            recursive_ids(function),
            "subtree preorder"
        );
        let subtree_end = function.end_byte();
        walk_tree_nodes(function, &mut |node| {
            assert!(
                node.start_byte() >= function.start_byte() && node.end_byte() <= subtree_end,
                "subtree walk escaped the function_definition span"
            );
        });

        let compound = first_named(root, "compound_statement");
        let mut named = 0usize;
        let mut unnamed = 0usize;
        let mut cursor = compound.walk();
        if cursor.goto_first_child() {
            loop {
                if cursor.node().is_named() {
                    named += 1;
                } else {
                    unnamed += 1;
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
        assert!(named > 0, "compound_statement needs named children");
        assert!(unnamed > 0, "compound_statement needs unnamed braces");

        let visited = walk_ids(root);
        walk_tree_nodes(root, &mut |node| {
            assert!(node.is_named(), "walker must skip unnamed nodes");
        });
        assert_eq!(visited, recursive_ids(root));
    }
}

#[test]
fn walk_tree_nodes_visits_unnamed_start_and_keeps_subtree_boundary() {
    let tree = parse_c(
        r#"
void boundary(void) {
    int left;
    int right;
}
"#,
    );
    let root = tree.root_node();
    let compound = first_named(root, "compound_statement");
    let mut unnamed_brace = None;
    let mut cursor = compound.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if !child.is_named() {
                unnamed_brace = Some(child);
                break;
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    let unnamed_start = unnamed_brace.expect("compound_statement opens with an unnamed brace");
    assert!(!unnamed_start.is_named());
    assert_eq!(
        walk_ids(unnamed_start),
        recursive_ids(unnamed_start),
        "unnamed start must still be visited"
    );
    assert_eq!(
        walk_ids(unnamed_start),
        vec![unnamed_start.id()],
        "unnamed brace is a leaf: no child descent, no sibling/parent leakage"
    );

    let mut escaped = false;
    walk_tree_nodes(compound, &mut |node| {
        if node.id() != compound.id()
            && (node.start_byte() < compound.start_byte() || node.end_byte() > compound.end_byte())
        {
            escaped = true;
        }
    });
    assert!(!escaped, "compound_statement walk must not leave its span");
    assert_eq!(walk_ids(compound), recursive_ids(compound));
}

#[test]
fn walk_tree_nodes_survives_deep_c_tree_child() {
    if std::env::var_os(DEEP_WALK_CHILD_ENV).is_none() {
        return;
    }
    let source = deep_compound_source(DEEP_COMPOUND_DEPTH);
    let tree = parse_c(&source);
    let mut visited = 0usize;
    walk_tree_nodes(tree.root_node(), &mut |_| visited += 1);
    assert!(
        visited > DEEP_COMPOUND_DEPTH,
        "deep fixture must visit nested compounds, got {visited}"
    );
    let names = crate::native_declarators::callable_names("c", &tree, &source);
    assert_eq!(names.len(), 1);
}

#[test]
fn walk_tree_nodes_survives_deep_c_tree_in_isolated_subprocess() {
    let status = Command::new(std::env::current_exe().expect("resolve indexer test executable"))
        .arg("--exact")
        .arg("tests::walk_tree_nodes::walk_tree_nodes_survives_deep_c_tree_child")
        .arg("--nocapture")
        .env(DEEP_WALK_CHILD_ENV, "1")
        .status()
        .expect("run deep C tree walk child");
    assert!(
        status.success(),
        "deep named-child walk aborted the isolated child: {status}"
    );
}
