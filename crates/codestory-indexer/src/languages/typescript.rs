//! TypeScript extraction rules.
//!
//! TypeScript's graph extraction lives here: the tree-sitter graph and tags
//! rule files, the compiled-rule cache, the member-call callsite marker, and
//! the receiver-call resolution engine that turns `repository.save(...)` into
//! an edge aimed at `Repository.save`. Every language-keyed dispatch in the
//! crate reaches it through [`super::EXTRACTIONS`] rather than by spelling
//! `"typescript"`.
//!
//! Three TypeScript-adjacent surfaces are deliberately *not* here, and none of
//! them is TypeScript content this package owns:
//!
//! * TSX. It is its own rollback unit (#1682) with its own grammar, graph rule
//!   file, compiled-rule cache and dispatch name. Until it lands, the residual
//!   `"tsx"` arms in `lib.rs` and `language_configs.rs` keep answering for it,
//!   and the one arm that shared TypeScript's receiver-call engine now calls
//!   into this module instead of a `lib.rs` local. Absorbing TSX here would
//!   merge two rollback units and silently hand `.tsx` files TypeScript's
//!   grammar.
//! * `TypeScriptSemanticResolver`, which stays in `semantic::typescript`
//!   because the resolver types are private to that module. The registry row
//!   records the choice (`uses_generic_semantic_resolver: false`) and
//!   `semantic::dedicated_semantic_resolver` still constructs it.
//! * `lib.rs`'s Express/React/SvelteKit route collectors and the
//!   `JavaScriptDialect::TypeScript` selector. The per-language route
//!   collectors take non-uniform arguments and per-framework preconditions, so
//!   routing them through the registry is one change for all sixteen
//!   languages, not part of TypeScript's rollback unit.
//!
//! Two functions below are `pub(crate)` because JavaScript's still-unmigrated
//! receiver-call engine in `lib.rs` calls them
//! (`collect_typescript_imported_type_bindings` and
//! `typescript_property_belongs_to_owner`). They are TypeScript rules that
//! JavaScript borrows rather than shared crate-root helpers, so they move with
//! the rest and JavaScript reaches them by path until #1680 lands.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both TypeScript fixtures so the move stays
//! output-equal.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, ImportedTypeBinding, LanguageRuleset, ManualReceiverCallSpec,
    ManualReceiverSource, OptionalReceiverOwnerBinding, ReceiverCallSiteKey, ReceiverOwnerBinding,
    collect_colon_parameter_types, collect_receiver_call_specs_in_callable, declaration_name,
    enclosing_node_with_kind, js_like_callable_source_name, js_ts_local_binding_visible_at_call,
    js_ts_visible_local_type_name, member_call_method_col,
    normalize_js_ts_private_receiver_surface, normalize_parameter_name, normalize_type_surface,
    normalized_receiver_variable, parameter_name_before_colon, parameter_type_after_colon,
    receiver_call_belongs_to_callable, receiver_callsite_key, same_ts_span,
    signature_parameter_surface, split_top_level_parameters, trimmed_node_text, ts_node_graph_span,
    walk_tree_nodes,
};

/// Callsite marker written onto edges produced from TypeScript member-call
/// syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:ts-member-call";

const GRAPH_QUERY: &str = include_str!("../../rules/typescript.graph.scm");

/// TSX reuses TypeScript's tags query verbatim, exactly as `lib.rs` did with
/// `TSX_TAGS_QUERY = TYPESCRIPT_TAGS_QUERY`; #1682 takes ownership of that
/// reference when TSX moves.
pub(crate) const TAGS_QUERY: &str = include_str!("../../rules/typescript.tags.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for TypeScript.
///
/// `tsx` is deliberately absent from both `dispatch_names` and `extensions`:
/// the contracts registry routes `.tsx` to the `typescript` language name, but
/// the indexer parses it with a different grammar and a different rule file,
/// and that row belongs to #1682.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["typescript"],
    language_name: "typescript",
    extensions: &["ts", "mts", "cts"],
    ruleset: LanguageRuleset::TypeScript,
    parser_language: typescript_language,
    graph_query: GRAPH_QUERY,
    tags_query: Some(TAGS_QUERY),
    compiled_rules: &RULES,
    member_edge_specs: None,
    receiver_call_specs: Some(receiver_call_specs),
    type_usage_specs: None,
    callsite_marker_families: &[("ts_member", MEMBER_CALLSITE_MARKER)],
    // TypeScript's rule file already projects class members as METHOD, so the
    // qualified-name traversal must not promote FUNCTION children on top of
    // it. `false` is the value the god file's match had.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: ".",
    route_comments_are_c_style: true,
    // `semantic::TypeScriptSemanticResolver` is a dedicated resolver whose type
    // is private to that module; the registry records the choice and the
    // residual match there constructs it.
    uses_generic_semantic_resolver: false,
    semantic_family: "webscript",
};

