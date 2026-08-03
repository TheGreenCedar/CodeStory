//! Go extraction rules.
//!
//! Go's graph extraction lives here: the tree-sitter rule file, the
//! compiled-rule cache, the selector-call callsite marker, the manual MEMBER
//! collector that ties `func (r *Repository) Save()` back to the `Repository`
//! declaration, and the receiver-call resolution engine that turns
//! `w.repo.Save(...)` into an edge aimed at `Repository.Save`. Every
//! language-keyed dispatch in the crate reaches them through
//! [`super::EXTRACTIONS`] rather than by spelling `"go"`.
//!
//! Three Go surfaces are deliberately *not* here, and all three are shared
//! seams rather than per-language registry rows:
//!
//! * `lib.rs::collect_go_route` / `go_route_framework` and their `"go"` arm in
//!   the framework-route scanner. The per-language route collectors take
//!   non-uniform arguments and a per-framework precondition, so routing them
//!   through the registry is one change for all sixteen languages, not part of
//!   Go's rollback unit.
//! * `lib.rs::append_text_only_go_symbols` and its text-symbol helpers, which
//!   belong to the parser-less fallback path (`index_text_only_file`) rather
//!   than to parser-backed extraction.
//! * `LanguageRuleset::Go`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//!
//! `SemanticResolverKind::Go` also stays in
//! `semantic::dedicated_semantic_resolver`: Go has a dedicated resolver type
//! and those types are private to that module, so the registry records the
//! choice (`uses_generic_semantic_resolver: false`) and the residual match
//! constructs it. Kotlin, being generic, could delete its arm; Go cannot.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both Go fixtures so the move stays output-equal.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, LanguageRuleset, ManualMemberEdgeSpec, ManualReceiverCallSpec,
    ManualReceiverSource, OptionalReceiverOwnerBinding, ReceiverCallSiteKey, ReceiverOwnerBinding,
    collect_receiver_call_specs_in_callable, declaration_name, descendant_by_field_name,
    enclosing_node_with_kind, member_call_method_col, node_is_same_or_ancestor,
    normalize_parameter_name, normalized_receiver_variable, receiver_call_belongs_to_callable,
    receiver_callsite_key, trimmed_node_text, ts_node_graph_span, walk_tree_nodes,
};

/// Callsite marker written onto edges produced from Go selector-call syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:go-selector-call";

const GRAPH_QUERY: &str = include_str!("../../rules/go.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for Go.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["go"],
    language_name: "go",
    extensions: &["go"],
    ruleset: LanguageRuleset::Go,
    parser_language: go_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    member_edge_specs: Some(member_edge_specs),
    receiver_call_specs: Some(receiver_call_specs),
    member_callsite_marker: Some(MEMBER_CALLSITE_MARKER),
    graph_call_syntax: Some("go_selector"),
    // A Go method is already a `method_declaration` with an explicit receiver,
    // so the rule file emits METHOD directly and the projection must not
    // re-promote a plain `func` that happens to sit under a type-like owner.
    // `false` is the value the god file's roster gave Go.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: ".",
    route_comments_are_c_style: true,
    // Go has a dedicated `GoSemanticResolver`; the residual match in
    // `semantic::dedicated_semantic_resolver` still constructs it.
    uses_generic_semantic_resolver: false,
    semantic_family: "go",
};

fn go_language() -> tree_sitter::Language {
    tree_sitter_go::LANGUAGE.into()
}

pub(crate) fn member_edge_specs(tree: &Tree, source: &str) -> Vec<ManualMemberEdgeSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| match node.kind() {
        "method_declaration" => {
            let Some(method_name_node) = node.child_by_field_name("name") else {
                return;
            };
            let Some(receiver_node) = node.child_by_field_name("receiver") else {
                return;
            };
            let Some(source_name) = go_receiver_owner_name(receiver_node, source) else {
                return;
            };
            let Some(target_name) = trimmed_node_text(method_name_node, source) else {
                return;
            };

            edges.push(ManualMemberEdgeSpec {
                source_name,
                target_name,
                source_span: ts_node_graph_span(
                    receiver_owner_declaration_node(tree.root_node(), source, receiver_node)
                        .unwrap_or(receiver_node),
                ),
                target_span: ts_node_graph_span(node),
                line: Some(node.start_position().row as u32 + 1),
            });
        }
        "method_elem" => {
            let Some(owner_node) = enclosing_node_with_kind(node, &["type_declaration"]) else {
                return;
            };
            let Some(owner_name_node) = descendant_by_field_name(owner_node, "name") else {
                return;
            };
            let Some(source_name) = trimmed_node_text(owner_name_node, source) else {
                return;
            };
            let Some(method_name_node) = node.child_by_field_name("name") else {
                return;
            };
            let Some(target_name) = trimmed_node_text(method_name_node, source) else {
                return;
            };

            edges.push(ManualMemberEdgeSpec {
                source_name,
                target_name,
                source_span: ts_node_graph_span(owner_node),
                target_span: ts_node_graph_span(node),
                line: Some(node.start_position().row as u32 + 1),
            });
        }
        _ => {}
    });
    edges
}

