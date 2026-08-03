//! C# extraction rules.
//!
//! C#'s graph extraction lives here: the tree-sitter rule file, the
//! compiled-rule cache, the member-call callsite marker, and the receiver-call
//! resolution engine that turns `repo.Save(...)` into an edge aimed at
//! `Repository.Save`. Every language-keyed dispatch in the crate reaches it
//! through [`super::EXTRACTIONS`] rather than by spelling `"csharp"`.
//!
//! Five C# surfaces are deliberately *not* here, and all five are shared seams
//! rather than C# content:
//!
//! * `lib.rs::collect_aspnet_route` and its `"csharp"` arm in the
//!   framework-route scanner. The per-language route collectors take
//!   non-uniform arguments and a per-framework `has_<framework>` precondition,
//!   so routing them through the registry is one change for all sixteen
//!   languages, not part of C#'s rollback unit.
//! * `lib.rs::language_member_specs`'s `"csharp"` arm, which asks the shared
//!   `collect_enclosing_type_member_edges` helper for type-to-method edges.
//!   Manual member edges are a cross-language dispatch with no
//!   [`super::LanguageExtraction`] field; giving it one is its own package.
//! * `lib.rs::text_only_language_name`'s `"cs" | "cshtml"` arm. That is the
//!   text-only fallback name for files the parser never sees (Razor companions
//!   and oversized sources), not the parser-backed extension claim this row
//!   makes.
//! * `semantic::CSharpSemanticResolver`, which stays behind
//!   `semantic::dedicated_semantic_resolver` because the resolver types are
//!   private to that module. The registry records the choice through
//!   `uses_generic_semantic_resolver: false`.
//! * `LanguageRuleset::CSharp`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both C# fixtures so the move stays output-equal.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, ImportedTypeBinding, LanguageRuleset, ManualMemberEdgeSpec,
    ManualReceiverCallSpec, ManualReceiverSource, OptionalReceiverOwnerBinding,
    ReceiverCallSiteKey, collect_enclosing_type_member_edges,
    collect_receiver_call_specs_in_callable, declaration_name, descendant_by_field_name,
    enclosing_node_with_kind, first_descendant_with_kind, member_call_method_col,
    node_is_same_or_ancestor, normalize_parameter_name, normalize_type_surface,
    normalized_receiver_variable, receiver_call_belongs_to_callable, receiver_callsite_key,
    same_ts_span, trimmed_node_text, ts_node_graph_span, walk_tree_nodes,
};

/// Callsite marker written onto edges produced from C# member-call syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:csharp-member-call";

const GRAPH_QUERY: &str = include_str!("../../rules/csharp.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for C#.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["csharp"],
    language_name: "csharp",
    // `cshtml` is a companion Razor extension, not a parser-backed C# claim;
    // `language_support_profile_for_ext` routes only `cs` to this language.
    extensions: &["cs"],
    ruleset: LanguageRuleset::CSharp,
    parser_language: csharp_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    member_edge_specs: Some(member_edge_specs),
    receiver_call_specs: Some(receiver_call_specs),
    member_callsite_marker: Some(MEMBER_CALLSITE_MARKER),
    graph_call_syntax: Some("csharp_member"),
    // C# spells a method `method_declaration`, so the rule file already emits
    // METHOD and there is nothing to promote; only Kotlin, Swift and Dart need
    // the promotion, because their member functions share
    // `function_declaration` with free functions. `false` is what `lib.rs`
    // answered before the move.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: ".",
    route_comments_are_c_style: true,
    // `semantic::CSharpSemanticResolver` is a dedicated resolver, not the
    // shared name-only one, and its type is private to `semantic`.
    uses_generic_semantic_resolver: false,
    semantic_family: "csharp",
};

fn csharp_language() -> tree_sitter::Language {
    tree_sitter_c_sharp::LANGUAGE.into()
}

