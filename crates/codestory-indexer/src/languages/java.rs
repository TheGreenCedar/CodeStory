//! Java extraction rules.
//!
//! Java's graph extraction lives here: the tree-sitter rule file, the
//! compiled-rule cache, the member-call callsite marker, and the receiver-call
//! resolution engine that turns `repo.save(...)` into an edge aimed at
//! `Repository.save`. Every language-keyed dispatch in the crate reaches it
//! through [`super::EXTRACTIONS`] rather than by spelling `"java"`.
//!
//! Four Java surfaces are deliberately *not* here, and all four are shared
//! seams rather than Java content:
//!
//! * `lib.rs::collect_spring_route` and its `"java"` arm in the
//!   framework-route scanner. The per-language route collectors take
//!   non-uniform arguments and a per-framework `has_<framework>` precondition,
//!   so routing them through the registry is one change for all sixteen
//!   languages, not part of Java's rollback unit.
//! * `lib.rs::collect_java_declaration_span_overrides` and the `("java",
//!   NodeKind::ANNOTATION, _)` span-policy arms. Declaration spans are a
//!   cross-language projection dispatch with no [`super::LanguageExtraction`]
//!   field; giving it one is its own package.
//! * `semantic::JavaSemanticResolver`, which stays behind
//!   `semantic::dedicated_semantic_resolver` because the resolver types are
//!   private to that module. The registry records the choice through
//!   `uses_generic_semantic_resolver: false`.
//! * `LanguageRuleset::Java`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both Java fixtures so the move stays output-equal.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, LanguageRuleset, ManualReceiverCallSpec, ManualReceiverSource,
    OptionalReceiverOwnerBinding, ReceiverCallSiteKey, collect_prefix_parameter_types,
    collect_receiver_call_specs_in_callable, declaration_name, enclosing_node_with_kind,
    member_call_method_col, node_is_same_or_ancestor, normalize_parameter_name,
    normalize_type_surface, normalized_receiver_variable, receiver_call_belongs_to_callable,
    receiver_callsite_key, same_ts_span, trimmed_node_text, ts_node_graph_span, walk_tree_nodes,
};

/// Callsite marker written onto edges produced from Java member-call syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:java-member-call";

const GRAPH_QUERY: &str = include_str!("../../rules/java.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for Java.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["java"],
    language_name: "java",
    extensions: &["java"],
    ruleset: LanguageRuleset::Java,
    parser_language: java_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    member_edge_specs: None,
    receiver_call_specs: Some(receiver_call_specs),
    member_callsite_marker: Some(MEMBER_CALLSITE_MARKER),
    graph_call_syntax: Some("java_member"),
    // Java spells a method `method_declaration`, so the rule file already emits
    // METHOD and there is nothing to promote — its only FUNCTION rule is a
    // lambda bound to a `local_variable_declaration`, whose owner is a method
    // rather than a type. Kotlin/Swift/Dart need the promotion because their
    // member functions share `function_declaration` with free functions.
    // `false` is what `lib.rs` answered before the move; flipping it is inert
    // for Java, so the snapshots below do not pin this field the way they pin
    // `qualified_name_delimiter`.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: ".",
    route_comments_are_c_style: true,
    // `semantic::JavaSemanticResolver` is a dedicated resolver, not the shared
    // name-only one, and its type is private to `semantic`.
    uses_generic_semantic_resolver: false,
    semantic_family: "java",
};

fn java_language() -> tree_sitter::Language {
    tree_sitter_java::LANGUAGE.into()
}

/// Manual receiver-call edges for one parsed Java file.
///
/// Was `lib.rs::collect_java_receiver_call_edges`.
pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let import_bindings = collect_java_import_type_bindings(tree.root_node(), source);
    let local_type_names = collect_java_top_level_type_names(tree.root_node(), source);
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if callable.kind() != "method_declaration" {
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
        collect_java_local_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &import_bindings,
            &local_type_names,
            &mut local_receiver_callsites,
            &mut edges,
        );
        let receiver_types = collect_prefix_parameter_types(callable, source);
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
            if let Some(module_name) = import_bindings.get(&spec.owner_name) {
                spec.owner_module = Some(module_name.clone());
            }
        }
        edges.extend(parameter_specs);
    });
    edges
}

