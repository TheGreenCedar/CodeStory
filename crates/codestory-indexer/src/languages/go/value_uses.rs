//! Proven values are references, never evidence of invocation.
use super::*;

#[derive(Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct GoValueTarget {
    pub owner: Option<String>,
    pub name: String,
    #[serde(default)]
    pub unresolved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<GoFieldTarget>,
}

#[derive(Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct GoFieldTarget {
    pub owner: String,
    pub path: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct GoStructFields {
    pub owner: String,
    pub fields: Option<HashMap<String, Option<GoFieldDeclaration>>>,
}

#[derive(Debug)]
pub(crate) struct GoFieldDeclaration {
    pub next_owner: Option<String>,
    pub span: GraphNodeSpan,
}

// Only direct named fields of package declarations authorize field paths.
// Descendant walks would promote anonymous, embedded or local type fields.
pub(crate) fn struct_fields(tree: &Tree, source: &str) -> Option<Vec<GoStructFields>> {
    if tree.root_node().has_error() {
        return None;
    }
    let mut declarations = Vec::new();
    let mut cursor = tree.root_node().walk();
    for declaration in tree.root_node().named_children(&mut cursor) {
        if declaration.kind() != "type_declaration" {
            continue;
        }
        let mut cursor = declaration.walk();
        for spec in declaration.named_children(&mut cursor) {
            if !matches!(spec.kind(), "type_spec" | "type_alias") {
                continue;
            }
            let Some(owner) = spec
                .child_by_field_name("name")
                .and_then(|name| trimmed_node_text(name, source))
            else {
                continue;
            };
            let mut fields = None;
            if spec.kind() == "type_spec"
                && spec.child_by_field_name("type_parameters").is_none()
                && let Some(ty) = spec
                    .child_by_field_name("type")
                    .filter(|ty| ty.kind() == "struct_type")
            {
                let mut direct = HashMap::new();
                let mut cursor = ty.walk();
                if let Some(list) = ty
                    .named_children(&mut cursor)
                    .find(|node| node.kind() == "field_declaration_list")
                {
                    let mut cursor = list.walk();
                    for field in list.named_children(&mut cursor) {
                        let mut cursor = field.walk();
                        let names: Vec<_> =
                            field.children_by_field_name("name", &mut cursor).collect();
                        let next_owner = (names.len() == 1)
                            .then(|| field.child_by_field_name("type"))
                            .flatten()
                            .and_then(|ty| {
                                let ty = if ty.kind() == "pointer_type" {
                                    ty.named_child(0)?
                                } else {
                                    ty
                                };
                                (ty.kind() == "type_identifier")
                                    .then(|| trimmed_node_text(ty, source))
                                    .flatten()
                            });
                        for name in names {
                            let Some(name_text) =
                                trimmed_node_text(name, source).filter(|name| name != "_")
                            else {
                                continue;
                            };
                            match direct.entry(name_text) {
                                std::collections::hash_map::Entry::Vacant(entry) => {
                                    entry.insert(Some(GoFieldDeclaration {
                                        next_owner: next_owner.clone(),
                                        span: ts_node_graph_span(name),
                                    }));
                                }
                                std::collections::hash_map::Entry::Occupied(mut entry) => {
                                    entry.insert(None);
                                }
                            }
                        }
                    }
                }
                fields = Some(direct);
            }
            // Unsupported declarations still deny a same-spelling owner.
            declarations.push(GoStructFields { owner, fields });
        }
    }
    Some(declarations)
}

fn field_path(node: TsNode<'_>, source: &str) -> Option<(String, Vec<String>)> {
    let mut node = node;
    let mut path = Vec::new();
    while node.kind() == "selector_expression" {
        path.push(trimmed_node_text(
            node.child_by_field_name("field")?,
            source,
        )?);
        node = node.child_by_field_name("operand")?;
    }
    if node.kind() != "identifier" {
        return None;
    }
    path.reverse();
    Some((trimmed_node_text(node, source)?, path))
}

