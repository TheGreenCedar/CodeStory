//! Ruby extraction rules.
//!
//! Ruby's graph extraction lives here: the tree-sitter rule file, the
//! compiled-rule cache, the member-call callsite marker, and the receiver-call
//! resolution engine that turns `@repo.save(...)` into an edge aimed at
//! `Repository.save`. Every language-keyed dispatch in the crate reaches it
//! through [`super::EXTRACTIONS`] rather than by spelling `"ruby"`.
//!
//! Four Ruby surfaces are deliberately *not* here, and each is a shared seam
//! rather than Ruby content:
//!
//! * `lib.rs::collect_ruby_bare_call_edges` and `collect_ruby_runtime_import_specs`,
//!   which hang off the manual-edge and runtime-import dispatches. Those take
//!   non-uniform arguments across all sixteen languages, so routing them
//!   through the registry is one change for the whole roster, not part of
//!   Ruby's rollback unit.
//! * `lib.rs::annotate_ruby_member_call_placeholders`, the pre-pass that stamps
//!   [`MEMBER_CALLSITE_MARKER`] onto unresolved CALL placeholders before the
//!   manual receiver-call edges are appended. It reaches into the edge buffer,
//!   the dedup key set, and the index feature flags, none of which the registry
//!   row describes; it calls [`member_call`] here for the syntax half.
//! * `lib.rs::collect_rails_route` and its `"ruby"` arm in the framework-route
//!   scanner, which is the same per-framework seam Kotlin's Ktor collector left
//!   behind.
//! * `LanguageRuleset::Ruby`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//!
//! Ruby's marker is *not* emitted by the rule file: `rules/ruby.scm` carries no
//! `call_syntax` attribute, so the registry row pairs `member_callsite_marker`
//! and `graph_call_syntax` as `None`/`None` and the marker is applied by the
//! pre-pass above.
//!
//! This is a move, not a rewrite. The bodies below are the ones that used to
//! sit in `lib.rs`, and `tests/language_extraction_snapshot.rs` pins the
//! rendered projection of both Ruby fixtures so the move stays output-equal.

use std::collections::HashSet;
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, LanguageRuleset, ManualMemberEdgeSpec, ManualReceiverCallSpec,
    ManualReceiverSource, code_before_hash_comment, collect_enclosing_type_member_edges,
    declaration_name, enclosing_node_with_kind, member_call_method_col, normalize_type_surface,
    normalized_receiver_variable, quoted_literal_surface, receiver_call_belongs_to_callable,
    same_ts_span, trimmed_node_text, ts_node_graph_span, walk_tree_nodes,
};

/// Callsite marker written onto edges produced from Ruby member-call syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:ruby-member-call";

const GRAPH_QUERY: &str = include_str!("../../rules/ruby.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for Ruby.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["ruby"],
    language_name: "ruby",
    extensions: &["rb"],
    ruleset: LanguageRuleset::Ruby,
    parser_language: ruby_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    member_edge_specs: Some(member_edge_specs),
    receiver_call_specs: Some(receiver_call_specs),
    // `rules/ruby.scm` emits no `call_syntax`, so there is no rule-file value
    // for the registry to map; `annotate_ruby_member_call_placeholders` stamps
    // `MEMBER_CALLSITE_MARKER` directly.
    member_callsite_marker: None,
    graph_call_syntax: None,
    // A Ruby `method` inside a `class` is already projected as METHOD by the
    // rule file, so the FUNCTION -> METHOD promotion does not apply.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: ".",
    route_comments_are_c_style: false,
    // `semantic::RubySemanticResolver` is private to that module, so the
    // registry records the choice and the residual match builds it.
    uses_generic_semantic_resolver: false,
    semantic_family: "ruby",
};

fn ruby_language() -> tree_sitter::Language {
    tree_sitter_ruby::LANGUAGE.into()
}

/// Manual receiver-call edges for one parsed Ruby file.
///
/// Was `lib.rs::collect_ruby_receiver_call_edges`.
pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let require_relative_module = ruby_single_require_relative_module(source);
    let local_type_names = collect_ruby_file_type_names(tree.root_node(), source);
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if !matches!(callable.kind(), "method" | "singleton_method") {
            return;
        }
        let Some(source_name) = declaration_name(callable, source) else {
            return;
        };
        collect_ruby_precise_receiver_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: &source_name,
                span: ts_node_graph_span(callable),
            },
            require_relative_module.as_deref(),
            &local_type_names,
            &mut edges,
        );
    });
    edges
}

