//! Python extraction rules.
//!
//! Python's graph extraction lives here: the tree-sitter rule file, the
//! compiled-rule cache, the attribute-call callsite marker, and the
//! receiver-call resolution engine that turns `repo.save(...)`,
//! `self.store.load(...)` and `@decorator`-shaped calls into edges aimed at the
//! owner they were declared against. Every language-keyed dispatch in the crate
//! reaches it through [`super::EXTRACTIONS`] rather than by spelling
//! `"python"`.
//!
//! Five Python entry points stay `pub(crate)` because their *call sites* are
//! shared seams that no registry field describes yet — the seam keeps its
//! `language_name == "python"` guard in `lib.rs` and calls in here for the
//! content:
//!
//! * [`decorator_call_specs`], invoked from the manual-edge collector next to
//!   the Rust macro and Ruby bare-call collectors;
//! * [`is_implicit_receiver`] and [`attribute_method_col`], read by
//!   `lib.rs::append_manual_receiver_call_edges` and
//!   `lib.rs::member_call_method_col`, which every language's receiver engine
//!   shares;
//! * [`is_constant_name`], read by the graph-node projection that promotes a
//!   SCREAMING_CASE Python `VARIABLE` to `CONSTANT`;
//! * [`MEMBER_CALLSITE_MARKER`], read by `resolution` and by the two `lib.rs`
//!   placeholder-annotation rosters.
//!
//! Two further Python surfaces are deliberately *not* here, and both are shared
//! seams rather than Python content:
//!
//! * `lib.rs::collect_python_route` and its `"python"` arm in the
//!   framework-route scanner. The per-language route collectors take
//!   non-uniform arguments, so routing them through the registry is one change
//!   for all sixteen languages, not part of Python's rollback unit.
//! * `LanguageRuleset::Python`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both Python fixtures so the move stays output-equal.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use codestory_contracts::graph::EdgeKind;
use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, ImportedTypeBinding, LanguageRuleset, ManualEdgeSpec,
    ManualReceiverCallSpec, ManualReceiverSource, OptionalReceiverOwnerBinding,
    ReceiverCallSiteKey, collect_colon_parameter_types, collect_receiver_call_specs_in_callable,
    declaration_name, enclosing_node_with_kind, first_descendant_with_kind, member_call_method_col,
    node_source_text, normalize_parameter_name, normalized_receiver_variable,
    parameter_name_before_colon, receiver_call_belongs_to_callable, receiver_callsite_key,
    same_ts_span, signature_parameter_surface, split_top_level_parameters, surface_member_call,
    trimmed_node_text, ts_node_graph_span, walk_tree_nodes,
};

/// Callsite marker written onto edges produced from Python attribute-call
/// syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:python-attribute-call";

const GRAPH_QUERY: &str = include_str!("../../rules/python.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for Python.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["python"],
    language_name: "python",
    extensions: &["py", "pyi"],
    ruleset: LanguageRuleset::Python,
    parser_language: python_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    receiver_call_specs: Some(receiver_call_specs),
    member_callsite_marker: Some(MEMBER_CALLSITE_MARKER),
    graph_call_syntax: Some("python_attribute"),
    // A Python `function_definition` under a `class_definition` is already
    // projected as a METHOD by the rule file, so the projection must not
    // promote it a second time.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: ".",
    // Python comments are `#`, so the route scanner takes the hash-stripping
    // branch rather than the C-style one.
    route_comments_are_c_style: false,
    // `semantic::PythonSemanticResolver` is a dedicated resolver, private to
    // that module; the registry records the choice and the residual match in
    // `semantic::dedicated_semantic_resolver` constructs it.
    uses_generic_semantic_resolver: false,
    semantic_family: "python",
};

fn python_language() -> tree_sitter::Language {
    tree_sitter_python::LANGUAGE.into()
}

pub(crate) fn is_constant_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
        && trimmed.chars().any(|ch| ch.is_ascii_uppercase())
}

