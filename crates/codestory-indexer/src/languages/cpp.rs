//! C++ extraction rules.
//!
//! C++'s graph extraction lives here: the tree-sitter rule file, the
//! compiled-rule cache, the member-call callsite marker, and the receiver-call
//! resolution engine that turns `repo.save(...)` and `notifier->notifyEvent(...)`
//! into edges aimed at `Repository::save` and `Notifier::notifyEvent`. Every
//! language-keyed dispatch in the crate reaches it through [`super::EXTRACTIONS`]
//! rather than by spelling `"cpp"`.
//!
//! Several C++ surfaces are deliberately *not* here, and each is a shared seam
//! rather than C++ content:
//!
//! * `lib.rs::cpp_language_config` and the `.h` header-inference pair
//!   (`infer_header_language_config`, `maybe_upgrade_header_language_from_source`).
//!   Those decide between C and C++ for an extension the registry does not own —
//!   `h` routes to `c` in the public registry — so they are a C/C++ boundary
//!   rather than a C++ rule. They now build their config from this module's row.
//! * `infer_cpp_access_from_tree`, which C shares (`"cpp" | "c"` in
//!   `access_from_tree`), plus `collect_cpp_declaration_span_overrides`,
//!   `cpp_unknown_capture_needs_terminal_identifier`, and
//!   `collect_cpp_template_type_argument_edges`, which hang off dispatches the
//!   [`LanguageExtraction`] struct does not model. Routing those through the
//!   registry is one change for all sixteen languages, not part of C++'s
//!   rollback unit.
//! * `semantic::CppSemanticResolver`, which stays in `semantic` because the
//!   dedicated resolver types are private to that module; the registry records
//!   the choice with `uses_generic_semantic_resolver: false`.
//! * `LanguageRuleset::Cpp`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both C++ fixtures so the move stays output-equal.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, LanguageRuleset, ManualReceiverCallSpec, ManualReceiverSource,
    ReceiverCallSiteKey, c_like_declarator_name_node, collect_receiver_call_specs_in_callable,
    declaration_name, enclosing_node_with_kind, member_call_method_col, node_is_same_or_ancestor,
    normalize_parameter_name, normalize_type_surface, normalized_receiver_surface,
    receiver_call_belongs_to_callable, receiver_callsite_key, same_ts_span,
    signature_parameter_surface, split_top_level_parameters, trimmed_node_text, ts_node_graph_span,
    walk_tree_nodes,
};

/// Callsite marker written onto edges produced from C++ member-call syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:cpp-member-call";

const GRAPH_QUERY: &str = include_str!("../../rules/cpp.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for C++.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["cpp"],
    language_name: "cpp",
    extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
    ruleset: LanguageRuleset::Cpp,
    parser_language: cpp_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    receiver_call_specs: Some(receiver_call_specs),
    member_callsite_marker: Some(MEMBER_CALLSITE_MARKER),
    graph_call_syntax: Some("cpp_member"),
    // C++ member functions project as FUNCTION, not METHOD: the rule file
    // already names them `Owner::member`, and the promotion roster held only
    // `kotlin`, `swift`, and `dart` before this move.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: "::",
    // Deliberately `false`. C++ *source* is C-style-commented, but the
    // framework-route text scanner's roster never listed `cpp`. That roster is
    // a claim about which languages carry scannable routes, not a syntax fact,
    // and widening it here would change route extraction.
    route_comments_are_c_style: false,
    // `semantic::CppSemanticResolver` is a dedicated resolver private to that
    // module, so the residual `dedicated_semantic_resolver` arm still builds it.
    uses_generic_semantic_resolver: false,
    semantic_family: "native",
};

fn cpp_language() -> tree_sitter::Language {
    tree_sitter_cpp::LANGUAGE.into()
}

/// Manual receiver-call edges for one parsed C++ file.
///
/// Was `lib.rs::collect_cpp_receiver_call_edges`.
pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if callable.kind() != "function_definition" {
            return;
        }
        let Some(source_name) = cpp_callable_name(callable, source) else {
            return;
        };
        let call_source = ManualReceiverSource {
            name: &source_name,
            span: ts_node_graph_span(callable),
        };
        let receiver_types = collect_cpp_parameter_types(callable, source);
        let mut local_receiver_callsites = HashSet::new();
        collect_cpp_precise_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &receiver_types,
            &mut local_receiver_callsites,
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
            .retain(|spec| !local_receiver_callsites.contains(&receiver_callsite_key(spec)));
        edges.extend(parameter_specs);
    });
    edges
}