/// Manual receiver-call edges for one parsed C# file.
///
/// Was `lib.rs::collect_csharp_receiver_call_edges`.
pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let root = tree.root_node();
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if callable.kind() != "method_declaration" {
            return;
        }
        let Some(source_name) = declaration_name(callable, source) else {
            return;
        };
        let visible_type_names = collect_csharp_visible_type_binding_names(root, callable, source);
        let imported_type_bindings = collect_csharp_visible_imported_type_bindings(
            root,
            callable,
            source,
            &visible_type_names,
        );
        let namespace_imports = collect_csharp_visible_namespace_imports(root, callable, source);
        let receiver_types = collect_csharp_parameter_types(callable, source);
        let call_source = ManualReceiverSource {
            name: &source_name,
            span: ts_node_graph_span(callable),
        };
        let mut precise_receiver_callsites = HashSet::new();
        let receiver_context = CsharpReceiverContext {
            visible_type_names: &visible_type_names,
            imported_type_bindings: &imported_type_bindings,
            namespace_imports: &namespace_imports,
            parameter_receiver_types: &receiver_types,
        };
        collect_csharp_precise_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            receiver_context,
            &mut precise_receiver_callsites,
            &mut edges,
        );
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
            .retain(|spec| !precise_receiver_callsites.contains(&receiver_callsite_key(spec)));
        for spec in &mut parameter_specs {
            if let Some(binding) = imported_type_bindings.get(&spec.owner_name) {
                spec.owner_name = binding.owner_name.clone();
                spec.owner_module = Some(binding.module_name.clone());
            } else if !visible_type_names.contains(&spec.owner_name)
                && let Some(module_name) =
                    csharp_plain_namespace_import_type_module(&spec.owner_name, &namespace_imports)
            {
                spec.owner_module = Some(module_name);
            }
        }
        edges.extend(parameter_specs);
    });
    edges
}

struct CsharpReceiverContext<'a> {
    visible_type_names: &'a HashSet<String>,
    imported_type_bindings: &'a HashMap<String, ImportedTypeBinding>,
    namespace_imports: &'a CsharpNamespaceImports,
    parameter_receiver_types: &'a HashMap<String, String>,
}

fn collect_csharp_precise_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    context: CsharpReceiverContext<'_>,
    precise_receiver_callsites: &mut HashSet<ReceiverCallSiteKey>,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    walk_tree_nodes(callable, &mut |node| {
        let Some((receiver_name, method_name)) = member_call(node, source) else {
            return;
        };
        if !receiver_call_belongs_to_callable(node, callable) {
            return;
        }
        let Some(owner) =
            csharp_visible_receiver_owner(callable, node, &receiver_name, source, &context)
        else {
            return;
        };
        let method_col = member_call_method_col(node, source, &method_name);
        precise_receiver_callsites.insert(ReceiverCallSiteKey {
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

fn csharp_visible_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    context: &CsharpReceiverContext<'_>,
) -> Option<OptionalReceiverOwnerBinding> {
    if let Some(owner) = csharp_self_receiver_owner(callable, receiver_name, source) {
        return Some(Some(owner));
    }
    if let Some(owner) = csharp_direct_new_owner_surface(
        receiver_name,
        context.visible_type_names,
        context.imported_type_bindings,
        context.namespace_imports,
    ) {
        return Some(Some(owner));
    }
    if let Some(owner) = csharp_visible_local_receiver_owner(
        callable,
        call_node,
        receiver_name,
        source,
        context.visible_type_names,
        context.imported_type_bindings,
        context.namespace_imports,
    ) {
        return Some(owner);
    }
    if !receiver_name.contains('.') && context.parameter_receiver_types.contains_key(receiver_name)
    {
        return None;
    }
    if let Some(owner) = csharp_field_receiver_owner(
        callable,
        receiver_name,
        source,
        context.visible_type_names,
        context.imported_type_bindings,
        context.namespace_imports,
    ) {
        return Some(Some(owner));
    }
    csharp_static_receiver_owner(
        receiver_name,
        context.visible_type_names,
        context.imported_type_bindings,
    )
    .map(Some)
}

fn csharp_self_receiver_owner(
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
            "struct_declaration",
            "interface_declaration",
            "record_declaration",
        ],
    )?;
    let owner_name = declaration_name(owner_node, source)?;
    Some((owner_name, None))
}

fn csharp_visible_local_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_imports: &CsharpNamespaceImports,
) -> Option<OptionalReceiverOwnerBinding> {
    let mut visible_bindings = Vec::new();
    walk_tree_nodes(callable, &mut |node| {
        if node.kind() != "local_declaration_statement" {
            return;
        }
        if !receiver_call_belongs_to_callable(node, callable)
            || node.end_byte() > call_node.start_byte()
            || !csharp_local_binding_visible_at_call(node, call_node)
        {
            return;
        }
        for (binding_name, owner) in csharp_local_declaration_receiver_bindings(
            node,
            source,
            visible_type_names,
            imported_type_bindings,
            namespace_imports,
        ) {
            if binding_name == receiver_name {
                visible_bindings.push((node.end_byte(), owner));
            }
        }
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner)| owner)
}