fn python_decorator_target_name(node: TsNode<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "decorator" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find_map(|child| python_decorator_target_name(child, source))
        }
        "call" => node
            .child_by_field_name("function")
            .and_then(|function| python_decorator_target_name(function, source)),
        "attribute" => node
            .child_by_field_name("attribute")
            .and_then(|attribute| node_source_text(attribute, source))
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty()),
        "identifier" => node_source_text(node, source)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty()),
        _ => None,
    }
}

pub(crate) fn decorator_call_specs(tree: &Tree, source: &str) -> Vec<ManualEdgeSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "decorated_definition" {
            return;
        }
        let Some(definition) = node.child_by_field_name("definition") else {
            return;
        };
        let Some(source_name) = definition
            .child_by_field_name("name")
            .and_then(|name| node_source_text(name, source))
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
        else {
            return;
        };
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() != "decorator" {
                continue;
            }
            let Some(target_name) = python_decorator_target_name(child, source) else {
                continue;
            };
            edges.push(ManualEdgeSpec {
                source_name: source_name.clone(),
                target_name,
                kind: EdgeKind::CALL,
                line: Some(child.start_position().row as u32 + 1),
            });
        }
    });
    edges
}

pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let imported_type_bindings = collect_python_imported_type_bindings(tree.root_node(), source);
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if callable.kind() != "function_definition" {
            return;
        }
        let Some(source_name) = declaration_name(callable, source) else {
            return;
        };
        let call_source = ManualReceiverSource {
            name: &source_name,
            span: ts_node_graph_span(callable),
        };
        let mut local_receiver_callsites = HashSet::new();
        collect_python_constructor_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &imported_type_bindings,
            &mut local_receiver_callsites,
            &mut edges,
        );
        collect_python_instance_property_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &imported_type_bindings,
            &mut edges,
        );
        let receiver_types = collect_python_receiver_types(callable, source);
        if receiver_types.is_empty() {
            return;
        }
        let start = edges.len();
        collect_receiver_call_specs_in_callable(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &receiver_types,
            member_call,
            true,
            &mut edges,
        );
        let mut fallback_specs = edges.split_off(start);
        fallback_specs
            .retain(|spec| !local_receiver_callsites.contains(&receiver_callsite_key(spec)));
        for spec in &mut fallback_specs {
            if !is_implicit_receiver(&spec.receiver_name)
                && let Some(binding) = imported_type_bindings.get(&spec.owner_name)
            {
                spec.owner_name = binding.owner_name.clone();
                spec.owner_module = Some(binding.module_name.clone());
            }
        }
        edges.extend(fallback_specs);
    });
    edges
}

pub(crate) fn is_implicit_receiver(receiver_name: &str) -> bool {
    matches!(receiver_name, "self" | "cls")
}

fn collect_python_instance_property_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    let Some(class_node) = enclosing_node_with_kind(callable, &["class_definition"]) else {
        return;
    };
    let Some(self_name) = python_instance_self_parameter(callable, source) else {
        return;
    };
    if python_function_has_static_or_classmethod_decorator(callable, source) {
        return;
    }
    walk_tree_nodes(callable, &mut |node| {
        let Some((receiver_name, method_name)) = member_call(node, source) else {
            return;
        };
        if !receiver_call_belongs_to_callable(node, callable)
            || !python_receiver_is_self_property(&receiver_name, &self_name)
        {
            return;
        }
        let Some(owner) = python_visible_instance_property_receiver_owner(
            class_node,
            callable,
            node,
            &receiver_name,
            source,
            imported_type_bindings,
        ) else {
            return;
        };
        if let Some((owner_name, owner_module)) = owner {
            edges.push(ManualReceiverCallSpec {
                source_name: call_source.name.to_string(),
                source_span: call_source.span,
                receiver_name,
                owner_name,
                owner_module,
                method_name: method_name.clone(),
                method_col: member_call_method_col(node, source, &method_name),
                line: Some(node.start_position().row as u32 + 1),
                allow_global_fallback: false,
            });
        }
    });
}

