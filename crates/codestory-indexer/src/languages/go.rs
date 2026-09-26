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
pub(crate) const PACKAGE_FUNCTION_CALLSITE_MARKER: &str = "syntax:go-package-function";
pub(crate) const PACKAGE_FUNCTION_IMPORT_SET_PREFIX: &str = "syntax:go-package-function:";
pub(crate) const RETURN_PATH_OWNER_PREFIX: &str = "go-return-path:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoReturnPath {
    pub module: String,
    pub package_name: Option<String>,
    pub function: String,
    pub result_index: usize,
    pub result_arity: usize,
    pub methods: Vec<String>,
}

impl GoReturnPath {
    fn owner_marker(&self) -> String {
        format!(
            "{RETURN_PATH_OWNER_PREFIX}{}\t{}\t{}\t{}\t{}\t{}",
            self.module,
            self.package_name.as_deref().unwrap_or_default(),
            self.function,
            self.result_index,
            self.result_arity,
            self.methods.join(",")
        )
    }

    pub(crate) fn from_owner_marker(marker: &str) -> Option<Self> {
        let encoded = marker.strip_prefix(RETURN_PATH_OWNER_PREFIX)?;
        let mut parts = encoded.splitn(6, '\t');
        let module = parts.next()?.to_string();
        let package_name = parts
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        let function = parts.next()?.to_string();
        let result_index = parts.next()?.parse().ok()?;
        let result_arity = parts.next()?.parse().ok()?;
        let methods = parts
            .next()
            .unwrap_or_default()
            .split(',')
            .filter(|method| !method.is_empty())
            .map(str::to_string)
            .collect();
        (!module.is_empty() && !function.is_empty()).then_some(Self {
            module,
            package_name,
            function,
            result_index,
            result_arity,
            methods,
        })
    }
}

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
    let package_imports = collect_go_package_imports(tree.root_node(), source);
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
        let method_receiver_bindings = collect_go_method_receiver_bindings(
            callable,
            source,
            &import_bindings,
            &package_owner_specs,
        );
        let resolution_context = GoCallableResolutionContext {
            source,
            import_bindings: &import_bindings,
            return_imports: &package_imports,
            file_scope_names: &file_scope_names,
            callable_name_visibility: go_callable_name_visibility(callable, source),
        };
        let mut local_binding_callsites = HashSet::new();
        collect_go_local_composite_receiver_call_specs(
            callable,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &resolution_context,
            &mut local_binding_callsites,
            &mut edges,
        );
        collect_go_package_function_call_specs(
            callable,
            source,
            ManualReceiverSource {
                name: call_source.name,
                span: call_source.span,
            },
            &package_imports,
            &file_scope_names,
            &resolution_context.callable_name_visibility,
            &local_binding_callsites,
            &mut edges,
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

#[derive(Default)]
struct GoPackageImports {
    explicit: HashMap<String, String>,
    implicit: Vec<String>,
}

struct GoCallableResolutionContext<'a> {
    source: &'a str,
    import_bindings: &'a HashMap<String, String>,
    return_imports: &'a GoPackageImports,
    file_scope_names: &'a HashSet<String>,
    callable_name_visibility: GoNameVisibilityIndex,
}

#[derive(Default)]
struct GoNameVisibilityIndex {
    names: HashMap<String, Vec<GoNameVisibilityPoint>>,
}

struct GoNameVisibilityPoint {
    start_byte: usize,
    prefix_max_end_byte: usize,
}

impl GoNameVisibilityIndex {
    fn is_visible(&self, name: &str, at_byte: usize) -> bool {
        let Some(points) = self.names.get(name) else {
            return false;
        };
        let count = points.partition_point(|point| {
            count_go_navigation_resolution_work(1);
            point.start_byte <= at_byte
        });
        count > 0 && points[count - 1].prefix_max_end_byte > at_byte
    }
}