fn collect_cpp_precise_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    parameter_receiver_types: &HashMap<String, String>,
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

        if let Some(owner_name) =
            cpp_visible_local_receiver_owner(callable, node, &receiver_name, source)
        {
            local_receiver_callsites.insert(callsite_key);
            if let Some(owner_name) = owner_name {
                edges.push(ManualReceiverCallSpec {
                    source_name: call_source.name.to_string(),
                    source_span: call_source.span,
                    receiver_name,
                    owner_name,
                    owner_module: None,
                    method_name,
                    method_col,
                    line: Some(node.start_position().row as u32 + 1),
                    allow_global_fallback: false,
                });
            }
            return;
        }

        let owner_name =
            if let Some(owner_name) = cpp_self_receiver_owner(callable, &receiver_name, source) {
                Some(owner_name)
            } else if !parameter_receiver_types.contains_key(&receiver_name) {
                cpp_field_receiver_owner(callable, &receiver_name, source)
            } else {
                None
            };
        if let Some(owner_name) = owner_name {
            local_receiver_callsites.insert(callsite_key);
            edges.push(ManualReceiverCallSpec {
                source_name: call_source.name.to_string(),
                source_span: call_source.span,
                receiver_name,
                owner_name,
                owner_module: None,
                method_name,
                method_col,
                line: Some(node.start_position().row as u32 + 1),
                allow_global_fallback: false,
            });
        }
    });
}

