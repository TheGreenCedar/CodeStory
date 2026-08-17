//! JavaScript extraction rules.
//!
//! JavaScript's graph extraction lives here: the tree-sitter rule file, the
//! compiled-rule cache, the member-call callsite marker, and the receiver-call
//! resolution engine that turns `workflow.run(...)` into an edge aimed at
//! `Workflow.run`. Every language-keyed dispatch in the crate reaches it
//! through [`super::EXTRACTIONS`] rather than by spelling `"javascript"`.
//!
//! Three JavaScript-adjacent surfaces are deliberately *not* here, and none of
//! them is JavaScript content:
//!
//! * the `js_*` / `js_ts_*` helpers in `lib.rs`
//!   (`js_like_callable_source_name`, `js_ts_visible_local_type_name`,
//!   `js_ts_local_binding_visible_at_call`,
//!   `normalize_js_ts_private_receiver_surface`,
//!   `normalized_receiver_variable`,
//!   `collect_typescript_imported_type_bindings`,
//!   `typescript_property_belongs_to_owner`), plus
//!   `collect_javascript_static_call_edges` and
//!   `collect_javascript_runtime_import_specs`. TypeScript and TSX call all of
//!   them, so they are the shared JS-family seam and move — if ever — with the
//!   last of the three dialects, not with this rollback unit.
//! * the `"javascript" | "typescript"` arm of the framework-route scanner. The
//!   per-language route collectors take non-uniform arguments and per-framework
//!   `has_<framework>` preconditions, so routing them through the registry is
//!   one change for all sixteen languages.
//! * `LanguageRuleset::JavaScript`, which stays in `lib.rs` because the enum is
//!   the compiled-rule cache key for every language at once.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both JavaScript fixtures so the move stays
//! output-equal.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use super::typescript::{
    collect_typescript_imported_type_bindings, typescript_property_belongs_to_owner,
};
use crate::{
    CompiledLanguageRules, ImportedTypeBinding, LanguageRuleset, ManualReceiverCallSpec,
    ManualReceiverSource, OptionalReceiverOwnerBinding, ReceiverCallSiteKey, ReceiverOwnerBinding,
    collect_receiver_call_specs_in_callable, declaration_name, enclosing_node_with_kind,
    javascript_binding_has_prior_write, js_like_callable_source_name,
    js_ts_local_binding_visible_at_call, js_ts_visible_local_type_name, member_call_method_col,
    normalize_js_ts_private_receiver_surface, normalize_parameter_name,
    normalized_receiver_variable, receiver_call_belongs_to_callable, receiver_callsite_key,
    same_ts_span, trimmed_node_text, ts_node_graph_span, walk_tree_nodes,
};

/// Callsite marker written onto edges produced from JavaScript member-call
/// syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:js-member-call";

/// Callsite marker for a bare call whose exact local name is also a runtime import binding.
pub(crate) const RUNTIME_IMPORT_CALLSITE_MARKER: &str = "syntax:js-runtime-import-call";

const GRAPH_QUERY: &str = include_str!("../../rules/javascript.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for JavaScript.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["javascript"],
    language_name: "javascript",
    extensions: &["js", "jsx", "mjs", "cjs"],
    ruleset: LanguageRuleset::JavaScript,
    parser_language: javascript_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    member_edge_specs: None,
    receiver_call_specs: Some(receiver_call_specs),
    member_callsite_marker: Some(MEMBER_CALLSITE_MARKER),
    graph_call_syntax: Some("js_member"),
    // `method_definition` already projects as METHOD straight out of the rule
    // file, so JavaScript never asked for the FUNCTION -> METHOD promotion;
    // `swift` and `dart` are the only languages that did.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: ".",
    route_comments_are_c_style: true,
    // `JavaScriptSemanticResolver` is private to `semantic`, so the registry
    // records the choice and `dedicated_semantic_resolver` still builds it.
    uses_generic_semantic_resolver: false,
    // Shared with typescript/vue/svelte/astro: candidates inside the JS family
    // must stay reachable across those surfaces.
    semantic_family: "webscript",
};

fn javascript_language() -> tree_sitter::Language {
    tree_sitter_javascript::LANGUAGE.into()
}

