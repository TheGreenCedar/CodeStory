//! Dart extraction rules.
//!
//! Dart's graph extraction lives here: the tree-sitter rule file, the
//! compiled-rule cache, the member-call callsite marker, the receiver-call
//! resolution engine that turns `repo.save(...)` into an edge aimed at
//! `Repository.save`, and the direct-call collector that recovers plain
//! `helper()` callsites the rule file leaves as generic placeholders. Every
//! language-keyed dispatch in the crate reaches them through
//! [`super::EXTRACTIONS`] rather than by spelling `"dart"`.
//!
//! Three Dart surfaces are deliberately *not* here, and all three are shared
//! seams rather than Dart content:
//!
//! * `lib.rs::language_precise_call_specs`, whose only row today is Dart's
//!   [`direct_call_specs`]. [`super::LanguageExtraction`] has no field for a
//!   precise-call collector because Dart is the only language with one; giving
//!   the registry a sixteenth field is a change for all sixteen packages, not
//!   part of Dart's rollback unit. The residual arm calls into this module.
//! * `lib.rs::append_manual_receiver_call_edges`'s `language_name == "dart"`
//!   branch, which *replaces* an annotated placeholder edge instead of
//!   suppressing the manual one. That is edge-assembly policy shared by every
//!   language's placeholder handling, not an extraction rule.
//! * `resolution::mod`'s `Some("dart")` import-owner lookup, which is
//!   resolution-side and still spells `"kotlin"` for the migrated language too.
//!
//! `LanguageRuleset::Dart` also stays in `lib.rs`, because the enum is the
//! compiled-rule cache key for every language at once.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both Dart fixtures so the move stays output-equal.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, LanguageRuleset, ManualPreciseCallSpec, ManualReceiverCallSpec,
    ManualReceiverSource, OptionalReceiverOwnerBinding, ReceiverCallSiteKey,
    collect_prefix_parameter_types, collect_receiver_call_specs_in_callable, declaration_name,
    descendant_by_field_name, enclosing_node_with_kind, first_descendant_with_kind,
    member_call_method_col, node_is_same_or_ancestor, normalize_parameter_name,
    normalize_type_surface, previous_named_sibling_with_kind, receiver_call_belongs_to_callable,
    receiver_callsite_key, same_ts_span, signature_parameter_surface, split_top_level_parameters,
    trimmed_node_text, ts_node_graph_span, walk_tree_nodes,
};

/// Callsite marker written onto edges produced from Dart member-call syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:dart-member-call";

const GRAPH_QUERY: &str = include_str!("../../rules/dart.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for Dart.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["dart"],
    language_name: "dart",
    extensions: &["dart"],
    ruleset: LanguageRuleset::Dart,
    parser_language: dart_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    member_edge_specs: None,
    receiver_call_specs: Some(receiver_call_specs),
    member_callsite_marker: Some(MEMBER_CALLSITE_MARKER),
    graph_call_syntax: Some("dart_member"),
    // Carried over verbatim from `lib.rs`'s `matches!(language_name, "swift" |
    // "dart")`. `rules/dart.scm` already labels class members METHOD, so the
    // promotion is a no-op on both snapshot fixtures — mutating this field
    // leaves them byte-identical. The value is preserved anyway because it is
    // the one the god file had for every Dart file, not just these two.
    promotes_type_member_functions_to_methods: true,
    qualified_name_delimiter: ".",
    route_comments_are_c_style: true,
    uses_generic_semantic_resolver: true,
    semantic_family: "dart",
};

fn dart_language() -> tree_sitter::Language {
    tree_sitter_dart_orchard::LANGUAGE.into()
}