fn collect_ruby_precise_receiver_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    require_relative_module: Option<&str>,
    local_type_names: &HashSet<String>,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    walk_tree_nodes(callable, &mut |node| {
        let Some((receiver_name, method_name)) = member_call(node, source) else {
            return;
        };
        if !receiver_call_belongs_to_callable(node, callable) {
            return;
        }
        let owner_name = if let Some(owner_name) = ruby_constructor_owner_surface(&receiver_name) {
            owner_name
        } else if let Some(owner) =
            ruby_visible_local_receiver_owner(callable, node, &receiver_name, source)
        {
            let Some(owner_name) = owner else {
                return;
            };
            owner_name
        } else if receiver_name.starts_with('@') {
            let Some(owner_name) =
                ruby_instance_variable_receiver_owner(callable, &receiver_name, source)
            else {
                return;
            };
            owner_name
        } else {
            return;
        };
        let owner_module =
            ruby_receiver_owner_module(&owner_name, require_relative_module, local_type_names);
        let method_col = member_call_method_col(node, source, &method_name);
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

fn ruby_receiver_owner_module(
    owner_name: &str,
    require_relative_module: Option<&str>,
    local_type_names: &HashSet<String>,
) -> Option<String> {
    if local_type_names.contains(owner_name) {
        return None;
    }
    require_relative_module.map(str::to_string)
}

fn collect_ruby_file_type_names(root: TsNode<'_>, source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    walk_tree_nodes(root, &mut |node| match node.kind() {
        "class" | "module" => {
            if let Some(name) = declaration_name(node, source) {
                names.insert(name);
            }
        }
        "assignment" => {
            if ruby_assignment_is_top_level(node)
                && let Some(name) = node
                    .child_by_field_name("left")
                    .and_then(|left| trimmed_node_text(left, source))
                    .and_then(|name| ruby_constant_name(&name))
            {
                names.insert(name);
            }
        }
        _ => {}
    });
    names
}

fn ruby_assignment_is_top_level(node: TsNode<'_>) -> bool {
    enclosing_node_with_kind(node, &["method", "singleton_method", "class", "module"]).is_none()
}

fn ruby_constant_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.contains("::") || name.contains('.') || name.starts_with('@') || name.starts_with('$') {
        return None;
    }
    let first = name.chars().next()?;
    first.is_uppercase().then(|| name.to_string())
}

fn ruby_single_require_relative_module(source: &str) -> Option<String> {
    let mut modules = source
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(ruby_require_relative_module_from_line)
        .collect::<Vec<_>>();
    modules.sort();
    modules.dedup();
    if modules.len() == 1 {
        modules.pop()
    } else {
        None
    }
}

fn ruby_require_relative_module_from_line(line: &str) -> Option<String> {
    let line = code_before_hash_comment(line).trim();
    let rest = line
        .strip_prefix("require_relative")
        .filter(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '(']))?
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    let module_name = quoted_literal_surface(rest)?;
    if module_name.starts_with("./") || module_name.starts_with("../") {
        Some(module_name.to_string())
    } else {
        Some(format!("./{module_name}"))
    }
}