/// Manual receiver-call edges for one parsed JavaScript file.
///
/// Was `lib.rs::collect_javascript_receiver_call_edges`.
pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let imported_type_bindings =
        collect_typescript_imported_type_bindings(tree.root_node(), source);
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if !matches!(
            callable.kind(),
            "method_definition" | "function_declaration" | "arrow_function" | "function_expression"
        ) {
            return;
        }
        let Some(source_name) = js_like_callable_source_name(callable, source) else {
            return;
        };
        let call_source = ManualReceiverSource {
            name: &source_name,
            span: ts_node_graph_span(callable),
        };
        let mut local_receiver_callsites = HashSet::new();
        collect_javascript_constructor_receiver_call_specs(
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
        let mut receiver_types = HashMap::new();
        if let Some(owner_name) = enclosing_node_with_kind(callable, &["class_declaration"])
            .and_then(|owner| declaration_name(owner, source))
            && callable.kind() == "method_definition"
        {
            receiver_types.insert("this".to_string(), owner_name);
        }
        if let Some(owner_name) = javascript_property_assigned_function_owner(callable, source) {
            collect_javascript_property_assigned_this_receiver_call_specs(
                callable,
                source,
                ManualReceiverSource {
                    name: call_source.name,
                    span: call_source.span,
                },
                &owner_name,
                &mut edges,
            );
            collect_javascript_property_assigned_alias_receiver_call_specs(
                callable,
                source,
                ManualReceiverSource {
                    name: call_source.name,
                    span: call_source.span,
                },
                &owner_name,
                &mut edges,
            );
        }
        let property_receiver_types = collect_javascript_class_property_receiver_types(
            callable,
            source,
            &imported_type_bindings,
        );
        receiver_types.extend(
            property_receiver_types
                .iter()
                .map(|(receiver_name, (owner_name, _))| {
                    (receiver_name.clone(), owner_name.clone())
                }),
        );
        if receiver_types.is_empty() {
            return;
        }
        let mut receiver_modules = HashMap::new();
        for (receiver_name, (_, owner_module)) in &property_receiver_types {
            if let Some(module_name) = owner_module {
                receiver_modules.insert(receiver_name.clone(), module_name.clone());
            }
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
            false,
            &mut edges,
        );
        let mut fallback_specs = edges.split_off(start);
        fallback_specs
            .retain(|spec| !local_receiver_callsites.contains(&receiver_callsite_key(spec)));
        for spec in &mut fallback_specs {
            if let Some(module_name) = receiver_modules.get(&spec.receiver_name) {
                spec.owner_module = Some(module_name.clone());
            }
        }
        edges.extend(fallback_specs);
    });
    edges
}

fn javascript_property_assigned_function_owner(
    callable: TsNode<'_>,
    source: &str,
) -> Option<String> {
    if callable.kind() != "function_expression" {
        return None;
    }
    let assignment = callable.parent().filter(|parent| {
        parent.kind() == "assignment_expression"
            && parent
                .child_by_field_name("right")
                .is_some_and(|right| same_ts_span(right, callable))
    })?;
    let left = assignment
        .child_by_field_name("left")
        .filter(|left| left.kind() == "member_expression")?;
    left.child_by_field_name("property")
        .filter(|property| property.kind() == "property_identifier")?;
    let owner = left.child_by_field_name("object")?;
    normalized_receiver_variable(owner, source)
}

fn collect_javascript_property_assigned_this_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    owner_name: &str,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    walk_tree_nodes(callable, &mut |call| {
        let Some((receiver_name, method_name)) = member_call(call, source) else {
            return;
        };
        if !javascript_this_receiver_inherits_from_property_callable(call, callable) {
            return;
        }
        let Some(suffix) = receiver_name.strip_prefix("this").map(str::to_string) else {
            return;
        };
        if !suffix.is_empty() && !suffix.starts_with('.') {
            return;
        }
        edges.push(ManualReceiverCallSpec {
            source_name: call_source.name.to_string(),
            source_span: call_source.span,
            receiver_name,
            owner_name: format!("{owner_name}{suffix}"),
            owner_module: None,
            method_name: method_name.clone(),
            method_col: member_call_method_col(call, source, &method_name),
            line: Some(call.start_position().row as u32 + 1),
            allow_global_fallback: false,
        });
    });
}