/// Manual receiver-call edges for one parsed Dart file.
///
/// Was `lib.rs::collect_dart_receiver_call_edges`.
pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let import_alias_bindings = collect_dart_import_alias_bindings(source);
    let local_type_names = collect_dart_local_type_names(tree, source);
    walk_tree_nodes(tree.root_node(), &mut |body| {
        if body.kind() != "function_body" {
            return;
        }
        let Some(signature) = dart_signature_for_body(body) else {
            return;
        };
        let Some(source_name) = dart_callable_name(signature, source) else {
            return;
        };
        let call_source = ManualReceiverSource {
            name: &source_name,
            span: ts_node_graph_span(signature),
        };
        let receiver_types = collect_prefix_parameter_types(signature, source);
        let mut local_receiver_callsites = HashSet::new();
        collect_dart_precise_receiver_call_specs(
            body,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            DartReceiverContext {
                parameter_receiver_types: &receiver_types,
                import_alias_bindings: &import_alias_bindings,
                local_type_names: &local_type_names,
            },
            &mut local_receiver_callsites,
            &mut edges,
        );
        if !receiver_types.is_empty() {
            let receiver_modules =
                collect_dart_parameter_type_modules(signature, source, &import_alias_bindings);
            let start = edges.len();
            collect_receiver_call_specs_in_callable(
                body,
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
                if let Some(module_name) = receiver_modules.get(&spec.receiver_name) {
                    spec.owner_module = Some(module_name.clone());
                }
            }
            edges.extend(parameter_specs);
        }
    });
    edges
}

struct DartReceiverContext<'a> {
    parameter_receiver_types: &'a HashMap<String, String>,
    import_alias_bindings: &'a HashMap<String, String>,
    local_type_names: &'a HashSet<String>,
}