pub(crate) struct GoValueUse {
    pub source_name: String,
    pub source_span: GraphNodeSpan,
    pub span: GraphNodeSpan,
    pub target: GoValueTarget,
}

/// Lexical type declarations visible at a binding's type expression. This
/// records origins, not type spelling at a later selector use.
pub(super) struct TypeOrigins(GoNameVisibilityIndex);

impl TypeOrigins {
    pub(super) fn prepare(callable: TsNode<'_>, source: &str) -> Self {
        let mut spans = HashMap::<String, Vec<(usize, usize)>>::new();
        walk_go_navigation_nodes(callable, callable, &mut |node, ancestors| {
            count_go_navigation_resolution_work(1);
            let (start, end) = match node.kind() {
                "type_spec" | "type_alias" => {
                    let Some(end) = ancestors.binding_scope() else {
                        return;
                    };
                    (node.start_byte(), end)
                }
                "type_parameter_declaration" => {
                    // Parameters belong to the declaration that owns this
                    // exact list, independently of its enclosing callable.
                    let Some(list) = node
                        .parent()
                        .filter(|parent| parent.kind() == "type_parameter_list")
                    else {
                        return;
                    };
                    let Some(declaration) = list.parent().filter(|owner| {
                        matches!(
                            owner.kind(),
                            "function_declaration" | "type_spec" | "type_alias"
                        )
                    }) else {
                        return;
                    };
                    if declaration
                        .child_by_field_name("type_parameters")
                        .is_none_or(|parameters| parameters.id() != list.id())
                    {
                        return;
                    }
                    count_go_navigation_resolution_work(3);
                    (declaration.start_byte(), declaration.end_byte())
                }
                _ => return,
            };
            let mut cursor = node.walk();
            for name in node.children_by_field_name("name", &mut cursor) {
                if let Some(name) = trimmed_node_text(name, source) {
                    spans.entry(name).or_default().push((start, end));
                }
            }
        });
        let names = spans
            .into_iter()
            .map(|(name, mut ranges)| {
                ranges.sort_unstable_by(|left, right| {
                    count_go_navigation_resolution_work(1);
                    left.cmp(right)
                });
                let mut end = 0;
                let points = ranges
                    .into_iter()
                    .map(|(start_byte, stop)| {
                        count_go_navigation_resolution_work(1);
                        end = end.max(stop);
                        GoNameVisibilityPoint {
                            start_byte,
                            prefix_max_end_byte: end,
                        }
                    })
                    .collect();
                (name, points)
            })
            .collect();
        Self(GoNameVisibilityIndex { names })
    }

    pub(super) fn retain(
        &self,
        owner: OptionalReceiverOwnerBinding,
        origin: usize,
    ) -> OptionalReceiverOwnerBinding {
        owner.filter(|(name, module)| module.is_some() || !self.0.is_visible(name, origin))
    }
}

fn value_position(node: TsNode<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() == "call_expression"
        && parent
            .child_by_field_name("function")
            .is_some_and(|callee| callee.id() == node.id())
    {
        return false;
    }
    matches!(
        parent.kind(),
        "argument_list" | "expression_list" | "return_statement"
    ) && !(parent.kind() == "expression_list"
        && parent.parent().is_some_and(|statement| {
            statement
                .child_by_field_name("left")
                .is_some_and(|left| left.id() == parent.id())
        }))
}