fn collect_go_package_function_call_specs(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    imports: &GoPackageImports,
    file_scope_names: &HashSet<String>,
    callable_name_visibility: &GoNameVisibilityIndex,
    local_binding_callsites: &HashSet<ReceiverCallSiteKey>,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    walk_tree_nodes(callable, &mut |node| {
        if !receiver_call_belongs_to_callable(node, callable) {
            return;
        }
        let Some((receiver_name, method_name)) = selector_call(node, source) else {
            return;
        };
        let method_col = member_call_method_col(node, source, &method_name);
        let key = ReceiverCallSiteKey {
            receiver_name: receiver_name.clone(),
            method_name: method_name.clone(),
            line: Some(node.start_position().row as u32 + 1),
            method_col,
        };
        if local_binding_callsites.contains(&key)
            || file_scope_names.contains(&receiver_name)
            || callable_name_visibility.is_visible(&receiver_name, node.start_byte())
        {
            return;
        }
        let (owner_module, binding_marker) =
            if let Some(module) = imports.explicit.get(&receiver_name) {
                (
                    Some(module.clone()),
                    PACKAGE_FUNCTION_CALLSITE_MARKER.to_string(),
                )
            } else if imports.implicit.is_empty() {
                return;
            } else {
                (
                    None,
                    format!(
                        "{PACKAGE_FUNCTION_IMPORT_SET_PREFIX}{}",
                        imports.implicit.join(",")
                    ),
                )
            };
        edges.push(ManualReceiverCallSpec {
            source_name: call_source.name.to_string(),
            source_span: call_source.span,
            receiver_name: receiver_name.clone(),
            owner_name: receiver_name,
            owner_module,
            method_name,
            method_col,
            line: Some(node.start_position().row as u32 + 1),
            allow_global_fallback: false,
            binding_marker: Some(binding_marker),
            required_callsite_marker: None,
            class_anchored: false,
            owner_is_syntactic: false,
        });
    });
}