fn python_visible_instance_property_receiver_owner(
    class_node: TsNode<'_>,
    call_method: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> Option<OptionalReceiverOwnerBinding> {
    let mut candidates = Vec::new();
    walk_tree_nodes(class_node, &mut |node| {
        let Some((assignment_receiver, assignment_method, owner)) =
            python_instance_property_assignment_owner(
                node,
                class_node,
                source,
                imported_type_bindings,
            )
        else {
            return;
        };
        if assignment_receiver != receiver_name
            || !python_property_assignment_visible_at_call(
                node,
                assignment_method,
                call_method,
                call_node,
            )
        {
            return;
        }
        candidates.push(owner);
    });
    if candidates.is_empty() {
        return None;
    }
    let Some(mut concrete_owners) = candidates.into_iter().collect::<Option<Vec<_>>>() else {
        return Some(None);
    };
    concrete_owners.sort();
    concrete_owners.dedup();
    if concrete_owners.len() == 1 {
        Some(Some(concrete_owners.remove(0)))
    } else {
        Some(None)
    }
}

fn python_instance_property_assignment_owner<'tree>(
    node: TsNode<'tree>,
    class_node: TsNode<'tree>,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> Option<(String, TsNode<'tree>, OptionalReceiverOwnerBinding)> {
    if node.kind() != "assignment" {
        return None;
    }
    let method = enclosing_node_with_kind(node, &["function_definition"])?;
    if !python_method_belongs_to_class(method, class_node)
        || !receiver_call_belongs_to_callable(node, method)
        || python_function_has_static_or_classmethod_decorator(method, source)
    {
        return None;
    }
    let self_name = python_instance_self_parameter(method, source)?;
    let receiver_name = node
        .child_by_field_name("left")
        .and_then(|left| python_self_property_receiver_name(left, source, &self_name))?;
    let owner =
        python_property_constructor_receiver_owner(node, method, source, imported_type_bindings);
    Some((receiver_name, method, owner))
}

fn python_property_assignment_visible_at_call(
    assignment: TsNode<'_>,
    assignment_method: TsNode<'_>,
    call_method: TsNode<'_>,
    call_node: TsNode<'_>,
) -> bool {
    !same_ts_span(assignment_method, call_method) || assignment.end_byte() <= call_node.start_byte()
}

fn python_instance_self_parameter(method: TsNode<'_>, source: &str) -> Option<String> {
    let self_name = first_python_self_parameter(method, source)?;
    (self_name == "self").then_some(self_name)
}

fn python_method_belongs_to_class(method: TsNode<'_>, class_node: TsNode<'_>) -> bool {
    let mut current = method.parent();
    while let Some(candidate) = current {
        if same_ts_span(candidate, class_node) {
            return true;
        }
        if matches!(candidate.kind(), "function_definition" | "class_definition") {
            return false;
        }
        current = candidate.parent();
    }
    false
}

fn python_receiver_is_self_property(receiver_name: &str, self_name: &str) -> bool {
    receiver_name
        .strip_prefix(self_name)
        .is_some_and(|suffix| suffix.starts_with('.') && suffix.len() > 1)
}

fn python_function_has_static_or_classmethod_decorator(function: TsNode<'_>, source: &str) -> bool {
    let Some(parent) = function.parent() else {
        return false;
    };
    if parent.kind() != "decorated_definition" {
        return false;
    }
    let mut cursor = parent.walk();
    parent.named_children(&mut cursor).any(|child| {
        child.kind() == "decorator"
            && trimmed_node_text(child, source)
                .as_deref()
                .and_then(python_decorator_terminal_name)
                .is_some_and(|name| matches!(name.as_str(), "staticmethod" | "classmethod"))
    })
}