fn collect_java_local_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
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
        let owner = if let Some(owner) = java_visible_receiver_owner(
            callable,
            node,
            &receiver_name,
            source,
            import_bindings,
            local_type_names,
        ) {
            owner
        } else {
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

fn java_visible_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> Option<OptionalReceiverOwnerBinding> {
    if let Some(owner) =
        java_direct_new_receiver_owner(call_node, source, import_bindings, local_type_names)
    {
        return Some(Some(owner));
    }
    if let Some(owner) = java_self_receiver_owner(callable, receiver_name, source) {
        return Some(Some(owner));
    }
    if let Some(owner) = java_visible_local_receiver_owner(
        callable,
        call_node,
        receiver_name,
        source,
        import_bindings,
        local_type_names,
    ) {
        return Some(owner);
    }
    if let Some(owner) = java_field_receiver_owner(
        callable,
        receiver_name,
        source,
        import_bindings,
        local_type_names,
    ) {
        return Some(Some(owner));
    }
    java_static_receiver_owner(receiver_name, import_bindings, local_type_names).map(Some)
}

fn java_self_receiver_owner(
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
            "record_declaration",
            "enum_declaration",
            "annotation_type_declaration",
        ],
    )?;
    let owner_name = declaration_name(owner_node, source)?;
    Some((owner_name, None))
}

