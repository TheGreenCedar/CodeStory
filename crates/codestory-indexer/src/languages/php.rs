//! PHP extraction rules.
//!
//! PHP's graph extraction lives here: the tree-sitter rule file, the
//! compiled-rule cache, the member-call callsite marker, the namespace/type
//! MEMBER collector, and the receiver-call resolution engine that turns
//! `$this->repo->save(...)` into an edge aimed at `Repository.save`. Every
//! language-keyed dispatch in the crate reaches those through
//! [`super::EXTRACTIONS`] rather than by spelling `"php"`.
//!
//! Three PHP-adjacent surfaces are deliberately *not* here, and all three are
//! shared seams rather than PHP content:
//!
//! * `lib.rs::collect_laravel_route` and its `"php"` arm in the framework-route
//!   scanner. The per-language route collectors take non-uniform arguments and
//!   a per-framework `has_<framework>` precondition, so routing them through
//!   the registry is one change for all sixteen languages, not part of PHP's
//!   rollback unit.
//! * `LanguageRuleset::Php`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//! * `lib.rs::language_member_specs`, which is still a plain `match` because
//!   [`super::LanguageExtraction`] has no manual-MEMBER field yet; adding one
//!   would touch every sibling package's row. Its `"php"` arm therefore stays
//!   in `lib.rs` and calls [`member_edge_specs`] here, so the rule body moved
//!   even though the dispatch has not.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both PHP fixtures so the move stays output-equal.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, ImportedTypeBinding, LanguageRuleset, ManualMemberEdgeSpec,
    ManualReceiverCallSpec, ManualReceiverSource, OptionalReceiverOwnerBinding,
    ReceiverCallSiteKey, ReceiverOwnerBinding, collect_enclosing_type_member_edges,
    collect_receiver_call_specs_in_callable, declaration_name, enclosing_node_with_kind,
    member_call_method_col, node_is_same_or_ancestor, normalize_parameter_name,
    normalize_type_surface, normalized_receiver_variable, receiver_call_belongs_to_callable,
    receiver_callsite_key, same_ts_span, split_top_level_parameters, trimmed_node_text,
    ts_node_graph_span, walk_tree_nodes,
};

/// Callsite marker written onto edges produced from PHP member-call syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:php-member-call";

/// Callsite marker written onto placeholder CALL edges produced from PHP
/// `new Type(...)` construction syntax.
pub(crate) const NEW_CALLSITE_MARKER: &str = "syntax:php-new";

/// Prefix of the combined foreach-element binding marker; the full marker is
/// `receiver-binding:loop-element@{loop_start}-{loop_end}` with the exact
/// 1-based line span of the `foreach` statement that bound the receiver.
pub(crate) const LOOP_ELEMENT_BINDING_MARKER_PREFIX: &str = "receiver-binding:loop-element@";

const GRAPH_QUERY: &str = include_str!("../../rules/php.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for PHP.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["php"],
    language_name: "php",
    extensions: &["php"],
    ruleset: LanguageRuleset::Php,
    parser_language: php_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    member_edge_specs: Some(member_edge_specs),
    receiver_call_specs: Some(receiver_call_specs),
    type_usage_specs: None,
    callsite_marker_families: &[
        ("php_member", MEMBER_CALLSITE_MARKER),
        ("php_new", NEW_CALLSITE_MARKER),
    ],
    // PHP methods are already `method_declaration` nodes, so the projection
    // never had to promote FUNCTION to METHOD for an owned member; `php` was
    // absent from the promotion roster before the move.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: ".",
    route_comments_are_c_style: true,
    // `semantic::PhpSemanticResolver` is a dedicated resolver type private to
    // that module, so the registry records the choice and `semantic::mod`
    // still constructs it.
    uses_generic_semantic_resolver: false,
    semantic_family: "php",
};

fn php_language() -> tree_sitter::Language {
    tree_sitter_php::LANGUAGE_PHP.into()
}

/// Manual MEMBER edges for one parsed PHP file.
///
/// Was `lib.rs::collect_php_member_edges`.
pub(crate) fn member_edge_specs(tree: &Tree, source: &str) -> Vec<ManualMemberEdgeSpec> {
    let mut edges = collect_enclosing_type_member_edges(
        tree,
        source,
        &[
            "class_declaration",
            "interface_declaration",
            "trait_declaration",
        ],
        &["method_declaration"],
    );
    edges.extend(collect_php_namespace_member_edges(tree, source));
    edges
}

