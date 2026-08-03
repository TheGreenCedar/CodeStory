//! Kotlin extraction rules.
//!
//! Kotlin's graph extraction lives here: the tree-sitter rule file, the
//! compiled-rule cache, the member-call callsite marker, and the receiver-call
//! resolution engine that turns `repo.save(...)` into an edge aimed at
//! `Repository.save`. Every language-keyed dispatch in the crate reaches it
//! through [`super::EXTRACTIONS`] rather than by spelling `"kotlin"`.
//!
//! Two Kotlin surfaces are deliberately *not* here yet, and both are shared
//! seams rather than Kotlin content:
//!
//! * `lib.rs::collect_ktor_route` and its `"kotlin"` arm in the framework-route
//!   scanner. The per-language route collectors take non-uniform arguments and
//!   a per-framework `has_<framework>` precondition, so routing them through
//!   the registry is one change for all sixteen languages, not part of Kotlin's
//!   rollback unit.
//! * `LanguageRuleset::Kotlin`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both Kotlin fixtures so the move stays output-equal.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, ImportedTypeBinding, LanguageRuleset, ManualReceiverCallSpec,
    ManualReceiverSource, OptionalReceiverOwnerBinding, ReceiverCallSiteKey,
    callable_parameter_list_node, collect_colon_parameter_types,
    collect_receiver_call_specs_in_callable, declaration_name, enclosing_node_with_kind,
    member_call_method_col, node_is_same_or_ancestor, normalize_parameter_name,
    normalize_type_surface, parameter_name_before_colon, parameter_type_after_colon,
    receiver_call_belongs_to_callable, receiver_callsite_key, same_ts_span,
    split_top_level_parameters, trimmed_node_text, ts_node_graph_span, walk_tree_nodes,
};

/// Callsite marker written onto edges produced from Kotlin member-call syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:kotlin-member-call";

const GRAPH_QUERY: &str = include_str!("../../rules/kotlin.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for Kotlin.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["kotlin"],
    language_name: "kotlin",
    extensions: &["kt", "kts"],
    ruleset: LanguageRuleset::Kotlin,
    parser_language: kotlin_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    // Kotlin's MEMBER edges come from `rules/kotlin.scm`; it never had an arm
    // in `lib.rs::language_member_specs`.
    member_edge_specs: None,
    receiver_call_specs: Some(receiver_call_specs),
    member_callsite_marker: Some(MEMBER_CALLSITE_MARKER),
    graph_call_syntax: Some("kotlin_member"),
    // A `function_declaration` whose owner is type-like is a method in Kotlin;
    // the projection promotes FUNCTION to METHOD for exactly these languages.
    promotes_type_member_functions_to_methods: true,
    qualified_name_delimiter: ".",
    route_comments_are_c_style: true,
    uses_generic_semantic_resolver: true,
    semantic_family: "kotlin",
};

fn kotlin_language() -> tree_sitter::Language {
    tree_sitter_kotlin_ng::LANGUAGE.into()
}

/// Manual receiver-call edges for one parsed Kotlin file.
///
/// Was `lib.rs::collect_kotlin_receiver_call_edges`.
pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let root = tree.root_node();
    let top_level_type_names = collect_kotlin_top_level_type_binding_names(root, source);
    let has_wildcard_import = has_kotlin_wildcard_import(root, source);
    let imported_type_bindings =
        collect_kotlin_imported_type_bindings(root, source, &top_level_type_names);
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if callable.kind() != "function_declaration" {
            return;
        }
        let Some(source_name) = declaration_name(callable, source) else {
            return;
        };
        let source_span = ts_node_graph_span(callable);
        let parameter_receiver_types = collect_colon_parameter_types(callable, source);
        let mut local_receiver_callsites = HashSet::new();
        collect_kotlin_precise_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: &source_name,
                span: source_span,
            },
            KotlinReceiverContext {
                parameter_receiver_types: &parameter_receiver_types,
                imported_type_bindings: &imported_type_bindings,
                top_level_type_names: &top_level_type_names,
                has_wildcard_import,
            },
            &mut local_receiver_callsites,
            &mut edges,
        );
        if !parameter_receiver_types.is_empty() {
            let start = edges.len();
            collect_receiver_call_specs_in_callable(
                callable,
                source,
                ManualReceiverSource {
                    name: &source_name,
                    span: source_span,
                },
                &parameter_receiver_types,
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
                } else if has_wildcard_import && !top_level_type_names.contains(&spec.owner_name) {
                    spec.owner_module = Some("*".to_string());
                }
            }
            edges.extend(parameter_specs);
        }
    });
    edges
}

