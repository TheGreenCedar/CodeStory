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
    member_call_method_col, normalize_parameter_name, normalize_type_surface,
    normalized_receiver_variable, receiver_call_belongs_to_callable, receiver_callsite_key,
    same_ts_span, split_top_level_parameters, trimmed_node_text, ts_node_graph_span,
    walk_tree_nodes,
};

/// Callsite marker written onto edges produced from PHP member-call syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:php-member-call";

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
    member_callsite_marker: Some(MEMBER_CALLSITE_MARKER),
    graph_call_syntax: Some("php_member"),
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
            });
        }
    });
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