fn collect_php_namespace_member_edges(tree: &Tree, source: &str) -> Vec<ManualMemberEdgeSpec> {
    let mut edges = Vec::new();
    let root = tree.root_node();
    walk_tree_nodes(root, &mut |namespace| {
        if namespace.kind() != "namespace_definition" {
            return;
        }
        let Some(body) = namespace.child_by_field_name("body") else {
            return;
        };
        collect_php_namespace_member_edges_in_scope(namespace, body, source, &mut edges);
    });

    let mut current_namespace = None;
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "namespace_definition" {
            current_namespace = child.child_by_field_name("body").is_none().then_some(child);
            continue;
        }
        let Some(namespace) = current_namespace else {
            continue;
        };
        collect_php_namespace_member_edge(namespace, child, source, &mut edges);
    }

    edges
}

fn collect_php_namespace_member_edges_in_scope(
    namespace: TsNode<'_>,
    scope: TsNode<'_>,
    source: &str,
    edges: &mut Vec<ManualMemberEdgeSpec>,
) {
    let mut cursor = scope.walk();
    for child in scope.named_children(&mut cursor) {
        collect_php_namespace_member_edge(namespace, child, source, edges);
    }
}

fn collect_php_namespace_member_edge(
    namespace: TsNode<'_>,
    child: TsNode<'_>,
    source: &str,
    edges: &mut Vec<ManualMemberEdgeSpec>,
) {
    if !matches!(child.kind(), "class_declaration" | "interface_declaration") {
        return;
    }
    let Some(source_name) = declaration_name(namespace, source) else {
        return;
    };
    let Some(target_name) = declaration_name(child, source) else {
        return;
    };
    edges.push(ManualMemberEdgeSpec {
        source_name,
        target_name,
        source_span: ts_node_graph_span(namespace),
        target_span: ts_node_graph_span(child),
        line: Some(child.start_position().row as u32 + 1),
    });
}

/// Manual receiver-call edges for one parsed PHP file.
///
/// Was `lib.rs::collect_php_receiver_call_edges`.
pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let root = tree.root_node();
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if !matches!(
            callable.kind(),
            "function_definition" | "method_declaration"
        ) {
            return;
        }
        let Some(source_name) = declaration_name(callable, source) else {
            return;
        };
        let visible_type_names = collect_php_visible_type_binding_names(root, callable, source);
        let imported_type_bindings =
            collect_php_visible_imported_type_bindings(root, callable, source, &visible_type_names);
        let call_source = ManualReceiverSource {
            name: &source_name,
            span: ts_node_graph_span(callable),
        };
        let mut local_receiver_callsites = HashSet::new();
        collect_php_local_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &visible_type_names,
            &imported_type_bindings,
            &mut local_receiver_callsites,
            &mut edges,
        );
        // Foreach element bindings run after the local pass (a prior local
        // binding for the same callsite therefore annotates first, and the
        // loop marker still lands through the engine's order-independent
        // fallback) and before the parameter pass, because inside the loop
        // body the element binding shadows a same-named parameter.
        collect_php_foreach_element_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &visible_type_names,
            &imported_type_bindings,
            &mut local_receiver_callsites,
            &mut edges,
        );
        collect_php_construction_call_specs(
            root,
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &visible_type_names,
            &imported_type_bindings,
            &mut edges,
        );
        let receiver_types = collect_php_parameter_types(callable, source);
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
            false,
            &mut edges,
        );
        let mut parameter_specs = edges.split_off(start);
        parameter_specs
            .retain(|spec| !local_receiver_callsites.contains(&receiver_callsite_key(spec)));
        for spec in &mut parameter_specs {
            if let Some(binding) = imported_type_bindings.get(&spec.owner_name) {
                spec.owner_name = binding.owner_name.clone();
                spec.owner_module = Some(binding.module_name.clone());
            }
        }
        edges.extend(parameter_specs);
    });
    edges
}

fn collect_php_local_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    visible_type_names: &HashSet<String>,
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
        let owner = php_self_receiver_owner(callable, &receiver_name, source)
            .map(Some)
            .or_else(|| {
                php_field_receiver_owner(
                    callable,
                    &receiver_name,
                    source,
                    visible_type_names,
                    imported_type_bindings,
                )
                .map(Some)
            })
            .or_else(|| {
                php_direct_new_owner_surface(
                    &receiver_name,
                    visible_type_names,
                    imported_type_bindings,
                )
                .map(Some)
            })
            .or_else(|| {
                php_visible_local_receiver_owner(
                    callable,
                    node,
                    &receiver_name,
                    source,
                    visible_type_names,
                    imported_type_bindings,
                )
            });
        let Some(owner) = owner else {
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
                binding_marker: None,
                required_callsite_marker: None,
                class_anchored: false,
                owner_is_syntactic: false,
            });
        }
    });
}

