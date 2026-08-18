//! Swift extraction rules.
//!
//! Swift's graph extraction lives here: the tree-sitter rule file, the
//! compiled-rule cache, the member-call callsite marker, and the receiver-call
//! resolution engine that turns `repository.save(...)` into an edge aimed at
//! `Repository.save`. Every language-keyed dispatch in the crate reaches it
//! through [`super::EXTRACTIONS`] rather than by spelling `"swift"`.
//!
//! Three Swift surfaces are deliberately *not* here, and all three are shared
//! seams rather than Swift content:
//!
//! * `lib.rs::collect_vapor_route` and its `"swift"` arm in the
//!   framework-route scanner. The per-language route collectors take
//!   non-uniform arguments and a per-framework `has_<framework>` precondition,
//!   so routing them through the registry is one change for all sixteen
//!   languages, not part of Swift's rollback unit.
//! * `LanguageRuleset::Swift`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//! * `resolution::find_swift_imported_owner_member_readonly` and the
//!   `Sources/<Module>` path matching it uses. That is import resolution over
//!   an already-built graph, owned by the resolver, not extraction.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both Swift fixtures so the move stays output-equal.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, LanguageRuleset, ManualReceiverCallSpec, ManualReceiverSource,
    OptionalReceiverOwnerBinding, ReceiverCallSiteKey, collect_colon_parameter_types,
    collect_receiver_call_specs_in_callable, declaration_name, enclosing_node_with_kind,
    member_call_method_col, node_is_same_or_ancestor, normalize_parameter_name,
    normalize_type_surface, parameter_name_before_colon, parameter_type_after_colon,
    receiver_call_belongs_to_callable, receiver_callsite_key, same_ts_span,
    signature_parameter_surface, split_top_level_parameters, trimmed_node_text, ts_node_graph_span,
    walk_tree_nodes,
};

/// Callsite marker written onto edges produced from Swift member-call syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:swift-member-call";

const GRAPH_QUERY: &str = include_str!("../../rules/swift.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for Swift.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["swift"],
    language_name: "swift",
    extensions: &["swift"],
    ruleset: LanguageRuleset::Swift,
    parser_language: swift_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    member_edge_specs: None,
    receiver_call_specs: Some(receiver_call_specs),
    type_usage_specs: None,
    callsite_marker_families: &[("swift_member", MEMBER_CALLSITE_MARKER)],
    // A `function_declaration` whose owner is type-like is a method in Swift;
    // the projection promotes FUNCTION to METHOD for exactly these languages.
    promotes_type_member_functions_to_methods: true,
    qualified_name_delimiter: ".",
    // Swift *does* use `//` and `/* */`, but the framework-route scanner has
    // never stripped them for Swift: its roster is a separate fact with its own
    // owner, and turning this on would change which Vapor route lines the
    // scanner claims. The CLI comment roster and this one disagree on purpose.
    route_comments_are_c_style: false,
    uses_generic_semantic_resolver: true,
    semantic_family: "swift",
};

fn swift_language() -> tree_sitter::Language {
    tree_sitter_swift::LANGUAGE.into()
}

/// Manual receiver-call edges for one parsed Swift file.
///
/// Was `lib.rs::collect_swift_receiver_call_edges`.
pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let imports = collect_swift_imports(source);
    let local_type_names = collect_swift_type_binding_names(tree.root_node(), source);
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if callable.kind() != "function_declaration" {
            return;
        }
        let Some(source_name) = declaration_name(callable, source) else {
            return;
        };
        let call_source = ManualReceiverSource {
            name: &source_name,
            span: ts_node_graph_span(callable),
        };
        let receiver_types = collect_colon_parameter_types(callable, source);
        let mut local_receiver_callsites = HashSet::new();
        collect_swift_precise_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            SwiftReceiverContext {
                parameter_receiver_types: &receiver_types,
                imports: &imports,
                local_type_names: &local_type_names,
            },
            &mut local_receiver_callsites,
            &mut edges,
        );
        if receiver_types.is_empty() {
            return;
        }
        let receiver_modules =
            collect_swift_parameter_type_modules(callable, source, &imports, &local_type_names);
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
            if let Some(module_name) = receiver_modules.get(&spec.receiver_name) {
                spec.owner_module = Some(module_name.clone());
            }
        }
        edges.extend(parameter_specs);
    });
    edges
}