fn java_direct_new_receiver_owner(
    call_node: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> OptionalReceiverOwnerBinding {
    let receiver_node = call_node.child_by_field_name("object")?;
    java_direct_new_owner(receiver_node, source, import_bindings, local_type_names)
}

fn java_visible_local_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> Option<OptionalReceiverOwnerBinding> {
    let mut visible_bindings = Vec::new();
    walk_tree_nodes(callable, &mut |node| match node.kind() {
        "local_variable_declaration" => {
            if !receiver_call_belongs_to_callable(node, callable)
                || node.end_byte() > call_node.start_byte()
                || !java_local_binding_visible_at_call(node, call_node)
            {
                return;
            }
            for (binding_name, owner) in java_local_variable_receiver_bindings(
                node,
                source,
                import_bindings,
                local_type_names,
            ) {
                if binding_name == receiver_name {
                    visible_bindings.push((node.end_byte(), owner));
                }
            }
        }
        "enhanced_for_statement"
        | "catch_clause"
        | "try_statement"
        | "try_with_resources_statement" => {
            if !receiver_call_belongs_to_callable(node, callable)
                || !java_scoped_binding_visible_at_call(node, call_node)
            {
                return;
            }
            if let Some((binding_name, owner)) =
                java_scoped_receiver_binding(node, source, import_bindings, local_type_names)
                && binding_name == receiver_name
            {
                visible_bindings.push((node.start_byte(), owner));
            }
        }
        _ => {}
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner)| owner)
}

fn java_field_receiver_owner(
    callable: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> OptionalReceiverOwnerBinding {
    let field_name = receiver_name
        .strip_prefix("this.")
        .unwrap_or(receiver_name)
        .trim();
    if field_name == "this" || field_name.contains('.') {
        return None;
    }
    let owner_node = enclosing_node_with_kind(
        callable,
        &[
            "class_declaration",
            "interface_declaration",
            "record_declaration",
            "enum_declaration",
            "annotation_type_declaration",
        ],
    )?;
    let mut field_bindings = Vec::new();
    walk_tree_nodes(owner_node, &mut |node| {
        if node.kind() != "field_declaration" {
            return;
        }
        if !enclosing_node_with_kind(
            node,
            &[
                "class_declaration",
                "interface_declaration",
                "record_declaration",
                "enum_declaration",
                "annotation_type_declaration",
            ],
        )
        .is_some_and(|owner| same_ts_span(owner, owner_node))
        {
            return;
        }
        for (binding_name, owner) in java_field_declaration_receiver_bindings(
            node,
            source,
            import_bindings,
            local_type_names,
        ) {
            if binding_name == field_name
                && let Some(owner) = owner
            {
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

fn java_field_declaration_receiver_bindings(
    node: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> Vec<(String, OptionalReceiverOwnerBinding)> {
    java_variable_declaration_receiver_bindings(node, source, import_bindings, local_type_names)
}

fn java_local_variable_receiver_bindings(
    node: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> Vec<(String, OptionalReceiverOwnerBinding)> {
    java_variable_declaration_receiver_bindings(node, source, import_bindings, local_type_names)
}

fn java_scoped_binding_visible_at_call(binding: TsNode<'_>, call_node: TsNode<'_>) -> bool {
    if matches!(
        binding.kind(),
        "try_statement" | "try_with_resources_statement"
    ) {
        let mut cursor = binding.walk();
        return binding
            .named_children(&mut cursor)
            .find(|child| child.kind() == "block")
            .is_some_and(|body| node_is_same_or_ancestor(body, call_node));
    }
    node_is_same_or_ancestor(binding, call_node)
}

fn java_scoped_receiver_binding(
    node: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> Option<(String, OptionalReceiverOwnerBinding)> {
    let surface = trimmed_node_text(node, source)?;
    let header = match node.kind() {
        "enhanced_for_statement" => surface
            .split_once('(')?
            .1
            .split_once(':')?
            .0
            .trim()
            .to_string(),
        "catch_clause" => surface
            .split_once('(')?
            .1
            .split_once(')')?
            .0
            .trim()
            .to_string(),
        "try_statement" | "try_with_resources_statement" => {
            let rest = surface.trim_start().strip_prefix("try")?.trim_start();
            let rest = rest.strip_prefix('(')?;
            rest.split_once('{')?
                .0
                .trim()
                .trim_end_matches(')')
                .trim()
                .to_string()
        }
        _ => return None,
    };
    java_typed_binding_header_owner(&header, import_bindings, local_type_names)
}

fn java_typed_binding_header_owner(
    header: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> Option<(String, OptionalReceiverOwnerBinding)> {
    for segment in header.split(';') {
        let head = segment
            .split('=')
            .next()
            .unwrap_or(segment)
            .trim()
            .trim_end_matches(')')
            .trim();
        let tokens = head
            .split_whitespace()
            .filter(|token| *token != "final" && !token.starts_with('@'))
            .collect::<Vec<_>>();
        if tokens.len() < 2 {
            continue;
        }
        let Some(binding_name) =
            normalize_parameter_name(tokens.last().copied().unwrap_or_default())
        else {
            continue;
        };
        let raw_type = tokens[..tokens.len() - 1].join(" ");
        return Some((
            binding_name,
            java_receiver_owner_from_type(&raw_type, import_bindings, local_type_names),
        ));
    }
    None
}

fn java_variable_declaration_receiver_bindings(
    node: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> Vec<(String, OptionalReceiverOwnerBinding)> {
    let declared_owner = node
        .child_by_field_name("type")
        .and_then(|type_node| trimmed_node_text(type_node, source))
        .and_then(|raw_type| {
            java_receiver_owner_from_type(&raw_type, import_bindings, local_type_names)
        });
    let declared_is_var = node
        .child_by_field_name("type")
        .and_then(|type_node| trimmed_node_text(type_node, source))
        .is_some_and(|raw_type| raw_type.trim() == "var");
    let mut bindings = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "variable_declarator" {
            continue;
        }
        let Some(name) = child
            .child_by_field_name("name")
            .and_then(|name_node| trimmed_node_text(name_node, source))
            .as_deref()
            .and_then(normalize_parameter_name)
        else {
            continue;
        };
        let owner = if declared_is_var {
            child.child_by_field_name("value").and_then(|value| {
                java_direct_new_owner(value, source, import_bindings, local_type_names)
            })
        } else {
            declared_owner.clone()
        };
        bindings.push((name, owner));
    }
    bindings
}

fn java_receiver_owner_from_type(
    raw_type: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> OptionalReceiverOwnerBinding {
    let owner_name = normalize_type_surface(raw_type)?;
    if owner_name == "var" {
        return None;
    }
    let owner_module = if local_type_names.contains(&owner_name) {
        None
    } else if let Some(module_name) = java_qualified_type_module_name(raw_type) {
        Some(module_name)
    } else {
        import_bindings.get(&owner_name).cloned()
    };
    Some((owner_name, owner_module))
}

fn java_static_receiver_owner(
    receiver_name: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> OptionalReceiverOwnerBinding {
    let owner_name = normalize_type_surface(receiver_name)?;
    if local_type_names.contains(&owner_name) {
        return Some((owner_name, None));
    }
    if let Some(module_name) = import_bindings.get(&owner_name) {
        return Some((owner_name, Some(module_name.clone())));
    }
    if let Some(module_name) = java_qualified_type_module_name(receiver_name)
        && owner_name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_uppercase())
    {
        return Some((owner_name, Some(module_name)));
    }
    None
}

fn java_direct_new_owner(
    value: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
    local_type_names: &HashSet<String>,
) -> OptionalReceiverOwnerBinding {
    if value.kind() != "object_creation_expression" {
        return None;
    }
    value
        .child_by_field_name("type")
        .and_then(|type_node| trimmed_node_text(type_node, source))
        .and_then(|raw_type| {
            java_receiver_owner_from_type(&raw_type, import_bindings, local_type_names)
        })
}

fn java_qualified_type_module_name(raw_type: &str) -> Option<String> {
    let base = raw_type
        .trim()
        .split(['<', '['])
        .next()
        .unwrap_or(raw_type)
        .trim();
    if !base.contains('.') || base.contains('*') || base.split_whitespace().count() != 1 {
        return None;
    }
    Some(base.to_string())
}

fn java_local_binding_visible_at_call(binding: TsNode<'_>, call_node: TsNode<'_>) -> bool {
    let Some(binding_scope) = java_lexical_scope(binding) else {
        return false;
    };
    let Some(call_scope) = java_lexical_scope(call_node) else {
        return false;
    };
    node_is_same_or_ancestor(binding_scope, call_scope)
}

fn java_lexical_scope(node: TsNode<'_>) -> Option<TsNode<'_>> {
    enclosing_node_with_kind(node, &["block"])
}

fn collect_java_import_type_bindings(root: TsNode<'_>, source: &str) -> HashMap<String, String> {
    let local_type_names = collect_java_top_level_type_names(root, source);
    let mut bindings = HashMap::new();
    let mut duplicates = HashSet::new();
    let mut cursor = root.walk();

    for child in root.named_children(&mut cursor) {
        if child.kind() != "import_declaration" {
            continue;
        }
        let Some(module_name) = java_import_type_module_name(child, source) else {
            continue;
        };
        let Some(local_name) = module_name
            .rsplit('.')
            .next()
            .and_then(normalize_parameter_name)
        else {
            continue;
        };
        if local_type_names.contains(&local_name) || duplicates.contains(&local_name) {
            continue;
        }
        if bindings.contains_key(&local_name) {
            bindings.remove(&local_name);
            duplicates.insert(local_name);
            continue;
        }
        bindings.insert(local_name, module_name);
    }

    bindings
}

fn collect_java_top_level_type_names(root: TsNode<'_>, source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if java_type_declaration_kind(child.kind())
            && let Some(name) = declaration_name(child, source)
        {
            names.insert(name);
        }
    }
    names
}

fn java_type_declaration_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration"
            | "interface_declaration"
            | "record_declaration"
            | "enum_declaration"
            | "annotation_type_declaration"
    )
}

fn java_import_type_module_name(import_node: TsNode<'_>, source: &str) -> Option<String> {
    let statement = trimmed_node_text(import_node, source)?;
    let rest = statement.strip_prefix("import")?.trim();
    let module_name = rest.trim_end_matches(';').trim();
    if module_name.starts_with("static ") || module_name.ends_with(".*") {
        return None;
    }
    if !module_name.contains('.')
        || module_name.contains('*')
        || module_name.contains('|')
        || module_name.split_whitespace().count() != 1
    {
        return None;
    }
    Some(module_name.to_string())
}

/// Receiver and member of one Java member call, read from the grammar.
///
/// Was `lib.rs::java_member_call`.
fn member_call(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    if node.kind() != "method_invocation" {
        return None;
    }
    let receiver = node.child_by_field_name("object")?;
    let method = node.child_by_field_name("name")?;
    Some((
        normalized_receiver_variable(receiver, source)?,
        trimmed_node_text(method, source)?,
    ))
}