fn receiver_owner_declaration_node<'tree>(
    root: TsNode<'tree>,
    source: &str,
    receiver_node: TsNode<'tree>,
) -> Option<TsNode<'tree>> {
    let owner_name = go_receiver_owner_name(receiver_node, source)?;
    find_go_type_declaration_by_name(root, source, &owner_name)
}

fn find_go_type_declaration_by_name<'tree>(
    node: TsNode<'tree>,
    source: &str,
    owner_name: &str,
) -> Option<TsNode<'tree>> {
    if node.kind() == "type_declaration"
        && let Some(name_node) = descendant_by_field_name(node, "name")
        && trimmed_node_text(name_node, source).as_deref() == Some(owner_name)
    {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = find_go_type_declaration_by_name(child, source, owner_name) {
            return Some(found);
        }
    }
    None
}

fn go_receiver_owner_name(receiver_node: TsNode<'_>, source: &str) -> Option<String> {
    let text = trimmed_node_text(receiver_node, source)?;
    let inner = text
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    let raw_owner = inner.split_whitespace().last()?.trim();
    normalize_go_type_surface(raw_owner)
}

fn normalize_go_type_surface(raw: &str) -> Option<String> {
    let mut surface = raw.trim();
    while let Some(stripped) = surface.strip_prefix('*') {
        surface = stripped.trim_start();
    }
    if let Some(stripped) = surface.strip_prefix("[]") {
        surface = stripped.trim_start();
    }
    let base = surface.split('[').next().unwrap_or(surface).trim();
    let terminal = base.rsplit('.').next().unwrap_or(base).trim();
    (!terminal.is_empty()).then(|| terminal.to_string())
}

pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let import_bindings = collect_go_import_bindings(source);
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
        let call_source = ManualReceiverSource {
            name: &source_name,
            span: ts_node_graph_span(callable),
        };
        let mut local_binding_callsites = HashSet::new();
        collect_go_local_composite_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &import_bindings,
            &mut local_binding_callsites,
            &mut edges,
        );
        let method_receiver_bindings = collect_go_method_receiver_bindings(
            callable,
            tree.root_node(),
            source,
            &import_bindings,
        );
        let mut receiver_types = method_receiver_bindings
            .iter()
            .map(|(receiver_name, (owner_name, _))| (receiver_name.clone(), owner_name.clone()))
            .collect::<HashMap<_, _>>();
        receiver_types.extend(collect_go_parameter_types(callable, source));
        if receiver_types.is_empty() {
            return;
        }
        let mut receiver_modules =
            collect_go_parameter_type_modules(callable, source, &import_bindings);
        for (receiver_name, (_, owner_module)) in &method_receiver_bindings {
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
            selector_call,
            false,
            &mut edges,
        );
        let mut parameter_specs = edges.split_off(start);
        parameter_specs
            .retain(|spec| !local_binding_callsites.contains(&receiver_callsite_key(spec)));
        for spec in &mut parameter_specs {
            if let Some(module_name) = receiver_modules.get(&spec.receiver_name) {
                spec.owner_module = Some(module_name.clone());
            }
        }
        edges.extend(parameter_specs);
    });
    edges
}