fn typescript_language() -> tree_sitter::Language {
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
}

/// Manual receiver-call edges for one parsed TypeScript file.
///
/// Was `lib.rs::collect_typescript_receiver_call_edges`.
pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let imported_type_bindings =
        collect_typescript_imported_type_bindings(tree.root_node(), source);
    let namespace_import_bindings =
        collect_typescript_namespace_import_bindings(tree.root_node(), source);
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if !matches!(
            callable.kind(),
            "function_declaration" | "method_definition" | "arrow_function"
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
        collect_typescript_constructor_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &imported_type_bindings,
            &namespace_import_bindings,
            &mut local_receiver_callsites,
            &mut edges,
        );
        let parameter_receiver_types = collect_colon_parameter_types(callable, source);
        let mut receiver_types = parameter_receiver_types.clone();
        if let Some(owner_name) = enclosing_node_with_kind(callable, &["class_declaration"])
            .and_then(|owner| declaration_name(owner, source))
            && callable.kind() == "method_definition"
        {
            receiver_types.insert("this".to_string(), owner_name);
        }
        let property_receiver_types = collect_typescript_class_property_receiver_bindings(
            callable,
            source,
            &imported_type_bindings,
            &namespace_import_bindings,
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
        let mut receiver_modules =
            collect_typescript_parameter_type_modules(callable, source, &namespace_import_bindings);
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
            if parameter_receiver_types.contains_key(&spec.receiver_name)
                && let Some(binding) = imported_type_bindings.get(&spec.owner_name)
            {
                spec.owner_name = binding.owner_name.clone();
                spec.owner_module = Some(binding.module_name.clone());
            } else if let Some(module_name) = receiver_modules.get(&spec.receiver_name) {
                spec.owner_module = Some(module_name.clone());
            }
        }
        edges.extend(fallback_specs);
    });
    edges
}

fn collect_typescript_constructor_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_import_bindings: &HashMap<String, String>,
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
        let Some(owner) = typescript_visible_local_constructor_receiver_owner(
            callable,
            node,
            &receiver_name,
            source,
            imported_type_bindings,
            namespace_import_bindings,
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
                binding_marker: None,
                required_callsite_marker: None,
                class_anchored: false,
                owner_is_syntactic: false,
            });
        }
    });
}

fn typescript_visible_local_constructor_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_import_bindings: &HashMap<String, String>,
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
            typescript_constructor_receiver_owner(
                node,
                callable,
                source,
                imported_type_bindings,
                namespace_import_bindings,
            ),
        ));
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner)| owner)
}

fn typescript_constructor_receiver_owner(
    node: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_import_bindings: &HashMap<String, String>,
) -> OptionalReceiverOwnerBinding {
    let constructor_type = node
        .child_by_field_name("value")
        .and_then(|value_node| typescript_new_expression_constructor_type(value_node, source))?;
    let owner_name = normalize_type_surface(&constructor_type)?;
    if typescript_type_import_qualifier(&constructor_type).is_none()
        && js_ts_visible_local_type_name(callable, node, &owner_name, source)
    {
        return Some((owner_name, None));
    }
    typescript_receiver_owner_from_type(
        &constructor_type,
        imported_type_bindings,
        namespace_import_bindings,
    )
}

fn typescript_new_expression_constructor_type(node: TsNode<'_>, source: &str) -> Option<String> {
    if node.kind() != "new_expression" {
        return None;
    }
    node.child_by_field_name("constructor")
        .and_then(|constructor| trimmed_node_text(constructor, source))
        .map(|constructor| constructor.trim().to_string())
        .filter(|constructor| !constructor.is_empty())
}