/// PHP's scope-visibility primitive: is `call_node` inside the scope that
/// `binding` covers?
///
/// Modeled on `java_scoped_binding_visible_at_call` (java.rs) and
/// `csharp_lexical_scope` (csharp.rs): a `foreach` element binding is visible
/// only within the statement's own body — never in the collection expression
/// and never after the loop ends. PHP's runtime actually leaks the loop
/// variable past the closing brace; the binding deliberately fails closed
/// there, because past the body the variable's value is no longer the proof
/// the binding asserts.
fn php_scoped_binding_visible_at_call(binding: TsNode<'_>, call_node: TsNode<'_>) -> bool {
    if binding.kind() == "foreach_statement" {
        return binding
            .child_by_field_name("body")
            .is_some_and(|body| node_is_same_or_ancestor(body, call_node));
    }
    node_is_same_or_ancestor(binding, call_node)
}

/// Combined foreach-element binding marker for one `foreach` statement:
/// `receiver-binding:loop-element@{start}-{end}`, both 1-based lines of the
/// exact statement span from the parser.
fn php_loop_element_binding_marker(foreach_node: TsNode<'_>) -> String {
    format!(
        "{LOOP_ELEMENT_BINDING_MARKER_PREFIX}{}-{}",
        foreach_node.start_position().row + 1,
        foreach_node.end_position().row + 1
    )
}

/// Receiver-call specs for member calls on foreach element bindings whose
/// collection carries a `@var list<T>` / `@var T[]` PHPDoc annotation.
///
/// Emits one spec per member call on the element variable inside the loop
/// body, carrying the combined loop marker. Claims each callsite in
/// `local_receiver_callsites` so the later parameter pass cannot bind a
/// same-named parameter that the loop element shadows. Fails closed on:
/// no docblock, a docblock naming another variable, an unresolvable or
/// non-bare element type, destructuring bindings, calls outside the loop
/// body, and calls a nested same-named foreach binding shadows.
fn collect_php_foreach_element_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    local_receiver_callsites: &mut HashSet<ReceiverCallSiteKey>,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    walk_tree_nodes(callable, &mut |foreach_node| {
        if foreach_node.kind() != "foreach_statement"
            || !receiver_call_belongs_to_callable(foreach_node, callable)
        {
            return;
        }
        let Some(body) = foreach_node.child_by_field_name("body") else {
            return;
        };
        let Some(element_name) = php_foreach_element_variable(foreach_node, source) else {
            return;
        };
        let Some((owner_name, owner_module)) = php_foreach_element_owner(
            foreach_node,
            callable,
            source,
            visible_type_names,
            imported_type_bindings,
        ) else {
            return;
        };
        let binding_marker = php_loop_element_binding_marker(foreach_node);
        walk_tree_nodes(body, &mut |node| {
            let Some((receiver_name, method_name)) = member_call(node, source) else {
                return;
            };
            if receiver_name != element_name
                || !receiver_call_belongs_to_callable(node, callable)
                || !php_scoped_binding_visible_at_call(foreach_node, node)
            {
                return;
            }
            // Innermost-binder rule: a nested foreach that rebinds the same
            // element name shadows this binding for everything in its body,
            // whether or not the inner collection's element type is known.
            if !php_innermost_foreach_binding(node, &element_name, source)
                .is_some_and(|innermost| same_ts_span(innermost, foreach_node))
            {
                return;
            }
            let method_col = member_call_method_col(node, source, &method_name);
            let line = Some(node.start_position().row as u32 + 1);
            local_receiver_callsites.insert(ReceiverCallSiteKey {
                receiver_name: receiver_name.clone(),
                method_name: method_name.clone(),
                line,
                method_col,
            });
            edges.push(ManualReceiverCallSpec {
                source_name: call_source.name.to_string(),
                source_span: call_source.span,
                receiver_name,
                owner_name: owner_name.clone(),
                owner_module: owner_module.clone(),
                method_name,
                method_col,
                line,
                allow_global_fallback: false,
                binding_marker: Some(binding_marker.clone()),
                required_callsite_marker: None,
                class_anchored: false,
                owner_is_syntactic: false,
            });
        });
    });
}

/// The innermost enclosing foreach statement that binds `element_name` and
/// whose body scope covers `call_node`.
fn php_innermost_foreach_binding<'tree>(
    call_node: TsNode<'tree>,
    element_name: &str,
    source: &str,
) -> Option<TsNode<'tree>> {
    let mut current = call_node.parent();
    while let Some(node) = current {
        if node.kind() == "foreach_statement"
            && php_scoped_binding_visible_at_call(node, call_node)
            && php_foreach_element_variable(node, source).as_deref() == Some(element_name)
        {
            return Some(node);
        }
        current = node.parent();
    }
    None
}