struct SwiftReceiverContext<'a> {
    parameter_receiver_types: &'a HashMap<String, String>,
    imports: &'a SwiftImportContext,
    local_type_names: &'a HashSet<String>,
}

#[derive(Debug, Default)]
struct SwiftImportContext {
    whole_modules: HashSet<String>,
    scoped_types: HashSet<(String, String)>,
}

impl SwiftImportContext {
    fn type_is_imported(&self, module_name: &str, owner_name: &str) -> bool {
        self.whole_modules.contains(module_name)
            || self
                .scoped_types
                .iter()
                .any(|(module, owner)| module == module_name && owner == owner_name)
    }

    fn unqualified_owner_module(&self, owner_name: &str) -> Option<String> {
        let mut candidates = self
            .scoped_types
            .iter()
            .filter_map(|(module_name, imported_owner)| {
                (imported_owner == owner_name).then_some(module_name.clone())
            })
            .collect::<HashSet<_>>();
        if self.whole_modules.len() == 1
            && let Some(module_name) = self.whole_modules.iter().next()
        {
            candidates.insert(module_name.clone());
        }
        (candidates.len() == 1)
            .then(|| candidates.into_iter().next())
            .flatten()
    }
}

fn collect_swift_precise_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    context: SwiftReceiverContext<'_>,
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
            swift_visible_local_receiver_owner(callable, node, &receiver_name, source, &context)
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
                    binding_marker: None,
                    required_callsite_marker: None,
                    class_anchored: false,
                    owner_is_syntactic: false,
                });
            }
            return;
        }

        let owner = if let Some(owner) = swift_self_receiver_owner(callable, &receiver_name, source)
        {
            Some(owner)
        } else if !context
            .parameter_receiver_types
            .contains_key(&receiver_name)
        {
            swift_property_receiver_owner(callable, &receiver_name, source, &context)
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
            binding_marker: None,
            required_callsite_marker: None,
            class_anchored: false,
            owner_is_syntactic: false,
        });
    });
}

fn swift_self_receiver_owner(
    callable: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> OptionalReceiverOwnerBinding {
    if receiver_name != "self" {
        return None;
    }
    let owner_node = enclosing_node_with_kind(callable, &["class_declaration"])?;
    let owner_name = declaration_name(owner_node, source)?;
    Some((owner_name, None))
}

fn swift_visible_local_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    context: &SwiftReceiverContext<'_>,
) -> Option<OptionalReceiverOwnerBinding> {
    let mut visible_bindings = Vec::new();
    walk_tree_nodes(callable, &mut |node| {
        if node.kind() != "property_declaration" {
            return;
        }
        if !receiver_call_belongs_to_callable(node, callable)
            || node.end_byte() > call_node.start_byte()
        {
            return;
        }
        let Some(binding_name) = swift_property_binding_name(node, source) else {
            return;
        };
        if binding_name != receiver_name || !swift_local_binding_visible_at_call(node, call_node) {
            return;
        }
        visible_bindings.push((
            node.end_byte(),
            swift_initialized_constructor_owner(node, source, context),
        ));
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner_name)| owner_name)
}