fn ruby_instance_variable_receiver_owner(
    callable: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> Option<String> {
    let class_node = enclosing_node_with_kind(callable, &["class"])?;
    let mut owner_names: Vec<Option<String>> = Vec::new();
    walk_tree_nodes(class_node, &mut |node| {
        if !ruby_assignment_like_kind(node.kind()) {
            return;
        }
        if !enclosing_node_with_kind(node, &["class"])
            .is_some_and(|owner| same_ts_span(owner, class_node))
        {
            return;
        }
        if !ruby_assignment_matches_receiver_domain(node, callable, class_node) {
            return;
        }
        let Some(left_node) = node.child_by_field_name("left") else {
            return;
        };
        if normalized_receiver_variable(left_node, source).as_deref() != Some(receiver_name) {
            return;
        }
        let owner_name = if node.kind() == "operator_assignment" {
            None
        } else {
            node.child_by_field_name("right")
                .and_then(|right_node| ruby_constructor_owner(right_node, source))
        };
        owner_names.push(owner_name);
    });
    let mut concrete_owners = owner_names.into_iter().collect::<Option<Vec<_>>>()?;
    concrete_owners.sort();
    concrete_owners.dedup();
    if concrete_owners.len() == 1 {
        concrete_owners.pop()
    } else {
        None
    }
}

fn ruby_assignment_like_kind(kind: &str) -> bool {
    matches!(kind, "assignment" | "operator_assignment")
}

fn ruby_assignment_matches_receiver_domain(
    assignment: TsNode<'_>,
    callable: TsNode<'_>,
    class_node: TsNode<'_>,
) -> bool {
    let enclosing_method = enclosing_node_with_kind(assignment, &["method", "singleton_method"]);
    match callable.kind() {
        "method" => enclosing_method.is_some_and(|method| {
            method.kind() == "method"
                && enclosing_node_with_kind(method, &["class"])
                    .is_some_and(|owner| same_ts_span(owner, class_node))
        }),
        "singleton_method" => enclosing_method.is_none_or(|method| {
            method.kind() == "singleton_method"
                && enclosing_node_with_kind(method, &["class"])
                    .is_some_and(|owner| same_ts_span(owner, class_node))
        }),
        _ => false,
    }
}

fn ruby_visible_local_receiver_owner(
    callable: TsNode<'_>,
    call_node: TsNode<'_>,
    receiver_name: &str,
    source: &str,
) -> Option<Option<String>> {
    let mut visible_bindings = Vec::new();
    walk_tree_nodes(callable, &mut |node| {
        if !ruby_assignment_like_kind(node.kind()) {
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
        let owner_name = if node.kind() == "operator_assignment" {
            None
        } else {
            node.child_by_field_name("right")
                .and_then(|right_node| ruby_constructor_owner(right_node, source))
        };
        visible_bindings.push((node.end_byte(), owner_name));
    });
    visible_bindings.sort_by_key(|(end_byte, _)| *end_byte);
    visible_bindings.pop().map(|(_, owner)| owner)
}

/// Receiver and member of one Ruby member call, read from the grammar.
///
/// Was `lib.rs::ruby_receiver_call`. `lib.rs` still calls it from
/// `annotate_ruby_member_call_placeholders`, which is why it is `pub(crate)`.
pub(crate) fn member_call(node: TsNode<'_>, source: &str) -> Option<(String, String)> {
    if node.kind() != "call" {
        return None;
    }
    let receiver = node.child_by_field_name("receiver")?;
    let method = node.child_by_field_name("method")?;
    let method_name = trimmed_node_text(method, source)?;
    if method_name == "new" {
        return None;
    }
    Some((normalized_receiver_variable(receiver, source)?, method_name))
}

fn ruby_constructor_owner(node: TsNode<'_>, source: &str) -> Option<String> {
    if node.kind() != "call" {
        return None;
    }
    let method = node.child_by_field_name("method")?;
    if trimmed_node_text(method, source).as_deref() != Some("new") {
        return None;
    }
    let receiver = node.child_by_field_name("receiver")?;
    let raw_owner = trimmed_node_text(receiver, source)?;
    normalize_type_surface(&raw_owner)
}

fn ruby_constructor_owner_surface(surface: &str) -> Option<String> {
    let surface = surface.trim();
    let (raw_owner, suffix) = surface.split_once(".new")?;
    if !(suffix.is_empty() || suffix.starts_with('(') && suffix.ends_with(')')) {
        return None;
    }
    let raw_owner = raw_owner.trim();
    normalize_type_surface(raw_owner)
}

/// The manual MEMBER-edge collector this language had in `lib.rs`.
///
/// `language_member_specs` consults the registry before its residual
/// `match`, so once this row exists the old arm is unreachable. Leaving the
/// field `None` would therefore drop ruby's MEMBER edges silently, with
/// nothing in the arm itself to show it had stopped running.
pub(crate) fn member_edge_specs(tree: &Tree, source: &str) -> Vec<ManualMemberEdgeSpec> {
    collect_enclosing_type_member_edges(
        tree,
        source,
        &["class", "module"],
        &["method", "singleton_method"],
    )
}