fn collect_typescript_class_property_receiver_bindings(
    callable: TsNode<'_>,
    source: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_import_bindings: &HashMap<String, String>,
) -> HashMap<String, ReceiverOwnerBinding> {
    let mut receiver_bindings = HashMap::new();
    if callable.kind() != "method_definition" {
        return receiver_bindings;
    }
    let Some(class_node) = enclosing_node_with_kind(callable, &["class_declaration"]) else {
        return receiver_bindings;
    };
    let mut candidates: HashMap<String, Vec<ReceiverOwnerBinding>> = HashMap::new();
    walk_tree_nodes(class_node, &mut |node| {
        if node.kind() != "public_field_definition"
            || !typescript_property_belongs_to_owner(node, class_node)
        {
            return;
        }
        let Some((field_name, raw_type)) = typescript_property_receiver_binding(node, source)
        else {
            return;
        };
        let Some(owner) = typescript_receiver_owner_from_type(
            &raw_type,
            imported_type_bindings,
            namespace_import_bindings,
        ) else {
            return;
        };
        candidates
            .entry(format!("this.{field_name}"))
            .or_default()
            .push(owner);
    });
    for (receiver_name, mut owners) in candidates {
        owners.sort();
        owners.dedup();
        if owners.len() == 1 {
            receiver_bindings.insert(receiver_name, owners.remove(0));
        }
    }
    receiver_bindings
}

pub(crate) fn typescript_property_belongs_to_owner(
    property: TsNode<'_>,
    class_node: TsNode<'_>,
) -> bool {
    let mut current = property.parent();
    while let Some(candidate) = current {
        if same_ts_span(candidate, class_node) {
            return true;
        }
        if matches!(candidate.kind(), "method_definition" | "class_declaration") {
            return false;
        }
        current = candidate.parent();
    }
    false
}

fn typescript_property_receiver_binding(
    node: TsNode<'_>,
    source: &str,
) -> Option<(String, String)> {
    let field_name = node
        .child_by_field_name("name")
        .and_then(|name| trimmed_node_text(name, source))
        .as_deref()
        .and_then(normalize_parameter_name)?;
    let surface = trimmed_node_text(node, source)?;
    let head = surface
        .split('=')
        .next()
        .unwrap_or(surface.as_str())
        .trim_end_matches(';')
        .trim();
    let (_, type_side) = head.split_once(':')?;
    Some((field_name, parameter_type_after_colon(type_side)))
}

fn typescript_receiver_owner_from_type(
    raw_type: &str,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_import_bindings: &HashMap<String, String>,
) -> OptionalReceiverOwnerBinding {
    let owner_name = normalize_type_surface(raw_type)?;
    if let Some(qualifier) = typescript_type_import_qualifier(raw_type) {
        let module_name = namespace_import_bindings.get(&qualifier)?;
        return Some((owner_name, Some(module_name.clone())));
    }
    if let Some(binding) = imported_type_bindings.get(&owner_name) {
        return Some((
            binding.owner_name.clone(),
            Some(binding.module_name.clone()),
        ));
    }
    Some((owner_name, None))
}

pub(crate) fn collect_typescript_imported_type_bindings(
    root: TsNode<'_>,
    source: &str,
) -> HashMap<String, ImportedTypeBinding> {
    let top_level_bindings = collect_typescript_top_level_type_binding_names(root, source);
    let mut bindings = HashMap::new();
    let mut duplicates = HashSet::new();
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        if statement.kind() != "import_statement" {
            continue;
        }
        let Some(module_name) = statement
            .child_by_field_name("source")
            .and_then(|module| trimmed_node_text(module, source))
        else {
            continue;
        };
        for (owner_name, local_name) in typescript_import_binding_names(statement, source) {
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
                    module_name: module_name.clone(),
                    owner_name,
                },
            );
        }
    }
    bindings
}