/// Element variable bound by a foreach statement, when it is a plain
/// variable. `tree-sitter-php` names only the `body` field; the collection
/// and the binding are positional children around the anonymous `as`, so the
/// binding is read positionally: the named child after `as` (unwrapping
/// `pair` values and by-reference bindings). Destructuring bindings
/// (`list(...)` / `[...]`) yield `None`.
fn php_foreach_element_variable(foreach_node: TsNode<'_>, source: &str) -> Option<String> {
    let body_id = foreach_node
        .child_by_field_name("body")
        .map(|body| body.id());
    let mut cursor = foreach_node.walk();
    let mut seen_as = false;
    let mut value = None;
    for child in foreach_node.children(&mut cursor) {
        if !child.is_named() {
            if child.kind() == "as" {
                seen_as = true;
            }
            continue;
        }
        if Some(child.id()) == body_id {
            break;
        }
        if seen_as && child.kind() != "comment" {
            value = Some(child);
        }
    }
    let mut value = value?;
    if value.kind() == "pair" {
        let mut pair_cursor = value.walk();
        value = value.named_children(&mut pair_cursor).last()?;
    }
    if value.kind() == "by_ref" {
        let mut by_ref_cursor = value.walk();
        value = value.named_children(&mut by_ref_cursor).next()?;
    }
    if value.kind() != "variable_name" {
        return None;
    }
    normalized_receiver_variable(value, source)
}