pub(super) fn selector_value(node: TsNode<'_>) -> Option<TsNode<'_>> {
    (node.kind() == "selector_expression" && value_position(node))
        .then(|| node.child_by_field_name("field"))
        .flatten()
}

pub(super) fn selector_names(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    Some((
        normalized_receiver_variable(node.child_by_field_name("operand")?, source)?,
        trimmed_node_text(node.child_by_field_name("field")?, source)?,
    ))
}

pub(crate) fn value_references(tree: &Tree, source: &str) -> Vec<GoValueUse> {
    if tree.root_node().has_error() {
        return Vec::new();
    }
    let imports = collect_go_import_bindings(source);
    let package_imports = collect_go_package_imports(tree.root_node(), source);
    let type_specs = go_package_type_specs_by_name(tree.root_node(), source);
    let file_names = go_file_scope_names(tree.root_node(), source);
    let mut result = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if !matches!(
            callable.kind(),
            "function_declaration" | "method_declaration"
        ) {
            return;
        }
        let Some(source_name) = declaration_name(callable, source) else {
            return;
        };
        let context = GoCallableResolutionContext {
            source,
            import_bindings: &imports,
            return_imports: &package_imports,
            file_scope_names: &file_names,
            callable_name_visibility: go_value_name_visibility(callable, source),
        };
        let (uses, intervals) = go_navigation_facts(callable, &context, true);
        let decisions = go_navigation_binding_decisions(
            callable.start_byte(),
            callable.end_byte(),
            &intervals,
            &uses,
        );
        let field_uses: Vec<_> = uses
            .iter()
            .filter_map(|usage| {
                let (root, _) = field_path(usage.node, source)?;
                selector_value(usage.node)?;
                Some(GoNavigationCall {
                    node: usage.node,
                    receiver_name: root,
                    method_name: usage.method_name.clone(),
                })
            })
            .collect();
        let field_decisions = go_navigation_binding_decisions(
            callable.start_byte(),
            callable.end_byte(),
            &intervals,
            &field_uses,
        );
        for usage in uses {
            let Some(field) = selector_value(usage.node) else {
                continue;
            };
            let owner = match decisions.get(&usage.node.id()) {
                Some(binding) => binding.clone(),
                None => None,
            };
            let proven_owner = owner
                .filter(|(owner, module)| {
                    module.is_none()
                        && !owner.starts_with(RETURN_PATH_OWNER_PREFIX)
                        && !type_specs.get(owner).is_some_and(|spec| {
                            spec.is_none_or(|spec| {
                                spec.child_by_field_name("type")
                                    .is_none_or(|ty| ty.kind() == "interface_type")
                            })
                        })
                })
                .map(|(owner, _)| owner);
            // Retain unsupported selector evidence without treating its name
            // as an unqualified function in the current package.
            let unresolved = proven_owner.is_none();
            result.push(GoValueUse {
                source_name: source_name.clone(),
                source_span: ts_node_graph_span(callable),
                span: ts_node_graph_span(field),
                target: GoValueTarget {
                    owner: proven_owner,
                    name: usage.method_name,
                    unresolved,
                    field: field_decisions
                        .get(&usage.node.id())
                        .cloned()
                        .flatten()
                        .filter(|(owner, module)| {
                            module.is_none() && !owner.starts_with(RETURN_PATH_OWNER_PREFIX)
                        })
                        .and_then(|(owner, _)| {
                            let (_, path) = field_path(usage.node, source)?;
                            Some(GoFieldTarget { owner, path })
                        }),
                },
            });
        }
        // Prepared lexical intervals prevent per-reference ancestor scans.
        walk_tree_nodes(callable, &mut |node| {
            if node.kind() != "identifier" || !value_position(node) {
                return;
            }
            let Some(name) = trimmed_node_text(node, source) else {
                return;
            };
            if name == "_"
                || imports.contains_key(&name)
                || context
                    .callable_name_visibility
                    .is_visible(&name, node.start_byte())
            {
                return;
            }
            result.push(GoValueUse {
                source_name: source_name.clone(),
                source_span: ts_node_graph_span(callable),
                span: ts_node_graph_span(node),
                target: GoValueTarget {
                    owner: None,
                    name,
                    unresolved: false,
                    field: None,
                },
            });
        });
    });
    result
}