fn python_decorator_terminal_name(surface: &str) -> Option<String> {
    let head = surface
        .trim()
        .trim_start_matches('@')
        .split('(')
        .next()
        .unwrap_or(surface)
        .trim();
    let terminal = head.rsplit('.').next().unwrap_or(head);
    normalize_parameter_name(terminal)
}

fn python_self_property_receiver_name(
    node: TsNode<'_>,
    source: &str,
    self_name: &str,
) -> Option<String> {
    if node.kind() != "attribute" {
        return None;
    }
    let object = node.child_by_field_name("object")?;
    if trimmed_node_text(object, source).as_deref() != Some(self_name) {
        return None;
    }
    let field_name = node
        .child_by_field_name("attribute")
        .and_then(|field| trimmed_node_text(field, source))
        .and_then(|name| normalize_parameter_name(&name))?;
    Some(format!("{self_name}.{field_name}"))
}

fn python_property_constructor_receiver_owner(
    node: TsNode<'_>,
    method: TsNode<'_>,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> OptionalReceiverOwnerBinding {
    let owner_name = node
        .child_by_field_name("right")
        .and_then(|right_node| python_direct_constructor_call_type(right_node, source))?;
    if python_visible_local_type_name(method, node, &owner_name, source) {
        return Some((owner_name, None));
    }
    if python_callable_has_local_binding_name(method, &owner_name, source) {
        return None;
    }
    if let Some(binding) = imported_type_bindings.get(&owner_name) {
        return Some((
            binding.owner_name.clone(),
            Some(binding.module_name.clone()),
        ));
    }
    if python_constructor_name_looks_like_type(&owner_name) {
        Some((owner_name, None))
    } else {
        None
    }
}

fn collect_python_constructor_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    local_receiver_callsites: &mut HashSet<ReceiverCallSiteKey>,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    walk_tree_nodes(callable, &mut |node| {
        let Some((receiver_name, method_name)) = member_call(node, source) else {
            return;
        };
        if !receiver_call_belongs_to_callable(node, callable) {
            return;
        }
        let Some(owner) = python_visible_local_constructor_receiver_owner(
            callable,
            node,
            &receiver_name,
            source,
            imported_type_bindings,
        ) else {
            return;
        };
        let method_col = member_call_method_col(node, source, &method_name);
        local_receiver_callsites.insert(ReceiverCallSiteKey {
            receiver_name: receiver_name.clone(),
            method_name: method_name.clone(),
            line: Some(node.start_position().row as u32 + 1),
            method_col,
        });
        if let Some((owner_name, owner_module)) = owner {
            edges.push(ManualReceiverCallSpec {
                source_name: call_source.name.to_string(),
                source_span: call_source.span,
                receiver_name,
                owner_name,
                owner_module,
                method_name,
                method_col,
                line: Some(node.start_position().row as u32 + 1),
                allow_global_fallback: false,
            });
        }
    });
}

fn python_visible_local_constructor_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> Option<OptionalReceiverOwnerBinding> {
    let mut visible_bindings = Vec::new();
    walk_tree_nodes(callable, &mut |node| {
        if node.kind() != "assignment"
            || !receiver_call_belongs_to_callable(node, callable)
            || node.end_byte() > call_node.start_byte()
        {
            return;
        }
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        if !python_assignment_target_binds_name(left, receiver_name, source) {
            return;
        }
        let owner = if python_plain_identifier_name(left, source).as_deref() == Some(receiver_name)
        {
            python_constructor_receiver_owner(node, callable, source, imported_type_bindings)
        } else {
            None
        };
        visible_bindings.push((node.end_byte(), owner));
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner)| owner)
}

