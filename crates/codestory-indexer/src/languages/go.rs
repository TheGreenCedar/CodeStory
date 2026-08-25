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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use tree_sitter::{Node as TsNode, Tree};

#[cfg(test)]
thread_local! {
    static GO_NAVIGATION_RESOLUTION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline]
fn count_go_navigation_resolution_work(amount: usize) {
    #[cfg(test)]
    GO_NAVIGATION_RESOLUTION_WORK.with(|work| work.set(work.get().saturating_add(amount)));
    #[cfg(not(test))]
    let _ = amount;
}

#[cfg(test)]
fn reset_go_navigation_resolution_work() {
    GO_NAVIGATION_RESOLUTION_WORK.with(|work| work.set(0));
}

#[cfg(test)]
fn go_navigation_resolution_work() -> usize {
    GO_NAVIGATION_RESOLUTION_WORK.with(std::cell::Cell::get)
}

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, LanguageRuleset, ManualMemberEdgeSpec, ManualReceiverCallSpec,
    ManualReceiverSource, OptionalReceiverOwnerBinding, ReceiverCallSiteKey, ReceiverOwnerBinding,
    collect_receiver_call_specs_in_callable, declaration_name, descendant_by_field_name,
    enclosing_node_with_kind, member_call_method_col, normalize_parameter_name,
    normalized_receiver_variable, receiver_call_belongs_to_callable, receiver_callsite_key,
    trimmed_node_text, ts_node_graph_span, walk_tree_nodes,
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
    type_usage_specs: None,
    callsite_marker_families: &[("go_selector", MEMBER_CALLSITE_MARKER)],
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
    let owner_specs = go_package_type_specs_by_name(tree.root_node(), source);
    walk_tree_nodes(tree.root_node(), &mut |node| {
        count_go_navigation_resolution_work(1);
        match node.kind() {
            "method_declaration" => {
                if node.has_error() || node.is_error() || node.is_missing() {
                    return;
                }
                let Some(method_name_node) = node.child_by_field_name("name") else {
                    return;
                };
                let Some(receiver_node) = node.child_by_field_name("receiver") else {
                    return;
                };
                let Some(source_name) = go_declared_receiver_owner_name(receiver_node, source)
                else {
                    return;
                };
                let Some(target_name) = trimmed_node_text(method_name_node, source) else {
                    return;
                };
                let source_span = owner_specs
                    .get(&source_name)
                    .copied()
                    .flatten()
                    .and_then(|owner| owner.parent())
                    .filter(|parent| parent.kind() == "type_declaration")
                    .map(ts_node_graph_span)
                    .unwrap_or_else(|| ts_node_graph_span(receiver_node));

                edges.push(ManualMemberEdgeSpec {
                    source_name,
                    target_name,
                    source_span,
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
        }
    });
    edges
}

fn go_package_type_specs_by_name<'tree>(
    root: TsNode<'tree>,
    source: &str,
) -> HashMap<String, Option<TsNode<'tree>>> {
    let mut specs = HashMap::new();
    let mut root_cursor = root.walk();
    for declaration in root.named_children(&mut root_cursor) {
        count_go_navigation_resolution_work(1);
        if declaration.kind() != "type_declaration" {
            continue;
        }
        let mut declaration_cursor = declaration.walk();
        for spec in declaration.named_children(&mut declaration_cursor) {
            count_go_navigation_resolution_work(1);
            if spec.kind() != "type_spec" {
                continue;
            }
            let Some(name) = spec
                .child_by_field_name("name")
                .and_then(|name_node| trimmed_node_text(name_node, source))
            else {
                continue;
            };
            match specs.entry(name) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(Some(spec));
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    entry.insert(None);
                }
            }
        }
    }
    specs
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

