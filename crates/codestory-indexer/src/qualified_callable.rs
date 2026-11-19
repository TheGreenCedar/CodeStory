//! Source declarations for the exact owner/member forms accepted by live lookup.
//! No graph, symbol resolution, artifact cache, or repository scan runs here.

use crate::{Parser, TsNode, get_language_for_ext, walk_tree_nodes};

/// Each supported spelling carries the language and the owner the source must declare.
pub enum CallableOwner<'a> {
    GoPointer(&'a str),
    RustImpl(&'a str),
    PythonClass(&'a str),
}

impl CallableOwner<'_> {
    pub fn name(&self) -> &str {
        match self {
            Self::GoPointer(name) | Self::RustImpl(name) | Self::PythonClass(name) => name,
        }
    }

    pub fn extension(&self) -> &str {
        match self {
            Self::GoPointer(_) => "go",
            Self::RustImpl(_) => "rs",
            Self::PythonClass(_) => "py",
        }
    }
}

/// One-based name lines for declarations with the exact syntactic owner.
/// An empty result establishes no binding. The caller bounds the input bytes.
pub fn qualified_callable_lines(source: &str, owner: &CallableOwner<'_>, name: &str) -> Vec<u32> {
    let Some(config) = get_language_for_ext(owner.extension()) else {
        return Vec::new();
    };
    let mut parser = Parser::new();
    if parser.set_language(&config.language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let matches_text = |node: Option<TsNode<'_>>, text: &str| {
        node.filter(|node| !node.has_error())
            .and_then(|node| node.utf8_text(source.as_bytes()).ok())
            == Some(text)
    };
    let mut lines = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        let field = |name| node.child_by_field_name(name);
        if node.has_error() || !matches_text(field("name"), name) {
            return;
        }
        let correct = match owner {
            CallableOwner::GoPointer(owner) if node.kind() == "method_declaration" => {
                field("receiver").is_some_and(|receiver| {
                    let mut cursor = receiver.walk();
                    let mut parameters = receiver
                        .named_children(&mut cursor)
                        .filter(|child| child.kind() == "parameter_declaration");
                    parameters.next().is_some_and(|parameter| {
                        parameter.child_by_field_name("type").is_some_and(|ty| {
                            ty.kind() == "pointer_type" && matches_text(ty.named_child(0), owner)
                        })
                    }) && parameters.next().is_none()
                })
            }
            CallableOwner::RustImpl(owner) if node.kind() == "function_item" => {
                enclosing(node, "impl_item").is_some_and(|implementation| {
                    implementation.child_by_field_name("trait").is_none()
                        && matches_text(implementation.child_by_field_name("type"), owner)
                })
            }
            CallableOwner::PythonClass(owner) if node.kind() == "function_definition" => {
                enclosing(node, "class_definition").is_some_and(|class| {
                    let module_level = class.parent().is_some_and(|parent| {
                        parent.kind() == "module"
                            || (parent.kind() == "decorated_definition"
                                && parent
                                    .parent()
                                    .is_some_and(|module| module.kind() == "module"))
                    });
                    module_level && matches_text(class.child_by_field_name("name"), owner)
                })
            }
            _ => false,
        };
        if correct && let Some(named) = field("name") {
            lines.push((named.start_position().row + 1) as u32);
        }
    });
    lines
}

fn enclosing<'tree>(node: TsNode<'tree>, kind: &str) -> Option<TsNode<'tree>> {
    let mut parent = node.parent();
    while let Some(current) = parent {
        if current.kind() == kind {
            return Some(current);
        }
        if !matches!(
            current.kind(),
            "block" | "declaration_list" | "decorated_definition"
        ) {
            return None;
        }
        parent = current.parent();
    }
    None
}