fn python_assignment_target_binds_name(
    node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> bool {
    let mut bindings = HashSet::new();
    collect_python_assignment_target_bindings(node, source, &mut bindings);
    bindings.contains(receiver_name)
}

fn python_constructor_receiver_owner(
    node: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> OptionalReceiverOwnerBinding {
    let owner_name = node
        .child_by_field_name("right")
        .and_then(|right_node| python_direct_constructor_call_type(right_node, source))?;
    if python_visible_local_type_name(callable, node, &owner_name, source) {
        return Some((owner_name, None));
    }
    if python_callable_has_local_binding_name(callable, &owner_name, source) {
        return None;
    }
    if let Some(binding) = imported_type_bindings.get(&owner_name) {
        return Some((
            binding.owner_name.clone(),
            Some(binding.module_name.clone()),
        ));
    }
    if python_constructor_name_looks_like_type(&owner_name) {
        Some((owner_name, None))
    } else {
        None
    }
}

fn python_direct_constructor_call_type(node: TsNode<'_>, source: &str) -> Option<String> {
    if node.kind() != "call" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    if function.kind() != "identifier" {
        return None;
    }
    trimmed_node_text(function, source).and_then(|name| normalize_parameter_name(&name))
}

fn python_visible_local_type_name(
    callable: TsNode<'_>,
    before_node: TsNode<'_>,
    owner_name: &str,
    source: &str,
) -> bool {
    let mut found = false;
    walk_tree_nodes(callable, &mut |node| {
        if found
            || node.kind() != "class_definition"
            || !receiver_call_belongs_to_callable(node, callable)
            || node.end_byte() > before_node.start_byte()
        {
            return;
        }
        if declaration_name(node, source).as_deref() == Some(owner_name) {
            found = true;
        }
    });
    found
}

fn python_callable_has_local_binding_name(
    callable: TsNode<'_>,
    binding_name: &str,
    source: &str,
) -> bool {
    let mut found = false;
    walk_tree_nodes(callable, &mut |node| {
        if found || !receiver_call_belongs_to_callable(node, callable) {
            return;
        }
        let candidate_names = python_local_binding_names(node, source);
        if candidate_names.iter().any(|name| name == binding_name) {
            found = true;
        }
    });
    found
}

fn python_local_binding_names(node: TsNode<'_>, source: &str) -> Vec<String> {
    match node.kind() {
        "class_definition" | "function_definition" => declaration_name(node, source)
            .and_then(|name| normalize_parameter_name(&name))
            .into_iter()
            .collect(),
        "assignment" => {
            let mut bindings = HashSet::new();
            if let Some(left) = node.child_by_field_name("left") {
                collect_python_assignment_target_bindings(left, source, &mut bindings);
            }
            bindings.into_iter().collect()
        }
        "import_from_statement" => python_from_import_local_binding_names(node, source),
        "import_statement" => python_import_local_binding_names(node, source),
        _ => Vec::new(),
    }
}

fn python_from_import_local_binding_names(node: TsNode<'_>, source: &str) -> Vec<String> {
    let Some(surface) = trimmed_node_text(node, source) else {
        return Vec::new();
    };
    let Some(rest) = surface.strip_prefix("from ") else {
        return Vec::new();
    };
    let Some((_, imports)) = rest.split_once(" import ") else {
        return Vec::new();
    };
    let imports = python_import_list_surface(imports);
    split_top_level_parameters(imports)
        .into_iter()
        .filter_map(|imported| python_import_binding_names(&imported).map(|(_, local)| local))
        .collect()
}

fn python_import_local_binding_names(node: TsNode<'_>, source: &str) -> Vec<String> {
    let Some(surface) = trimmed_node_text(node, source) else {
        return Vec::new();
    };
    let Some(imports) = surface.strip_prefix("import ") else {
        return Vec::new();
    };
    split_top_level_parameters(imports)
        .into_iter()
        .filter_map(|imported| python_import_local_binding_name(&imported))
        .collect()
}

fn python_import_local_binding_name(imported: &str) -> Option<String> {
    let imported = imported.trim();
    if imported.is_empty() {
        return None;
    }
    let tokens = imported.split_whitespace().collect::<Vec<_>>();
    let local_name = match tokens.as_slice() {
        [module] => module.split('.').next()?,
        [_, "as", local] => local,
        _ => return None,
    };
    normalize_parameter_name(local_name)
}

fn python_plain_identifier_name(node: TsNode<'_>, source: &str) -> Option<String> {
    if node.kind() != "identifier" {
        return None;
    }
    trimmed_node_text(node, source).and_then(|name| normalize_parameter_name(&name))
}

fn python_constructor_name_looks_like_type(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_uppercase())
}

fn collect_python_imported_type_bindings(
    root: TsNode<'_>,
    source: &str,
) -> HashMap<String, ImportedTypeBinding> {
    let top_level_bindings = collect_python_top_level_binding_names(root, source);
    let mut bindings = HashMap::new();
    let mut duplicates = HashSet::new();
    for statement in python_top_level_from_import_statements(source) {
        let Some(rest) = statement.strip_prefix("from ") else {
            continue;
        };
        let Some((module_name, imports)) = rest.split_once(" import ") else {
            continue;
        };
        let module_name = module_name.trim();
        if module_name.is_empty() {
            continue;
        }
        let imports = python_import_list_surface(imports);
        for imported in split_top_level_parameters(imports) {
            let Some((owner_name, local_name)) = python_import_binding_names(&imported) else {
                continue;
            };
            if top_level_bindings.contains(&local_name) {
                continue;
            }
            if duplicates.contains(&local_name) {
                continue;
            }
            if bindings.contains_key(&local_name) {
                bindings.remove(&local_name);
                duplicates.insert(local_name);
                continue;
            }
            bindings.insert(
                local_name,
                ImportedTypeBinding {
                    module_name: module_name.to_string(),
                    owner_name,
                },
            );
        }
    }
    bindings
}

fn collect_python_top_level_binding_names(root: TsNode<'_>, source: &str) -> HashSet<String> {
    let mut bindings = HashSet::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        collect_python_top_level_binding_name(child, source, &mut bindings);
    }
    bindings
}