fn collect_go_local_composite_receiver_call_specs(
    callable: TsNode<'_>,
    call_source: ManualReceiverSource<'_>,
    context: &GoCallableResolutionContext<'_>,
    local_binding_callsites: &mut HashSet<ReceiverCallSiteKey>,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    let source = context.source;
    let mut calls = Vec::new();
    let mut intervals = Vec::new();
    let mut top_level_return_bindings = HashMap::<String, GoReturnPath>::new();
    let method_receiver_name = callable
        .child_by_field_name("receiver")
        .and_then(|receiver| go_receiver_variable_name(receiver, source));
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
        let Some(scope_end) = go_navigation_binding_scope(node, callable) else {
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
                let left_arity = left.len();
                let tuple_result = right.len() == 1 && left_arity > 1;
                let top_level_binding = go_binding_is_in_callable_outer_block(node, callable);
                let builtin_new_unshadowed = !context.import_bindings.contains_key("new")
                    && !context.file_scope_names.contains("new")
                    && !context
                        .callable_name_visibility
                        .is_visible("new", node.start_byte());
                for (index, left) in left.into_iter().enumerate() {
                    let Some(name) = normalized_receiver_variable(left, source) else {
                        continue;
                    };
                    let owner = if node.kind() == "assignment_statement" && !top_level_binding {
                        None
                    } else {
                        right
                            .get(index)
                            .and_then(|value| {
                                go_direct_composite_literal_owner(
                                    *value,
                                    source,
                                    context.import_bindings,
                                    builtin_new_unshadowed,
                                )
                            })
                            .or_else(|| {
                                let value = if tuple_result {
                                    right.first()?
                                } else {
                                    right.get(index)?
                                };
                                go_return_path_from_expression(
                                    *value,
                                    context,
                                    if tuple_result { index } else { 0 },
                                    if tuple_result { left_arity } else { 1 },
                                    0,
                                )
                                .map(|path| (path.owner_marker(), None))
                                .or_else(|| {
                                    top_level_binding
                                        .then(|| {
                                            go_return_path_from_bound_method(
                                                *value,
                                                source,
                                                &top_level_return_bindings,
                                            )
                                        })
                                        .flatten()
                                        .map(|path| (path.owner_marker(), None))
                                })
                            })
                    };
                    if top_level_binding {
                        if let Some(path) = owner
                            .as_ref()
                            .and_then(|(owner, _)| GoReturnPath::from_owner_marker(owner))
                        {
                            top_level_return_bindings.insert(name.clone(), path);
                        } else {
                            top_level_return_bindings.remove(&name);
                        }
                    }
                    intervals.push(GoNavigationBindingInterval {
                        name,
                        start_byte: node.end_byte(),
                        end_byte: if node.kind() == "assignment_statement" {
                            callable.end_byte()
                        } else {
                            scope_end
                        },
                        binding_priority: if node.kind() == "assignment_statement"
                            && owner.is_none()
                        {
                            GO_NAVIGATION_UNCERTAIN_ASSIGNMENT_PRIORITY
                        } else {
                            GO_NAVIGATION_DECLARATION_PRIORITY
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
                    .and_then(|raw_type| {
                        go_receiver_owner_from_type(raw_type, context.import_bindings)
                    });
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
                        binding_priority: GO_NAVIGATION_DECLARATION_PRIORITY,
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
                        binding_priority: GO_NAVIGATION_DECLARATION_PRIORITY,
                        owner: None,
                    });
                }
            }
            "range_clause" | "receive_statement" | "type_switch_guard" => {
                if let Some((names, end_byte, priority)) =
                    go_navigation_special_binding(node, callable, source)
                {
                    for name in names {
                        intervals.push(GoNavigationBindingInterval {
                            name,
                            start_byte: node.end_byte(),
                            end_byte,
                            binding_priority: priority,
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
                            binding_priority: GO_NAVIGATION_DECLARATION_PRIORITY,
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
                        binding_priority: GO_NAVIGATION_CAPTURE_PRIORITY,
                        owner: None,
                    });
                }
            }
            _ => {}
        }
        if node.kind() != "identifier" {
            return;
        }
        let Some((capture_start, capture_end)) = go_navigation_capture_span(node, callable) else {
            return;
        };
        let Some(name) = normalized_receiver_variable(node, source) else {
            return;
        };
        // Capturing a statically declared method receiver does not change its
        // Go type outside that closure. Keep nested closure calls fail closed,
        // and keep real local shadow intervals above, without erasing the
        // enclosing receiver's owner for the rest of the method.
        if method_receiver_name.as_deref() == Some(name.as_str()) {
            intervals.push(GoNavigationBindingInterval {
                name,
                start_byte: capture_start,
                end_byte: capture_end,
                binding_priority: GO_NAVIGATION_CAPTURE_PRIORITY,
                owner: None,
            });
            return;
        }
        intervals.push(GoNavigationBindingInterval {
            name,
            start_byte: callable.start_byte(),
            end_byte: callable.end_byte(),
            binding_priority: GO_NAVIGATION_CAPTURE_PRIORITY,
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
        let inferred_from_expression = go_selector_receiver_node(call.node).and_then(|receiver| {
            go_return_path_from_expression(receiver, context, 0, 1, 0)
                .map(|path| (path.owner_marker(), None))
        });
        let decision = decisions.get(&call.node.id());
        let (owner, handled) = match decision {
            Some(owner) => (owner.clone(), true),
            None => {
                let handled = inferred_from_expression.is_some();
                (inferred_from_expression, handled)
            }
        };
        if !handled {
            continue;
        }
        let method_col = member_call_method_col(call.node, source, &call.method_name);
        local_binding_callsites.insert(ReceiverCallSiteKey {
            receiver_name: call.receiver_name.clone(),
            method_name: call.method_name.clone(),
            line: Some(call.node.start_position().row as u32 + 1),
            method_col,
        });
        let Some((owner_name, owner_module)) = owner else {
            continue;
        };
        edges.push(ManualReceiverCallSpec {
            source_name: call_source.name.to_string(),
            source_span: call_source.span,
            receiver_name: call.receiver_name,
            owner_name,
            owner_module,
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

struct GoNavigationCall<'tree> {
    node: TsNode<'tree>,
    receiver_name: String,
    method_name: String,
}

struct GoNavigationBindingInterval {
    name: String,
    start_byte: usize,
    end_byte: usize,
    binding_priority: usize,
    owner: OptionalReceiverOwnerBinding,
}

const GO_NAVIGATION_DECLARATION_PRIORITY: usize = 0;
// Lexical declarations compete by their activation byte, so an inner header,
// block, or case declaration shadows an outer one until its interval ends.
// Writes and captures stay above declarations because they deliberately deny
// stale owner inference when the assigned value or closure flow is uncertain.
const GO_NAVIGATION_SPECIAL_WRITE_PRIORITY: usize = usize::MAX - 2;
const GO_NAVIGATION_UNCERTAIN_ASSIGNMENT_PRIORITY: usize = usize::MAX - 1;
const GO_NAVIGATION_CAPTURE_PRIORITY: usize = usize::MAX;

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
            if let Some(priorities) = active.get_mut(&interval.name) {
                if let Some(entries) = priorities.get_mut(&interval.binding_priority) {
                    entries.remove(&index);
                    if entries.is_empty() {
                        priorities.remove(&interval.binding_priority);
                    }
                }
                if priorities.is_empty() {
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
                .entry(interval.binding_priority)
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

fn go_navigation_binding_scope(node: TsNode<'_>, callable: TsNode<'_>) -> Option<usize> {
    let container = go_nearest_navigation_lexical_container(node, callable)?;
    Some(container.end_byte())
}

fn go_binding_is_in_callable_outer_block(node: TsNode<'_>, callable: TsNode<'_>) -> bool {
    let Some(body) = callable.child_by_field_name("body") else {
        return false;
    };
    go_nearest_navigation_lexical_container(node, callable)
        .is_some_and(|container| container.kind() == "block" && container.id() == body.id())
}

fn go_nearest_navigation_lexical_container<'tree>(
    mut node: TsNode<'tree>,
    callable: TsNode<'tree>,
) -> Option<TsNode<'tree>> {
    while let Some(parent) = node.parent() {
        count_go_navigation_resolution_work(1);
        if matches!(
            parent.kind(),
            "block"
                | "if_statement"
                | "for_statement"
                | "expression_switch_statement"
                | "type_switch_statement"
                | "expression_case"
                | "default_case"
                | "communication_case"
        ) {
            return Some(parent);
        }
        if parent.id() == callable.id() {
            return Some(parent);
        }
        node = parent;
    }
    None
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
            GO_NAVIGATION_DECLARATION_PRIORITY
        } else {
            GO_NAVIGATION_SPECIAL_WRITE_PRIORITY
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

fn go_navigation_capture_span(
    mut node: TsNode<'_>,
    callable: TsNode<'_>,
) -> Option<(usize, usize)> {
    while node.id() != callable.id() {
        if node.kind() == "func_literal" {
            return Some((node.start_byte(), node.end_byte()));
        }
        node = node.parent()?;
    }
    None
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

fn go_selector_receiver_node(call: TsNode<'_>) -> Option<TsNode<'_>> {
    let function = call.child_by_field_name("function")?;
    (function.kind() == "selector_expression")
        .then(|| function.child_by_field_name("operand"))
        .flatten()
}

fn go_return_path_from_expression(
    expression: TsNode<'_>,
    context: &GoCallableResolutionContext<'_>,
    result_index: usize,
    result_arity: usize,
    depth: usize,
) -> Option<GoReturnPath> {
    if expression.kind() != "call_expression" || depth >= 64 {
        return None;
    }
    let function = expression.child_by_field_name("function")?;
    if function.kind() == "identifier" {
        let name = trimmed_node_text(function, context.source)?;
        if context
            .callable_name_visibility
            .is_visible(&name, function.start_byte())
        {
            return None;
        }
        return Some(GoReturnPath {
            module: ".".to_string(),
            package_name: None,
            function: name,
            result_index,
            result_arity,
            methods: Vec::new(),
        });
    }
    if function.kind() != "selector_expression" {
        return None;
    }
    let operand = function.child_by_field_name("operand")?;
    let member = function
        .child_by_field_name("field")
        .and_then(|field| trimmed_node_text(field, context.source))?;
    if operand.kind() == "identifier" {
        let qualifier = trimmed_node_text(operand, context.source)?;
        if context.file_scope_names.contains(&qualifier)
            || context
                .callable_name_visibility
                .is_visible(&qualifier, operand.start_byte())
        {
            return None;
        }
        let (module, package_name) =
            if let Some(module) = context.return_imports.explicit.get(&qualifier) {
                (module.clone(), None)
            } else if !context.return_imports.implicit.is_empty() {
                (context.return_imports.implicit.join(","), Some(qualifier))
            } else {
                return None;
            };
        return Some(GoReturnPath {
            module,
            package_name,
            function: member,
            result_index,
            result_arity,
            methods: Vec::new(),
        });
    }
    let mut path =
        go_return_path_from_expression(operand, context, result_index, result_arity, depth + 1)?;
    path.methods.push(member);
    Some(path)
}

fn go_return_path_from_bound_method(
    expression: TsNode<'_>,
    source: &str,
    bindings: &HashMap<String, GoReturnPath>,
) -> Option<GoReturnPath> {
    if expression.kind() != "call_expression" {
        return None;
    }
    let function = expression.child_by_field_name("function")?;
    if function.kind() != "selector_expression" {
        return None;
    }
    let receiver = function.child_by_field_name("operand")?;
    if receiver.kind() != "identifier" {
        return None;
    }
    let receiver_name = trimmed_node_text(receiver, source)?;
    let method = function
        .child_by_field_name("field")
        .and_then(|field| trimmed_node_text(field, source))?;
    let mut path = bindings.get(&receiver_name)?.clone();
    if path.methods.len() >= 64 {
        return None;
    }
    path.methods.push(method);
    Some(path)
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

fn go_callable_name_visibility(callable: TsNode<'_>, source: &str) -> GoNameVisibilityIndex {
    let mut spans = HashMap::<String, Vec<(usize, usize)>>::new();
    walk_tree_nodes(callable, &mut |node| {
        count_go_navigation_resolution_work(1);
        let (names, start_byte, end_byte) = match node.kind() {
            "parameter_declaration" | "variadic_parameter_declaration" => {
                let Some((start_byte, end_byte)) = go_parameter_visibility_span(node, callable)
                else {
                    return;
                };
                let type_start = node
                    .child_by_field_name("type")
                    .map(|type_node| type_node.start_byte())
                    .unwrap_or(usize::MAX);
                let mut cursor = node.walk();
                let names = node
                    .named_children(&mut cursor)
                    .take_while(|child| child.start_byte() < type_start)
                    .filter_map(|child| normalized_receiver_variable(child, source))
                    .collect::<Vec<_>>();
                (names, start_byte, end_byte)
            }
            "short_var_declaration" | "assignment_statement" => {
                let Some(end_byte) = go_navigation_binding_scope(node, callable) else {
                    return;
                };
                let names = node
                    .child_by_field_name("left")
                    .map(go_expression_list_items)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|left| normalized_receiver_variable(left, source))
                    .collect::<Vec<_>>();
                (names, node.end_byte(), end_byte)
            }
            "var_spec" | "const_spec" | "type_spec" => {
                let Some(end_byte) = go_navigation_binding_scope(node, callable) else {
                    return;
                };
                (
                    go_navigation_declared_names(node, source),
                    node.end_byte(),
                    end_byte,
                )
            }
            "range_clause" | "receive_statement" | "type_switch_guard" => {
                let Some((names, end_byte, _)) =
                    go_navigation_special_binding(node, callable, source)
                else {
                    return;
                };
                (names, node.end_byte(), end_byte)
            }
            _ => return,
        };
        if start_byte >= end_byte {
            return;
        }
        for name in names {
            spans.entry(name).or_default().push((start_byte, end_byte));
        }
    });
    let names = spans
        .into_iter()
        .map(|(name, mut spans)| {
            spans.sort_unstable_by(|left, right| {
                count_go_navigation_resolution_work(1);
                left.cmp(right)
            });
            debug_assert!(spans.windows(2).all(|pair| pair[0].0 <= pair[1].0));
            let mut prefix_max_end_byte = 0usize;
            let points = spans
                .into_iter()
                .map(|(start_byte, end_byte)| {
                    prefix_max_end_byte = prefix_max_end_byte.max(end_byte);
                    GoNameVisibilityPoint {
                        start_byte,
                        prefix_max_end_byte,
                    }
                })
                .collect();
            (name, points)
        })
        .collect();
    GoNameVisibilityIndex { names }
}

fn go_parameter_visibility_span(
    mut node: TsNode<'_>,
    callable: TsNode<'_>,
) -> Option<(usize, usize)> {
    loop {
        if node.id() == callable.id() || node.kind() == "func_literal" {
            let body = node.child_by_field_name("body")?;
            return Some((body.start_byte(), body.end_byte()));
        }
        node = node.parent()?;
    }
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

fn collect_go_package_imports(root: TsNode<'_>, source: &str) -> GoPackageImports {
    let mut imports = GoPackageImports::default();
    let mut duplicate_explicit = HashSet::new();
    walk_tree_nodes(root, &mut |node| {
        if node.kind() != "import_spec" || node.has_error() || node.is_missing() {
            return;
        }
        let Some(module) = node
            .child_by_field_name("path")
            .and_then(|path| trimmed_node_text(path, source))
            .and_then(|path| go_import_module_name(&path))
        else {
            return;
        };
        let alias = node
            .child_by_field_name("name")
            .and_then(|name| trimmed_node_text(name, source));
        match alias.as_deref() {
            Some("." | "_") => {}
            Some(alias) => {
                if let Some(alias) = normalize_parameter_name(alias) {
                    insert_unique_import_binding(
                        &mut imports.explicit,
                        &mut duplicate_explicit,
                        alias,
                        module,
                    );
                }
            }
            None => imports.implicit.push(module),
        }
    });
    imports.implicit.sort();
    imports.implicit.dedup();
    imports
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

pub(crate) fn parse_module_path(source: &str) -> Option<String> {
    if source.contains("/*") || source.contains("*/") {
        return None;
    }
    let mut module = None;
    for raw_line in source.lines() {
        let line = go_strip_line_comment(raw_line).trim();
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        if !tokens.contains(&"module") {
            continue;
        }
        if tokens.first().copied() != Some("module") || tokens.len() != 2 || module.is_some() {
            return None;
        }
        let value = go_module_directive_path(tokens[1])?;
        module = Some(value);
    }
    module
}

fn go_module_directive_path(token: &str) -> Option<String> {
    let value = if let Some(value) = token.strip_prefix('"') {
        let value = value.strip_suffix('"')?;
        if value.contains(['"', '\\']) {
            return None;
        }
        value
    } else if let Some(value) = token.strip_prefix('`') {
        let value = value.strip_suffix('`')?;
        if value.contains('`') {
            return None;
        }
        value
    } else {
        if token.contains(['"', '`']) {
            return None;
        }
        token
    };
    if value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains(['(', ')', '|', ','])
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '-' | '_' | '~'))
        || value
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return None;
    }
    Some(value.to_string())
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

#[derive(Debug, Clone)]
pub(crate) struct GoReturnDeclaration {
    pub function: String,
    pub owner: Option<String>,
    pub results: Vec<Option<String>>,
}

pub(crate) fn go_concrete_return_types(tree: &Tree, source: &str) -> HashSet<String> {
    let mut concrete_types = HashSet::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "type_spec" || node.child_by_field_name("type_parameters").is_some() {
            return;
        }
        let Some(type_node) = node.child_by_field_name("type") else {
            return;
        };
        // A defined Go type may declare methods regardless of whether its
        // underlying type is a struct, primitive, slice, map, or function.
        // Aliases use a distinct `type_alias` node, while interfaces and
        // parameterized declarations remain intentionally unsupported here.
        if type_node.kind() == "interface_type" {
            return;
        }
        if let Some(name) = node
            .child_by_field_name("name")
            .and_then(|name| trimmed_node_text(name, source))
        {
            concrete_types.insert(name);
        }
    });
    concrete_types
}

pub(crate) fn go_return_declarations(tree: &Tree, source: &str) -> Vec<GoReturnDeclaration> {
    let mut declarations = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if !matches!(node.kind(), "function_declaration" | "method_declaration")
            || node.has_error()
            || node.is_missing()
            || node.child_by_field_name("type_parameters").is_some()
        {
            return;
        }
        let Some(function) = node
            .child_by_field_name("name")
            .and_then(|name| trimmed_node_text(name, source))
        else {
            return;
        };
        let owner = node
            .child_by_field_name("receiver")
            .and_then(|receiver| go_receiver_owner_name(receiver, source));
        let results = node
            .child_by_field_name("result")
            .map(|result| go_declared_result_owners(result, source))
            .unwrap_or_default();
        declarations.push(GoReturnDeclaration {
            function,
            owner,
            results,
        });
    });
    declarations
}

fn go_declared_result_owners(result: TsNode<'_>, source: &str) -> Vec<Option<String>> {
    if result.kind() != "parameter_list" {
        return vec![go_concrete_result_owner(result, source)];
    }
    let mut results = Vec::new();
    let mut cursor = result.walk();
    for parameter in result.named_children(&mut cursor) {
        if parameter.kind() != "parameter_declaration" {
            results.push(None);
            continue;
        }
        let Some(type_node) = parameter.child_by_field_name("type") else {
            results.push(None);
            continue;
        };
        let owner = go_concrete_result_owner(type_node, source);
        let mut parameter_cursor = parameter.walk();
        let named_count = parameter
            .named_children(&mut parameter_cursor)
            .filter(|child| child.end_byte() <= type_node.start_byte())
            .filter(|child| matches!(child.kind(), "identifier" | "field_identifier"))
            .count();
        results.extend(std::iter::repeat_n(owner, named_count.max(1)));
    }
    results
}

fn go_concrete_result_owner(type_node: TsNode<'_>, source: &str) -> Option<String> {
    let surface = trimmed_node_text(type_node, source)?;
    let surface = surface.trim();
    let owner = surface.strip_prefix('*').unwrap_or(surface).trim();
    if owner.starts_with('*') || normalize_parameter_name(owner).as_deref() != Some(owner) {
        return None;
    }
    Some(owner.to_string())
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

    fn measured_factory_assignment_work(assignment_count: usize, imported: bool) -> usize {
        let mut source = if imported {
            String::from("package proof\nimport worker \"example.com/worker\"\nfunc caller() {\n")
        } else {
            String::from(
                "package proof\ntype Worker struct{}\nfunc New() *Worker { return nil }\nfunc caller() {\n",
            )
        };
        for index in 0..assignment_count {
            let factory = if imported { "worker.New()" } else { "New()" };
            source.push_str(&format!(
                "  value{index} := {factory}\n  value{index}.Finish()\n"
            ));
        }
        source.push_str("}\n");
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .expect("Go grammar must load");
        let tree = parser.parse(&source, None).expect("Go source must parse");
        reset_go_navigation_resolution_work();
        let specs = receiver_call_specs(&tree, &source);
        assert!(
            specs.len() >= assignment_count,
            "each factory assignment must retain its receiver call"
        );
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

    fn assert_factory_assignment_work_is_linear(imported: bool) {
        let work_32 = measured_factory_assignment_work(32, imported);
        let work_64 = measured_factory_assignment_work(64, imported);
        let work_128 = measured_factory_assignment_work(128, imported);
        assert!(work_32 > 0, "Go factory return work was not counted");
        assert!(
            work_64 <= work_32 * 2 + 512,
            "Go factory return work grew superlinearly at 64 assignments (imported={imported}): {work_32} -> {work_64}"
        );
        assert!(
            work_128 <= work_64 * 2 + 512,
            "Go factory return work grew superlinearly at 128 assignments (imported={imported}): {work_64} -> {work_128}"
        );
    }

    #[test]
    fn local_factory_assignment_return_inference_work_is_linear() {
        assert_factory_assignment_work_is_linear(false);
    }

    #[test]
    fn imported_factory_assignment_return_inference_work_is_linear() {
        assert_factory_assignment_work_is_linear(true);
    }
}
