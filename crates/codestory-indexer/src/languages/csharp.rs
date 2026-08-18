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
    CompiledLanguageRules, GraphNodeSpan, ImportedTypeBinding, LanguageRuleset,
    ManualMemberEdgeSpec, ManualReceiverCallSpec, ManualReceiverSource, ManualTypeUsageSpec,
    OptionalReceiverOwnerBinding, ReceiverCallSiteKey, collect_enclosing_type_member_edges,
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
    type_usage_specs: Some(type_usage_specs),
    callsite_marker_families: &[("csharp_member", MEMBER_CALLSITE_MARKER)],
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
/// Was `lib.rs::collect_csharp_receiver_call_edges`. P2b extends the walk to
/// `constructor_declaration` bodies: C# emits no constructor node, so those
/// specs are CLASS-anchored — their source is the enclosing class node — and
/// carry the `class_anchored` flag the engine's source resolution reads.
pub(crate) fn receiver_call_specs(tree: &Tree, source: &str) -> Vec<ManualReceiverCallSpec> {
    let mut edges = Vec::new();
    let root = tree.root_node();
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        let (source_name, source_span, class_anchored, precise_only) = match callable.kind() {
            "method_declaration" => {
                let Some(name) = declaration_name(callable, source) else {
                    return;
                };
                (name, ts_node_graph_span(callable), false, false)
            }
            "constructor_declaration" => {
                let Some(class_node) = enclosing_node_with_kind(
                    callable,
                    &["class_declaration", "struct_declaration"],
                ) else {
                    return;
                };
                let Some(name) = declaration_name(class_node, source) else {
                    return;
                };
                (name, ts_node_graph_span(class_node), true, false)
            }
            // Field and property initializers run in constructor context;
            // their receiver calls (chiefly `new X(args).Method()` chains)
            // anchor at the type itself. Only the precise pass applies — a
            // type body has no parameter receivers — and the
            // belongs-to-callable filter keeps exactly the calls whose
            // nearest boundary is this type, so method and constructor
            // bodies are never double-collected.
            "class_declaration" | "struct_declaration" => {
                let Some(name) = declaration_name(callable, source) else {
                    return;
                };
                (name, ts_node_graph_span(callable), true, true)
            }
            _ => return,
        };
        let callable_start = edges.len();
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
            span: source_span,
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
        if !precise_only && !receiver_types.is_empty() {
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
                    && let Some(module_name) = csharp_plain_namespace_import_type_module(
                        &spec.owner_name,
                        &namespace_imports,
                    )
                {
                    spec.owner_module = Some(module_name);
                }
            }
            edges.extend(parameter_specs);
        }
        if class_anchored {
            for spec in &mut edges[callable_start..] {
                spec.class_anchored = true;
            }
        }
    });
    edges
}