fn collect_python_top_level_binding_name(
    node: TsNode<'_>,
    source: &str,
    bindings: &mut HashSet<String>,
) {
    match node.kind() {
        "class_definition" | "function_definition" => {
            if let Some(name) =
                declaration_name(node, source).and_then(|name| normalize_parameter_name(&name))
            {
                bindings.insert(name);
            }
        }
        "decorated_definition" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if matches!(child.kind(), "class_definition" | "function_definition") {
                    collect_python_top_level_binding_name(child, source, bindings);
                    break;
                }
            }
        }
        "assignment" | "type_alias_statement" => {
            collect_python_top_level_assignment_bindings(node, source, bindings);
        }
        "expression_statement" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if matches!(child.kind(), "assignment" | "type_alias_statement") {
                    collect_python_top_level_assignment_bindings(child, source, bindings);
                }
            }
        }
        _ => {}
    }
}

fn collect_python_top_level_assignment_bindings(
    node: TsNode<'_>,
    source: &str,
    bindings: &mut HashSet<String>,
) {
    let Some(target) = node
        .child_by_field_name("left")
        .or_else(|| node.child_by_field_name("name"))
    else {
        return;
    };
    collect_python_assignment_target_bindings(target, source, bindings);
}

fn collect_python_assignment_target_bindings(
    node: TsNode<'_>,
    source: &str,
    bindings: &mut HashSet<String>,
) {
    match node.kind() {
        "identifier" => {
            if let Some(name) =
                trimmed_node_text(node, source).and_then(|name| normalize_parameter_name(&name))
            {
                bindings.insert(name);
            }
        }
        "list"
        | "list_pattern"
        | "parenthesized_expression"
        | "pattern_list"
        | "tuple"
        | "tuple_pattern" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_python_assignment_target_bindings(child, source, bindings);
            }
        }
        _ => {}
    }
}