fn javascript_this_receiver_inherits_from_property_callable(
    call: TsNode<'_>,
    callable: TsNode<'_>,
) -> bool {
    let mut current = call;
    while let Some(parent) = current.parent() {
        if same_ts_span(parent, callable) {
            return true;
        }
        if javascript_callable_node(parent) && parent.kind() != "arrow_function" {
            return false;
        }
        current = parent;
    }
    false
}

fn collect_javascript_property_assigned_alias_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    owner_name: &str,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    walk_tree_nodes(callable, &mut |call| {
        let Some((receiver_name, method_name)) = member_call(call, source) else {
            return;
        };
        if receiver_name.contains('.') {
            return;
        }
        let Some(origin) =
            javascript_visible_property_alias_origin(callable, call, &receiver_name, source)
        else {
            return;
        };
        let Some(property) = origin.strip_prefix("this.") else {
            return;
        };
        edges.push(ManualReceiverCallSpec {
            source_name: call_source.name.to_string(),
            source_span: call_source.span,
            receiver_name,
            owner_name: format!("{owner_name}.{property}"),
            owner_module: None,
            method_name: method_name.clone(),
            method_col: member_call_method_col(call, source, &method_name),
            line: Some(call.start_position().row as u32 + 1),
            allow_global_fallback: false,
        });
    });
}

fn javascript_visible_property_alias_origin(
    callable: TsNode<'_>,
    call: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> Option<String> {
    if javascript_enclosing_binding_shadows_alias(callable, call, receiver_name, source) {
        return None;
    }

    let mut declarations = Vec::new();
    walk_tree_nodes(callable, &mut |node| {
        if node.kind() != "variable_declarator"
            || !js_ts_local_binding_visible_at_call(node, call)
            || node
                .child_by_field_name("name")
                .and_then(|name| trimmed_node_text(name, source))
                .as_deref()
                != Some(receiver_name)
        {
            return;
        }
        declarations.push(node);
    });
    let [declaration] = declarations.as_slice() else {
        return None;
    };
    if declaration.end_byte() >= call.start_byte()
        || !receiver_call_belongs_to_callable(*declaration, callable)
    {
        return None;
    }
    let origin = declaration
        .child_by_field_name("value")
        .and_then(|value| javascript_exact_this_property(value, source))?;

    (!javascript_binding_has_prior_write(
        callable,
        source,
        declaration
            .child_by_field_name("name")
            .expect("exact alias declaration name"),
        declaration.end_byte(),
        call,
    ))
    .then_some(origin)
}

fn javascript_exact_this_property(node: TsNode<'_>, source: &str) -> Option<String> {
    if node.kind() != "member_expression"
        || node
            .child_by_field_name("object")
            .is_none_or(|object| object.kind() != "this")
        || node
            .child_by_field_name("property")
            .is_none_or(|property| property.kind() != "property_identifier")
    {
        return None;
    }
    normalized_receiver_variable(node, source).filter(|surface| {
        surface
            .strip_prefix("this.")
            .is_some_and(|property| !property.is_empty() && !property.contains('.'))
    })
}

fn javascript_enclosing_binding_shadows_alias(
    callable: TsNode<'_>,
    call: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> bool {
    let mut current = Some(call);
    while let Some(node) = current {
        if javascript_callable_node(node)
            && javascript_callable_binds_name(node, receiver_name, source)
        {
            return true;
        }
        if node.kind() == "catch_clause"
            && node
                .child_by_field_name("parameter")
                .is_some_and(|parameter| {
                    javascript_binding_pattern_has_name(parameter, receiver_name, source)
                })
        {
            return true;
        }
        if same_ts_span(node, callable) {
            break;
        }
        current = node.parent();
    }

    let mut shadowed = false;
    walk_tree_nodes(callable, &mut |node| {
        if shadowed
            || !matches!(node.kind(), "function_declaration" | "class_declaration")
            || !js_ts_local_binding_visible_at_call(node, call)
        {
            return;
        }
        shadowed = node
            .child_by_field_name("name")
            .is_some_and(|name| javascript_simple_identifier_is(name, receiver_name, source));
    });
    shadowed
}

fn javascript_callable_node(node: TsNode<'_>) -> bool {
    matches!(
        node.kind(),
        "function_declaration"
            | "function_expression"
            | "generator_function_declaration"
            | "generator_function"
            | "arrow_function"
            | "method_definition"
    )
}

fn javascript_callable_binds_name(callable: TsNode<'_>, name: &str, source: &str) -> bool {
    callable
        .child_by_field_name("name")
        .is_some_and(|binding| javascript_simple_identifier_is(binding, name, source))
        || callable
            .child_by_field_name("parameters")
            .or_else(|| callable.child_by_field_name("parameter"))
            .is_some_and(|parameters| javascript_binding_pattern_has_name(parameters, name, source))
}

fn javascript_binding_pattern_has_name(node: TsNode<'_>, name: &str, source: &str) -> bool {
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => {
            javascript_simple_identifier_is(node, name, source)
        }
        "pair_pattern" => node
            .child_by_field_name("value")
            .is_some_and(|value| javascript_binding_pattern_has_name(value, name, source)),
        "assignment_pattern" => node
            .child_by_field_name("left")
            .is_some_and(|left| javascript_binding_pattern_has_name(left, name, source)),
        "rest_pattern" => node
            .child_by_field_name("argument")
            .is_some_and(|argument| javascript_binding_pattern_has_name(argument, name, source)),
        _ => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .any(|child| javascript_binding_pattern_has_name(child, name, source))
        }
    }
}