fn go_declared_receiver_owner_name(receiver_node: TsNode<'_>, source: &str) -> Option<String> {
    if receiver_node.kind() != "parameter_list"
        || receiver_node.has_error()
        || receiver_node.is_error()
        || receiver_node.is_missing()
    {
        return None;
    }
    let mut receiver_cursor = receiver_node.walk();
    let parameters = receiver_node
        .named_children(&mut receiver_cursor)
        .collect::<Vec<_>>();
    let [parameter] = parameters.as_slice() else {
        return None;
    };
    if parameter.kind() != "parameter_declaration"
        || parameter.has_error()
        || parameter.is_error()
        || parameter.is_missing()
    {
        return None;
    }
    let mut name_cursor = parameter.walk();
    if parameter
        .children_by_field_name("name", &mut name_cursor)
        .count()
        > 1
    {
        return None;
    }
    let type_node = parameter.child_by_field_name("type")?;
    if type_node.has_error() || type_node.is_error() || type_node.is_missing() {
        return None;
    }
    let owner_node = match type_node.kind() {
        "type_identifier" => type_node,
        "pointer_type" => {
            let mut pointer_cursor = type_node.walk();
            let children = type_node
                .named_children(&mut pointer_cursor)
                .collect::<Vec<_>>();
            let [owner] = children.as_slice() else {
                return None;
            };
            if owner.kind() != "type_identifier"
                || owner.has_error()
                || owner.is_error()
                || owner.is_missing()
            {
                return None;
            }
            *owner
        }
        _ => return None,
    };
    let owner = trimmed_node_text(owner_node, source)?;
    if normalize_parameter_name(&owner).as_deref() == Some(owner.as_str()) {
        Some(owner)
    } else {
        None
    }
}

fn normalize_go_type_surface(raw: &str) -> Option<String> {
    go_exact_type_surface(raw).map(|(_, owner)| owner)
}

fn go_exact_type_surface(raw: &str) -> Option<(Option<String>, String)> {
    let surface = raw.trim();
    let surface = if let Some(stripped) = surface.strip_prefix('*') {
        let stripped = stripped.trim_start();
        if stripped.starts_with('*') {
            return None;
        }
        stripped
    } else {
        surface
    };
    if surface.contains(char::is_whitespace)
        || surface.contains(['[', ']', '(', ')', '{', '}', '/', '&'])
    {
        return None;
    }
    match surface.split_once('.') {
        Some((qualifier, owner))
            if !owner.contains('.')
                && normalize_parameter_name(qualifier).as_deref() == Some(qualifier)
                && normalize_parameter_name(owner).as_deref() == Some(owner) =>
        {
            Some((Some(qualifier.to_string()), owner.to_string()))
        }
        None if normalize_parameter_name(surface).as_deref() == Some(surface) => {
            Some((None, surface.to_string()))
        }
        _ => None,
    }
}

pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let import_bindings = collect_go_import_bindings(source);
    let package_owner_specs = go_package_type_specs_by_name(tree.root_node(), source);
    let file_scope_names = go_file_scope_names(tree.root_node(), source);
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        count_go_navigation_resolution_work(1);
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
            &file_scope_names,
            &mut local_binding_callsites,
            &mut edges,
        );
        let method_receiver_bindings = collect_go_method_receiver_bindings(
            callable,
            source,
            &import_bindings,
            &package_owner_specs,
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
    file_scope_names: &HashSet<String>,
    local_binding_callsites: &mut HashSet<ReceiverCallSiteKey>,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    let mut calls = Vec::new();
    let mut intervals = Vec::new();
    let builtin_new_unshadowed = !import_bindings.contains_key("new")
        && !file_scope_names.contains("new")
        && !go_callable_declares_name(callable, "new", source);
    walk_tree_nodes(callable, &mut |node| {
        count_go_navigation_resolution_work(1);
        if !receiver_call_belongs_to_callable(node, callable) {
            return;
        }
        if let Some((receiver_name, method_name)) = selector_call(node, source) {
            calls.push(GoNavigationCall {
                node,
                receiver_name,
                method_name,
            });
        }
        let Some((scope_end, scope_depth)) = go_navigation_binding_scope(node, callable) else {
            return;
        };
        match node.kind() {
            "short_var_declaration" | "assignment_statement" => {
                let left = node
                    .child_by_field_name("left")
                    .map(go_expression_list_items)
                    .unwrap_or_default();
                let right = node
                    .child_by_field_name("right")
                    .map(go_expression_list_items)
                    .unwrap_or_default();
                for (index, left) in left.into_iter().enumerate() {
                    let Some(name) = normalized_receiver_variable(left, source) else {
                        continue;
                    };
                    let owner = right.get(index).and_then(|value| {
                        go_direct_composite_literal_owner(
                            *value,
                            source,
                            import_bindings,
                            builtin_new_unshadowed,
                        )
                    });
                    intervals.push(GoNavigationBindingInterval {
                        name,
                        start_byte: node.end_byte(),
                        end_byte: if node.kind() == "assignment_statement" {
                            callable.end_byte()
                        } else {
                            scope_end
                        },
                        scope_depth: if node.kind() == "assignment_statement" {
                            usize::MAX - 1
                        } else {
                            scope_depth
                        },
                        owner,
                    });
                }
            }
            "var_spec" => {
                let Some(type_node) = node.child_by_field_name("type") else {
                    return;
                };
                let owner = trimmed_node_text(type_node, source)
                    .as_deref()
                    .and_then(|raw_type| go_receiver_owner_from_type(raw_type, import_bindings));
                let mut cursor = node.walk();
                for name_node in node
                    .named_children(&mut cursor)
                    .take_while(|child| child.start_byte() < type_node.start_byte())
                {
                    let Some(name) = normalized_receiver_variable(name_node, source) else {
                        continue;
                    };
                    intervals.push(GoNavigationBindingInterval {
                        name,
                        start_byte: node.end_byte(),
                        end_byte: scope_end,
                        scope_depth,
                        owner: owner.clone(),
                    });
                }
            }
            "const_spec" | "type_spec" => {
                for name in go_navigation_declared_names(node, source) {
                    intervals.push(GoNavigationBindingInterval {
                        name,
                        start_byte: node.end_byte(),
                        end_byte: scope_end,
                        scope_depth,
                        owner: None,
                    });
                }
            }
            "range_clause" | "receive_statement" | "type_switch_guard" => {
                if let Some((names, end_byte, depth)) =
                    go_navigation_special_binding(node, callable, source)
                {
                    for name in names {
                        intervals.push(GoNavigationBindingInterval {
                            name,
                            start_byte: node.end_byte(),
                            end_byte,
                            scope_depth: depth,
                            owner: None,
                        });
                    }
                }
            }
            "type_switch_statement" => {
                if let Some(header) = trimmed_node_text(node, source).and_then(|surface| {
                    surface
                        .split_once('{')
                        .map(|(header, _)| header.trim().to_string())
                }) {
                    for name in go_navigation_special_names(
                        header.strip_prefix("switch").unwrap_or(&header),
                    ) {
                        intervals.push(GoNavigationBindingInterval {
                            name,
                            start_byte: node.start_byte(),
                            end_byte: node.end_byte(),
                            scope_depth: scope_depth.saturating_add(1),
                            owner: None,
                        });
                    }
                }
            }
            "unary_expression" => {
                let Some(surface) = trimmed_node_text(node, source) else {
                    return;
                };
                let Some(name) = surface.strip_prefix('&').map(str::trim) else {
                    return;
                };
                if normalize_parameter_name(name).as_deref() == Some(name) {
                    intervals.push(GoNavigationBindingInterval {
                        name: name.to_string(),
                        start_byte: callable.start_byte(),
                        end_byte: callable.end_byte(),
                        scope_depth: usize::MAX,
                        owner: None,
                    });
                }
            }
            _ => {}
        }
        if !go_navigation_node_is_captured(node, callable) || node.kind() != "identifier" {
            return;
        }
        let Some(name) = normalized_receiver_variable(node, source) else {
            return;
        };
        intervals.push(GoNavigationBindingInterval {
            name,
            start_byte: callable.start_byte(),
            end_byte: callable.end_byte(),
            scope_depth: usize::MAX,
            owner: None,
        });
    });
    let decisions = go_navigation_binding_decisions(
        callable.start_byte(),
        callable.end_byte(),
        &intervals,
        &calls,
    );
    for call in calls {
        let Some(owner) = decisions.get(&call.node.id()) else {
            continue;
        };
        let method_col = member_call_method_col(call.node, source, &call.method_name);
        local_binding_callsites.insert(ReceiverCallSiteKey {
            receiver_name: call.receiver_name.clone(),
            method_name: call.method_name.clone(),
            line: Some(call.node.start_position().row as u32 + 1),
            method_col,
        });
        if let Some((owner_name, owner_module)) = owner {
            edges.push(ManualReceiverCallSpec {
                source_name: call_source.name.to_string(),
                source_span: call_source.span,
                receiver_name: call.receiver_name,
                owner_name: owner_name.clone(),
                owner_module: owner_module.clone(),
                method_name: call.method_name,
                method_col,
                line: Some(call.node.start_position().row as u32 + 1),
                allow_global_fallback: false,
                binding_marker: None,
                required_callsite_marker: None,
                class_anchored: false,
                owner_is_syntactic: false,
            });
        }
    }
}