fn python_top_level_from_import_statements(source: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0usize;
    let mut collecting = false;

    for raw_line in source.lines() {
        if !collecting {
            if raw_line.starts_with(char::is_whitespace) {
                continue;
            }
            let line = python_strip_comment(raw_line).trim();
            if !line.starts_with("from ") {
                continue;
            }
            current.clear();
            current.push_str(line);
            paren_depth = python_paren_depth_delta(0, line);
            if paren_depth == 0 {
                statements.push(current.clone());
            } else {
                collecting = true;
            }
            continue;
        }

        let line = python_strip_comment(raw_line).trim();
        if !line.is_empty() {
            current.push('\n');
            current.push_str(line);
            paren_depth = python_paren_depth_delta(paren_depth, line);
        }
        if paren_depth == 0 {
            statements.push(current.clone());
            current.clear();
            collecting = false;
        }
    }

    statements
}

fn python_strip_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or(line)
}

fn python_paren_depth_delta(mut depth: usize, line: &str) -> usize {
    for ch in line.chars() {
        match ch {
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

fn python_import_list_surface(imports: &str) -> &str {
    let imports = imports.trim();
    imports
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .map(str::trim)
        .unwrap_or(imports)
}

fn python_import_binding_names(imported: &str) -> Option<(String, String)> {
    let imported = imported.trim();
    if imported.is_empty() || imported == "*" {
        return None;
    }

    let tokens = imported.split_whitespace().collect::<Vec<_>>();
    let (owner_name, local_name) = match tokens.as_slice() {
        [owner] => (*owner, *owner),
        [owner, "as", local] => (*owner, *local),
        _ => return None,
    };
    Some((
        normalize_parameter_name(owner_name)?,
        normalize_parameter_name(local_name)?,
    ))
}

fn collect_python_receiver_types(callable: TsNode<'_>, source: &str) -> HashMap<String, String> {
    let mut receiver_types = collect_colon_parameter_types(callable, source);
    if let Some(owner_name) = enclosing_node_with_kind(callable, &["class_definition"])
        .and_then(|owner| declaration_name(owner, source))
        && let Some(self_name) = first_python_self_parameter(callable, source)
    {
        receiver_types.insert(self_name, owner_name);
    }
    receiver_types
}

fn first_python_self_parameter(callable: TsNode<'_>, source: &str) -> Option<String> {
    let parameters = signature_parameter_surface(callable, source)?;
    let first = split_top_level_parameters(&parameters).into_iter().next()?;
    let name_side = first
        .split_once(':')
        .map(|(name_side, _)| name_side)
        .unwrap_or(first.as_str());
    let name = parameter_name_before_colon(name_side)?;
    matches!(name.as_str(), "self" | "cls").then_some(name)
}

fn python_attribute_call_nodes<'tree>(
    node: TsNode<'tree>,
) -> Option<(TsNode<'tree>, TsNode<'tree>)> {
    if node.kind() != "call" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    let attribute = if function.kind() == "attribute" {
        function
    } else {
        first_descendant_with_kind(function, "attribute")?
    };
    Some((
        attribute.child_by_field_name("object")?,
        attribute.child_by_field_name("attribute")?,
    ))
}

pub(crate) fn attribute_method_col(
    node: TsNode<'_>,
    source: &str,
    method_name: &str,
) -> Option<u32> {
    let (_, method) = python_attribute_call_nodes(node)?;
    if trimmed_node_text(method, source).as_deref() != Some(method_name) {
        return None;
    }
    Some(method.start_position().column as u32 + 1)
}

fn member_call(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    if node.kind() != "call" {
        return None;
    }
    if let Some((receiver, method)) = python_attribute_call_nodes(node) {
        return Some((
            normalized_receiver_variable(receiver, source)?,
            trimmed_node_text(method, source)?,
        ));
    }
    surface_member_call(node, source)
}