/// Manual type-usage facts for one parsed C# file (P2a).
///
/// Three families feed the channel: field declarations (CLASS-anchored),
/// method and constructor parameter types (METHOD- and CLASS-anchored
/// respectively), and object creations (anchored at the enclosing method, or
/// at the enclosing class for constructor bodies and field initializers).
///
/// The emit gate is the whole point: a spec exists ONLY when the type surface
/// resolved against this file's binding tables — same-file type declarations,
/// the `using`-alias table, or the single plain namespace import. The
/// unknown-type fallthrough that `csharp_receiver_owner_from_type` keeps for
/// CALL annotation (it returns `Some((owner_name, None))` for ANY surface)
/// deliberately does not exist here: an unresolved type produces no spec, so
/// the engine's emit-time `certainty = Certain` stamp is justified for every
/// edge this collector produces.
pub(crate) fn type_usage_specs(tree: &Tree, source: &str) -> Vec<ManualTypeUsageSpec> {
    let mut specs = Vec::new();
    let root = tree.root_node();
    walk_tree_nodes(root, &mut |node| match node.kind() {
        "field_declaration" => {
            let Some(class_node) =
                enclosing_node_with_kind(node, &["class_declaration", "struct_declaration"])
            else {
                return;
            };
            let Some(variable_declaration) =
                first_descendant_with_kind(node, "variable_declaration")
            else {
                return;
            };
            let Some(type_node) = variable_declaration.child_by_field_name("type") else {
                return;
            };
            push_csharp_type_usage_spec(
                root,
                source,
                CsharpTypeUsageAnchor {
                    node: class_node,
                    class_anchored: true,
                },
                type_node,
                &mut specs,
            );
        }
        "parameter" => {
            // Method, constructor, and PRIMARY-constructor parameters feed
            // the channel (a C#12 primary constructor hangs its
            // parameter_list directly off the type declaration); lambda and
            // delegate parameters stay out of it.
            let Some(owner) = node.parent().and_then(|list| list.parent()) else {
                return;
            };
            let anchor = match owner.kind() {
                "method_declaration" => CsharpTypeUsageAnchor {
                    node: owner,
                    class_anchored: false,
                },
                "constructor_declaration" => {
                    let Some(class_node) = enclosing_node_with_kind(
                        owner,
                        &["class_declaration", "struct_declaration"],
                    ) else {
                        return;
                    };
                    CsharpTypeUsageAnchor {
                        node: class_node,
                        class_anchored: true,
                    }
                }
                // Primary constructor: the parameter list's owner IS the
                // type declaration — anchor there (P2's written rule for
                // constructor-context facts).
                "class_declaration" | "struct_declaration" => CsharpTypeUsageAnchor {
                    node: owner,
                    class_anchored: true,
                },
                _ => return,
            };
            let Some(type_node) = node.child_by_field_name("type") else {
                return;
            };
            push_csharp_type_usage_spec(root, source, anchor, type_node, &mut specs);
        }
        "object_creation_expression" => {
            // `new()` (implicit_object_creation_expression) has no type child
            // and never reaches this arm.
            let Some(type_node) = node.child_by_field_name("type") else {
                return;
            };
            let Some(anchor_node) = enclosing_node_with_kind(
                node,
                &[
                    "method_declaration",
                    "constructor_declaration",
                    "class_declaration",
                    "struct_declaration",
                ],
            ) else {
                return;
            };
            let anchor = match anchor_node.kind() {
                "method_declaration" => CsharpTypeUsageAnchor {
                    node: anchor_node,
                    class_anchored: false,
                },
                // Constructor bodies and field/property initializers are both
                // class-context facts (P2: class anchoring is the written rule
                // for constructor-context facts).
                "constructor_declaration" => {
                    let Some(class_node) = enclosing_node_with_kind(
                        anchor_node,
                        &["class_declaration", "struct_declaration"],
                    ) else {
                        return;
                    };
                    CsharpTypeUsageAnchor {
                        node: class_node,
                        class_anchored: true,
                    }
                }
                _ => CsharpTypeUsageAnchor {
                    node: anchor_node,
                    class_anchored: true,
                },
            };
            push_csharp_type_usage_spec(root, source, anchor, type_node, &mut specs);
        }
        _ => {}
    });
    specs
}

struct CsharpTypeUsageAnchor<'tree> {
    node: TsNode<'tree>,
    class_anchored: bool,
}

fn push_csharp_type_usage_spec(
    root: TsNode<'_>,
    source: &str,
    anchor: CsharpTypeUsageAnchor<'_>,
    type_node: TsNode<'_>,
    specs: &mut Vec<ManualTypeUsageSpec>,
) {
    let Some(source_name) = declaration_name(anchor.node, source) else {
        return;
    };
    let Some(raw_type) = trimmed_node_text(type_node, source) else {
        return;
    };
    let generic_type_parameters = csharp_generic_type_parameters_in_scope(type_node, source);
    let visible_type_spans = collect_csharp_visible_type_declaration_spans(root, type_node, source);
    let visible_type_names = visible_type_spans.keys().cloned().collect::<HashSet<_>>();
    let imported_type_bindings =
        collect_csharp_visible_imported_type_bindings(root, type_node, source, &visible_type_names);
    let namespace_imports = collect_csharp_visible_namespace_imports(root, type_node, source);
    let Some(target) = csharp_type_usage_target(
        &raw_type,
        CsharpTypeUsageTables {
            root,
            type_node,
            source,
            generic_type_parameters: &generic_type_parameters,
            visible_type_spans: &visible_type_spans,
            imported_type_bindings: &imported_type_bindings,
            namespace_imports: &namespace_imports,
        },
    ) else {
        return;
    };
    let (target_name, target_module, target_declaration_span, pending_namespace) = match target {
        CsharpTypeUsageTarget::SameFile {
            name,
            declaration_span,
        } => (name, None, Some(declaration_span), None),
        CsharpTypeUsageTarget::Imported { name, module } => (name, Some(module), None, None),
        CsharpTypeUsageTarget::SameRootPending {
            name,
            referencing_namespace,
        } => (name, None, None, Some(referencing_namespace)),
    };
    specs.push(ManualTypeUsageSpec {
        source_name,
        source_span: ts_node_graph_span(anchor.node),
        class_anchored: anchor.class_anchored,
        target_name,
        target_module,
        target_declaration_span,
        reference_span: ts_node_graph_span(type_node),
        line: Some(type_node.start_position().row as u32 + 1),
        pending_namespace,
    });
}