fn collect_dart_precise_receiver_call_specs(
    body: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    context: DartReceiverContext<'_>,
    local_receiver_callsites: &mut HashSet<ReceiverCallSiteKey>,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    walk_tree_nodes(body, &mut |node| {
        let Some((receiver_name, method_name)) = member_call(node, source) else {
            return;
        };
        if !receiver_call_belongs_to_callable(node, body) {
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
            dart_visible_local_receiver_owner(body, node, &receiver_name, source, &context)
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

        let owner = if let Some(owner) = dart_self_receiver_owner(body, &receiver_name, source) {
            Some(owner)
        } else if !context
            .parameter_receiver_types
            .contains_key(&receiver_name)
        {
            dart_property_receiver_owner(body, &receiver_name, source, &context)
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

fn dart_self_receiver_owner(
    body: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> OptionalReceiverOwnerBinding {
    if receiver_name != "this" {
        return None;
    }
    let owner_node = enclosing_node_with_kind(body, &["class_definition"])?;
    let owner_name = declaration_name(owner_node, source)?;
    Some((owner_name, None))
}

fn dart_visible_local_receiver_owner(
    body: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    context: &DartReceiverContext<'_>,
) -> Option<OptionalReceiverOwnerBinding> {
    let mut visible_bindings = Vec::new();
    walk_tree_nodes(body, &mut |node| {
        if node.kind() != "initialized_variable_definition" {
            return;
        }
        if !receiver_call_belongs_to_callable(node, body)
            || node.end_byte() > call_node.start_byte()
        {
            return;
        }
        let Some(binding_name) = dart_variable_binding_name(node, source) else {
            return;
        };
        if binding_name != receiver_name || !dart_local_binding_visible_at_call(node, call_node) {
            return;
        }
        visible_bindings.push((
            node.end_byte(),
            dart_initialized_constructor_owner(node, source, context),
        ));
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner_name)| owner_name)
}

fn dart_property_receiver_owner(
    body: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    context: &DartReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    let field_name = receiver_name
        .strip_prefix("this.")
        .unwrap_or(receiver_name)
        .trim();
    if field_name == "this" || field_name.contains('.') {
        return None;
    }
    let owner_node = enclosing_node_with_kind(body, &["class_definition"])?;
    let mut property_bindings = Vec::new();
    walk_tree_nodes(owner_node, &mut |node| {
        if !matches!(
            node.kind(),
            "initialized_variable_definition" | "field_signature" | "declaration"
        ) || !dart_property_belongs_to_owner(node, owner_node)
        {
            return;
        }
        let Some((binding_name, raw_type)) = dart_typed_variable_binding(node, source) else {
            return;
        };
        if binding_name != field_name {
            return;
        }
        if let Some(owner) = dart_receiver_owner_from_type(&raw_type, context) {
            property_bindings.push(owner);
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

fn dart_property_belongs_to_owner(property: TsNode<'_>, owner_node: TsNode<'_>) -> bool {
    let mut current = property.parent();
    while let Some(candidate) = current {
        if same_ts_span(candidate, owner_node) {
            return true;
        }
        if candidate.kind() == "function_body" || candidate.kind() == "class_definition" {
            return false;
        }
        current = candidate.parent();
    }
    false
}

fn dart_variable_binding_name(node: TsNode<'_>, source: &str) -> Option<String> {
    if let Some(name) = node
        .child_by_field_name("name")
        .and_then(|name| trimmed_node_text(name, source))
        .as_deref()
        .and_then(normalize_parameter_name)
    {
        return Some(name);
    }
    let surface = trimmed_node_text(node, source)?;
    let head = surface
        .split('=')
        .next()
        .unwrap_or(surface.as_str())
        .trim_end_matches(';')
        .trim();
    head.split_whitespace()
        .last()
        .and_then(normalize_parameter_name)
}

fn dart_typed_variable_binding(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    let binding_name = dart_variable_binding_name(node, source)?;
    let surface = trimmed_node_text(node, source)?;
    let head = surface
        .split('=')
        .next()
        .unwrap_or(surface.as_str())
        .trim_end_matches(';')
        .trim();
    let tokens = head
        .split_whitespace()
        .filter(|token| {
            !matches!(
                *token,
                "abstract"
                    | "covariant"
                    | "external"
                    | "final"
                    | "late"
                    | "static"
                    | "const"
                    | "var"
                    | "required"
            )
        })
        .collect::<Vec<_>>();
    if tokens.len() < 2 {
        return None;
    }
    let raw_type = tokens[..tokens.len() - 1].join(" ");
    Some((binding_name, raw_type))
}

fn dart_receiver_owner_from_type(
    raw_type: &str,
    context: &DartReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    let owner_name = normalize_type_surface(raw_type)?;
    if let Some(qualifier) = dart_type_import_qualifier(raw_type) {
        let module_name = context.import_alias_bindings.get(&qualifier)?;
        return Some((owner_name, Some(module_name.clone())));
    }
    context
        .local_type_names
        .contains(&owner_name)
        .then_some((owner_name, None))
}

fn dart_initialized_constructor_owner(
    node: TsNode<'_>,
    source: &str,
    context: &DartReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    if let Some(owner_name) = node
        .child_by_field_name("value")
        .and_then(|value| dart_constructor_owner(value, source, context))
    {
        return Some(owner_name);
    }
    let surface = trimmed_node_text(node, source)?;
    let (_, value_surface) = surface.split_once('=')?;
    dart_constructor_owner_surface(value_surface, context)
}

fn dart_constructor_owner(
    value: TsNode<'_>,
    source: &str,
    context: &DartReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    trimmed_node_text(value, source)
        .as_deref()
        .and_then(|surface| dart_constructor_owner_surface(surface, context))
}

fn dart_constructor_owner_surface(
    surface: &str,
    context: &DartReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    let surface = surface.trim().trim_end_matches(';').trim();
    let surface = surface
        .strip_prefix("const ")
        .or_else(|| surface.strip_prefix("new "))
        .unwrap_or(surface)
        .trim();
    let (constructor_name, _) = surface.split_once('(')?;
    dart_constructor_owner_from_type_surface(constructor_name, context)
}

fn dart_constructor_owner_from_type_surface(
    type_surface: &str,
    context: &DartReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    let type_surface = type_surface.trim();
    if type_surface.contains("::") {
        return None;
    }
    let owner_name = normalize_type_surface(type_surface)?;
    if !owner_name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
    {
        return None;
    }
    if let Some(qualifier) = dart_type_import_qualifier(type_surface) {
        let module_name = context.import_alias_bindings.get(&qualifier)?;
        return Some((owner_name, Some(module_name.clone())));
    }
    if type_surface.contains('.') {
        return None;
    }
    context
        .local_type_names
        .contains(&owner_name)
        .then_some((owner_name, None))
}

fn collect_dart_local_type_names(tree: &Tree, source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if !matches!(
            node.kind(),
            "class_definition" | "mixin_declaration" | "enum_declaration" | "extension_declaration"
        ) {
            return;
        }
        if let Some(name) = declaration_name(node, source) {
            names.insert(name);
        }
    });
    names
}

fn dart_local_binding_visible_at_call(binding: TsNode<'_>, call_node: TsNode<'_>) -> bool {
    let Some(binding_scope) = dart_lexical_scope(binding) else {
        return false;
    };
    let Some(call_scope) = dart_lexical_scope(call_node) else {
        return false;
    };
    node_is_same_or_ancestor(binding_scope, call_scope)
}

fn dart_lexical_scope(node: TsNode<'_>) -> Option<TsNode<'_>> {
    enclosing_node_with_kind(node, &["block", "function_body"])
}

/// Manual direct-call edges for one parsed Dart file.
///
/// Was `lib.rs::collect_dart_direct_call_edges`.
pub(crate) fn direct_call_specs(tree: &Tree, source: &str) -> Vec<ManualPreciseCallSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |body| {
        if body.kind() != "function_body" {
            return;
        }
        let Some(signature) = dart_signature_for_body(body) else {
            return;
        };
        let Some(source_name) = dart_callable_name(signature, source) else {
            return;
        };
        let source_span = ts_node_graph_span(signature);
        walk_tree_nodes(body, &mut |node| {
            let Some(target_name) = dart_direct_call(node, source) else {
                return;
            };
            if !receiver_call_belongs_to_callable(node, body) {
                return;
            }
            edges.push(ManualPreciseCallSpec {
                source_name: source_name.clone(),
                source_span,
                target_name,
                line: Some(node.start_position().row as u32 + 1),
            });
        });
    });
    edges
}

fn dart_signature_for_body<'tree>(body: TsNode<'tree>) -> Option<TsNode<'tree>> {
    previous_named_sibling_with_kind(body, &["method_signature", "function_signature"])
}

fn collect_dart_parameter_type_modules(
    callable: TsNode<'_>,
    source: &str,
    import_alias_bindings: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut receiver_modules = HashMap::new();
    let Some(parameters) = signature_parameter_surface(callable, source) else {
        return receiver_modules;
    };
    for parameter in split_top_level_parameters(&parameters) {
        let parameter = parameter
            .split('=')
            .next()
            .unwrap_or(parameter.as_str())
            .trim();
        let tokens = parameter
            .split_whitespace()
            .filter(|token| !matches!(*token, "final" | "const" | "var" | "required"))
            .collect::<Vec<_>>();
        if tokens.len() < 2 {
            continue;
        }
        let Some(receiver_name) =
            normalize_parameter_name(tokens.last().copied().unwrap_or_default())
        else {
            continue;
        };
        let raw_type = tokens[..tokens.len() - 1].join(" ");
        let Some(qualifier) = dart_type_import_qualifier(&raw_type) else {
            continue;
        };
        let Some(module_name) = import_alias_bindings.get(&qualifier) else {
            continue;
        };
        receiver_modules.insert(receiver_name, module_name.clone());
    }
    receiver_modules
}

fn dart_type_import_qualifier(raw_type: &str) -> Option<String> {
    if raw_type.contains('|') || raw_type.contains('&') {
        return None;
    }
    let surface = raw_type.trim().trim_end_matches('?').trim();
    let base = surface
        .split(['<', '[', '('])
        .next()
        .unwrap_or(surface)
        .trim();
    let (qualifier, _) = base.rsplit_once('.')?;
    normalize_parameter_name(qualifier)
}

fn collect_dart_import_alias_bindings(source: &str) -> HashMap<String, String> {
    let mut bindings = HashMap::new();
    let mut duplicates = HashSet::new();
    for statement in dart_import_statements(source) {
        let Some(module_name) = dart_import_module_name(&statement) else {
            continue;
        };
        let Some(alias) = dart_import_alias_name(&statement) else {
            continue;
        };
        if duplicates.contains(&alias) {
            continue;
        }
        if bindings.contains_key(&alias) {
            bindings.remove(&alias);
            duplicates.insert(alias);
            continue;
        }
        bindings.insert(alias, module_name);
    }
    bindings
}

fn dart_import_statements(source: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut collecting = false;

    for raw_line in source.lines() {
        let line = raw_line.split("//").next().unwrap_or(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if !collecting {
            if !line.starts_with("import ") {
                continue;
            }
            current.clear();
            current.push_str(line);
            if line.contains(';') {
                statements.push(current.clone());
            } else {
                collecting = true;
            }
            continue;
        }

        current.push(' ');
        current.push_str(line);
        if line.contains(';') {
            statements.push(current.clone());
            current.clear();
            collecting = false;
        }
    }

    statements
}

fn dart_import_module_name(statement: &str) -> Option<String> {
    let rest = statement.strip_prefix("import")?.trim();
    let quote = rest.chars().find(|ch| matches!(*ch, '"' | '\''))?;
    let start = rest.find(quote)? + quote.len_utf8();
    let end = rest[start..].find(quote)? + start;
    let module_name = rest[start..end].trim();
    (!module_name.is_empty()).then(|| module_name.to_string())
}

fn dart_import_alias_name(statement: &str) -> Option<String> {
    let mut tokens = statement
        .trim_end_matches(';')
        .split_whitespace()
        .collect::<Vec<_>>();
    while let Some(token) = tokens.pop() {
        if token == "as" {
            return None;
        }
        if tokens.last().copied() == Some("as") {
            return normalize_parameter_name(token);
        }
    }
    None
}

fn member_call(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    let mut cursor = node.walk();
    let children = node.named_children(&mut cursor).collect::<Vec<_>>();
    for window in children.windows(2) {
        if window[0].kind() != "selector"
            || window[1].kind() != "selector"
            || first_descendant_with_kind(window[1], "argument_part").is_none()
        {
            continue;
        }
        let selector = first_descendant_with_kind(window[0], "unconditional_assignable_selector")?;
        let method = first_descendant_with_kind(selector, "identifier")
            .and_then(|identifier| trimmed_node_text(identifier, source))?;
        let receiver = source
            .get(node.start_byte()..window[0].start_byte())?
            .trim();
        return Some((
            normalized_dart_receiver_surface(receiver)?,
            normalize_parameter_name(&method)?,
        ));
    }
    None
}

fn normalized_dart_receiver_surface(raw: &str) -> Option<String> {
    let receiver = raw
        .rsplit([' ', '\t', '\n', '\r', '(', '[', '{'])
        .find(|part| !part.trim().is_empty())
        .unwrap_or(raw)
        .trim()
        .trim_end_matches('?')
        .trim();
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

fn dart_direct_call(node: TsNode<'_>, source: &str) -> Option<String> {
    if node.kind() != "method_invocation" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    if function.kind() != "identifier" {
        return None;
    }
    trimmed_node_text(function, source)
        .as_deref()
        .and_then(normalize_parameter_name)
}

fn dart_callable_name(node: TsNode<'_>, source: &str) -> Option<String> {
    descendant_by_field_name(node, "name")
        .or_else(|| first_descendant_with_kind(node, "identifier"))
        .and_then(|name_node| trimmed_node_text(name_node, source))
}