fn collect_go_local_composite_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    import_bindings: &HashMap<String, String>,
    local_binding_callsites: &mut HashSet<ReceiverCallSiteKey>,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    walk_tree_nodes(callable, &mut |node| {
        let Some((receiver_name, method_name)) = selector_call(node, source) else {
            return;
        };
        if !receiver_call_belongs_to_callable(node, callable) {
            return;
        }
        let Some(owner_name) = go_visible_local_composite_owner(
            callable,
            node,
            &receiver_name,
            source,
            import_bindings,
        ) else {
            return;
        };
        let method_col = member_call_method_col(node, source, &method_name);
        local_binding_callsites.insert(ReceiverCallSiteKey {
            receiver_name: receiver_name.clone(),
            method_name: method_name.clone(),
            line: Some(node.start_position().row as u32 + 1),
            method_col,
        });
        if let Some((owner_name, owner_module)) = owner_name {
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

fn go_visible_local_composite_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    import_bindings: &HashMap<String, String>,
) -> Option<OptionalReceiverOwnerBinding> {
    let mut visible_bindings = Vec::new();
    walk_tree_nodes(callable, &mut |node| {
        if !matches!(
            node.kind(),
            "short_var_declaration" | "assignment_statement"
        ) {
            return;
        }
        if !receiver_call_belongs_to_callable(node, callable)
            || node.end_byte() > call_node.start_byte()
        {
            return;
        }
        if !go_local_binding_visible_at_call(node, call_node) {
            return;
        }
        let Some(owner_name) =
            go_receiver_write_owner(node, receiver_name, source, import_bindings)
        else {
            return;
        };
        visible_bindings.push((node.end_byte(), owner_name));
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner_name)| owner_name)
}

fn go_receiver_write_owner(
    node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    import_bindings: &HashMap<String, String>,
) -> Option<OptionalReceiverOwnerBinding> {
    if !matches!(
        node.kind(),
        "short_var_declaration" | "assignment_statement"
    ) {
        return None;
    }
    let left_items = node
        .child_by_field_name("left")
        .map(go_expression_list_items)
        .unwrap_or_default();
    let receiver_index = left_items.iter().position(|left| {
        normalized_receiver_variable(*left, source).as_deref() == Some(receiver_name)
    })?;
    let owner_name = node
        .child_by_field_name("right")
        .map(go_expression_list_items)
        .and_then(|right_items| {
            right_items.get(receiver_index).and_then(|right| {
                go_direct_composite_literal_owner(*right, source, import_bindings)
            })
        });
    Some(owner_name)
}

fn go_expression_list_items(node: TsNode<'_>) -> Vec<TsNode<'_>> {
    if node.kind() != "expression_list" {
        return vec![node];
    }
    let mut items = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        items.push(child);
    }
    items
}

fn go_direct_composite_literal_owner(
    node: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
) -> OptionalReceiverOwnerBinding {
    if node.kind() == "composite_literal" {
        return node
            .child_by_field_name("type")
            .and_then(|type_node| trimmed_node_text(type_node, source))
            .as_deref()
            .and_then(|type_surface| {
                go_composite_literal_owner_binding_from_type(type_surface, import_bindings)
            });
    }
    trimmed_node_text(node, source)
        .as_deref()
        .and_then(|surface| go_direct_composite_literal_owner_surface(surface, import_bindings))
}

fn go_direct_composite_literal_owner_surface(
    surface: &str,
    import_bindings: &HashMap<String, String>,
) -> OptionalReceiverOwnerBinding {
    let surface = surface.trim().trim_start_matches('&').trim();
    if !surface.contains('{') {
        return None;
    }
    let type_surface = surface
        .split_once('{')
        .map(|(type_surface, _)| type_surface)
        .unwrap_or(surface)
        .trim();
    go_composite_literal_owner_binding_from_type(type_surface, import_bindings)
}

fn go_composite_literal_owner_binding_from_type(
    type_surface: &str,
    import_bindings: &HashMap<String, String>,
) -> OptionalReceiverOwnerBinding {
    let type_surface = type_surface.trim().trim_start_matches('&').trim();
    if type_surface.contains('(')
        || type_surface.contains(')')
        || type_surface.starts_with("[]")
        || type_surface.starts_with("map[")
    {
        return None;
    }
    let owner_name = normalize_go_type_surface(type_surface)?;
    if !owner_name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
    {
        return None;
    }
    if let Some(qualifier) = go_type_import_qualifier(type_surface) {
        let module_name = import_bindings.get(&qualifier)?;
        return Some((owner_name, Some(module_name.clone())));
    }
    if type_surface.contains('.') {
        return None;
    }
    Some((owner_name, None))
}

fn go_local_binding_visible_at_call(binding: TsNode<'_>, call_node: TsNode<'_>) -> bool {
    let Some(binding_scope) = go_lexical_scope(binding) else {
        return false;
    };
    let Some(call_scope) = go_lexical_scope(call_node) else {
        return false;
    };
    node_is_same_or_ancestor(binding_scope, call_scope)
}

fn go_lexical_scope(node: TsNode<'_>) -> Option<TsNode<'_>> {
    enclosing_node_with_kind(node, &["block"])
}

fn collect_go_method_receiver_bindings(
    callable: TsNode<'_>,
    root: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
) -> HashMap<String, ReceiverOwnerBinding> {
    let mut receiver_types = HashMap::new();
    if callable.kind() != "method_declaration" {
        return receiver_types;
    }
    let Some(receiver_node) = callable.child_by_field_name("receiver") else {
        return receiver_types;
    };
    let Some(receiver_name) = go_receiver_variable_name(receiver_node, source) else {
        return receiver_types;
    };
    let Some(owner_name) = go_receiver_owner_name(receiver_node, source) else {
        return receiver_types;
    };
    receiver_types.insert(receiver_name.clone(), (owner_name.clone(), None));
    let Some(owner_node) = find_go_type_declaration_by_name(root, source, &owner_name) else {
        return receiver_types;
    };
    for (field_name, field_owner) in
        collect_go_struct_field_types(owner_node, source, import_bindings)
    {
        receiver_types.insert(format!("{receiver_name}.{field_name}"), field_owner);
    }
    receiver_types
}

fn go_receiver_variable_name(receiver_node: TsNode<'_>, source: &str) -> Option<String> {
    let text = trimmed_node_text(receiver_node, source)?;
    let inner = text
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    let tokens = inner.split_whitespace().collect::<Vec<_>>();
    if tokens.len() < 2 {
        return None;
    }
    normalize_parameter_name(tokens[0])
}

fn collect_go_struct_field_types(
    owner_node: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
) -> HashMap<String, ReceiverOwnerBinding> {
    let mut field_types = HashMap::new();
    walk_tree_nodes(owner_node, &mut |node| {
        if node.kind() != "field_declaration" {
            return;
        }
        for (field_name, owner_name) in go_field_declaration_bindings(node, source, import_bindings)
        {
            field_types.insert(field_name, owner_name);
        }
    });
    field_types
}

fn go_field_declaration_bindings(
    node: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
) -> Vec<(String, ReceiverOwnerBinding)> {
    if let Some(type_node) = node.child_by_field_name("type")
        && let Some(raw_type) = trimmed_node_text(type_node, source)
        && let Some(owner_binding) = go_receiver_owner_from_type(&raw_type, import_bindings)
    {
        let mut names = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.end_byte() > type_node.start_byte() {
                continue;
            }
            if matches!(child.kind(), "field_identifier" | "identifier")
                && let Some(name) = normalized_receiver_variable(child, source)
            {
                names.push(name);
            }
        }
        if !names.is_empty() {
            return names
                .into_iter()
                .map(|name| (name, owner_binding.clone()))
                .collect();
        }
    }

    trimmed_node_text(node, source)
        .as_deref()
        .map(|surface| go_field_declaration_bindings_surface(surface, import_bindings))
        .unwrap_or_default()
}

fn go_field_declaration_bindings_surface(
    surface: &str,
    import_bindings: &HashMap<String, String>,
) -> Vec<(String, ReceiverOwnerBinding)> {
    let surface = surface.split('`').next().unwrap_or(surface).trim();
    let tokens = surface.split_whitespace().collect::<Vec<_>>();
    let Some(raw_type) = tokens.last() else {
        return Vec::new();
    };
    if tokens.len() < 2 {
        return Vec::new();
    }
    let Some(owner_binding) = go_receiver_owner_from_type(raw_type, import_bindings) else {
        return Vec::new();
    };
    let names_surface = tokens[..tokens.len() - 1].join(" ");
    names_surface
        .split(',')
        .filter_map(normalize_parameter_name)
        .map(|name| (name, owner_binding.clone()))
        .collect()
}

fn go_receiver_owner_from_type(
    raw_type: &str,
    import_bindings: &HashMap<String, String>,
) -> OptionalReceiverOwnerBinding {
    let owner_name = normalize_go_type_surface(raw_type)?;
    if let Some(qualifier) = go_type_import_qualifier(raw_type) {
        let module_name = import_bindings.get(&qualifier)?;
        return Some((owner_name, Some(module_name.clone())));
    }
    Some((owner_name, None))
}

fn collect_go_import_bindings(source: &str) -> HashMap<String, String> {
    let mut bindings = HashMap::new();
    let mut duplicates = HashSet::new();
    let mut in_import_list = false;
    for raw_line in source.lines() {
        let line = go_strip_line_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if in_import_list {
            if line.starts_with(')') {
                in_import_list = false;
                continue;
            }
            if let Some((local_name, module_name)) = go_import_binding_from_spec(line) {
                insert_unique_import_binding(
                    &mut bindings,
                    &mut duplicates,
                    local_name,
                    module_name,
                );
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("import") else {
            continue;
        };
        let rest = rest.trim();
        if rest.starts_with('(') {
            in_import_list = true;
            continue;
        }
        if let Some((local_name, module_name)) = go_import_binding_from_spec(rest) {
            insert_unique_import_binding(&mut bindings, &mut duplicates, local_name, module_name);
        }
    }
    bindings
}

fn insert_unique_import_binding(
    bindings: &mut HashMap<String, String>,
    duplicates: &mut HashSet<String>,
    local_name: String,
    module_name: String,
) {
    if duplicates.contains(&local_name) {
        return;
    }
    if bindings.contains_key(&local_name) {
        bindings.remove(&local_name);
        duplicates.insert(local_name);
        return;
    }
    bindings.insert(local_name, module_name);
}

fn go_strip_line_comment(line: &str) -> &str {
    line.split("//").next().unwrap_or(line)
}

fn go_import_binding_from_spec(spec: &str) -> Option<(String, String)> {
    let spec = spec.trim().trim_end_matches(';').trim();
    if spec.is_empty() {
        return None;
    }
    let tokens = spec.split_whitespace().collect::<Vec<_>>();
    let (local_name, module_name) = match tokens.as_slice() {
        [module] => {
            let module_name = go_import_module_name(module)?;
            (go_default_import_local_name(&module_name)?, module_name)
        }
        [alias, module] if *alias != "." && *alias != "_" => {
            let module_name = go_import_module_name(module)?;
            (normalize_parameter_name(alias)?, module_name)
        }
        _ => return None,
    };
    Some((local_name, module_name))
}

fn go_import_module_name(raw: &str) -> Option<String> {
    let module = raw.trim().trim_matches(|ch| matches!(ch, '"' | '\'' | '`'));
    (!module.is_empty()).then(|| module.to_string())
}

fn go_default_import_local_name(module_name: &str) -> Option<String> {
    module_name
        .rsplit('/')
        .next()
        .and_then(normalize_parameter_name)
}

fn collect_go_parameter_types(callable: TsNode<'_>, source: &str) -> HashMap<String, String> {
    let mut receiver_types = HashMap::new();
    let Some(parameters) = callable.child_by_field_name("parameters") else {
        return receiver_types;
    };
    walk_tree_nodes(parameters, &mut |node| {
        if !matches!(
            node.kind(),
            "parameter_declaration" | "variadic_parameter_declaration"
        ) {
            return;
        }
        let Some(type_node) = node.child_by_field_name("type") else {
            return;
        };
        let Some(raw_type) = trimmed_node_text(type_node, source) else {
            return;
        };
        let Some(owner_name) = normalize_go_type_surface(&raw_type) else {
            return;
        };
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "identifier"
                && child.end_byte() <= type_node.start_byte()
                && let Some(name) = normalized_receiver_variable(child, source)
            {
                receiver_types.insert(name, owner_name.clone());
            }
        }
    });
    receiver_types
}

fn collect_go_parameter_type_modules(
    callable: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut receiver_modules = HashMap::new();
    let Some(parameters) = callable.child_by_field_name("parameters") else {
        return receiver_modules;
    };
    walk_tree_nodes(parameters, &mut |node| {
        if !matches!(
            node.kind(),
            "parameter_declaration" | "variadic_parameter_declaration"
        ) {
            return;
        }
        let Some(type_node) = node.child_by_field_name("type") else {
            return;
        };
        let Some(raw_type) = trimmed_node_text(type_node, source) else {
            return;
        };
        let Some(qualifier) = go_type_import_qualifier(&raw_type) else {
            return;
        };
        let Some(module_name) = import_bindings.get(&qualifier) else {
            return;
        };
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "identifier"
                && child.end_byte() <= type_node.start_byte()
                && let Some(name) = normalized_receiver_variable(child, source)
            {
                receiver_modules.insert(name, module_name.clone());
            }
        }
    });
    receiver_modules
}

fn go_type_import_qualifier(raw_type: &str) -> Option<String> {
    let mut surface = raw_type.trim();
    while let Some(stripped) = surface.strip_prefix('*') {
        surface = stripped.trim_start();
    }
    while let Some(stripped) = surface.strip_prefix("[]") {
        surface = stripped.trim_start();
    }
    let base = surface.split('[').next().unwrap_or(surface).trim();
    let (qualifier, _) = base.rsplit_once('.')?;
    normalize_parameter_name(qualifier)
}

fn selector_call(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    if function.kind() != "selector_expression" {
        return None;
    }
    let receiver = function.child_by_field_name("operand")?;
    let method = function.child_by_field_name("field")?;
    Some((
        normalized_receiver_variable(receiver, source)?,
        trimmed_node_text(method, source)?,
    ))
}
