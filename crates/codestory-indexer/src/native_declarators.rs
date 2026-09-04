//! Name native function definitions by their declarator chain, independently of
//! pointer depth. Parameters and unrelated descendants cannot supply the name.

use std::collections::HashMap;
use tree_sitter::{Node, Tree};

use crate::{GraphNodeSpan, ts_node_graph_span, walk_tree_nodes};

pub(crate) fn callable_names(
    language: &str,
    tree: &Tree,
    source: &str,
) -> HashMap<GraphNodeSpan, Option<String>> {
    let mut names = HashMap::new();
    if !matches!(language, "c" | "cpp") {
        return names;
    }
    walk_tree_nodes(tree.root_node(), &mut |definition| {
        if definition.kind() != "function_definition" {
            return;
        }
        let name = definition
            .child_by_field_name("declarator")
            .and_then(declarator_name)
            .and_then(|node| node.utf8_text(source.as_bytes()).ok())
            .map(str::to_string);
        names.insert(ts_node_graph_span(definition), name);
    });
    names
}

fn declarator_name(mut node: Node<'_>) -> Option<Node<'_>> {
    loop {
        match node.kind() {
            "identifier"
            | "field_identifier"
            | "qualified_identifier"
            | "template_function"
            | "destructor_name"
            | "operator_name" => return Some(node),
            "function_declarator" | "pointer_declarator" | "array_declarator" => {
                node = node.child_by_field_name("declarator")?;
            }
            "parenthesized_declarator" | "reference_declarator" | "attributed_declarator" => {
                // These grammar wrappers do not name a `declarator` field.
                // Only the sole non-attribute child can carry the declaration;
                // never search arbitrary descendants or parameter lists.
                let mut cursor = node.walk();
                let mut children = node.named_children(&mut cursor).filter(|child| {
                    !matches!(
                        child.kind(),
                        "comment" | "attribute_declaration" | "ms_call_modifier"
                    )
                });
                let child = children.next()?;
                if children.next().is_some() {
                    return None;
                }
                node = child;
            }
            _ => return None,
        }
    }
}