struct KotlinReceiverContext<'a> {
    parameter_receiver_types: &'a HashMap<String, String>,
    imported_type_bindings: &'a HashMap<String, ImportedTypeBinding>,
    top_level_type_names: &'a HashSet<String>,
    has_wildcard_import: bool,
}

fn collect_kotlin_precise_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    context: KotlinReceiverContext<'_>,
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
        let method_col = member_call_method_col(node, source, &method_name);
        let callsite_key = ReceiverCallSiteKey {
            receiver_name: receiver_name.clone(),
            method_name: method_name.clone(),
            line: Some(node.start_position().row as u32 + 1),
            method_col,
        };

        if let Some(owner) =
            kotlin_visible_local_receiver_owner(callable, node, &receiver_name, source, &context)
        {
            local_receiver_callsites.insert(callsite_key);
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
            return;
        }

        let owner =
            if let Some(owner) = kotlin_self_receiver_owner(callable, &receiver_name, source) {
                Some(owner)
            } else if !context
                .parameter_receiver_types
                .contains_key(&receiver_name)
            {
                kotlin_property_receiver_owner(callable, &receiver_name, source, &context)
            } else {
                None
            };
        let Some((owner_name, owner_module)) = owner else {
            return;
        };
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
    });
}

fn kotlin_self_receiver_owner(
    callable: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> OptionalReceiverOwnerBinding {
    if receiver_name != "this" {
        return None;
    }
    let owner_node =
        enclosing_node_with_kind(callable, &["class_declaration", "object_declaration"])?;
    let owner_name = declaration_name(owner_node, source)?;
    Some((owner_name, None))
}

fn kotlin_visible_local_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    context: &KotlinReceiverContext<'_>,
) -> Option<OptionalReceiverOwnerBinding> {
    let mut visible_bindings = Vec::new();
    walk_tree_nodes(callable, &mut |node| {
        if !kotlin_local_declaration_candidate(node) {
            return;
        }
        if !receiver_call_belongs_to_callable(node, callable)
            || node.end_byte() > call_node.start_byte()
        {
            return;
        }
        let Some(surface) = trimmed_node_text(node, source) else {
            return;
        };
        let Some((binding_name, owner)) = kotlin_local_receiver_binding(&surface, context) else {
            return;
        };
        if binding_name != receiver_name || !kotlin_local_binding_visible_at_call(node, call_node) {
            return;
        }
        visible_bindings.push((node.end_byte(), owner));
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner)| owner)
}

fn kotlin_property_receiver_owner(
    callable: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    context: &KotlinReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    let field_name = receiver_name
        .strip_prefix("this.")
        .unwrap_or(receiver_name)
        .trim();
    if field_name == "this" || field_name.contains('.') {
        return None;
    }
    let owner_node =
        enclosing_node_with_kind(callable, &["class_declaration", "object_declaration"])?;
    let mut property_bindings = Vec::new();
    for (binding_name, owner) in
        kotlin_primary_constructor_property_bindings(owner_node, source, context)
    {
        if binding_name == field_name
            && let Some(owner) = owner
        {
            property_bindings.push(owner);
        }
    }
    walk_tree_nodes(owner_node, &mut |node| {
        if node.kind() != "property_declaration"
            || !kotlin_property_belongs_to_owner(node, owner_node)
        {
            return;
        }
        for (binding_name, owner) in
            kotlin_property_declaration_receiver_bindings(node, source, context)
        {
            if binding_name == field_name
                && let Some(owner) = owner
            {
                property_bindings.push(owner);
            }
        }
    });
    property_bindings.sort();
    property_bindings.dedup();
    if property_bindings.len() == 1 {
        Some(property_bindings.remove(0))
    } else {
        None
    }
}