fn typescript_import_binding_names(statement: TsNode<'_>, source: &str) -> Vec<(String, String)> {
    let mut bindings = Vec::new();
    let mut cursor = statement.walk();
    for child in statement.named_children(&mut cursor) {
        if child.kind() != "import_clause" {
            continue;
        }
        let mut import_clause_cursor = child.walk();
        for clause_child in child.named_children(&mut import_clause_cursor) {
            match clause_child.kind() {
                "identifier" => {
                    if let Some(local_name) = trimmed_node_text(clause_child, source)
                        .and_then(|name| normalize_parameter_name(&name))
                    {
                        bindings.push((local_name.clone(), local_name));
                    }
                }
                "named_imports" => {
                    let mut named_cursor = clause_child.walk();
                    for import_specifier in clause_child.named_children(&mut named_cursor) {
                        if import_specifier.kind() == "import_specifier"
                            && let Some(binding) =
                                typescript_import_specifier_binding_names(import_specifier, source)
                        {
                            bindings.push(binding);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    bindings
}

fn collect_typescript_namespace_import_bindings(
    root: TsNode<'_>,
    source: &str,
) -> HashMap<String, String> {
    let top_level_bindings = collect_typescript_top_level_type_binding_names(root, source);
    let import_local_bindings = collect_typescript_import_local_binding_names(root, source);
    let mut bindings = HashMap::new();
    let mut duplicates = HashSet::new();
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        if statement.kind() != "import_statement" {
            continue;
        }
        let Some(module_name) = statement
            .child_by_field_name("source")
            .and_then(|module| trimmed_node_text(module, source))
        else {
            continue;
        };
        for local_name in typescript_namespace_import_names(statement, source) {
            if top_level_bindings.contains(&local_name)
                || import_local_bindings.contains(&local_name)
            {
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
            bindings.insert(local_name, module_name.clone());
        }
    }
    bindings
}

fn collect_typescript_import_local_binding_names(
    root: TsNode<'_>,
    source: &str,
) -> HashSet<String> {
    let mut bindings = HashSet::new();
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        if statement.kind() != "import_statement" {
            continue;
        }
        for (_, local_name) in typescript_import_binding_names(statement, source) {
            bindings.insert(local_name);
        }
    }
    bindings
}

fn typescript_namespace_import_names(statement: TsNode<'_>, source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = statement.walk();
    for child in statement.named_children(&mut cursor) {
        if child.kind() != "import_clause" {
            continue;
        }
        let mut import_clause_cursor = child.walk();
        for clause_child in child.named_children(&mut import_clause_cursor) {
            if clause_child.kind() != "namespace_import" {
                continue;
            }
            let mut namespace_cursor = clause_child.walk();
            for namespace_child in clause_child.named_children(&mut namespace_cursor) {
                if namespace_child.kind() == "identifier"
                    && let Some(local_name) = trimmed_node_text(namespace_child, source)
                        .and_then(|name| normalize_parameter_name(&name))
                {
                    names.push(local_name);
                }
            }
        }
    }
    names
}

fn typescript_import_specifier_binding_names(
    import_specifier: TsNode<'_>,
    source: &str,
) -> Option<(String, String)> {
    let name_node = import_specifier.child_by_field_name("name")?;
    let owner_name =
        trimmed_node_text(name_node, source).and_then(|name| normalize_parameter_name(&name))?;
    let local_node = import_specifier
        .child_by_field_name("alias")
        .unwrap_or(name_node);
    let local_name =
        trimmed_node_text(local_node, source).and_then(|name| normalize_parameter_name(&name))?;
    Some((owner_name, local_name))
}

fn collect_typescript_top_level_type_binding_names(
    root: TsNode<'_>,
    source: &str,
) -> HashSet<String> {
    let mut bindings = HashSet::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        collect_typescript_top_level_type_binding_name(child, source, &mut bindings);
    }
    bindings
}

fn collect_typescript_top_level_type_binding_name(
    node: TsNode<'_>,
    source: &str,
    bindings: &mut HashSet<String>,
) {
    match node.kind() {
        "class_declaration"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration" => {
            if let Some(name) =
                declaration_name(node, source).and_then(|name| normalize_parameter_name(&name))
            {
                bindings.insert(name);
            }
        }
        "export_statement" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_typescript_top_level_type_binding_name(child, source, bindings);
            }
        }
        _ => {}
    }
}

fn collect_typescript_parameter_type_modules(
    callable: TsNode<'_>,
    source: &str,
    namespace_import_bindings: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut receiver_modules = HashMap::new();
    let Some(parameters) = signature_parameter_surface(callable, source) else {
        return receiver_modules;
    };
    for parameter in split_top_level_parameters(&parameters) {
        let Some((name_side, type_side)) = parameter.split_once(':') else {
            continue;
        };
        let Some(receiver_name) = parameter_name_before_colon(name_side) else {
            continue;
        };
        let Some(qualifier) =
            typescript_type_import_qualifier(&parameter_type_after_colon(type_side))
        else {
            continue;
        };
        let Some(module_name) = namespace_import_bindings.get(&qualifier) else {
            continue;
        };
        receiver_modules.insert(receiver_name, module_name.clone());
    }
    receiver_modules
}

fn typescript_type_import_qualifier(raw_type: &str) -> Option<String> {
    if raw_type.contains('|') || raw_type.contains('&') {
        return None;
    }
    let mut surface = raw_type.trim().trim_end_matches('?').trim();
    while let Some(stripped) = surface.strip_prefix("readonly") {
        surface = stripped.trim_start();
    }
    let base = surface
        .split(['<', '[', '('])
        .next()
        .unwrap_or(surface)
        .trim();
    let (qualifier, _) = base.rsplit_once('.')?;
    normalize_parameter_name(qualifier)
}

/// Receiver and member of one TypeScript member call, read from the grammar.
///
/// Was `lib.rs::typescript_member_call`.
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