struct GoNavigationCall<'tree> {
    node: TsNode<'tree>,
    receiver_name: String,
    method_name: String,
}

struct GoNavigationBindingInterval {
    name: String,
    start_byte: usize,
    end_byte: usize,
    scope_depth: usize,
    owner: OptionalReceiverOwnerBinding,
}

#[derive(Clone, Copy)]
enum GoNavigationEvent {
    End(usize),
    Start(usize),
    Call(usize),
}

fn go_navigation_binding_decisions(
    range_start: usize,
    range_end: usize,
    intervals: &[GoNavigationBindingInterval],
    calls: &[GoNavigationCall<'_>],
) -> HashMap<usize, OptionalReceiverOwnerBinding> {
    let mut events = vec![Vec::new(); range_end.saturating_sub(range_start).saturating_add(1)];
    for (index, interval) in intervals.iter().enumerate() {
        if interval.start_byte < range_start
            || interval.start_byte >= interval.end_byte
            || interval.end_byte > range_end
        {
            continue;
        }
        events[interval.start_byte - range_start].push(GoNavigationEvent::Start(index));
        events[interval.end_byte - range_start].push(GoNavigationEvent::End(index));
        count_go_navigation_resolution_work(2);
    }
    for (index, call) in calls.iter().enumerate() {
        if (range_start..=range_end).contains(&call.node.start_byte()) {
            events[call.node.start_byte() - range_start].push(GoNavigationEvent::Call(index));
            count_go_navigation_resolution_work(1);
        }
    }
    let mut active = HashMap::<String, BTreeMap<usize, HashSet<usize>>>::new();
    let mut decisions = HashMap::new();
    for bucket in events {
        count_go_navigation_resolution_work(1);
        for event in bucket
            .iter()
            .copied()
            .filter(|event| matches!(event, GoNavigationEvent::End(_)))
        {
            let GoNavigationEvent::End(index) = event else {
                unreachable!()
            };
            let interval = &intervals[index];
            if let Some(depths) = active.get_mut(&interval.name) {
                if let Some(entries) = depths.get_mut(&interval.scope_depth) {
                    entries.remove(&index);
                    if entries.is_empty() {
                        depths.remove(&interval.scope_depth);
                    }
                }
                if depths.is_empty() {
                    active.remove(&interval.name);
                }
            }
            count_go_navigation_resolution_work(1);
        }
        for event in bucket
            .iter()
            .copied()
            .filter(|event| matches!(event, GoNavigationEvent::Start(_)))
        {
            let GoNavigationEvent::Start(index) = event else {
                unreachable!()
            };
            let interval = &intervals[index];
            active
                .entry(interval.name.clone())
                .or_default()
                .entry(interval.scope_depth)
                .or_default()
                .insert(index);
            count_go_navigation_resolution_work(1);
        }
        for event in bucket
            .iter()
            .copied()
            .filter(|event| matches!(event, GoNavigationEvent::Call(_)))
        {
            let GoNavigationEvent::Call(index) = event else {
                unreachable!()
            };
            let call = &calls[index];
            let Some((_, entries)) = active
                .get(&call.receiver_name)
                .and_then(BTreeMap::last_key_value)
            else {
                continue;
            };
            let latest_start = entries
                .iter()
                .map(|index| intervals[*index].start_byte)
                .max()
                .expect("active binding set is non-empty");
            let mut latest = entries
                .iter()
                .filter(|index| intervals[**index].start_byte == latest_start);
            let owner = latest
                .next()
                .filter(|_| latest.next().is_none())
                .and_then(|index| intervals[*index].owner.clone());
            decisions.insert(call.node.id(), owner);
            count_go_navigation_resolution_work(1);
        }
    }
    decisions
}

fn go_navigation_binding_scope(
    mut node: TsNode<'_>,
    callable: TsNode<'_>,
) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    loop {
        if node.kind() == "block" {
            return Some((node.end_byte(), depth.saturating_add(1)));
        }
        if node.id() == callable.id() {
            return Some((callable.end_byte(), depth));
        }
        node = node.parent()?;
        depth = depth.saturating_add(usize::from(node.kind() == "block"));
    }
}