fn csharp_field_receiver_owner(
    callable: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_imports: &CsharpNamespaceImports,
) -> OptionalReceiverOwnerBinding {
    let field_name = receiver_name
        .strip_prefix("this.")
        .unwrap_or(receiver_name)
        .trim();
    let class_node = enclosing_node_with_kind(callable, &["class_declaration"])?;
    let mut field_bindings = Vec::new();
    walk_tree_nodes(class_node, &mut |node| {
        if node.kind() != "field_declaration" {
            return;
        }
        if !enclosing_node_with_kind(node, &["class_declaration"])
            .is_some_and(|owner| same_ts_span(owner, class_node))
        {
            return;
        }
        for (binding_name, owner) in csharp_field_declaration_receiver_bindings(
            node,
            source,
            visible_type_names,
            imported_type_bindings,
            namespace_imports,
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

fn csharp_field_declaration_receiver_bindings(
    node: TsNode<'_>,
    source: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_imports: &CsharpNamespaceImports,
) -> Vec<(String, OptionalReceiverOwnerBinding)> {
    let Some(variable_declaration) = first_descendant_with_kind(node, "variable_declaration")
    else {
        return Vec::new();
    };
    csharp_variable_declaration_receiver_bindings(
        variable_declaration,
        source,
        visible_type_names,
        imported_type_bindings,
        namespace_imports,
    )
}

fn csharp_local_declaration_receiver_bindings(
    node: TsNode<'_>,
    source: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_imports: &CsharpNamespaceImports,
) -> Vec<(String, OptionalReceiverOwnerBinding)> {
    let Some(variable_declaration) = first_descendant_with_kind(node, "variable_declaration")
    else {
        return Vec::new();
    };
    csharp_variable_declaration_receiver_bindings(
        variable_declaration,
        source,
        visible_type_names,
        imported_type_bindings,
        namespace_imports,
    )
}

fn csharp_variable_declaration_receiver_bindings(
    node: TsNode<'_>,
    source: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_imports: &CsharpNamespaceImports,
) -> Vec<(String, OptionalReceiverOwnerBinding)> {
    let declared_type = node
        .child_by_field_name("type")
        .and_then(|type_node| trimmed_node_text(type_node, source));
    let declared_owner = declared_type.as_deref().and_then(|raw_type| {
        csharp_receiver_owner_from_type(
            raw_type,
            visible_type_names,
            imported_type_bindings,
            namespace_imports,
            true,
        )
    });
    let declared_is_var = declared_type
        .as_deref()
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
            trimmed_node_text(child, source)
                .as_deref()
                .and_then(|surface| surface.split_once('='))
                .and_then(|(_, value)| {
                    csharp_direct_new_owner_surface(
                        value,
                        visible_type_names,
                        imported_type_bindings,
                        namespace_imports,
                    )
                })
        } else {
            declared_owner.clone()
        };
        bindings.push((name, owner));
    }
    bindings
}

fn csharp_receiver_owner_from_type(
    raw_type: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_imports: &CsharpNamespaceImports,
    allow_plain_namespace_import: bool,
) -> OptionalReceiverOwnerBinding {
    let owner_name = normalize_type_surface(raw_type)?;
    if owner_name == "var" {
        return None;
    }
    if let Some(module_name) = csharp_qualified_type_module_name(raw_type) {
        return Some((owner_name, Some(module_name)));
    }
    if visible_type_names.contains(&owner_name) {
        return Some((owner_name, None));
    }
    if let Some(binding) = imported_type_bindings.get(&owner_name) {
        return Some((
            binding.owner_name.clone(),
            Some(binding.module_name.clone()),
        ));
    }
    if allow_plain_namespace_import
        && let Some(module_name) =
            csharp_plain_namespace_import_type_module(&owner_name, namespace_imports)
    {
        return Some((owner_name, Some(module_name)));
    }
    Some((owner_name, None))
}

fn csharp_direct_new_owner_surface(
    surface: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
    namespace_imports: &CsharpNamespaceImports,
) -> OptionalReceiverOwnerBinding {
    let surface = surface.trim();
    let rest = surface.strip_prefix("new ")?;
    let type_surface = rest.split(['(', '{']).next().unwrap_or(rest).trim();
    csharp_receiver_owner_from_type(
        type_surface,
        visible_type_names,
        imported_type_bindings,
        namespace_imports,
        false,
    )
}

fn csharp_static_receiver_owner(
    receiver_name: &str,
    visible_type_names: &HashSet<String>,
    imported_type_bindings: &HashMap<String, ImportedTypeBinding>,
) -> OptionalReceiverOwnerBinding {
    let owner_name = normalize_type_surface(receiver_name)?;
    if let Some(module_name) = csharp_qualified_type_module_name(receiver_name)
        && owner_name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_uppercase())
    {
        return Some((owner_name, Some(module_name)));
    }
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

fn csharp_qualified_type_module_name(raw_type: &str) -> Option<String> {
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

fn csharp_local_binding_visible_at_call(binding: TsNode<'_>, call_node: TsNode<'_>) -> bool {
    let Some(binding_scope) = csharp_lexical_scope(binding) else {
        return false;
    };
    let Some(call_scope) = csharp_lexical_scope(call_node) else {
        return false;
    };
    node_is_same_or_ancestor(binding_scope, call_scope)
}

fn csharp_lexical_scope(node: TsNode<'_>) -> Option<TsNode<'_>> {
    enclosing_node_with_kind(node, &["block"])
}

fn collect_csharp_visible_imported_type_bindings(
    root: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
    visible_type_names: &HashSet<String>,
) -> HashMap<String, ImportedTypeBinding> {
    let mut bindings = HashMap::new();
    let mut duplicates = HashSet::new();

    collect_csharp_imported_type_bindings_in_scope(
        root,
        source,
        visible_type_names,
        &mut bindings,
        &mut duplicates,
    );
    if let Some(namespace) = enclosing_node_with_kind(callable, &["namespace_declaration"])
        && let Some(body) = namespace.child_by_field_name("body")
    {
        collect_csharp_imported_type_bindings_in_scope(
            body,
            source,
            visible_type_names,
            &mut bindings,
            &mut duplicates,
        );
    }

    bindings
}

fn collect_csharp_imported_type_bindings_in_scope(
    scope: TsNode<'_>,
    source: &str,
    visible_type_names: &HashSet<String>,
    bindings: &mut HashMap<String, ImportedTypeBinding>,
    duplicates: &mut HashSet<String>,
) {
    let mut cursor = scope.walk();
    for statement in scope.named_children(&mut cursor) {
        if statement.kind() != "using_directive" {
            continue;
        }
        let Some((owner_name, local_name, module_name)) =
            csharp_import_type_binding_names(statement, source)
        else {
            continue;
        };
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

fn collect_csharp_visible_namespace_imports(
    root: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
) -> CsharpNamespaceImports {
    let mut imports = CsharpNamespaceImports::default();
    collect_csharp_namespace_imports_in_scope(root, source, &mut imports);
    if let Some(namespace) = enclosing_node_with_kind(callable, &["namespace_declaration"])
        && let Some(body) = namespace.child_by_field_name("body")
    {
        collect_csharp_namespace_imports_in_scope(body, source, &mut imports);
    }
    imports
}

#[derive(Default)]
struct CsharpNamespaceImports {
    plain_import_count: usize,
    module_candidates: HashSet<String>,
}

fn collect_csharp_namespace_imports_in_scope(
    scope: TsNode<'_>,
    source: &str,
    imports: &mut CsharpNamespaceImports,
) {
    let mut cursor = scope.walk();
    for statement in scope.named_children(&mut cursor) {
        if statement.kind() != "using_directive" {
            continue;
        }
        if let Some(namespace_name) = csharp_namespace_import_name(statement, source) {
            imports.plain_import_count = imports.plain_import_count.saturating_add(1);
            if namespace_name.contains('.') {
                imports.module_candidates.insert(namespace_name);
            }
        }
    }
}

fn collect_csharp_visible_type_binding_names(
    root: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_csharp_type_binding_names_in_scope(root, source, &mut names);
    if let Some(namespace) = enclosing_node_with_kind(callable, &["namespace_declaration"])
        && let Some(body) = namespace.child_by_field_name("body")
    {
        collect_csharp_type_binding_names_in_scope(body, source, &mut names);
    }
    names
}

fn collect_csharp_type_binding_names_in_scope(
    scope: TsNode<'_>,
    source: &str,
    names: &mut HashSet<String>,
) {
    let mut cursor = scope.walk();
    for child in scope.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_declaration" | "interface_declaration" | "struct_declaration"
        ) && let Some(name) = declaration_name(child, source)
        {
            names.insert(name);
        }
    }
}

fn csharp_import_type_binding_names(
    statement: TsNode<'_>,
    source: &str,
) -> Option<(String, String, String)> {
    let surface = trimmed_node_text(statement, source)?;
    let rest = surface
        .strip_prefix("global ")
        .unwrap_or(surface.as_str())
        .strip_prefix("using")?
        .trim()
        .trim_end_matches(';')
        .trim();
    if rest.starts_with("static ") {
        return None;
    }
    let (alias_surface, module_surface) = rest.split_once('=')?;
    let local_name = normalize_parameter_name(alias_surface.trim())?;
    let module_name = module_surface.trim();
    if !module_name.contains('.') || module_name.contains('*') || module_name.contains('|') {
        return None;
    }
    if module_name.split_whitespace().count() != 1 {
        return None;
    }
    let owner_name = module_name
        .rsplit('.')
        .next()
        .and_then(normalize_parameter_name)?;
    Some((owner_name, local_name, module_name.to_string()))
}

fn csharp_namespace_import_name(statement: TsNode<'_>, source: &str) -> Option<String> {
    let surface = trimmed_node_text(statement, source)?;
    let rest = surface
        .strip_prefix("global ")
        .unwrap_or(surface.as_str())
        .strip_prefix("using")?
        .trim()
        .trim_end_matches(';')
        .trim();
    if rest.starts_with("static ") || rest.contains('=') {
        return None;
    }
    if rest.contains('*') || rest.contains('|') {
        return None;
    }
    if rest.split_whitespace().count() != 1 {
        return None;
    }
    Some(rest.to_string())
}

fn csharp_plain_namespace_import_type_module(
    owner_name: &str,
    namespace_imports: &CsharpNamespaceImports,
) -> Option<String> {
    if namespace_imports.plain_import_count != 1
        || namespace_imports.module_candidates.len() != 1
        || owner_name.contains('.')
        || owner_name.contains('|')
        || owner_name.trim().is_empty()
    {
        return None;
    }
    let namespace_name = namespace_imports.module_candidates.iter().next()?;
    Some(format!("{namespace_name}.{owner_name}"))
}

fn collect_csharp_parameter_types(callable: TsNode<'_>, source: &str) -> HashMap<String, String> {
    let mut receiver_types = HashMap::new();
    let Some(parameters) = callable.child_by_field_name("parameters") else {
        return receiver_types;
    };
    walk_tree_nodes(parameters, &mut |node| {
        if node.kind() != "parameter" {
            return;
        }
        let Some(type_node) = descendant_by_field_name(node, "type") else {
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

/// Receiver and member of one C# member call.
///
/// Was `lib.rs::csharp_member_call`.
fn member_call(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    if node.kind() != "invocation_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    if function.kind() != "member_access_expression" {
        return None;
    }
    let receiver = function.child_by_field_name("expression")?;
    let method = function.child_by_field_name("name")?;
    Some((
        normalized_receiver_variable(receiver, source)?,
        trimmed_node_text(method, source)?,
    ))
}

/// The manual MEMBER-edge collector this language had in `lib.rs`.
///
/// `language_member_specs` consults the registry before its residual
/// `match`, so once this row exists the old arm is unreachable. Leaving the
/// field `None` would therefore drop csharp's MEMBER edges silently, with
/// nothing in the arm itself to show it had stopped running.
pub(crate) fn member_edge_specs(tree: &Tree, source: &str) -> Vec<ManualMemberEdgeSpec> {
    collect_enclosing_type_member_edges(
        tree,
        source,
        &[
            "class_declaration",
            "interface_declaration",
            "struct_declaration",
        ],
        &["method_declaration"],
    )
}