fn kotlin_property_belongs_to_owner(property: TsNode<'_>, owner_node: TsNode<'_>) -> bool {
    let mut current = property.parent();
    while let Some(candidate) = current {
        if same_ts_span(candidate, owner_node) {
            return true;
        }
        if candidate.kind() == "function_declaration"
            || matches!(candidate.kind(), "class_declaration" | "object_declaration")
        {
            return false;
        }
        current = candidate.parent();
    }
    false
}

/// Property bindings declared in a Kotlin class's primary constructor.
///
/// The surface used to be the class text up to its first `{`, scanned from the
/// first `(`. An annotated declaration puts that `(` inside the annotation:
/// `@Table(name = "users") data class User(val id: Long)` yielded `name =
/// "users"` and the class lost every primary-constructor binding (CR-009). The
/// grammar keeps `primary_constructor` as its own child, after `modifiers`.
fn kotlin_primary_constructor_property_bindings(
    owner_node: TsNode<'_>,
    source: &str,
    context: &KotlinReceiverContext<'_>,
) -> Vec<(String, OptionalReceiverOwnerBinding)> {
    let mut cursor = owner_node.walk();
    let parameters = owner_node
        .named_children(&mut cursor)
        .find(|child| child.kind() == "primary_constructor")
        .and_then(callable_parameter_list_node)
        .and_then(|node| trimmed_node_text(node, source))
        .map(|text| {
            text.strip_prefix('(')
                .and_then(|inner| inner.strip_suffix(')'))
                .unwrap_or(text.as_str())
                .to_string()
        })
        .or_else(|| {
            let owner_surface = trimmed_node_text(owner_node, source)?;
            let head = owner_surface
                .split('{')
                .next()
                .unwrap_or(owner_surface.as_str());
            signature_parameter_surface_text(head)
        });
    let Some(parameters) = parameters else {
        return Vec::new();
    };
    split_top_level_parameters(&parameters)
        .into_iter()
        .filter_map(|parameter| {
            let (name_side, type_side) = parameter.split_once(':')?;
            if !kotlin_property_parameter_name_side(name_side) {
                return None;
            }
            let binding_name = parameter_name_before_colon(name_side)?;
            let owner =
                kotlin_receiver_owner_from_type(&parameter_type_after_colon(type_side), context);
            Some((binding_name, owner))
        })
        .collect()
}