fn go_navigation_declared_names(node: TsNode<'_>, source: &str) -> Vec<String> {
    let boundary = node
        .child_by_field_name("type")
        .or_else(|| node.child_by_field_name("value"))
        .map(|child| child.start_byte())
        .unwrap_or(usize::MAX);
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .take_while(|child| child.start_byte() < boundary)
        .filter_map(|child| normalized_receiver_variable(child, source))
        .collect()
}

fn go_navigation_special_binding(
    node: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
) -> Option<(Vec<String>, usize, usize)> {
    let surface = trimmed_node_text(node, source)?;
    let names = go_navigation_special_names(&surface);
    if names.is_empty() {
        return None;
    }
    let boundary_kind = match node.kind() {
        "range_clause" => "for_statement",
        "receive_statement" => "communication_case",
        "type_switch_guard" => "type_switch_statement",
        _ => return None,
    };
    let mut boundary = node;
    while boundary.kind() != boundary_kind {
        boundary = boundary.parent()?;
        if boundary.id() == callable.id() {
            return None;
        }
    }
    let declaration = surface.contains(":=");
    Some((
        names,
        if declaration {
            boundary.end_byte()
        } else {
            callable.end_byte()
        },
        if declaration {
            go_navigation_binding_scope(boundary, callable)?
                .1
                .saturating_add(1)
        } else {
            usize::MAX - 2
        },
    ))
}

fn go_navigation_special_names(surface: &str) -> Vec<String> {
    let left = surface
        .split_once(":=")
        .or_else(|| surface.split_once('='))
        .map(|(left, _)| left)
        .unwrap_or_default();
    left.rsplit([';', '{', ':'])
        .next()
        .unwrap_or(left)
        .split(',')
        .filter_map(normalize_parameter_name)
        .filter(|name| name != "_")
        .collect()
}

fn go_navigation_node_is_captured(mut node: TsNode<'_>, callable: TsNode<'_>) -> bool {
    let mut crossed_closure = false;
    while node.id() != callable.id() {
        crossed_closure |= node.kind() == "func_literal";
        let Some(parent) = node.parent() else {
            return false;
        };
        node = parent;
    }
    crossed_closure
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
    builtin_new_unshadowed: bool,
) -> OptionalReceiverOwnerBinding {
    if let Some(owner) = go_builtin_new_owner(node, source, import_bindings, builtin_new_unshadowed)
    {
        return Some(owner);
    }
    if node.kind() == "composite_literal" {
        return node
            .child_by_field_name("type")
            .and_then(|type_node| trimmed_node_text(type_node, source))
            .as_deref()
            .and_then(|type_surface| {
                go_composite_literal_owner_binding_from_type(type_surface, import_bindings)
            });
    }
    if node.kind() == "unary_expression"
        && trimmed_node_text(node, source)
            .as_deref()
            .is_some_and(|surface| surface.trim_start().starts_with('&'))
    {
        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        let [literal] = children.as_slice() else {
            return None;
        };
        if literal.kind() != "composite_literal" {
            return None;
        }
        return literal
            .child_by_field_name("type")
            .and_then(|type_node| trimmed_node_text(type_node, source))
            .as_deref()
            .and_then(|type_surface| {
                go_composite_literal_owner_binding_from_type(type_surface, import_bindings)
            });
    }
    None
}