/// Predefined C# type keywords. They can never name a project declaration,
/// so the pending same-root channel refuses them outright instead of minting
/// pending facts that every finalize pass would re-check and delete.
const CSHARP_PREDEFINED_TYPE_KEYWORDS: &[&str] = &[
    "bool", "byte", "char", "decimal", "double", "float", "int", "long", "nint", "nuint", "object",
    "sbyte", "short", "string", "uint", "ulong", "ushort", "void",
];

enum CsharpTypeUsageTarget {
    /// Declared in this file; the edge binds the declaration node exactly.
    SameFile {
        name: String,
        declaration_span: GraphNodeSpan,
    },
    /// Resolved through an import table; the edge lands on a module-qualified
    /// reference node.
    Imported { name: String, module: String },
    /// A bare name no per-file table resolves, inside a namespaced file: the
    /// edge is emitted PENDING (uncertain) and the post-flush finalize pass
    /// proves or deletes it against the project's declarations under the
    /// same root namespace.
    SameRootPending {
        name: String,
        referencing_namespace: String,
    },
}

struct CsharpTypeUsageTables<'a, 'tree> {
    root: TsNode<'tree>,
    type_node: TsNode<'tree>,
    source: &'a str,
    generic_type_parameters: &'a HashSet<String>,
    visible_type_spans: &'a HashMap<String, GraphNodeSpan>,
    imported_type_bindings: &'a HashMap<String, ImportedTypeBinding>,
    namespace_imports: &'a CsharpNamespaceImports,
}

/// Resolve a type surface for the TYPE_USAGE channel, tables-first.
///
/// Fails closed on: generic type parameters in scope, `var`/`dynamic`,
/// predefined type keywords, qualified surfaces (they name a namespace
/// inline instead of resolving through a table), and — in files without a
/// namespace — any name none of the binding tables knows. The
/// `csharp_receiver_owner_from_type` unknown-type fallthrough is deliberately
/// absent here. The one non-table outcome is `SameRootPending`: a bare name
/// in a namespaced file defers to the post-flush declaration lookup, and the
/// edge it produces stays uncertain until that lookup proves it.
fn csharp_type_usage_target(
    raw_type: &str,
    tables: CsharpTypeUsageTables<'_, '_>,
) -> Option<CsharpTypeUsageTarget> {
    let trimmed = raw_type.trim();
    let base = trimmed
        .split(['<', '[', '?'])
        .next()
        .unwrap_or(trimmed)
        .trim();
    if base.contains('.') {
        return None;
    }
    let owner_name = normalize_type_surface(raw_type)?;
    if owner_name == "var"
        || owner_name == "dynamic"
        || CSHARP_PREDEFINED_TYPE_KEYWORDS.contains(&owner_name.as_str())
        || tables.generic_type_parameters.contains(&owner_name)
    {
        return None;
    }
    if let Some(span) = tables.visible_type_spans.get(&owner_name) {
        return Some(CsharpTypeUsageTarget::SameFile {
            name: owner_name,
            declaration_span: *span,
        });
    }
    if let Some(binding) = tables.imported_type_bindings.get(&owner_name) {
        return Some(CsharpTypeUsageTarget::Imported {
            name: binding.owner_name.clone(),
            module: binding.module_name.clone(),
        });
    }
    if let Some(module_name) =
        csharp_plain_namespace_import_type_module(&owner_name, tables.namespace_imports)
    {
        return Some(CsharpTypeUsageTarget::Imported {
            name: owner_name,
            module: module_name,
        });
    }
    let referencing_namespace =
        csharp_enclosing_namespace_path(tables.root, tables.type_node, tables.source)?;
    Some(CsharpTypeUsageTarget::SameRootPending {
        name: owner_name,
        referencing_namespace,
    })
}