fn swift_property_receiver_owner(
    callable: TsNode<'_>,
    receiver_name: &str,
    source: &str,
    context: &SwiftReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    let field_name = receiver_name
        .strip_prefix("self.")
        .unwrap_or(receiver_name)
        .trim();
    if field_name == "self" || field_name.contains('.') {
        return None;
    }
    let owner_node = enclosing_node_with_kind(callable, &["class_declaration"])?;
    let mut property_bindings = Vec::new();
    walk_tree_nodes(owner_node, &mut |node| {
        if node.kind() != "property_declaration"
            || !swift_property_belongs_to_owner(node, owner_node)
        {
            return;
        }
        let Some(binding_name) = swift_property_binding_name(node, source) else {
            return;
        };
        if binding_name != field_name {
            return;
        }
        if let Some(owner) = swift_typed_property_owner(node, source, context) {
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

fn swift_property_belongs_to_owner(property: TsNode<'_>, owner_node: TsNode<'_>) -> bool {
    let mut current = property.parent();
    while let Some(candidate) = current {
        if same_ts_span(candidate, owner_node) {
            return true;
        }
        if candidate.kind() == "function_declaration" || candidate.kind() == "class_declaration" {
            return false;
        }
        current = candidate.parent();
    }
    false
}

fn swift_property_binding_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .and_then(|name| trimmed_node_text(name, source))
        .as_deref()
        .and_then(|surface| {
            surface
                .split_whitespace()
                .filter(|token| !matches!(*token, "let" | "var"))
                .filter_map(normalize_parameter_name)
                .next_back()
        })
}

fn swift_initialized_constructor_owner(
    node: TsNode<'_>,
    source: &str,
    context: &SwiftReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    if let Some(owner_name) = node
        .child_by_field_name("value")
        .and_then(|value| swift_constructor_owner(value, source, context))
    {
        return Some(owner_name);
    }
    let surface = trimmed_node_text(node, source)?;
    let (_, value_surface) = surface.split_once('=')?;
    swift_constructor_owner_surface(value_surface, context)
}

fn swift_typed_property_owner(
    node: TsNode<'_>,
    source: &str,
    context: &SwiftReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    let surface = trimmed_node_text(node, source)?;
    let head = surface.split('=').next().unwrap_or(surface.as_str()).trim();
    let rest = head
        .rsplit_once(" let ")
        .map(|(_, rest)| rest)
        .or_else(|| head.rsplit_once(" var ").map(|(_, rest)| rest))
        .or_else(|| head.strip_prefix("let "))
        .or_else(|| head.strip_prefix("var "))?
        .trim();
    let (_, type_side) = rest.split_once(':')?;
    swift_receiver_owner_from_type(&parameter_type_after_colon(type_side), context)
}

fn swift_receiver_owner_from_type(
    raw_type: &str,
    context: &SwiftReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    let owner_name = normalize_type_surface(raw_type)?;
    if let Some(module_name) = swift_type_import_qualifier(raw_type) {
        if context.imports.type_is_imported(&module_name, &owner_name) {
            return Some((owner_name, Some(module_name)));
        }
        return None;
    }
    if context.local_type_names.contains(&owner_name) {
        return Some((owner_name, None));
    }
    if let Some(module_name) = context.imports.unqualified_owner_module(&owner_name) {
        return Some((owner_name, Some(module_name)));
    }
    Some((owner_name, None))
}

fn swift_constructor_owner(
    value: TsNode<'_>,
    source: &str,
    context: &SwiftReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    trimmed_node_text(value, source)
        .as_deref()
        .and_then(|surface| swift_constructor_owner_surface(surface, context))
}

fn swift_constructor_owner_surface(
    surface: &str,
    context: &SwiftReceiverContext<'_>,
) -> OptionalReceiverOwnerBinding {
    let surface = surface.trim().trim_end_matches(';').trim();
    let (constructor_name, _) = surface.split_once('(')?;
    swift_constructor_owner_from_type_surface(constructor_name, context)
}

fn swift_constructor_owner_from_type_surface(
    type_surface: &str,
    context: &SwiftReceiverContext<'_>,
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
    swift_receiver_owner_from_type(type_surface, context)
}

fn swift_local_binding_visible_at_call(binding: TsNode<'_>, call_node: TsNode<'_>) -> bool {
    let Some(binding_scope) = swift_lexical_scope(binding) else {
        return false;
    };
    let Some(call_scope) = swift_lexical_scope(call_node) else {
        return false;
    };
    node_is_same_or_ancestor(binding_scope, call_scope)
}

fn swift_lexical_scope(node: TsNode<'_>) -> Option<TsNode<'_>> {
    enclosing_node_with_kind(node, &["statements", "function_body"])
}

fn collect_swift_parameter_type_modules(
    callable: TsNode<'_>,
    source: &str,
    imports: &SwiftImportContext,
    local_type_names: &HashSet<String>,
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
        let raw_type = parameter_type_after_colon(type_side);
        let Some(owner_name) = normalize_type_surface(&raw_type) else {
            continue;
        };
        if let Some(module_name) = swift_type_import_qualifier(&raw_type) {
            if imports.type_is_imported(&module_name, &owner_name) {
                receiver_modules.insert(receiver_name, module_name);
            }
            continue;
        }
        if local_type_names.contains(&owner_name) {
            continue;
        }
        if let Some(module_name) = imports.unqualified_owner_module(&owner_name) {
            receiver_modules.insert(receiver_name, module_name);
        }
    }
    receiver_modules
}

fn collect_swift_imports(source: &str) -> SwiftImportContext {
    let mut imports = SwiftImportContext::default();
    for swift_import in source.lines().filter_map(swift_import_from_line) {
        match swift_import {
            SwiftImport::WholeModule(module_name) => {
                imports.whole_modules.insert(module_name);
            }
            SwiftImport::ScopedType {
                module_name,
                owner_name,
            } => {
                imports.scoped_types.insert((module_name, owner_name));
            }
        }
    }
    imports
}

enum SwiftImport {
    WholeModule(String),
    ScopedType {
        module_name: String,
        owner_name: String,
    },
}

fn swift_import_from_line(raw_line: &str) -> Option<SwiftImport> {
    let line = raw_line
        .split("//")
        .next()
        .unwrap_or(raw_line)
        .trim()
        .trim_end_matches(';')
        .trim();
    if line.is_empty() {
        return None;
    }
    let tokens = line.split_whitespace().collect::<Vec<_>>();
    let import_index = tokens.iter().position(|token| *token == "import")?;
    let first_import_token = tokens.get(import_index + 1)?;
    if matches!(
        *first_import_token,
        "class" | "struct" | "enum" | "protocol" | "typealias"
    ) {
        let scoped_surface = tokens.get(import_index + 2)?.trim();
        let module_name = swift_import_module_name(scoped_surface)?;
        let owner_name = swift_import_scoped_owner_name(scoped_surface)?;
        return Some(SwiftImport::ScopedType {
            module_name,
            owner_name,
        });
    }
    if matches!(*first_import_token, "func" | "var") {
        return None;
    }
    let module_surface = first_import_token.trim();
    swift_import_module_name(module_surface).map(SwiftImport::WholeModule)
}

fn swift_import_module_name(module_surface: &str) -> Option<String> {
    let module_name = module_surface
        .split('.')
        .next()
        .unwrap_or(module_surface)
        .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '_');
    (!module_name.is_empty()).then(|| module_name.to_string())
}

fn swift_import_scoped_owner_name(module_surface: &str) -> Option<String> {
    let (_, owner_surface) = module_surface.rsplit_once('.')?;
    normalize_parameter_name(owner_surface)
}

fn collect_swift_type_binding_names(root: TsNode<'_>, source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    walk_tree_nodes(root, &mut |node| {
        if matches!(
            node.kind(),
            "class_declaration"
                | "protocol_declaration"
                | "struct_declaration"
                | "enum_declaration"
                | "typealias_declaration"
        ) && let Some(name) =
            declaration_name(node, source).and_then(|name| normalize_parameter_name(&name))
        {
            names.insert(name);
        }
    });
    names
}

fn swift_type_import_qualifier(raw_type: &str) -> Option<String> {
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

fn member_call(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    if node.kind() != "call_expression" {
        return None;
    }
    let text = trimmed_node_text(node, source)?;
    let callable = text
        .split('(')
        .next()
        .unwrap_or(text.as_str())
        .trim()
        .trim_end_matches(';')
        .trim();
    let separator = callable.rfind('.')?;
    let receiver = callable[..separator].trim().trim_end_matches('?').trim();
    let method = callable[separator + 1..]
        .trim()
        .trim_start_matches('?')
        .trim();
    Some((
        normalized_swift_receiver_surface(receiver)?,
        normalize_parameter_name(method)?,
    ))
}

fn normalized_swift_receiver_surface(raw: &str) -> Option<String> {
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