fn go_builtin_new_owner(
    node: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
    builtin_new_unshadowed: bool,
) -> OptionalReceiverOwnerBinding {
    if node.kind() != "call_expression" || !builtin_new_unshadowed {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    if function.kind() != "identifier"
        || trimmed_node_text(function, source).as_deref() != Some("new")
    {
        return None;
    }
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let argument_nodes = arguments.named_children(&mut cursor).collect::<Vec<_>>();
    if argument_nodes.len() != 1 {
        return None;
    }
    let raw_type = trimmed_node_text(argument_nodes[0], source)?;
    go_composite_literal_owner_binding_from_type(&raw_type, import_bindings)
}

fn go_callable_declares_name(callable: TsNode<'_>, name: &str, source: &str) -> bool {
    let mut shadowed = false;
    walk_tree_nodes(callable, &mut |node| {
        count_go_navigation_resolution_work(1);
        if shadowed {
            return;
        }
        match node.kind() {
            "parameter_declaration" | "variadic_parameter_declaration" => {
                let type_start = node
                    .child_by_field_name("type")
                    .map(|type_node| type_node.start_byte())
                    .unwrap_or(usize::MAX);
                let mut cursor = node.walk();
                shadowed = node.named_children(&mut cursor).any(|child| {
                    child.start_byte() < type_start
                        && normalized_receiver_variable(child, source).as_deref() == Some(name)
                });
            }
            "short_var_declaration" | "assignment_statement" => {
                shadowed = node
                    .child_by_field_name("left")
                    .map(go_expression_list_items)
                    .unwrap_or_default()
                    .into_iter()
                    .any(|left| {
                        normalized_receiver_variable(left, source).as_deref() == Some(name)
                    });
            }
            "var_spec" | "const_spec" | "type_spec" => {
                shadowed = go_navigation_declared_names(node, source)
                    .iter()
                    .any(|declared| declared == name);
            }
            "range_clause" | "receive_statement" | "type_switch_guard" => {
                shadowed = trimmed_node_text(node, source)
                    .map(|surface| go_navigation_special_names(&surface))
                    .unwrap_or_default()
                    .iter()
                    .any(|declared| declared == name);
            }
            _ => {}
        }
    });
    shadowed
}

fn go_file_scope_names(root: TsNode<'_>, source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut cursor = root.walk();
    for declaration in root.named_children(&mut cursor) {
        count_go_navigation_resolution_work(1);
        match declaration.kind() {
            "function_declaration" => {
                if let Some(name) = declaration
                    .child_by_field_name("name")
                    .and_then(|node| trimmed_node_text(node, source))
                {
                    names.insert(name);
                }
            }
            "type_declaration" | "var_declaration" | "const_declaration" => {
                let mut declaration_cursor = declaration.walk();
                let mut pending = declaration
                    .named_children(&mut declaration_cursor)
                    .collect::<Vec<_>>();
                while let Some(spec) = pending.pop() {
                    count_go_navigation_resolution_work(1);
                    if spec.kind() == "var_spec_list" {
                        let mut list_cursor = spec.walk();
                        pending.extend(spec.named_children(&mut list_cursor));
                        continue;
                    }
                    if !matches!(spec.kind(), "type_spec" | "var_spec" | "const_spec") {
                        continue;
                    }
                    let mut name_cursor = spec.walk();
                    for name_node in spec.children_by_field_name("name", &mut name_cursor) {
                        if let Some(name) = normalized_receiver_variable(name_node, source) {
                            names.insert(name);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    names
}

fn go_composite_literal_owner_binding_from_type(
    type_surface: &str,
    import_bindings: &HashMap<String, String>,
) -> OptionalReceiverOwnerBinding {
    let type_surface = type_surface.trim();
    let (qualifier, owner_name) = go_exact_type_surface(type_surface)?;
    if !owner_name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
    {
        return None;
    }
    if let Some(qualifier) = qualifier {
        let module_name = import_bindings.get(&qualifier)?;
        return Some((owner_name, Some(module_name.clone())));
    }
    Some((owner_name, None))
}

fn collect_go_method_receiver_bindings<'tree>(
    callable: TsNode<'_>,
    source: &str,
    import_bindings: &HashMap<String, String>,
    package_owner_specs: &HashMap<String, Option<TsNode<'tree>>>,
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
    let Some(Some(owner_node)) = package_owner_specs.get(&owner_name) else {
        return receiver_types;
    };
    for (field_name, field_owner) in
        collect_go_struct_field_types(*owner_node, source, import_bindings)
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
    go_exact_type_surface(raw_type)?.0
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

#[cfg(test)]
mod complexity_tests {
    use super::*;
    use tree_sitter::Parser;

    fn measured_receiver_work(binding_count: usize, call_count: usize) -> usize {
        let mut source = String::from(
            "package proof\ntype Worker struct{}\nfunc (*Worker) Run() {}\nfunc caller() {\n",
        );
        for index in 0..binding_count {
            source.push_str(&format!("  worker{index} := &Worker{{}}\n"));
        }
        for index in 0..call_count {
            source.push_str(&format!("  worker{}.Run()\n", index % binding_count.max(1)));
        }
        source.push_str("}\n");
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .expect("Go grammar must load");
        let tree = parser.parse(&source, None).expect("Go source must parse");
        reset_go_navigation_resolution_work();
        let _ = receiver_call_specs(&tree, &source);
        go_navigation_resolution_work()
    }

    fn measured_method_identity_collection_work(owner_count: usize) -> usize {
        let mut source = String::from("package proof\ntype (\n");
        for index in 0..owner_count {
            source.push_str(&format!("  Owner{index} struct{{}}\n"));
        }
        source.push_str(")\n");
        for index in 0..owner_count {
            source.push_str(&format!("func (Owner{index}) Method{index}() {{}}\n"));
        }
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .expect("Go grammar must load");
        let tree = parser.parse(&source, None).expect("Go source must parse");
        reset_go_navigation_resolution_work();
        let specs = member_edge_specs(&tree, &source);
        assert_eq!(specs.len(), owner_count);
        go_navigation_resolution_work()
    }

    fn measured_complete_receiver_call_work(callable_count: usize) -> usize {
        let mut source = String::from("package proof\ntype (\n");
        for index in 0..callable_count {
            if index % 2 == 0 {
                source.push_str(&format!("  Owner{index} struct{{}}\n"));
            }
        }
        source.push_str(")\n");
        for index in 0..callable_count {
            let owner = if index % 2 == 0 {
                format!("Owner{index}")
            } else {
                format!("CrossFileOwner{index}")
            };
            source.push_str(&format!(
                "func (value *{owner}) Method{index}() {{ copy := new({owner}); copy.Method{index}() }}\n"
            ));
        }
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .expect("Go grammar must load");
        let tree = parser.parse(&source, None).expect("Go source must parse");
        reset_go_navigation_resolution_work();
        let specs = receiver_call_specs(&tree, &source);
        assert_eq!(specs.len(), callable_count);
        go_navigation_resolution_work()
    }

    #[test]
    fn receiver_binding_preparation_and_lookup_work_is_independently_linear() {
        let baseline = measured_receiver_work(32, 32);
        let more_bindings = measured_receiver_work(64, 32);
        let more_calls = measured_receiver_work(32, 64);
        let combined = measured_receiver_work(64, 64);
        assert!(baseline > 0, "Go navigation work was not counted");
        assert!(
            more_bindings <= baseline * 2 + 128,
            "Go receiver binding preparation grew superlinearly: {baseline} -> {more_bindings}"
        );
        assert!(
            more_calls <= baseline * 2 + 128,
            "Go receiver lookup work grew superlinearly: {baseline} -> {more_calls}"
        );
        assert!(
            combined <= baseline * 2 + 256,
            "combined Go receiver work grew superlinearly: {baseline} -> {combined}"
        );
    }

    #[test]
    fn grouped_method_identity_collection_work_is_linear() {
        let baseline = measured_method_identity_collection_work(64);
        let doubled = measured_method_identity_collection_work(128);
        assert!(baseline > 0, "Go method identity work was not counted");
        assert!(
            doubled <= baseline * 2 + 64,
            "Go method identity collection grew superlinearly: {baseline} -> {doubled}"
        );
    }

    #[test]
    fn complete_receiver_call_collection_work_is_linear() {
        let baseline = measured_complete_receiver_call_work(24);
        let doubled = measured_complete_receiver_call_work(48);
        assert!(baseline > 0, "Go receiver-call work was not counted");
        assert!(
            doubled <= baseline * 2 + 512,
            "complete Go receiver-call collection grew superlinearly: {baseline} -> {doubled}"
        );
    }
}