fn cpp_self_receiver_owner(
    callable: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> Option<String> {
    if receiver_name != "this" {
        return None;
    }
    let owner_node = enclosing_node_with_kind(callable, &["class_specifier", "struct_specifier"])?;
    declaration_name(owner_node, source)
}

fn cpp_field_receiver_owner(
    callable: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> Option<String> {
    let field_name = receiver_name
        .strip_prefix("this->")
        .unwrap_or(receiver_name)
        .trim();
    if field_name == "this" || field_name.contains('.') || field_name.contains("->") {
        return None;
    }
    let owner_node = enclosing_node_with_kind(callable, &["class_specifier", "struct_specifier"])?;
    let mut field_bindings = Vec::new();
    walk_tree_nodes(owner_node, &mut |node| {
        if node.kind() != "field_declaration" || !cpp_field_belongs_to_owner(node, owner_node) {
            return;
        }
        for (binding_name, owner_name) in cpp_local_declaration_receiver_bindings(node, source) {
            if binding_name == field_name
                && let Some(owner_name) = owner_name
            {
                field_bindings.push(owner_name);
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

fn cpp_field_belongs_to_owner(field: TsNode<'_>, owner_node: TsNode<'_>) -> bool {
    let mut current = field.parent();
    while let Some(candidate) = current {
        if same_ts_span(candidate, owner_node) {
            return true;
        }
        if matches!(
            candidate.kind(),
            "function_definition" | "class_specifier" | "struct_specifier"
        ) {
            return false;
        }
        current = candidate.parent();
    }
    false
}

fn cpp_visible_local_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> Option<Option<String>> {
    let mut visible_bindings = Vec::new();
    walk_tree_nodes(callable, &mut |node| {
        if node.kind() != "declaration" {
            return;
        }
        if !receiver_call_belongs_to_callable(node, callable)
            || node.end_byte() > call_node.start_byte()
            || !cpp_local_binding_visible_at_call(node, call_node)
        {
            return;
        }
        for (binding_name, owner_name) in cpp_local_declaration_receiver_bindings(node, source) {
            if binding_name == receiver_name {
                visible_bindings.push((node.end_byte(), owner_name));
            }
        }
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner_name)| owner_name)
}

fn cpp_local_declaration_receiver_bindings(
    node: TsNode<'_>,
    source: &str,
) -> Vec<(String, Option<String>)> {
    let Some(surface) = trimmed_node_text(node, source) else {
        return Vec::new();
    };
    let surface = surface.trim().trim_end_matches(';').trim();
    if surface.is_empty() || surface.starts_with("using ") || surface.starts_with("typedef ") {
        return Vec::new();
    }
    if surface.contains(',') {
        return surface
            .split(',')
            .filter_map(cpp_declarator_binding_name)
            .map(|name| (name, None))
            .collect();
    }
    let declarator_head = surface
        .split('=')
        .next()
        .unwrap_or(surface)
        .split('{')
        .next()
        .unwrap_or(surface)
        .trim();
    if declarator_head.contains('(') {
        return Vec::new();
    }
    let Some((receiver_name, name_start)) = cpp_trailing_parameter_name(declarator_head) else {
        return Vec::new();
    };
    let raw_type = declarator_head[..name_start].trim();
    let owner_name = normalize_cpp_type_surface(raw_type)
        .or_else(|| cpp_auto_local_initializer_owner(raw_type, surface));
    vec![(receiver_name, owner_name)]
}

fn cpp_auto_local_initializer_owner(raw_type: &str, surface: &str) -> Option<String> {
    if !cpp_type_surface_is_auto(raw_type) {
        return None;
    }
    let (_, initializer) = surface.split_once('=')?;
    cpp_direct_constructor_owner_surface(initializer)
}

fn cpp_type_surface_is_auto(raw_type: &str) -> bool {
    let normalized = raw_type.replace(['*', '&'], " ");
    let mut has_auto = false;
    for token in normalized.split_whitespace() {
        match token {
            "auto" => has_auto = true,
            "const" | "volatile" | "mutable" | "constexpr" => {}
            _ => return false,
        }
    }
    has_auto
}

fn cpp_direct_constructor_owner_surface(surface: &str) -> Option<String> {
    let surface = surface.trim().trim_end_matches(';').trim();
    let surface = surface.strip_prefix("new ").unwrap_or(surface).trim();
    let delimiter = surface.find(['{', '('])?;
    let owner_surface = surface[..delimiter].trim();
    if owner_surface.contains("::")
        || owner_surface.contains('.')
        || owner_surface.split_whitespace().count() != 1
    {
        return None;
    }
    cpp_initializer_suffix_consumes_surface(&surface[delimiter..])?;
    normalize_parameter_name(owner_surface)
}

fn cpp_initializer_suffix_consumes_surface(surface: &str) -> Option<()> {
    let mut chars = surface.char_indices();
    let (_, opener) = chars.next()?;
    let closer = match opener {
        '{' => '}',
        '(' => ')',
        _ => return None,
    };
    let mut stack = vec![closer];
    for (index, ch) in chars {
        match ch {
            '{' => stack.push('}'),
            '(' => stack.push(')'),
            '}' | ')' => {
                if stack.pop() != Some(ch) {
                    return None;
                }
                if stack.is_empty() {
                    return surface[index + ch.len_utf8()..]
                        .trim()
                        .is_empty()
                        .then_some(());
                }
            }
            _ => {}
        }
    }
    None
}

fn cpp_declarator_binding_name(declarator: &str) -> Option<String> {
    let declarator_head = declarator
        .split('=')
        .next()
        .unwrap_or(declarator)
        .split('{')
        .next()
        .unwrap_or(declarator)
        .trim();
    if declarator_head.contains('(') {
        return None;
    }
    cpp_trailing_parameter_name(declarator_head).map(|(name, _)| name)
}

fn cpp_local_binding_visible_at_call(binding: TsNode<'_>, call_node: TsNode<'_>) -> bool {
    let Some(binding_scope) = cpp_lexical_scope(binding) else {
        return false;
    };
    let Some(call_scope) = cpp_lexical_scope(call_node) else {
        return false;
    };
    node_is_same_or_ancestor(binding_scope, call_scope)
}

fn cpp_lexical_scope(node: TsNode<'_>) -> Option<TsNode<'_>> {
    enclosing_node_with_kind(node, &["compound_statement"])
}

fn cpp_callable_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("declarator")
        .and_then(c_like_declarator_name_node)
        .and_then(|name_node| trimmed_node_text(name_node, source))
}

fn collect_cpp_parameter_types(callable: TsNode<'_>, source: &str) -> HashMap<String, String> {
    let mut receiver_types = HashMap::new();
    let Some(parameters) = signature_parameter_surface(callable, source) else {
        return receiver_types;
    };
    for parameter in split_top_level_parameters(&parameters) {
        let parameter = parameter
            .split('=')
            .next()
            .unwrap_or(parameter.as_str())
            .trim();
        if parameter.is_empty() || parameter == "void" {
            continue;
        }
        let Some((receiver_name, name_start)) = cpp_trailing_parameter_name(parameter) else {
            continue;
        };
        let raw_type = parameter[..name_start].trim();
        let Some(owner_name) = normalize_cpp_type_surface(raw_type) else {
            continue;
        };
        receiver_types.insert(receiver_name, owner_name);
    }
    receiver_types
}

fn cpp_trailing_parameter_name(parameter: &str) -> Option<(String, usize)> {
    let mut end = None;
    for (index, ch) in parameter.char_indices().rev() {
        if end.is_none() {
            if ch == '_' || ch.is_ascii_alphanumeric() {
                end = Some(index + ch.len_utf8());
            }
            continue;
        }
        if !(ch == '_' || ch.is_ascii_alphanumeric()) {
            let start = index + ch.len_utf8();
            let name = &parameter[start..end?];
            return normalize_parameter_name(name).map(|name| (name, start));
        }
    }
    let end = end?;
    let name = &parameter[..end];
    normalize_parameter_name(name).map(|name| (name, 0))
}

fn normalize_cpp_type_surface(raw_type: &str) -> Option<String> {
    let without_pointers = raw_type.replace(['*', '&'], " ");
    let base = without_pointers
        .split('<')
        .next()
        .unwrap_or(without_pointers.as_str());
    let owner = base.split_whitespace().rfind(|token| {
        !matches!(
            *token,
            "const"
                | "volatile"
                | "mutable"
                | "constexpr"
                | "typename"
                | "class"
                | "struct"
                | "enum"
                | "auto"
        )
    })?;
    normalize_type_surface(owner)
}

/// Receiver and member of one C++ member call.
///
/// Was `lib.rs::cpp_member_call`.
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
    let dot = callable.rfind('.').map(|index| (index, 1usize));
    let arrow = callable.rfind("->").map(|index| (index, 2usize));
    let (separator, width) = match (dot, arrow) {
        (Some(dot), Some(arrow)) => {
            if dot.0 > arrow.0 {
                dot
            } else {
                arrow
            }
        }
        (Some(dot), None) => dot,
        (None, Some(arrow)) => arrow,
        (None, None) => return None,
    };
    let receiver = callable[..separator].trim();
    let method = callable[separator + width..].trim();
    Some((
        normalized_receiver_surface(receiver)?,
        normalize_parameter_name(method)?,
    ))
}