fn signature_parameter_surface_text(text: &str) -> Option<String> {
    let start = text.find('(')?;
    let mut depth = 0usize;
    let mut parameter_start = None;
    for (index, ch) in text.char_indices().skip_while(|(index, _)| *index < start) {
        match ch {
            '(' => {
                depth = depth.saturating_add(1);
                if depth == 1 {
                    parameter_start = Some(index + ch.len_utf8());
                }
            }
            ')' => {
                if depth == 1 {
                    let parameter_start = parameter_start?;
                    return Some(text[parameter_start..index].to_string());
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    None
}

fn kotlin_property_parameter_name_side(name_side: &str) -> bool {
    name_side
        .split_whitespace()
        .any(|token| matches!(token, "val" | "var"))
}

fn kotlin_property_declaration_receiver_bindings(
    node: TsNode<'_>,
    source: &str,
    context: &KotlinReceiverContext<'_>,
) -> Vec<(String, OptionalReceiverOwnerBinding)> {
    trimmed_node_text(node, source)
        .as_deref()
        .and_then(|surface| kotlin_typed_property_binding(surface, context))
        .into_iter()
        .collect()
}

fn kotlin_local_declaration_candidate(node: TsNode<'_>) -> bool {
    matches!(
        node.kind(),
        "property_declaration" | "variable_declaration" | "local_declaration"
    )
}

fn kotlin_local_binding_visible_at_call(binding: TsNode<'_>, call_node: TsNode<'_>) -> bool {
    let Some(binding_scope) = kotlin_lexical_scope(binding) else {
        return false;
    };
    let Some(call_scope) = kotlin_lexical_scope(call_node) else {
        return false;
    };
    node_is_same_or_ancestor(binding_scope, call_scope)
}

fn kotlin_lexical_scope(node: TsNode<'_>) -> Option<TsNode<'_>> {
    enclosing_node_with_kind(node, &["block", "function_body"])
}

fn kotlin_local_receiver_binding(
    surface: &str,
    context: &KotlinReceiverContext<'_>,
) -> Option<(String, OptionalReceiverOwnerBinding)> {
    let surface = surface.trim().trim_end_matches(';').trim();
    let rest = surface
        .strip_prefix("val ")
        .or_else(|| surface.strip_prefix("var "))?
        .trim();
    let (binding_surface, value_surface) = rest.split_once('=')?;
    let binding_name = binding_surface
        .split(':')
        .next()
        .unwrap_or(binding_surface)
        .split_whitespace()
        .next()
        .and_then(normalize_parameter_name)?;
    let constructor_surface = value_surface.trim();
    let Some((constructor_name, _)) = constructor_surface.split_once('(') else {
        return Some((binding_name, None));
    };
    let constructor_name = constructor_name.trim();
    if constructor_name.contains('.') || constructor_name.contains("::") {
        return Some((binding_name, None));
    }
    let Some(owner_name) = normalize_type_surface(constructor_name) else {
        return Some((binding_name, None));
    };
    if !owner_name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
    {
        return Some((binding_name, None));
    }
    let owner = kotlin_receiver_owner_from_constructor_name(&owner_name, context);
    Some((binding_name, owner))
}

fn kotlin_receiver_owner_from_constructor_name(
    constructor_name: &str,
    context: &KotlinReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    if context.top_level_type_names.contains(constructor_name) {
        return Some((constructor_name.to_string(), None));
    }
    if let Some(binding) = context.imported_type_bindings.get(constructor_name) {
        return Some((
            binding.owner_name.clone(),
            Some(binding.module_name.clone()),
        ));
    }
    Some((constructor_name.to_string(), None))
}

fn kotlin_typed_property_binding(
    surface: &str,
    context: &KotlinReceiverContext<'_>,
) -> Option<(String, OptionalReceiverOwnerBinding)> {
    let surface = surface
        .split('=')
        .next()
        .unwrap_or(surface)
        .trim()
        .trim_end_matches(';')
        .trim();
    let rest = surface
        .rsplit_once(" val ")
        .map(|(_, rest)| rest)
        .or_else(|| surface.rsplit_once(" var ").map(|(_, rest)| rest))
        .or_else(|| surface.strip_prefix("val "))
        .or_else(|| surface.strip_prefix("var "))?
        .trim();
    let (name_side, type_side) = rest.split_once(':')?;
    let binding_name = parameter_name_before_colon(name_side)?;
    let owner = kotlin_receiver_owner_from_type(&parameter_type_after_colon(type_side), context);
    Some((binding_name, owner))
}

fn kotlin_receiver_owner_from_type(
    raw_type: &str,
    context: &KotlinReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    let owner_name = normalize_type_surface(raw_type)?;
    if context.top_level_type_names.contains(&owner_name) {
        return Some((owner_name, None));
    }
    if let Some(binding) = context.imported_type_bindings.get(&owner_name) {
        return Some((
            binding.owner_name.clone(),
            Some(binding.module_name.clone()),
        ));
    }
    if context.has_wildcard_import {
        return Some((owner_name, Some("*".to_string())));
    }
    Some((owner_name, None))
}

fn collect_kotlin_imported_type_bindings(
    root: TsNode<'_>,
    source: &str,
    top_level_bindings: &HashSet<String>,
) -> HashMap<String, ImportedTypeBinding> {
    let mut bindings = HashMap::new();
    let mut duplicates = HashSet::new();
    let mut cursor = root.walk();

    for statement in root.named_children(&mut cursor) {
        if statement.kind() != "import" {
            continue;
        }
        let Some((owner_name, local_name, module_name)) =
            kotlin_import_type_binding_names(statement, source)
        else {
            continue;
        };
        if top_level_bindings.contains(&local_name) || duplicates.contains(&local_name) {
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

    bindings
}

fn has_kotlin_wildcard_import(root: TsNode<'_>, source: &str) -> bool {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .filter(|statement| statement.kind() == "import")
        .filter_map(|statement| trimmed_node_text(statement, source))
        .any(|surface| {
            surface
                .strip_prefix("import")
                .map(|rest| rest.trim().trim_end_matches(';').trim().ends_with(".*"))
                .unwrap_or(false)
        })
}

fn collect_kotlin_top_level_type_binding_names(root: TsNode<'_>, source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_declaration" | "object_declaration" | "type_alias"
        ) && let Some(name) = declaration_name(child, source)
        {
            names.insert(name);
        }
    }
    names
}

fn kotlin_import_type_binding_names(
    statement: TsNode<'_>,
    source: &str,
) -> Option<(String, String, String)> {
    let surface = trimmed_node_text(statement, source)?;
    let rest = surface
        .strip_prefix("import")?
        .trim()
        .trim_end_matches(';')
        .trim();
    if rest.is_empty() || rest.ends_with(".*") || rest.contains('*') {
        return None;
    }

    let (module_surface, alias_surface) = rest
        .rsplit_once(" as ")
        .map(|(module, alias)| (module.trim(), Some(alias.trim())))
        .unwrap_or((rest, None));
    if !module_surface.contains('.') || module_surface.split_whitespace().count() != 1 {
        return None;
    }
    let owner_name = module_surface
        .rsplit('.')
        .next()
        .and_then(normalize_parameter_name)?;
    let local_name = alias_surface
        .and_then(normalize_parameter_name)
        .unwrap_or_else(|| owner_name.clone());
    Some((owner_name, local_name, module_surface.to_string()))
}
/// Receiver and member of one Kotlin member call, read from the grammar.
///
/// The text scan this replaces cut the callee at the first `(` and then split
/// on the last `.` of what remained, which is wrong for the two most idiomatic
/// Kotlin call shapes (CR-010). A trailing lambda has no parentheses at all, so
/// `user.apply { this.name = n }` split on the dot inside the lambda body and
/// produced the receiver `user.apply { this`. A chain such as
/// `repo.findAll().filter { … }` truncated at the first `(` and reported the
/// outer `.filter` call as `repo.findAll`, duplicating the inner call and
/// losing the outer one. `call_expression` keeps its callee as its first child
/// and a member callee is a `navigation_expression`, so both shapes read
/// directly off the tree.
fn member_call(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    if node.kind() != "call_expression" {
        return None;
    }
    let mut cursor = node.walk();
    let callee = node.named_children(&mut cursor).next()?;
    if callee.kind() != "navigation_expression" {
        return None;
    }
    let mut callee_cursor = callee.walk();
    let callee_children = callee
        .named_children(&mut callee_cursor)
        .collect::<Vec<_>>();
    let receiver = callee_children.first()?;
    let member = callee_children.last()?;
    if receiver.id() == member.id() {
        return None;
    }
    let receiver_text = trimmed_node_text(*receiver, source)?;
    let member_text = trimmed_node_text(*member, source)?;
    Some((
        normalized_kotlin_receiver_surface(receiver_text.trim_end_matches('?'))?,
        normalize_parameter_name(member_text.trim_start_matches('?'))?,
    ))
}

fn normalized_kotlin_receiver_surface(raw: &str) -> Option<String> {
    let receiver = raw.trim().trim_end_matches('?').trim();
    if receiver.contains('.') {
        let cleaned = receiver
            .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '.')
            .trim();
        let valid = cleaned
            .split('.')
            .all(|part| normalize_parameter_name(part).is_some());
        return (valid && !cleaned.is_empty()).then(|| cleaned.to_string());
    }
    normalize_parameter_name(receiver)
}