/// Collection expression a foreach statement iterates: the first named child
/// before the anonymous `as`.
fn php_foreach_collection_node(foreach_node: TsNode<'_>) -> Option<TsNode<'_>> {
    let mut cursor = foreach_node.walk();
    for child in foreach_node.children(&mut cursor) {
        if !child.is_named() {
            if child.kind() == "as" {
                return None;
            }
            continue;
        }
        if child.kind() == "comment" {
            continue;
        }
        return Some(child);
    }
    None
}

/// Receiver owner for a foreach element, read from the collection's PHPDoc
/// `@var` annotation.
///
/// Two collection shapes are supported, both fail-closed: a plain local
/// variable annotated by a `/** @var list<T> $collection */` docblock
/// immediately preceding the foreach statement, and a `$this->property`
/// collection whose unique property declaration in the enclosing class
/// carries the docblock.
fn php_foreach_element_owner(
    foreach_node: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> OptionalReceiverOwnerBinding {
    let collection = php_foreach_collection_node(foreach_node)?;
    let element_type = match collection.kind() {
        "variable_name" => {
            let collection_name = normalized_receiver_variable(collection, source)?;
            let docblock = php_preceding_docblock(foreach_node, source)?;
            php_docblock_var_element_type(&docblock, Some(&collection_name))?
        }
        "member_access_expression" => {
            let object = collection.child_by_field_name("object")?;
            if normalized_receiver_variable(object, source).as_deref() != Some("this") {
                return None;
            }
            let property_name = trimmed_node_text(collection.child_by_field_name("name")?, source)?;
            php_property_docblock_element_type(callable, &property_name, source)?
        }
        _ => return None,
    };
    php_receiver_owner_from_type(&element_type, visible_type_names, imported_type_bindings)
}

/// The comment node immediately preceding `node`, as source text.
fn php_preceding_docblock(node: TsNode<'_>, source: &str) -> Option<String> {
    let sibling = node.prev_named_sibling()?;
    if sibling.kind() != "comment" {
        return None;
    }
    trimmed_node_text(sibling, source)
}

/// Element type from the docblock of the unique `$property` declaration in
/// the class enclosing `callable`. Ambiguous or missing declarations fail
/// closed.
fn php_property_docblock_element_type(
    callable: TsNode<'_>,
    property_name: &str,
    source: &str,
) -> Option<String> {
    let class_node = enclosing_node_with_kind(callable, &["class_declaration"])?;
    let mut element_types = Vec::new();
    walk_tree_nodes(class_node, &mut |node| {
        if node.kind() != "property_declaration" {
            return;
        }
        if !enclosing_node_with_kind(node, &["class_declaration"])
            .is_some_and(|owner| same_ts_span(owner, class_node))
        {
            return;
        }
        let Some(surface) = trimmed_node_text(node, source) else {
            return;
        };
        let declares_property = surface
            .split('$')
            .skip(1)
            .map(|part| {
                part.chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                    .collect::<String>()
            })
            .any(|name| name == property_name);
        if !declares_property {
            return;
        }
        let Some(docblock) = php_preceding_docblock(node, source) else {
            return;
        };
        if let Some(element_type) = php_docblock_var_element_type(&docblock, None) {
            element_types.push(element_type);
        }
    });
    element_types.sort();
    element_types.dedup();
    if element_types.len() == 1 {
        element_types.pop()
    } else {
        None
    }
}

/// Element type of a `list<T>` / `T[]` PHPDoc `@var` docblock.
///
/// The generic surface is dismantled HERE, not in `normalize_type_surface` —
/// the shared normalizer strips at `<` and would hand back the container
/// name, so only the bare element name ever leaves this reader. Fail-closed
/// on everything else: no `@var`, a mismatched or missing variable name when
/// one is required, unions, intersections, qualified names, nested generics,
/// and any non-bare element surface.
fn php_docblock_var_element_type(
    comment_surface: &str,
    variable_name: Option<&str>,
) -> Option<String> {
    let offset = comment_surface.find("@var")?;
    let after = &comment_surface[offset + "@var".len()..];
    if !after.starts_with(char::is_whitespace) {
        return None;
    }
    let mut tokens = after.split_whitespace();
    let type_surface = tokens.next()?;
    let doc_variable = tokens.next().filter(|token| token.starts_with('$'));
    if let Some(variable_name) = variable_name {
        // An inline `@var` must name the variable it annotates; a docblock
        // naming a different variable never binds this one.
        if doc_variable?.trim_start_matches('$') != variable_name {
            return None;
        }
    }
    let element = if let Some(inner) = type_surface
        .strip_prefix("list<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        inner
    } else {
        type_surface.strip_suffix("[]")?
    };
    let element = element.trim();
    if element.is_empty()
        || !element
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some(element.to_string())
}

/// Construction specs for `new Type(...)` sites: one marker-override spec per
/// plain-named construction, nested constructions included — every
/// `object_creation_expression` node is visited on its own, so a `new` inside
/// another `new`'s (possibly named) argument list shadows nothing.
///
/// The spec is keyed off the AST node, never a sliced text surface: the
/// callee TYPE text comes from the node's `name` child — the same text the
/// rule file names the `php_new` placeholder with, so the annotate pass
/// matches — and multi-line argument lists cannot perturb it. Qualified
/// (`new \A\B()`), dynamic (`new $c()`), and anonymous (`new class {}`)
/// constructions have no `name` child, produce no spec, and fail closed.
fn collect_php_construction_call_specs(
    root: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    walk_tree_nodes(callable, &mut |node| {
        if node.kind() != "object_creation_expression"
            || !receiver_call_belongs_to_callable(node, callable)
        {
            return;
        }
        let mut cursor = node.walk();
        let Some(type_node) = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "name")
        else {
            return;
        };
        let Some(type_text) = trimmed_node_text(type_node, source) else {
            return;
        };
        let Some((owner_name, owner_module)) = php_construction_owner(
            root,
            callable,
            &type_text,
            source,
            visible_type_names,
            imported_type_bindings,
        ) else {
            return;
        };
        edges.push(ManualReceiverCallSpec {
            source_name: call_source.name.to_string(),
            source_span: call_source.span,
            receiver_name: type_text.clone(),
            owner_name,
            owner_module,
            method_name: type_text,
            method_col: Some(type_node.start_position().column as u32 + 1),
            line: Some(node.start_position().row as u32 + 1),
            allow_global_fallback: false,
            binding_marker: None,
            required_callsite_marker: Some(NEW_CALLSITE_MARKER),
            class_anchored: false,
            owner_is_syntactic: false,
        });
    });
}

/// Receiver owner for a constructed type, from the callee TYPE text.
///
/// Resolution order mirrors the member-call tables — same-file visible types,
/// then `use` imports — extended by one construction-specific arm: PHP
/// resolves an unqualified class name against the file's own namespace before
/// anything outside it, so a bare name the tables do not know binds to
/// `{namespace}.{Type}` when the callable sits inside a `namespace`
/// declaration. The annotation only records that namespace-derived module;
/// whether such a class exists is the resolution pass's question, and its
/// exact qualified-name filter fails a same-namespace miss closed. Outside
/// any namespace declaration an unknown bare name yields no owner at all.
fn php_construction_owner(
    root: TsNode<'_>,
    callable: TsNode<'_>,
    type_text: &str,
    source: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> OptionalReceiverOwnerBinding {
    if let Some(owner) =
        php_receiver_owner_from_type(type_text, visible_type_names, imported_type_bindings)
    {
        return Some(owner);
    }
    let namespace_name = php_enclosing_namespace_name(root, callable, source)?;
    Some((
        type_text.to_string(),
        Some(format!("{namespace_name}.{type_text}")),
    ))
}

/// Name of the namespace declaration governing `callable`: the enclosing
/// bracketed `namespace X { ... }`, or the last unbracketed `namespace X;`
/// above it. `None` when the callable sits in the global namespace.
fn php_enclosing_namespace_name(
    root: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
) -> Option<String> {
    if let Some(namespace) = enclosing_node_with_kind(callable, &["namespace_definition"])
        && namespace.child_by_field_name("body").is_some()
    {
        return declaration_name(namespace, source);
    }
    let mut governing = None;
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "namespace_definition" || child.child_by_field_name("body").is_some() {
            continue;
        }
        if child.start_byte() <= callable.start_byte() {
            governing = Some(child);
        }
    }
    declaration_name(governing?, source)
}

fn php_self_receiver_owner(
    callable: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> OptionalReceiverOwnerBinding {
    if receiver_name != "this" {
        return None;
    }
    let owner_node = enclosing_node_with_kind(
        callable,
        &[
            "class_declaration",
            "interface_declaration",
            "trait_declaration",
        ],
    )?;
    let owner_name = declaration_name(owner_node, source)?;
    Some((owner_name, None))
}

fn php_field_receiver_owner(
    callable: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> OptionalReceiverOwnerBinding {
    let field_name = receiver_name.strip_prefix("this->")?.trim();
    let class_node = enclosing_node_with_kind(callable, &["class_declaration"])?;
    let mut field_bindings = Vec::new();
    walk_tree_nodes(class_node, &mut |node| {
        if !matches!(
            node.kind(),
            "property_declaration" | "property_promotion_parameter"
        ) {
            return;
        }
        if !enclosing_node_with_kind(node, &["class_declaration"])
            .is_some_and(|owner| same_ts_span(owner, class_node))
        {
            return;
        }
        let Some(surface) = trimmed_node_text(node, source) else {
            return;
        };
        for (binding_name, owner) in
            php_typed_member_bindings_surface(&surface, visible_type_names, imported_type_bindings)
        {
            if binding_name == field_name {
                field_bindings.push(owner);
            }
        }
    });
    field_bindings.sort();
    field_bindings.dedup();
    if field_bindings.len() == 1 {
        Some(field_bindings.remove(0))
    } else {
        None
    }
}

fn php_typed_member_bindings_surface(
    surface: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> Vec<(String, ReceiverOwnerBinding)> {
    let surface = surface
        .split('=')
        .next()
        .unwrap_or(surface)
        .trim()
        .trim_end_matches([';', ','])
        .trim();
    let Some((type_side, _)) = surface.split_once('$') else {
        return Vec::new();
    };
    let Some(raw_type) = type_side
        .split_whitespace()
        .last()
        .filter(|token| !php_member_modifier_token(token))
    else {
        return Vec::new();
    };
    let Some(owner) =
        php_receiver_owner_from_type(raw_type, visible_type_names, imported_type_bindings)
    else {
        return Vec::new();
    };

    surface
        .split('$')
        .skip(1)
        .filter_map(|part| {
            let name = part
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .collect::<String>();
            normalize_parameter_name(&name).map(|name| (name, owner.clone()))
        })
        .collect()
}

fn php_member_modifier_token(token: &str) -> bool {
    matches!(
        token,
        "public" | "protected" | "private" | "readonly" | "static" | "var" | "final" | "abstract"
    )
}

fn php_visible_local_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> Option<OptionalReceiverOwnerBinding> {
    let mut visible_bindings = Vec::new();
    walk_tree_nodes(callable, &mut |node| {
        if node.kind() != "assignment_expression" {
            return;
        }
        if !receiver_call_belongs_to_callable(node, callable)
            || node.end_byte() > call_node.start_byte()
        {
            return;
        }
        let Some(left_node) = node.child_by_field_name("left") else {
            return;
        };
        if normalized_receiver_variable(left_node, source).as_deref() != Some(receiver_name) {
            return;
        }
        let owner = node.child_by_field_name("right").and_then(|right_node| {
            php_direct_new_owner(
                right_node,
                source,
                visible_type_names,
                imported_type_bindings,
            )
        });
        visible_bindings.push((node.end_byte(), owner));
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner)| owner)
}

fn php_direct_new_owner(
    node: TsNode<'_>,
    source: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> OptionalReceiverOwnerBinding {
    trimmed_node_text(node, source)
        .as_deref()
        .and_then(|surface| {
            php_direct_new_owner_surface(surface, visible_type_names, imported_type_bindings)
        })
}

fn php_direct_new_owner_surface(
    surface: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> OptionalReceiverOwnerBinding {
    let surface = surface
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    let rest = surface.strip_prefix("new ")?;
    let type_surface = rest.split(['(', '{']).next().unwrap_or(rest).trim();
    if type_surface.contains('\\') {
        return None;
    }
    php_receiver_owner_from_type(type_surface, visible_type_names, imported_type_bindings)
}

fn php_receiver_owner_from_type(
    raw_type: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> OptionalReceiverOwnerBinding {
    let raw_type = raw_type.trim().trim_start_matches('?').trim();
    if raw_type.contains('\\') || raw_type.contains('|') || raw_type.contains('&') {
        return None;
    }
    let owner_name = normalize_type_surface(raw_type)?;
    if visible_type_names.contains(&owner_name) {
        return Some((owner_name, None));
    }
    if let Some(binding) = imported_type_bindings.get(&owner_name) {
        return Some((
            binding.owner_name.clone(),
            Some(binding.module_name.clone()),
        ));
    }
    None
}

fn collect_php_visible_imported_type_bindings(
    root: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
    visible_type_names: &HashSet<String>,
) -> HashMap<String, ImportedTypeBinding> {
    let mut bindings = HashMap::new();
    let mut duplicates = HashSet::new();

    if let Some(namespace) = enclosing_node_with_kind(callable, &["namespace_definition"])
        && let Some(body) = namespace.child_by_field_name("body")
    {
        collect_php_imported_type_bindings_in_scope(
            body,
            source,
            visible_type_names,
            &mut bindings,
            &mut duplicates,
        );
    } else {
        let (start_byte, end_byte) = php_unbracketed_namespace_segment(root, callable);
        collect_php_imported_type_bindings_in_root_segment(
            root,
            source,
            visible_type_names,
            &mut bindings,
            &mut duplicates,
            start_byte,
            end_byte,
        );
    }

    bindings
}

fn collect_php_imported_type_bindings_in_scope(
    scope: TsNode<'_>,
    source: &str,
    visible_type_names: &HashSet<String>,
    bindings: &mut HashMap<String, ImportedTypeBinding>,
    duplicates: &mut HashSet<String>,
) {
    let mut cursor = scope.walk();
    for statement in scope.named_children(&mut cursor) {
        if statement.kind() != "namespace_use_declaration" {
            continue;
        }
        let Some(statement_surface) = trimmed_node_text(statement, source) else {
            continue;
        };
        for (owner_name, local_name, module_name) in
            php_import_type_binding_names(&statement_surface)
        {
            if visible_type_names.contains(&local_name) || duplicates.contains(&local_name) {
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
                    module_name,
                    owner_name,
                },
            );
        }
    }
}

fn collect_php_imported_type_bindings_in_root_segment(
    root: TsNode<'_>,
    source: &str,
    visible_type_names: &HashSet<String>,
    bindings: &mut HashMap<String, ImportedTypeBinding>,
    duplicates: &mut HashSet<String>,
    start_byte: usize,
    end_byte: usize,
) {
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        if statement.start_byte() < start_byte || statement.start_byte() >= end_byte {
            continue;
        }
        if statement.kind() != "namespace_use_declaration" {
            continue;
        }
        let Some(statement_surface) = trimmed_node_text(statement, source) else {
            continue;
        };
        for (owner_name, local_name, module_name) in
            php_import_type_binding_names(&statement_surface)
        {
            if visible_type_names.contains(&local_name) || duplicates.contains(&local_name) {
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
                    module_name,
                    owner_name,
                },
            );
        }
    }
}

fn collect_php_visible_type_binding_names(
    root: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Some(namespace) = enclosing_node_with_kind(callable, &["namespace_definition"])
        && let Some(body) = namespace.child_by_field_name("body")
    {
        collect_php_type_binding_names_in_scope(body, source, &mut names);
    } else {
        let (start_byte, end_byte) = php_unbracketed_namespace_segment(root, callable);
        collect_php_type_binding_names_in_root_segment(
            root, source, &mut names, start_byte, end_byte,
        );
    }
    names
}

fn collect_php_type_binding_names_in_scope(
    scope: TsNode<'_>,
    source: &str,
    names: &mut HashSet<String>,
) {
    let mut cursor = scope.walk();
    for child in scope.named_children(&mut cursor) {
        if matches!(child.kind(), "class_declaration" | "interface_declaration")
            && let Some(name) = declaration_name(child, source)
        {
            names.insert(name);
        }
    }
}

fn collect_php_type_binding_names_in_root_segment(
    root: TsNode<'_>,
    source: &str,
    names: &mut HashSet<String>,
    start_byte: usize,
    end_byte: usize,
) {
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.start_byte() < start_byte || child.start_byte() >= end_byte {
            continue;
        }
        if matches!(child.kind(), "class_declaration" | "interface_declaration")
            && let Some(name) = declaration_name(child, source)
        {
            names.insert(name);
        }
    }
}

fn php_unbracketed_namespace_segment(root: TsNode<'_>, node: TsNode<'_>) -> (usize, usize) {
    let mut start_byte = root.start_byte();
    let mut end_byte = root.end_byte();
    let node_start = node.start_byte();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "namespace_definition" || child.child_by_field_name("body").is_some() {
            continue;
        }
        if node_start < child.start_byte() {
            end_byte = child.start_byte();
            break;
        }
        start_byte = child.end_byte();
        end_byte = root.end_byte();
    }
    (start_byte, end_byte)
}

fn php_import_type_binding_names(statement: &str) -> Vec<(String, String, String)> {
    let Some(rest) = statement.strip_prefix("use") else {
        return Vec::new();
    };
    let rest = rest.trim().trim_end_matches(';').trim();
    if starts_with_case_insensitive_keyword(rest, "function")
        || starts_with_case_insensitive_keyword(rest, "const")
        || rest.contains('{')
    {
        return Vec::new();
    }

    split_top_level_parameters(rest)
        .into_iter()
        .filter_map(|part| php_import_type_binding_name(&part))
        .collect()
}

fn php_import_type_binding_name(import_surface: &str) -> Option<(String, String, String)> {
    let import_surface = import_surface.trim();
    if starts_with_case_insensitive_keyword(import_surface, "function")
        || starts_with_case_insensitive_keyword(import_surface, "const")
    {
        return None;
    }
    let (module_surface, alias_surface) =
        split_case_insensitive_alias(import_surface, "as").unwrap_or((import_surface, ""));
    let (owner_name, module_name) = php_imported_owner_module_name(module_surface)?;
    let local_name = if alias_surface.trim().is_empty() {
        owner_name.clone()
    } else {
        normalize_parameter_name(alias_surface)?
    };
    Some((owner_name, local_name, module_name))
}

fn split_case_insensitive_alias<'a>(surface: &'a str, keyword: &str) -> Option<(&'a str, &'a str)> {
    let mut tokens = surface.split_whitespace();
    let module_surface = tokens.next()?;
    let separator = tokens.next()?;
    let alias_surface = tokens.next()?;
    if tokens.next().is_some() || !separator.eq_ignore_ascii_case(keyword) {
        return None;
    }
    Some((module_surface, alias_surface))
}

fn starts_with_case_insensitive_keyword(surface: &str, keyword: &str) -> bool {
    let mut parts = surface.split_whitespace();
    parts
        .next()
        .is_some_and(|token| token.eq_ignore_ascii_case(keyword))
}

fn php_imported_owner_module_name(raw_module: &str) -> Option<(String, String)> {
    let module = raw_module.trim().trim_start_matches('\\').trim();
    if module.is_empty()
        || module.contains('*')
        || module.contains('|')
        || module.split_whitespace().count() != 1
    {
        return None;
    }
    let (namespace, owner_name) = module.rsplit_once('\\')?;
    let owner_name = normalize_parameter_name(owner_name)?;
    if namespace.trim().is_empty() {
        return None;
    }
    Some((owner_name.clone(), format!("{namespace}.{owner_name}")))
}

fn collect_php_parameter_types(callable: TsNode<'_>, source: &str) -> HashMap<String, String> {
    let mut receiver_types = HashMap::new();
    let Some(parameters) = callable.child_by_field_name("parameters") else {
        return receiver_types;
    };
    walk_tree_nodes(parameters, &mut |node| {
        if !matches!(
            node.kind(),
            "simple_parameter" | "variadic_parameter" | "property_promotion_parameter"
        ) {
            return;
        }
        let Some(type_node) = node.child_by_field_name("type") else {
            return;
        };
        let Some(raw_type) = trimmed_node_text(type_node, source) else {
            return;
        };
        let Some(owner_name) = normalize_type_surface(&raw_type) else {
            return;
        };
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        if let Some(name) = normalized_receiver_variable(name_node, source) {
            receiver_types.insert(name, owner_name);
        }
    });
    receiver_types
}

/// Receiver and member of one PHP member call, read from the grammar.
///
/// Was `lib.rs::php_member_call`.
fn member_call(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    if !matches!(
        node.kind(),
        "member_call_expression" | "nullsafe_member_call_expression"
    ) {
        return None;
    }
    let receiver = node.child_by_field_name("object")?;
    let method = node.child_by_field_name("name")?;
    Some((
        normalized_receiver_variable(receiver, source)?,
        trimmed_node_text(method, source)?,
    ))
}