fn javascript_simple_identifier_is(node: TsNode<'_>, expected: &str, source: &str) -> bool {
    node.kind() == "identifier" && trimmed_node_text(node, source).as_deref() == Some(expected)
}

fn collect_javascript_constructor_receiver_call_specs(
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
        let Some(owner) = javascript_visible_local_constructor_receiver_owner(
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

fn javascript_visible_local_constructor_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> Option<OptionalReceiverOwnerBinding> {
    let mut visible_bindings = Vec::new();
    walk_tree_nodes(callable, &mut |node| {
        if node.kind() != "variable_declarator"
            || !receiver_call_belongs_to_callable(node, callable)
            || node.end_byte() > call_node.start_byte()
            || !js_ts_local_binding_visible_at_call(node, call_node)
        {
            return;
        }
        let Some(binding_name) = node
            .child_by_field_name("name")
            .and_then(|name_node| trimmed_node_text(name_node, source))
            .as_deref()
            .and_then(normalize_parameter_name)
        else {
            return;
        };
        if binding_name != receiver_name {
            return;
        }
        visible_bindings.push((
            node.end_byte(),
            javascript_constructor_receiver_owner(node, callable, source, imported_type_bindings),
        ));
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner)| owner)
}

fn javascript_constructor_receiver_owner(
    node: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> OptionalReceiverOwnerBinding {
    let value_node = node.child_by_field_name("value")?;
    javascript_new_expression_receiver_owner(
        value_node,
        callable,
        node,
        source,
        imported_type_bindings,
    )
}

fn javascript_new_expression_receiver_owner(
    value_node: TsNode<'_>,
    scope_node: TsNode<'_>,
    before_node: TsNode<'_>,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> OptionalReceiverOwnerBinding {
    if value_node.kind() != "new_expression" {
        return None;
    }
    let owner_name = value_node
        .child_by_field_name("constructor")
        .filter(|constructor| constructor.kind() == "identifier")
        .and_then(|constructor| trimmed_node_text(constructor, source))
        .as_deref()
        .and_then(normalize_parameter_name)?;
    if js_ts_visible_local_type_name(scope_node, before_node, &owner_name, source) {
        return Some((owner_name, None));
    }
    imported_type_bindings
        .get(&owner_name)
        .map(|binding| {
            (
                binding.owner_name.clone(),
                Some(binding.module_name.clone()),
            )
        })
        .or(Some((owner_name, None)))
}

fn collect_javascript_class_property_receiver_types(
    callable: TsNode<'_>,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> HashMap<String, ReceiverOwnerBinding> {
    let mut receiver_types = HashMap::new();
    if callable.kind() != "method_definition" || javascript_method_is_static(callable, source) {
        return receiver_types;
    }
    let Some(class_node) = enclosing_node_with_kind(callable, &["class_declaration"]) else {
        return receiver_types;
    };
    let mut candidates: HashMap<String, Vec<OptionalReceiverOwnerBinding>> = HashMap::new();
    walk_tree_nodes(class_node, &mut |node| {
        let Some((receiver_name, scope_node, value_node)) =
            javascript_property_receiver_candidate(node, class_node, source)
        else {
            return;
        };
        let owner_name = javascript_new_expression_receiver_owner(
            value_node,
            scope_node,
            node,
            source,
            imported_type_bindings,
        );
        candidates
            .entry(receiver_name)
            .or_default()
            .push(owner_name);
    });
    for (receiver_name, owners) in candidates {
        let Some(mut concrete_owners) = owners.into_iter().collect::<Option<Vec<_>>>() else {
            continue;
        };
        concrete_owners.sort();
        concrete_owners.dedup();
        if concrete_owners.len() == 1 {
            receiver_types.insert(receiver_name, concrete_owners.remove(0));
        }
    }
    receiver_types
}

fn javascript_property_receiver_candidate<'tree>(
    node: TsNode<'tree>,
    class_node: TsNode<'tree>,
    source: &str,
) -> Option<(String, TsNode<'tree>, TsNode<'tree>)> {
    if node.kind() == "assignment_expression"
        && javascript_assignment_matches_instance_property_domain(node, class_node, source)
    {
        let receiver_name = node
            .child_by_field_name("left")
            .and_then(|left| javascript_this_property_receiver_name(left, source))?;
        let scope_node =
            enclosing_node_with_kind(node, &["method_definition"]).unwrap_or(class_node);
        return Some((
            receiver_name,
            scope_node,
            node.child_by_field_name("right")?,
        ));
    }
    if matches!(node.kind(), "field_definition" | "public_field_definition")
        && typescript_property_belongs_to_owner(node, class_node)
        && !javascript_surface_starts_with_static(node, source)
    {
        let field_name = javascript_class_field_name(node, source)?;
        return Some((
            format!("this.{field_name}"),
            class_node,
            node.child_by_field_name("value")?,
        ));
    }
    None
}

fn javascript_class_field_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .and_then(|name| trimmed_node_text(name, source))
        .or_else(|| {
            trimmed_node_text(node, source).map(|surface| {
                surface
                    .split('=')
                    .next()
                    .unwrap_or(surface.as_str())
                    .trim()
                    .to_string()
            })
        })
        .as_deref()
        .and_then(normalize_parameter_name)
}

fn javascript_assignment_matches_instance_property_domain(
    assignment: TsNode<'_>,
    class_node: TsNode<'_>,
    source: &str,
) -> bool {
    if !enclosing_node_with_kind(assignment, &["class_declaration"])
        .is_some_and(|owner| same_ts_span(owner, class_node))
    {
        return false;
    }
    let Some(method) = enclosing_node_with_kind(assignment, &["method_definition"]) else {
        return false;
    };
    if javascript_method_is_static(method, source)
        || !enclosing_node_with_kind(method, &["class_declaration"])
            .is_some_and(|owner| same_ts_span(owner, class_node))
    {
        return false;
    }
    receiver_call_belongs_to_callable(assignment, method)
}

fn javascript_method_is_static(method: TsNode<'_>, source: &str) -> bool {
    javascript_surface_starts_with_static(method, source)
}

fn javascript_surface_starts_with_static(node: TsNode<'_>, source: &str) -> bool {
    trimmed_node_text(node, source).is_some_and(|surface| {
        surface
            .trim_start()
            .strip_prefix("static")
            .is_some_and(|rest| rest.chars().next().is_none_or(|ch| ch.is_whitespace()))
    })
}

fn javascript_this_property_receiver_name(node: TsNode<'_>, source: &str) -> Option<String> {
    let receiver_name = normalized_receiver_variable(node, source)?;
    let field_name = receiver_name.strip_prefix("this.")?;
    Some(format!("this.{}", normalize_parameter_name(field_name)?))
}

fn member_call(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    if function.kind() != "member_expression" {
        return None;
    }
    let receiver = function.child_by_field_name("object")?;
    let method = function.child_by_field_name("property")?;
    Some((
        normalize_js_ts_private_receiver_surface(&normalized_receiver_variable(receiver, source)?),
        trimmed_node_text(method, source)?,
    ))
}