/// Full namespace path enclosing a node: the file-scoped namespace (a SIBLING
/// of the declarations it governs, so the enclosing-ancestor walk never sees
/// it) followed by any block namespaces from outermost to innermost.
fn csharp_enclosing_namespace_path(
    root: TsNode<'_>,
    node: TsNode<'_>,
    source: &str,
) -> Option<String> {
    let mut segments = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "file_scoped_namespace_declaration"
            && let Some(name) = declaration_name(child, source)
        {
            segments.push(name);
            break;
        }
    }
    let mut block_segments = Vec::new();
    let mut current = Some(node);
    while let Some(candidate) = current {
        if candidate.kind() == "namespace_declaration"
            && let Some(name) = declaration_name(candidate, source)
        {
            block_segments.push(name);
        }
        current = candidate.parent();
    }
    segments.extend(block_segments.into_iter().rev());
    let path = segments.join(".");
    (!path.is_empty()).then_some(path)
}

/// Generic type parameters visible at a use site.
///
/// Walks the ancestor chain and collects every `type_parameter_list` declared
/// by an enclosing type or method, so `T` in `class Box<T> { private T item; }`
/// can never resolve through a namespace-import table.
fn csharp_generic_type_parameters_in_scope(node: TsNode<'_>, source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut current = Some(node);
    while let Some(candidate) = current {
        if matches!(
            candidate.kind(),
            "class_declaration"
                | "struct_declaration"
                | "interface_declaration"
                | "record_declaration"
                | "method_declaration"
                | "local_function_statement"
        ) {
            let mut cursor = candidate.walk();
            for child in candidate.named_children(&mut cursor) {
                if child.kind() != "type_parameter_list" {
                    continue;
                }
                let mut parameters = child.walk();
                for parameter in child.named_children(&mut parameters) {
                    if parameter.kind() == "type_parameter"
                        && let Some(name) = declaration_name(parameter, source)
                    {
                        names.insert(name);
                    }
                }
            }
        }
        current = candidate.parent();
    }
    names
}

/// Same shape as `collect_csharp_visible_type_binding_names`, but keeping the
/// declaration spans so a same-file TYPE_USAGE target binds the declaration
/// node exactly (name+span) instead of racing same-named reference nodes.
fn collect_csharp_visible_type_declaration_spans(
    root: TsNode<'_>,
    anchor: TsNode<'_>,
    source: &str,
) -> HashMap<String, GraphNodeSpan> {
    let mut spans = HashMap::new();
    collect_csharp_type_declaration_spans_in_scope(root, source, &mut spans);
    if let Some(namespace) = enclosing_node_with_kind(anchor, &["namespace_declaration"])
        && let Some(body) = namespace.child_by_field_name("body")
    {
        collect_csharp_type_declaration_spans_in_scope(body, source, &mut spans);
    }
    spans
}

fn collect_csharp_type_declaration_spans_in_scope(
    scope: TsNode<'_>,
    source: &str,
    spans: &mut HashMap<String, GraphNodeSpan>,
) {
    let mut cursor = scope.walk();
    for child in scope.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_declaration" | "interface_declaration" | "struct_declaration"
        ) && let Some(name) = declaration_name(child, source)
        {
            spans.insert(name, ts_node_graph_span(child));
        }
    }
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
            // A `new X(args).Method()` chain names its owner in the call
            // syntax itself; the flag lets the engine annotate the callsite
            // even when no module is known, so the resolution pass's
            // same-root-namespace arm can finish cross-file shapes that no
            // per-file using table can see (csproj-level usings,
            // parent-namespace visibility).
            let owner_is_syntactic = receiver_name.starts_with("new ");
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
                owner_is_syntactic,
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
