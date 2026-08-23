use crate::cache::{
    CachedCallResolutionInput, CachedDirectExport, CachedIndexArtifact, CachedInherentMethod,
    CachedResolutionBinding, CachedResolutionFile, CachedTopLevelDeclaration,
};
use crate::source_content_hash;
use anyhow::{Context, Result, anyhow};
use codestory_contracts::graph::{Edge, EdgeKind, Node, NodeId, NodeKind};
use codestory_contracts::proof_resolution::{
    CallResolutionFact, CalleeForm, DependencyFileHash, EXACT_CALL_RESOLUTION_ALGORITHM,
    ExactCallsite, ExactCallsiteCorrelationFailure, ExactSyntaxCallsiteCorrelationInput, FileId,
    INTERNAL_RESOLUTION_PRODUCER, OrdinaryCallEdgeCorrelationInput,
    PROOF_RESOLUTION_FACT_SCHEMA_VERSION, ProofResolutionAdapter, ProofResolutionFunnelCounts,
    ProofResolutionFunnelRow, ProofResolutionProjection, ProofResolutionReason,
    ProofResolutionStatus, ResolutionEvidence, ResolutionEvidenceKind, ResolutionProvenance,
    correlate_exact_syntax_callsites,
};
use codestory_store::{IndexPublicationRecord, ProofResolutionPublication, Store};
use codestory_workspace::{WorkspacePathIdentity, workspace_path_identity};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use tree_sitter::{Node as TsNode, Tree};

const ADAPTER_VERSION: &str = "reference-v5";
const RESOLUTION_INPUT_SCHEMA_VERSION: u32 = 3;
const INSTALLED_ADAPTERS: &[(&str, &str)] = &[
    ("rust", ADAPTER_VERSION),
    ("tsx", ADAPTER_VERSION),
    ("typescript", ADAPTER_VERSION),
];

pub(crate) struct CollectedResolutionInputs {
    pub calls: Vec<CachedCallResolutionInput>,
    pub file: Option<CachedResolutionFile>,
}

pub(crate) fn collect_call_resolution_inputs(
    tree: &Tree,
    source: &str,
    language: &str,
    parser_fingerprint: &str,
    file_id: NodeId,
    nodes: &[Node],
) -> CollectedResolutionInputs {
    if !is_installed_language(language) {
        return CollectedResolutionInputs {
            calls: Vec::new(),
            file: None,
        };
    }
    let complete = !tree.root_node().has_error();
    let mut lookup_input_complete = complete;
    let source_sha256 = source_content_hash(source.as_bytes());
    let direct_exports = if matches!(language, "typescript" | "tsx") {
        match collect_typescript_direct_exports(tree, source, file_id, nodes) {
            Some(exports) => exports,
            None => {
                lookup_input_complete = false;
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    if matches!(language, "typescript" | "tsx")
        && contains_dynamic_construct(tree.root_node(), source)
    {
        lookup_input_complete = false;
    }
    if language == "rust" && rust_file_has_item_domain_macro_invocation(tree.root_node()) {
        lookup_input_complete = false;
    }
    if language == "rust" && rust_file_has_attribute_domain(tree.root_node()) {
        lookup_input_complete = false;
    }
    let typescript_module =
        matches!(language, "typescript" | "tsx") && typescript_file_is_module(tree.root_node());
    if matches!(language, "typescript" | "tsx")
        && typescript_module
        && !typescript_module_root_is_closed(tree.root_node(), source)
    {
        lookup_input_complete = false;
    }
    let top_level_declarations = if matches!(language, "typescript" | "tsx" | "rust") {
        match collect_top_level_declarations(tree, source, language, file_id, nodes) {
            Some(declarations) => declarations,
            None => {
                lookup_input_complete = false;
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let inherent_methods = if language == "rust" {
        match collect_rust_inherent_methods(tree, source, file_id, nodes) {
            Some(methods) => methods,
            None => {
                lookup_input_complete = false;
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let mut calls = Vec::new();
    collect_calls(tree.root_node(), source, &mut |callee, form, raw_target| {
        let mut callsite = ExactCallsite {
            file_id: FileId(file_id.0),
            source_sha256: source_sha256.clone(),
            start_byte: callee.start_byte() as u64,
            end_byte_exclusive: callee.end_byte() as u64,
            line: callee.start_position().row as u32 + 1,
            column: callee.start_position().column as u32 + 1,
            callee_form: form,
            raw_target: raw_target.clone(),
        };
        let (caller, mut binding) = if language == "rust" {
            resolve_rust_syntax_claim(tree, source, file_id, nodes, callee, form, &raw_target)
        } else {
            resolve_typescript_syntax_claim(tree, source, file_id, nodes, callee, form, &raw_target)
        };
        if !lookup_input_complete {
            binding = CachedResolutionBinding::IncompleteDomain;
        }
        if matches!(binding, CachedResolutionBinding::StaticImport { .. }) {
            callsite.callee_form = CalleeForm::NamedImport;
        }
        calls.push(CachedCallResolutionInput {
            callsite,
            caller,
            binding,
            language: language.to_string(),
            adapter_version: ADAPTER_VERSION.to_string(),
            parser_fingerprint: parser_fingerprint.to_string(),
        });
    });
    calls.sort_by_key(|input| (input.callsite.start_byte, input.callsite.end_byte_exclusive));
    CollectedResolutionInputs {
        calls,
        file: Some(CachedResolutionFile {
            file_id,
            source_sha256,
            language: language.to_string(),
            adapter_version: ADAPTER_VERSION.to_string(),
            parser_fingerprint: parser_fingerprint.to_string(),
            complete,
            lookup_input_complete,
            typescript_module,
            top_level_declarations,
            inherent_methods,
            direct_exports,
        }),
    }
}

fn is_installed_language(language: &str) -> bool {
    INSTALLED_ADAPTERS
        .iter()
        .any(|(installed, _)| *installed == language)
}

fn expected_parser_fingerprint(path: &Path, language: &str) -> Option<String> {
    let extension = path.extension()?.to_str()?;
    let config = crate::get_language_for_ext(extension)?;
    (config.language_name == language).then(|| crate::resolution_parser_fingerprint(&config))
}

pub(crate) fn cached_resolution_inputs_are_current(
    artifact: &CachedIndexArtifact,
    language: &str,
    expected_parser_fingerprint: &str,
) -> bool {
    !is_installed_language(language)
        || (artifact.resolution_input_schema_version == RESOLUTION_INPUT_SCHEMA_VERSION
            && artifact.resolution_file.as_ref().is_some_and(|file| {
                file.language == language
                    && file.adapter_version == ADAPTER_VERSION
                    && file.parser_fingerprint == expected_parser_fingerprint
                    && artifact.call_resolution_inputs.iter().all(|call| {
                        call.language == language
                            && call.adapter_version == ADAPTER_VERSION
                            && call.parser_fingerprint == expected_parser_fingerprint
                    })
            }))
}

fn collect_calls(
    node: TsNode<'_>,
    source: &str,
    emit: &mut impl FnMut(TsNode<'_>, CalleeForm, String),
) {
    if node.kind() == "call_expression"
        && let Some(function) = node.child_by_field_name("function")
        && let Some((callee, form, raw_target)) = classify_callee(function, source)
    {
        emit(callee, form, raw_target);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_calls(child, source, emit);
    }
}

fn classify_callee<'tree>(
    function: TsNode<'tree>,
    source: &str,
) -> Option<(TsNode<'tree>, CalleeForm, String)> {
    let text = |node: TsNode<'tree>| node_text(node, source).map(str::to_string);
    match function.kind() {
        "identifier" | "type_identifier" => {
            Some((function, CalleeForm::Identifier, text(function)?))
        }
        "field_expression" => {
            let field = function.child_by_field_name("field")?;
            let receiver = function.child_by_field_name("value")?;
            let form = if text(receiver)?.trim() == "self" {
                CalleeForm::ImplicitReceiver
            } else {
                CalleeForm::ExplicitReceiver
            };
            Some((field, form, text(field)?))
        }
        "member_expression" => {
            let property = function.child_by_field_name("property")?;
            Some((property, CalleeForm::ExplicitReceiver, text(property)?))
        }
        "scoped_identifier" => {
            let name = function.child_by_field_name("name")?;
            Some((name, CalleeForm::QualifiedPath, text(name)?))
        }
        _ => {
            let mut cursor = function.walk();
            let leaf = function
                .named_children(&mut cursor)
                .last()
                .unwrap_or(function);
            Some((
                leaf,
                CalleeForm::DynamicAccess,
                text(leaf).unwrap_or_else(|| function.kind().to_string()),
            ))
        }
    }
}

fn resolve_typescript_syntax_claim(
    tree: &Tree,
    source: &str,
    file_id: NodeId,
    nodes: &[Node],
    callee: TsNode<'_>,
    form: CalleeForm,
    raw_target: &str,
) -> (Option<NodeId>, CachedResolutionBinding) {
    let Some(callable) = enclosing_ancestor(callee, &["function_declaration"]) else {
        return (None, CachedResolutionBinding::MissingBinding);
    };
    let Some(caller) = map_callable_declaration(nodes, file_id, callable, source) else {
        return (None, CachedResolutionBinding::Ambiguous);
    };
    if !typescript_callable_is_top_level(callable) {
        return (Some(caller), CachedResolutionBinding::Unsupported);
    }
    if !typescript_file_is_module(tree.root_node()) {
        return (Some(caller), CachedResolutionBinding::Unsupported);
    }
    if form != CalleeForm::Identifier {
        return (Some(caller), CachedResolutionBinding::Unsupported);
    }
    if !typescript_callable_domain_is_closed(callable) {
        return (Some(caller), CachedResolutionBinding::IncompleteDomain);
    }
    if contains_dynamic_construct(tree.root_node(), source)
        || callable_has_shadow_or_write("typescript", callable, callee, raw_target, source)
        || root_has_write(tree.root_node(), raw_target, source)
    {
        return (Some(caller), CachedResolutionBinding::Ambiguous);
    }
    let local = top_level_typescript_functions(tree.root_node())
        .into_iter()
        .filter(|declaration| declaration_name(*declaration, source) == Some(raw_target))
        .collect::<Vec<_>>();
    let imports = typescript_import_bindings(tree.root_node(), source)
        .into_iter()
        .filter(|binding| binding.local_name == raw_target)
        .collect::<Vec<_>>();
    if local.len() + imports.len() > 1 {
        return (Some(caller), CachedResolutionBinding::Ambiguous);
    }
    if let Some(declaration) = local.first().copied() {
        return (
            Some(caller),
            map_callable_declaration(nodes, file_id, declaration, source)
                .map(|declaration| CachedResolutionBinding::SameFile { declaration })
                .unwrap_or(CachedResolutionBinding::Ambiguous),
        );
    }
    if let Some(binding) = imports.into_iter().next() {
        let import_nodes = nodes
            .iter()
            .filter(|node| {
                node.file_node_id == Some(file_id)
                    && node.start_line == Some(binding.line)
                    && node.start_col == Some(binding.column)
                    && node.serialized_name == binding.local_name
            })
            .collect::<Vec<_>>();
        if import_nodes.len() != 1 {
            return (Some(caller), CachedResolutionBinding::Ambiguous);
        }
        return (
            Some(caller),
            CachedResolutionBinding::StaticImport {
                import: import_nodes[0].id,
                module_specifier: binding.module_specifier,
                imported_name: binding.imported_name,
                is_default: binding.is_default,
            },
        );
    }
    (Some(caller), CachedResolutionBinding::MissingBinding)
}

fn resolve_rust_syntax_claim(
    tree: &Tree,
    source: &str,
    file_id: NodeId,
    nodes: &[Node],
    callee: TsNode<'_>,
    form: CalleeForm,
    raw_target: &str,
) -> (Option<NodeId>, CachedResolutionBinding) {
    let Some(callable) = enclosing_ancestor(callee, &["function_item"]) else {
        return (None, CachedResolutionBinding::MissingBinding);
    };
    let Some(caller) = map_callable_declaration(nodes, file_id, callable, source) else {
        return (None, CachedResolutionBinding::Ambiguous);
    };
    if !rust_callable_is_in_root_module(callable) {
        return (Some(caller), CachedResolutionBinding::Unsupported);
    }
    if contains_node_kind(callable, "macro_invocation") {
        return (Some(caller), CachedResolutionBinding::IncompleteDomain);
    }
    if callable_has_shadow_or_write("rust", callable, callee, raw_target, source)
        || rust_root_has_competing_value_binding(tree.root_node(), raw_target, source)
    {
        return (Some(caller), CachedResolutionBinding::Ambiguous);
    }
    if form == CalleeForm::Identifier {
        let declarations = top_level_rust_functions(tree.root_node())
            .into_iter()
            .filter(|declaration| declaration_name(*declaration, source) == Some(raw_target))
            .collect::<Vec<_>>();
        return match declarations.as_slice() {
            [declaration] => (
                Some(caller),
                map_callable_declaration(nodes, file_id, *declaration, source)
                    .map(|declaration| CachedResolutionBinding::SameFile { declaration })
                    .unwrap_or(CachedResolutionBinding::Ambiguous),
            ),
            [] => (Some(caller), CachedResolutionBinding::MissingBinding),
            _ => (Some(caller), CachedResolutionBinding::Ambiguous),
        };
    }
    if form != CalleeForm::ImplicitReceiver {
        return (Some(caller), CachedResolutionBinding::Unsupported);
    }
    let Some(impl_item) = enclosing_ancestor(callable, &["impl_item"]) else {
        return (Some(caller), CachedResolutionBinding::MissingBinding);
    };
    let Some(owner_name) = simple_inherent_impl_owner(impl_item, source) else {
        return (Some(caller), CachedResolutionBinding::Unsupported);
    };
    let owner_nodes = nodes
        .iter()
        .filter(|node| {
            node.file_node_id == Some(file_id)
                && node.kind == NodeKind::STRUCT
                && node.serialized_name == owner_name
        })
        .collect::<Vec<_>>();
    let methods = direct_impl_functions(impl_item)
        .into_iter()
        .filter(|method| declaration_name(*method, source) == Some(raw_target))
        .collect::<Vec<_>>();
    let project_visible_methods = collect_simple_inherent_method_nodes(tree.root_node(), source)
        .into_iter()
        .filter(|(owner, method)| {
            *owner == owner_name && declaration_name(*method, source) == Some(raw_target)
        })
        .collect::<Vec<_>>();
    if owner_nodes.len() != 1 || methods.len() != 1 || project_visible_methods.len() != 1 {
        return (Some(caller), CachedResolutionBinding::Ambiguous);
    }
    let Some(declaration) = map_callable_declaration(nodes, file_id, methods[0], source) else {
        return (Some(caller), CachedResolutionBinding::Ambiguous);
    };
    (
        Some(caller),
        CachedResolutionBinding::ImplicitReceiver {
            owner: owner_nodes[0].id,
            declaration,
            owner_name: owner_name.to_string(),
        },
    )
}

fn node_text<'a>(node: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

fn enclosing_ancestor<'tree>(mut node: TsNode<'tree>, kinds: &[&str]) -> Option<TsNode<'tree>> {
    while let Some(parent) = node.parent() {
        if kinds.contains(&parent.kind()) {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn declaration_name<'a>(node: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    node.child_by_field_name("name")
        .and_then(|name| node_text(name, source))
}

fn map_callable_declaration(
    nodes: &[Node],
    file_id: NodeId,
    declaration: TsNode<'_>,
    source: &str,
) -> Option<NodeId> {
    let name = declaration_name(declaration, source)?;
    let line = declaration.start_position().row as u32 + 1;
    let matches = nodes
        .iter()
        .filter(|node| {
            node.file_node_id == Some(file_id)
                && matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                && node.start_line == Some(line)
                && graph_leaf_name(&node.serialized_name) == name
        })
        .map(|node| node.id)
        .collect::<Vec<_>>();
    (matches.len() == 1).then_some(matches[0])
}

fn graph_leaf_name(name: &str) -> &str {
    name.rsplit(['.', ':'])
        .find(|part| !part.is_empty())
        .unwrap_or(name)
}

fn top_level_typescript_functions(root: TsNode<'_>) -> Vec<TsNode<'_>> {
    let mut result = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "function_declaration" {
            result.push(child);
        } else if child.kind() == "export_statement" {
            let mut nested_cursor = child.walk();
            result.extend(
                child
                    .named_children(&mut nested_cursor)
                    .filter(|nested| nested.kind() == "function_declaration"),
            );
        }
    }
    result
}

fn top_level_rust_functions(root: TsNode<'_>) -> Vec<TsNode<'_>> {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .filter(|child| child.kind() == "function_item")
        .collect()
}

fn typescript_callable_is_top_level(callable: TsNode<'_>) -> bool {
    match callable.parent().map(|parent| parent.kind()) {
        Some("program") => true,
        Some("export_statement") => callable
            .parent()
            .and_then(|export| export.parent())
            .is_some_and(|parent| parent.kind() == "program"),
        _ => false,
    }
}

fn typescript_file_is_module(root: TsNode<'_>) -> bool {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .any(|child| matches!(child.kind(), "import_statement" | "export_statement"))
}

fn typescript_module_root_is_closed(root: TsNode<'_>, source: &str) -> bool {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .all(|child| match child.kind() {
            "comment" | "empty_statement" => true,
            "function_declaration" => typescript_direct_function_shape_is_closed(child),
            "import_statement" => typescript_import_bindings_for_statement(child, source).is_some(),
            "export_statement" => typescript_direct_export_shape(child).is_some(),
            _ => false,
        })
}

fn typescript_direct_function_shape_is_closed(declaration: TsNode<'_>) -> bool {
    let Some(name) = declaration.child_by_field_name("name") else {
        return false;
    };
    let Some(parameters) = declaration.child_by_field_name("parameters") else {
        return false;
    };
    let Some(body) = declaration.child_by_field_name("body") else {
        return false;
    };
    if name.kind() != "identifier"
        || parameters.kind() != "formal_parameters"
        || body.kind() != "statement_block"
    {
        return false;
    }
    let allowed = [name.id(), parameters.id(), body.id()];
    let mut cursor = declaration.walk();
    declaration
        .named_children(&mut cursor)
        .all(|child| child.kind() == "comment" || allowed.contains(&child.id()))
}

fn typescript_callable_domain_is_closed(callable: TsNode<'_>) -> bool {
    if !typescript_direct_function_shape_is_closed(callable) {
        return false;
    }
    let Some(parameters) = callable.child_by_field_name("parameters") else {
        return false;
    };
    let mut parameter_cursor = parameters.walk();
    if parameters
        .named_children(&mut parameter_cursor)
        .any(|child| child.kind() != "comment")
    {
        return false;
    }
    let Some(body) = callable.child_by_field_name("body") else {
        return false;
    };
    let mut body_cursor = body.walk();
    body.named_children(&mut body_cursor)
        .all(typescript_safe_direct_call_statement)
}

fn typescript_safe_direct_call_statement(statement: TsNode<'_>) -> bool {
    if matches!(statement.kind(), "comment" | "empty_statement") {
        return true;
    }
    if statement.kind() != "expression_statement" {
        return false;
    }
    let mut statement_cursor = statement.walk();
    let expressions = statement
        .named_children(&mut statement_cursor)
        .filter(|child| child.kind() != "comment")
        .collect::<Vec<_>>();
    let [call] = expressions.as_slice() else {
        return false;
    };
    if call.kind() != "call_expression" {
        return false;
    }
    let Some(function) = call.child_by_field_name("function") else {
        return false;
    };
    let Some(arguments) = call.child_by_field_name("arguments") else {
        return false;
    };
    if function.kind() != "identifier" || arguments.kind() != "arguments" {
        return false;
    }
    let allowed = [function.id(), arguments.id()];
    let mut call_cursor = call.walk();
    if !call
        .named_children(&mut call_cursor)
        .all(|child| child.kind() == "comment" || allowed.contains(&child.id()))
    {
        return false;
    }
    let mut argument_cursor = arguments.walk();
    arguments
        .named_children(&mut argument_cursor)
        .all(|child| child.kind() == "comment")
}

fn typescript_direct_export_shape(statement: TsNode<'_>) -> Option<(TsNode<'_>, bool)> {
    if statement.child_by_field_name("source").is_some()
        || statement.child_by_field_name("value").is_some()
    {
        return None;
    }
    let declaration = statement.child_by_field_name("declaration")?;
    if declaration.kind() != "function_declaration"
        || !typescript_direct_function_shape_is_closed(declaration)
    {
        return None;
    }
    let mut cursor = statement.walk();
    if !statement
        .named_children(&mut cursor)
        .all(|child| child.kind() == "comment" || child.id() == declaration.id())
    {
        return None;
    }
    Some((declaration, export_statement_has_default_token(statement)?))
}

fn rust_callable_is_in_root_module(callable: TsNode<'_>) -> bool {
    match callable.parent().map(|parent| parent.kind()) {
        Some("source_file") => true,
        Some("declaration_list") => callable
            .parent()
            .and_then(|body| body.parent())
            .is_some_and(|owner| {
                owner.kind() == "impl_item"
                    && owner
                        .parent()
                        .is_some_and(|parent| parent.kind() == "source_file")
            }),
        _ => false,
    }
}

fn contains_node_kind(root: TsNode<'_>, kind: &str) -> bool {
    let mut found = false;
    walk_nodes(root, &mut |node| found |= node.kind() == kind);
    found
}

fn rust_file_has_item_domain_macro_invocation(root: TsNode<'_>) -> bool {
    let mut found = false;
    walk_nodes(root, &mut |node| {
        if found || node.kind() != "macro_invocation" {
            return;
        }
        let mut ancestor = node.parent();
        while let Some(current) = ancestor {
            if current.kind() == "function_item" {
                return;
            }
            ancestor = current.parent();
        }
        found = true;
    });
    found
}

fn rust_file_has_attribute_domain(root: TsNode<'_>) -> bool {
    contains_node_kind(root, "attribute_item") || contains_node_kind(root, "inner_attribute_item")
}

fn direct_impl_functions(impl_item: TsNode<'_>) -> Vec<TsNode<'_>> {
    let Some(body) = impl_item.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|child| child.kind() == "function_item")
        .collect()
}

fn collect_simple_inherent_method_nodes<'tree, 'source>(
    root: TsNode<'tree>,
    source: &'source str,
) -> Vec<(&'source str, TsNode<'tree>)> {
    let mut methods = Vec::new();
    let mut cursor = root.walk();
    for item in root.named_children(&mut cursor) {
        if item.kind() != "impl_item" {
            continue;
        }
        let Some(owner) = simple_inherent_impl_owner(item, source) else {
            continue;
        };
        methods.extend(
            direct_impl_functions(item)
                .into_iter()
                .map(|method| (owner, method)),
        );
    }
    methods
}

fn simple_inherent_impl_owner<'a>(impl_item: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    let text = node_text(impl_item, source)?;
    let header = text.split_once('{')?.0.trim();
    let owner = header.strip_prefix("impl ")?.trim();
    if owner.is_empty()
        || owner.contains('<')
        || owner.contains('>')
        || owner.contains(" for ")
        || owner.contains(" where ")
        || !owner
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(owner)
}

fn callable_has_shadow_or_write(
    language: &str,
    callable: TsNode<'_>,
    callee: TsNode<'_>,
    name: &str,
    source: &str,
) -> bool {
    let mut found = false;
    walk_nodes(callable, &mut |node| {
        if found || node.id() == callee.id() {
            return;
        }
        let write_target = match language {
            "rust" => rust_write_target(node),
            _ => typescript_write_target(node),
        };
        if let Some(target) = write_target {
            found = target
                .map(|target| subtree_binds(target, name, source))
                .unwrap_or(true);
            if found {
                return;
            }
        }
        let binding_regions = match language {
            "rust" => rust_binding_regions(node),
            _ => typescript_binding_regions(node),
        };
        match binding_regions {
            Err(()) => found = true,
            Ok(Some(regions)) => {
                let binds_outer_callable = node.id() != callable.id()
                    || !matches!(node.kind(), "function_item" | "function_declaration");
                if binds_outer_callable
                    && regions
                        .into_iter()
                        .any(|region| subtree_binds(region, name, source))
                {
                    found = true;
                }
            }
            Ok(None) => {}
        }
    });
    found
}

fn root_has_write(root: TsNode<'_>, name: &str, source: &str) -> bool {
    let mut found = false;
    walk_nodes(root, &mut |node| {
        if found {
            return;
        }
        if let Some(target) = typescript_write_target(node) {
            found = target
                .map(|target| subtree_binds(target, name, source))
                .unwrap_or(true);
        }
    });
    found
}

fn rust_root_has_competing_value_binding(root: TsNode<'_>, name: &str, source: &str) -> bool {
    let mut cursor = root.walk();
    root.named_children(&mut cursor).any(|node| {
        if node.kind() == "function_item" {
            return false;
        }
        match rust_binding_regions(node) {
            Err(()) => true,
            Ok(Some(regions)) => regions
                .into_iter()
                .any(|region| subtree_binds(region, name, source)),
            Ok(None) => false,
        }
    })
}

fn typescript_write_target(node: TsNode<'_>) -> Option<Option<TsNode<'_>>> {
    match node.kind() {
        "assignment_expression" | "augmented_assignment_expression" => {
            Some(node.child_by_field_name("left"))
        }
        "update_expression" => Some(node.child_by_field_name("argument")),
        "for_in_statement" => Some(node.child_by_field_name("left")),
        _ => None,
    }
}

fn rust_write_target(node: TsNode<'_>) -> Option<Option<TsNode<'_>>> {
    match node.kind() {
        "assignment_expression" | "compound_assignment_expr" => {
            Some(node.child_by_field_name("left"))
        }
        _ => None,
    }
}

fn typescript_binding_regions(node: TsNode<'_>) -> Result<Option<Vec<TsNode<'_>>>, ()> {
    let required = |field| {
        node.child_by_field_name(field)
            .map(|child| vec![child])
            .ok_or(())
    };
    let optional = |field| {
        Ok(node
            .child_by_field_name(field)
            .into_iter()
            .collect::<Vec<_>>())
    };
    let one_of = |fields: &[&str]| {
        let regions = fields
            .iter()
            .filter_map(|field| node.child_by_field_name(field))
            .collect::<Vec<_>>();
        (!regions.is_empty()).then_some(regions).ok_or(())
    };
    match node.kind() {
        "variable_declarator" => required("name").map(Some),
        "required_parameter" | "optional_parameter" => one_of(&["name", "pattern"]).map(Some),
        "arrow_function" => one_of(&["parameter", "parameters"]).map(Some),
        "formal_parameters" | "rest_pattern" => Ok(Some(vec![node])),
        "catch_clause" => optional("parameter").map(Some),
        "function_declaration"
        | "generator_function_declaration"
        | "function_signature"
        | "class_declaration"
        | "abstract_class_declaration"
        | "enum_declaration"
        | "internal_module"
        | "module" => required("name").map(Some),
        "function_expression" | "generator_function" | "class" => optional("name").map(Some),
        "import_statement" => Ok(Some(vec![node])),
        _ => Ok(None),
    }
}

fn rust_binding_regions(node: TsNode<'_>) -> Result<Option<Vec<TsNode<'_>>>, ()> {
    let required = |field| {
        node.child_by_field_name(field)
            .map(|child| vec![child])
            .ok_or(())
    };
    let optional = |field| {
        Ok(node
            .child_by_field_name(field)
            .into_iter()
            .collect::<Vec<_>>())
    };
    match node.kind() {
        "parameter" => required("pattern").map(Some),
        "variadic_parameter" => optional("pattern").map(Some),
        "closure_parameters" => Ok(Some(vec![node])),
        "let_declaration" | "let_condition" | "for_expression" | "match_arm" => {
            required("pattern").map(Some)
        }
        "function_item" | "const_item" | "const_parameter" | "static_item" | "struct_item"
        | "enum_variant" => required("name").map(Some),
        "use_declaration" => required("argument").map(Some),
        _ => Ok(None),
    }
}

fn subtree_binds(node: TsNode<'_>, name: &str, source: &str) -> bool {
    let mut found = false;
    walk_nodes(node, &mut |child| {
        if matches!(
            child.kind(),
            "identifier" | "type_identifier" | "shorthand_property_identifier_pattern"
        ) && node_text(child, source) == Some(name)
        {
            found = true;
        }
    });
    found
}

fn contains_dynamic_construct(root: TsNode<'_>, source: &str) -> bool {
    let mut found = false;
    walk_nodes(root, &mut |node| {
        found |= node.kind() == "with_statement"
            || (node.kind() == "call_expression"
                && node
                    .child_by_field_name("function")
                    .and_then(|function| node_text(function, source))
                    == Some("eval"));
    });
    found
}

fn walk_nodes(node: TsNode<'_>, visit: &mut impl FnMut(TsNode<'_>)) {
    visit(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_nodes(child, visit);
    }
}

#[derive(Debug)]
struct TypescriptImportBinding {
    local_name: String,
    imported_name: String,
    module_specifier: String,
    is_default: bool,
    line: u32,
    column: u32,
}

fn typescript_import_bindings(root: TsNode<'_>, source: &str) -> Vec<TypescriptImportBinding> {
    let mut result = Vec::new();
    let mut cursor = root.walk();
    for statement in root
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "import_statement")
    {
        result.extend(
            typescript_import_bindings_for_statement(statement, source).unwrap_or_default(),
        );
    }
    result
}

fn typescript_import_bindings_for_statement(
    statement: TsNode<'_>,
    source: &str,
) -> Option<Vec<TypescriptImportBinding>> {
    let source_node = statement.child_by_field_name("source")?;
    let module_specifier = simple_typescript_string(source_node, source)?;
    if !module_specifier.starts_with("./") && !module_specifier.starts_with("../") {
        return None;
    }
    if contains_unnamed_token(statement, "type") {
        return None;
    }
    let mut statement_cursor = statement.walk();
    let clauses = statement
        .named_children(&mut statement_cursor)
        .filter(|child| child.kind() != "comment" && child.id() != source_node.id())
        .collect::<Vec<_>>();
    let [clause] = clauses.as_slice() else {
        return None;
    };
    if clause.kind() != "import_clause" {
        return None;
    }
    let mut clause_cursor = clause.walk();
    let entries = clause
        .named_children(&mut clause_cursor)
        .filter(|child| child.kind() != "comment")
        .collect::<Vec<_>>();
    let [entry] = entries.as_slice() else {
        return None;
    };
    let mut bindings = Vec::new();
    match entry.kind() {
        "identifier" => bindings.push(typescript_import_binding(
            *entry,
            "default",
            module_specifier,
            true,
            source,
        )?),
        "named_imports" => {
            let mut imports_cursor = entry.walk();
            let specifiers = entry
                .named_children(&mut imports_cursor)
                .filter(|child| child.kind() != "comment")
                .collect::<Vec<_>>();
            if specifiers.is_empty()
                || specifiers
                    .iter()
                    .any(|specifier| specifier.kind() != "import_specifier")
            {
                return None;
            }
            for specifier in specifiers {
                let imported = specifier.child_by_field_name("name")?;
                if imported.kind() != "identifier" {
                    return None;
                }
                let local = specifier.child_by_field_name("alias").unwrap_or(imported);
                if local.kind() != "identifier" {
                    return None;
                }
                let imported_name = node_text(imported, source)?;
                bindings.push(typescript_import_binding(
                    local,
                    imported_name,
                    module_specifier,
                    false,
                    source,
                )?);
            }
        }
        _ => return None,
    }
    Some(bindings)
}

fn typescript_import_binding(
    local: TsNode<'_>,
    imported_name: &str,
    module_specifier: &str,
    is_default: bool,
    source: &str,
) -> Option<TypescriptImportBinding> {
    let local_name = node_text(local, source)?;
    if !typescript_identifier_is_supported(local_name) {
        return None;
    }
    Some(TypescriptImportBinding {
        local_name: local_name.to_string(),
        imported_name: imported_name.to_string(),
        module_specifier: module_specifier.to_string(),
        is_default,
        line: local.start_position().row as u32 + 1,
        column: local.start_position().column as u32 + 1,
    })
}

fn typescript_identifier_is_supported(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier
            .chars()
            .all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn simple_typescript_string<'a>(node: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    let literal = node_text(node, source)?;
    let quote = literal.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"') || literal.as_bytes().last().copied()? != quote {
        return None;
    }
    let value = literal.get(1..literal.len().checked_sub(1)?)?;
    (!value.contains('\\')).then_some(value)
}

fn contains_unnamed_token(node: TsNode<'_>, token: &str) -> bool {
    let mut found = false;
    let mut visit = |current: TsNode<'_>| {
        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            if !child.is_named() && child.kind() == token {
                found = true;
                return;
            }
        }
    };
    walk_nodes(node, &mut visit);
    found
}

fn collect_typescript_direct_exports(
    tree: &Tree,
    source: &str,
    file_id: NodeId,
    nodes: &[Node],
) -> Option<Vec<CachedDirectExport>> {
    if contains_dynamic_construct(tree.root_node(), source) {
        return None;
    }
    let mut exports = Vec::new();
    let mut cursor = tree.root_node().walk();
    for statement in tree.root_node().named_children(&mut cursor) {
        if statement.kind() != "export_statement" {
            continue;
        }
        let (declaration, is_default) = typescript_direct_export_shape(statement)?;
        let name = declaration_name(declaration, source)?;
        let declaration_count = top_level_typescript_functions(tree.root_node())
            .into_iter()
            .filter(|candidate| declaration_name(*candidate, source) == Some(name))
            .count();
        let import_count = typescript_import_bindings(tree.root_node(), source)
            .into_iter()
            .filter(|binding| binding.local_name == name)
            .count();
        if root_has_write(tree.root_node(), name, source) || declaration_count + import_count != 1 {
            continue;
        }
        let node_id = map_callable_declaration(nodes, file_id, declaration, source)?;
        exports.push(CachedDirectExport {
            exported_name: if is_default { "default" } else { name }.to_string(),
            declaration: node_id,
            is_default,
        });
    }
    exports.sort_by(|left, right| {
        left.exported_name
            .cmp(&right.exported_name)
            .then(left.declaration.cmp(&right.declaration))
    });
    if exports.windows(2).any(|pair| {
        pair[0].exported_name == pair[1].exported_name && pair[0].is_default == pair[1].is_default
    }) {
        return None;
    }
    Some(exports)
}

fn export_statement_has_default_token(statement: TsNode<'_>) -> Option<bool> {
    let mut cursor = statement.walk();
    let defaults = statement
        .children(&mut cursor)
        .filter(|child| !child.is_named() && child.kind() == "default")
        .count();
    (defaults <= 1).then_some(defaults == 1)
}

fn collect_top_level_declarations(
    tree: &Tree,
    source: &str,
    language: &str,
    file_id: NodeId,
    nodes: &[Node],
) -> Option<Vec<CachedTopLevelDeclaration>> {
    let declarations = if language == "rust" {
        top_level_rust_functions(tree.root_node())
    } else {
        top_level_typescript_functions(tree.root_node())
    };
    let mut result = Vec::with_capacity(declarations.len());
    for declaration in declarations {
        let name = declaration_name(declaration, source)?.to_string();
        let declaration = map_callable_declaration(nodes, file_id, declaration, source)?;
        result.push(CachedTopLevelDeclaration { name, declaration });
    }
    result.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.declaration.cmp(&right.declaration))
    });
    Some(result)
}

fn collect_rust_inherent_methods(
    tree: &Tree,
    source: &str,
    file_id: NodeId,
    nodes: &[Node],
) -> Option<Vec<CachedInherentMethod>> {
    let methods = collect_simple_inherent_method_nodes(tree.root_node(), source);
    let mut result = Vec::with_capacity(methods.len());
    for (owner_name, method) in methods {
        result.push(CachedInherentMethod {
            owner_name: owner_name.to_string(),
            method_name: declaration_name(method, source)?.to_string(),
            declaration: map_callable_declaration(nodes, file_id, method, source)?,
        });
    }
    result.sort_by(|left, right| {
        left.owner_name
            .cmp(&right.owner_name)
            .then(left.method_name.cmp(&right.method_name))
            .then(left.declaration.cmp(&right.declaration))
    });
    Some(result)
}

struct ResolutionCacheRecord {
    path: PathBuf,
    file: CachedResolutionFile,
    calls: Vec<CachedCallResolutionInput>,
}

fn cache_entry_identity_for_indexed_file(
    cache_path: &Path,
    indexed_path: &Path,
) -> Result<WorkspacePathIdentity> {
    let observed_path = if cache_path.is_absolute() {
        cache_path.to_path_buf()
    } else {
        let components = cache_path.components().collect::<Vec<_>>();
        if components.is_empty()
            || components
                .iter()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(anyhow!(
                "proof resolution parser cache path is not a portable project path: {}",
                cache_path.display()
            ));
        }
        let mut project_root = indexed_path;
        for _ in &components {
            project_root = project_root.parent().ok_or_else(|| {
                anyhow!(
                    "proof resolution parser cache path has more components than indexed path {}",
                    indexed_path.display()
                )
            })?;
        }
        project_root.join(cache_path)
    };
    workspace_path_identity(&observed_path).with_context(|| {
        format!(
            "proof resolution native identity is unavailable for parser cache path {}",
            cache_path.display()
        )
    })
}

fn cache_entry_matches_any_governed_file(
    cache_path: &Path,
    governed: &[&codestory_store::FileInfo],
    governed_identities: &HashMap<i64, WorkspacePathIdentity>,
) -> Result<bool> {
    for file in governed {
        let observed = cache_entry_identity_for_indexed_file(cache_path, &file.path)?;
        if governed_identities
            .get(&file.id)
            .is_some_and(|identity| *identity == observed)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn rematerialize_proof_resolution_projection(
    store: &mut Store,
    publication: &IndexPublicationRecord,
) -> Result<ProofResolutionPublication> {
    let files = store.get_files()?;
    let file_by_id = files
        .iter()
        .map(|file| (file.id, file))
        .collect::<HashMap<_, _>>();
    let mut file_content_hash_by_id = HashMap::new();
    for file in &files {
        if let Some(source_hash) = store.get_file_content_hash(file.id)? {
            file_content_hash_by_id.insert(file.id, source_hash);
        }
    }
    let nodes = store.get_nodes()?;
    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let edges = store.get_edges()?;
    let governed = files
        .iter()
        .filter(|file| file.indexed && is_installed_language(&file.language))
        .collect::<Vec<_>>();
    let governed_by_id = governed
        .iter()
        .map(|file| (file.id, *file))
        .collect::<HashMap<_, _>>();
    let mut governed_identities = HashMap::<i64, WorkspacePathIdentity>::new();
    let mut governed_identity_owners = HashMap::<WorkspacePathIdentity, i64>::new();
    for file in &governed {
        let identity = workspace_path_identity(&file.path).with_context(|| {
            format!(
                "proof resolution native identity is unavailable for {}",
                file.path.display()
            )
        })?;
        if let Some(previous) = governed_identity_owners.insert(identity.clone(), file.id)
            && previous != file.id
        {
            return Err(anyhow!(
                "proof resolution native path identity collision between indexed files {previous} and {}",
                file.id
            ));
        }
        governed_identities.insert(file.id, identity);
    }
    let mut records_by_id = HashMap::<i64, Vec<ResolutionCacheRecord>>::new();
    for entry in store.get_index_artifact_cache_entries()? {
        let artifact: CachedIndexArtifact = match serde_json::from_slice(&entry.artifact_blob) {
            Ok(artifact) => artifact,
            Err(error) => {
                if cache_entry_matches_any_governed_file(
                    &entry.file_path,
                    &governed,
                    &governed_identities,
                )? {
                    return Err(anyhow!(
                        "proof resolution parser cache is corrupt for {}: {error}",
                        entry.file_path.display()
                    ));
                }
                continue;
            }
        };
        let Some(file) = artifact.resolution_file else {
            if cache_entry_matches_any_governed_file(
                &entry.file_path,
                &governed,
                &governed_identities,
            )? {
                return Err(anyhow!(
                    "proof resolution parser cache has no file coverage for {}",
                    entry.file_path.display()
                ));
            }
            continue;
        };
        if !governed_by_id.contains_key(&file.file_id.0) {
            continue;
        }
        let indexed_file = governed_by_id[&file.file_id.0];
        let entry_identity =
            cache_entry_identity_for_indexed_file(&entry.file_path, &indexed_file.path)?;
        if governed_identities.get(&indexed_file.id) != Some(&entry_identity) {
            return Err(anyhow!(
                "proof resolution parser cache native path does not match indexed file {}",
                indexed_file.path.display()
            ));
        }
        if artifact.resolution_input_schema_version != RESOLUTION_INPUT_SCHEMA_VERSION {
            return Err(anyhow!(
                "proof resolution parser cache has no schema-v{RESOLUTION_INPUT_SCHEMA_VERSION} inputs for {}",
                entry.file_path.display()
            ));
        }
        records_by_id
            .entry(file.file_id.0)
            .or_default()
            .push(ResolutionCacheRecord {
                path: indexed_file.path.clone(),
                file,
                calls: artifact.call_resolution_inputs,
            });
    }
    let mut records = Vec::with_capacity(governed.len());
    for indexed_file in governed {
        let Some(mut matches) = records_by_id.remove(&indexed_file.id) else {
            return Err(anyhow!(
                "proof resolution parser cache coverage is missing for {}",
                indexed_file.path.display()
            ));
        };
        if matches.len() != 1 {
            return Err(anyhow!(
                "proof resolution parser cache coverage is duplicated for {}",
                indexed_file.path.display()
            ));
        }
        let record = matches.pop().expect("one cache record");
        let stored_hash = file_content_hash_by_id
            .get(&indexed_file.id)
            .ok_or_else(|| {
                anyhow!(
                    "proof resolution indexed file {} has no source hash",
                    indexed_file.path.display()
                )
            })?;
        let expected_parser_fingerprint = expected_parser_fingerprint(
            &indexed_file.path,
            &indexed_file.language,
        )
        .ok_or_else(|| {
            anyhow!(
                "proof resolution installed adapter has no compiled parser fingerprint for {} ({})",
                indexed_file.language,
                indexed_file.path.display()
            )
        })?;
        if record.file.parser_fingerprint != expected_parser_fingerprint
            || record
                .calls
                .iter()
                .any(|call| call.parser_fingerprint != expected_parser_fingerprint)
        {
            return Err(anyhow!(
                "proof resolution parser fingerprint does not match the compiled parser/rules for {}",
                indexed_file.path.display()
            ));
        }
        if record.file.file_id != NodeId(indexed_file.id)
            || record.file.source_sha256 != *stored_hash
            || record.file.language != indexed_file.language
            || record.file.complete != indexed_file.complete
            || record.file.adapter_version != ADAPTER_VERSION
            || record.calls.iter().any(|call| {
                call.callsite.file_id != FileId(indexed_file.id)
                    || call.callsite.source_sha256 != *stored_hash
                    || call.language != indexed_file.language
                    || call.adapter_version != record.file.adapter_version
            })
        {
            return Err(anyhow!(
                "proof resolution parser cache coverage is stale or hash-mismatched for {}",
                indexed_file.path.display()
            ));
        }
        records.push(record);
    }
    records.sort_by(|left, right| left.path.cmp(&right.path));
    let mut record_by_path = HashMap::new();
    for record in &records {
        let identity = governed_identities
            .get(&record.file.file_id.0)
            .ok_or_else(|| anyhow!("proof resolution cache record has no native identity"))?
            .clone();
        if record_by_path.insert(identity, record).is_some() {
            return Err(anyhow!(
                "proof resolution native path identity collision in parser cache records"
            ));
        }
    }
    let mut inputs = records
        .iter()
        .flat_map(|record| record.calls.iter().cloned().map(move |call| (record, call)))
        .collect::<Vec<_>>();
    inputs.sort_by(|left, right| {
        left.1
            .callsite
            .file_id
            .cmp(&right.1.callsite.file_id)
            .then(left.1.callsite.start_byte.cmp(&right.1.callsite.start_byte))
            .then(
                left.1
                    .callsite
                    .end_byte_exclusive
                    .cmp(&right.1.callsite.end_byte_exclusive),
            )
    });
    if inputs.windows(2).any(|pair| {
        pair[0].1.callsite.file_id == pair[1].1.callsite.file_id
            && pair[0].1.callsite.start_byte == pair[1].1.callsite.start_byte
            && pair[0].1.callsite.end_byte_exclusive == pair[1].1.callsite.end_byte_exclusive
    }) {
        return Err(anyhow!(
            "proof resolution projection has duplicate exact callsites"
        ));
    }
    let record_by_file_id = records
        .iter()
        .map(|record| (record.file.file_id.0, record))
        .collect::<HashMap<_, _>>();
    let mut claims = inputs
        .into_iter()
        .map(|(source_record, input)| {
            resolve_syntax_claim(&file_by_id, &record_by_path, &records, source_record, input)
        })
        .collect::<Result<Vec<_>>>()?;
    enforce_exact_dependency_eligibility(
        &mut claims,
        &file_by_id,
        &node_by_id,
        &file_content_hash_by_id,
        &governed_by_id,
        &record_by_file_id,
    )?;
    let exact_claim_indices = claims
        .iter()
        .enumerate()
        .filter_map(|(index, claim)| {
            (claim.status == ProofResolutionStatus::Exact).then_some(index)
        })
        .collect::<Vec<_>>();
    let syntax_correlation_inputs = exact_claim_indices
        .iter()
        .map(|index| {
            let claim = &claims[*index];
            ExactSyntaxCallsiteCorrelationInput {
                file_id: claim.input.callsite.file_id,
                line: claim.input.callsite.line,
                start_byte: claim.input.callsite.start_byte,
                end_byte_exclusive: claim.input.callsite.end_byte_exclusive,
                column: claim.input.callsite.column,
                caller: claim.caller,
                target: claim.target.expect("Exact syntax claim has a target"),
                raw_target: &claim.input.callsite.raw_target,
            }
        })
        .collect::<Vec<_>>();
    let ordinary_edge_indices = edges
        .iter()
        .enumerate()
        .filter_map(|(index, edge)| {
            (edge.kind == EdgeKind::CALL && node_by_id.contains_key(&edge.target)).then_some(index)
        })
        .collect::<Vec<_>>();
    let edge_correlation_inputs = ordinary_edge_indices
        .iter()
        .map(|index| {
            let edge = &edges[*index];
            let raw = node_by_id[&edge.target];
            OrdinaryCallEdgeCorrelationInput {
                file_id: edge.file_node_id.map(|file| FileId(file.0)),
                line: edge.line,
                caller: edge.effective_source(),
                target: edge.effective_target(),
                raw_edge_target: edge.target,
                raw_file_id: raw.file_node_id.map(|file| FileId(file.0)),
                raw_line: raw.start_line,
                raw_target: graph_leaf_name(&raw.serialized_name),
                callsite_identity: edge.callsite_identity.as_deref(),
                semantic_exact: edge.resolved_target == Some(edge.effective_target())
                    && edge.candidate_targets.is_empty(),
            }
        })
        .collect::<Vec<_>>();
    let correlations =
        correlate_exact_syntax_callsites(&syntax_correlation_inputs, &edge_correlation_inputs)
            .into_iter()
            .map(|result| result.map(|edge_index| ordinary_edge_indices[edge_index]))
            .collect::<Vec<_>>();
    let mut claim_correlations = vec![None; claims.len()];
    for (correlation_index, claim_index) in exact_claim_indices.iter().copied().enumerate() {
        claim_correlations[claim_index] = Some(correlations[correlation_index]);
    }
    let mut facts = Vec::with_capacity(claims.len());
    for claim_index in 0..claims.len() {
        facts.push(seal_resolved_claim(
            &file_content_hash_by_id,
            &node_by_id,
            &edges,
            &claims,
            claim_index,
            claim_correlations[claim_index],
        )?);
    }
    let funnel = build_funnel(&facts);
    store
        .replace_proof_resolution_projection(
            publication,
            &ProofResolutionProjection {
                adapter_roster: INSTALLED_ADAPTERS
                    .iter()
                    .map(|(language, adapter_version)| ProofResolutionAdapter {
                        language: (*language).to_string(),
                        adapter_version: (*adapter_version).to_string(),
                    })
                    .collect(),
                facts,
                funnel,
            },
        )
        .map_err(Into::into)
}

fn resolve_relative_import<'a>(
    source_record: &ResolutionCacheRecord,
    module_specifier: &str,
    records: &'a HashMap<WorkspacePathIdentity, &ResolutionCacheRecord>,
) -> Result<Option<&'a ResolutionCacheRecord>> {
    let Some(parent) = source_record.path.parent() else {
        return Ok(None);
    };
    let base = parent.join(module_specifier);
    let candidates = if base.extension().is_some() {
        vec![base]
    } else {
        vec![
            base.with_extension("ts"),
            base.with_extension("tsx"),
            base.join("index.ts"),
            base.join("index.tsx"),
        ]
    };
    let mut matches = Vec::new();
    for candidate in candidates {
        if candidate.is_absolute() && !candidate.exists() {
            continue;
        }
        let key = workspace_path_identity(&candidate).with_context(|| {
            format!(
                "proof resolution native identity is unavailable for import candidate {}",
                candidate.display()
            )
        })?;
        if let Some(record) = records.get(&key) {
            matches.push(*record);
        }
    }
    matches.sort_by_key(|record| record.file.file_id);
    matches.dedup_by_key(|record| record.file.file_id);
    Ok((matches.len() == 1).then_some(matches[0]))
}

#[derive(Debug)]
struct ResolvedSyntaxClaim {
    input: CachedCallResolutionInput,
    caller: NodeId,
    target: Option<NodeId>,
    status: ProofResolutionStatus,
    reason: ProofResolutionReason,
    evidence_chain: Vec<ResolutionEvidence>,
    exact_node_file_expectations: Vec<(NodeId, FileId)>,
}

fn resolve_syntax_claim(
    files: &HashMap<i64, &codestory_store::FileInfo>,
    records: &HashMap<WorkspacePathIdentity, &ResolutionCacheRecord>,
    all_records: &[ResolutionCacheRecord],
    source_record: &ResolutionCacheRecord,
    input: CachedCallResolutionInput,
) -> Result<ResolvedSyntaxClaim> {
    let source_file = files
        .get(&input.callsite.file_id.0)
        .ok_or_else(|| anyhow!("proof callsite file is missing"))?;
    let mut status;
    let mut reason;
    let mut target = None;
    let mut evidence_chain = Vec::new();
    let caller = input.caller.unwrap_or(NodeId(input.callsite.file_id.0));
    let mut exact_node_file_expectations = vec![(caller, input.callsite.file_id)];
    match &input.binding {
        CachedResolutionBinding::SameFile { declaration } => {
            let declaration_is_recorded =
                source_record
                    .file
                    .top_level_declarations
                    .iter()
                    .any(|binding| {
                        binding.name == input.callsite.raw_target
                            && binding.declaration == *declaration
                    });
            let typescript_script =
                matches!(source_record.file.language.as_str(), "typescript" | "tsx")
                    && !source_record.file.typescript_module;
            if typescript_script {
                status = ProofResolutionStatus::Unsupported;
                reason = ProofResolutionReason::UnsupportedConstruct;
            } else if !declaration_is_recorded {
                status = ProofResolutionStatus::Ambiguous;
                reason = ProofResolutionReason::MultipleBindings;
            } else {
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(*declaration);
                evidence_chain.push(ResolutionEvidence::SameFileDeclaration {
                    declaration: *declaration,
                });
                exact_node_file_expectations.push((*declaration, input.callsite.file_id));
            }
        }
        CachedResolutionBinding::ImplicitReceiver {
            owner,
            declaration,
            owner_name,
        } => {
            let rust_records = all_records
                .iter()
                .filter(|record| record.file.language == "rust")
                .collect::<Vec<_>>();
            let matching_methods = rust_records
                .iter()
                .flat_map(|record| record.file.inherent_methods.iter())
                .filter(|method| {
                    method.owner_name == *owner_name
                        && method.method_name == input.callsite.raw_target
                })
                .collect::<Vec<_>>();
            if rust_records
                .iter()
                .any(|record| !record.file.lookup_input_complete)
            {
                status = ProofResolutionStatus::IncompleteDomain;
                reason = ProofResolutionReason::LookupDomainIncomplete;
            } else if matching_methods.len() != 1 || matching_methods[0].declaration != *declaration
            {
                status = ProofResolutionStatus::Ambiguous;
                reason = ProofResolutionReason::MultipleBindings;
            } else {
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(*declaration);
                evidence_chain.push(ResolutionEvidence::ImplicitReceiver { owner: *owner });
                evidence_chain.push(ResolutionEvidence::SameFileDeclaration {
                    declaration: *declaration,
                });
                exact_node_file_expectations.push((*owner, input.callsite.file_id));
                exact_node_file_expectations.push((*declaration, input.callsite.file_id));
            }
        }
        CachedResolutionBinding::StaticImport {
            import,
            module_specifier,
            imported_name,
            is_default,
        } => {
            let target_record = resolve_relative_import(source_record, &module_specifier, records)?;
            let declarations = target_record
                .filter(|record| record.file.lookup_input_complete)
                .into_iter()
                .flat_map(|record| record.file.direct_exports.iter())
                .filter(|export| {
                    export.is_default == *is_default && export.exported_name == *imported_name
                })
                .collect::<Vec<_>>();
            if let [declaration] = declarations.as_slice() {
                let target_file_id = FileId(
                    target_record
                        .expect("one direct export requires a resolved target record")
                        .file
                        .file_id
                        .0,
                );
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(declaration.declaration);
                evidence_chain.push(ResolutionEvidence::StaticImportBinding {
                    import: *import,
                    declaration: declaration.declaration,
                });
                exact_node_file_expectations.push((*import, input.callsite.file_id));
                exact_node_file_expectations.push((declaration.declaration, target_file_id));
            } else if target_record.is_some_and(|record| !record.file.lookup_input_complete) {
                status = ProofResolutionStatus::IncompleteDomain;
                reason = ProofResolutionReason::LookupDomainIncomplete;
            } else if declarations.len() > 1 {
                status = ProofResolutionStatus::Ambiguous;
                reason = ProofResolutionReason::MultipleBindings;
            } else {
                status = ProofResolutionStatus::MissingBinding;
                reason = ProofResolutionReason::MissingBinding;
            }
        }
        CachedResolutionBinding::Ambiguous => {
            status = ProofResolutionStatus::Ambiguous;
            reason = ProofResolutionReason::MultipleBindings;
        }
        CachedResolutionBinding::MissingBinding => {
            status = ProofResolutionStatus::MissingBinding;
            reason = ProofResolutionReason::MissingBinding;
        }
        CachedResolutionBinding::Unsupported => {
            status = ProofResolutionStatus::Unsupported;
            reason = ProofResolutionReason::UnsupportedConstruct;
        }
        CachedResolutionBinding::IncompleteDomain => {
            status = ProofResolutionStatus::IncompleteDomain;
            reason = ProofResolutionReason::LookupDomainIncomplete;
        }
    }
    if !source_file.complete
        || !source_record.file.complete
        || !source_record.file.lookup_input_complete
    {
        status = ProofResolutionStatus::IncompleteDomain;
        reason = ProofResolutionReason::LookupDomainIncomplete;
        target = None;
        evidence_chain.clear();
    }
    Ok(ResolvedSyntaxClaim {
        input,
        caller,
        target,
        status,
        reason,
        evidence_chain,
        exact_node_file_expectations,
    })
}

fn enforce_exact_dependency_eligibility(
    claims: &mut [ResolvedSyntaxClaim],
    files: &HashMap<i64, &codestory_store::FileInfo>,
    nodes: &HashMap<NodeId, &Node>,
    file_content_hashes: &HashMap<i64, String>,
    governed_files: &HashMap<i64, &codestory_store::FileInfo>,
    records: &HashMap<i64, &ResolutionCacheRecord>,
) -> Result<()> {
    for claim in claims
        .iter_mut()
        .filter(|claim| claim.status == ProofResolutionStatus::Exact)
    {
        let mut eligible = true;
        let mut expected_file_ids = HashSet::from([claim.input.callsite.file_id.0]);
        for (node_id, expected_file_id) in &claim.exact_node_file_expectations {
            expected_file_ids.insert(expected_file_id.0);
            let node = nodes.get(node_id).ok_or_else(|| {
                anyhow!(
                    "proof exact dependency node {} is missing from the graph",
                    node_id.0
                )
            })?;
            let Some(actual_file_id) = node.file_node_id else {
                if *node_id == claim.caller {
                    return Err(anyhow!(
                        "proof exact caller {} has no source-file ownership",
                        node_id.0
                    ));
                }
                eligible = false;
                continue;
            };
            if !files.contains_key(&actual_file_id.0) {
                return Err(anyhow!(
                    "proof exact dependency node {} names missing file {}",
                    node_id.0,
                    actual_file_id.0
                ));
            }
            if !file_content_hashes.contains_key(&actual_file_id.0) {
                return Err(anyhow!(
                    "proof exact dependency file {} has no source hash",
                    actual_file_id.0
                ));
            }
            if actual_file_id.0 != expected_file_id.0 {
                if *node_id == claim.caller {
                    return Err(anyhow!(
                        "proof exact caller {} ownership does not match source file {}",
                        node_id.0,
                        expected_file_id.0
                    ));
                }
                eligible = false;
            }
        }
        for file_id in expected_file_ids {
            let file = files
                .get(&file_id)
                .ok_or_else(|| anyhow!("proof exact dependency file {file_id} is missing"))?;
            let source_hash = file_content_hashes.get(&file_id).ok_or_else(|| {
                anyhow!("proof exact dependency file {file_id} has no source hash")
            })?;
            let record = records.get(&file_id);
            if !file.indexed
                || !file.complete
                || !governed_files.contains_key(&file_id)
                || record.is_none()
                || record.is_some_and(|record| {
                    !record.file.complete || !record.file.lookup_input_complete
                })
            {
                eligible = false;
                continue;
            }
            if record.is_some_and(|record| record.file.source_sha256 != *source_hash) {
                return Err(anyhow!(
                    "proof exact dependency file {file_id} hash does not match parser coverage"
                ));
            }
        }
        if !eligible {
            claim.status = ProofResolutionStatus::IncompleteDomain;
            claim.reason = ProofResolutionReason::LookupDomainIncomplete;
            claim.target = None;
            claim.evidence_chain.clear();
        }
    }
    Ok(())
}

fn seal_resolved_claim(
    file_content_hashes: &HashMap<i64, String>,
    nodes: &HashMap<NodeId, &Node>,
    edges: &[Edge],
    claims: &[ResolvedSyntaxClaim],
    claim_index: usize,
    correlation: Option<Result<usize, ExactCallsiteCorrelationFailure>>,
) -> Result<CallResolutionFact> {
    let claim = &claims[claim_index];
    let mut status = claim.status;
    let mut reason = claim.reason;
    let mut target = claim.target;
    let mut evidence_chain = claim.evidence_chain.clone();
    let edge = if status == ProofResolutionStatus::Exact {
        match correlation.expect("Exact syntax claim has a correlation result") {
            Ok(edge_index) => Some(&edges[edge_index]),
            Err(ExactCallsiteCorrelationFailure::Ambiguous) => {
                status = ProofResolutionStatus::Ambiguous;
                reason = ProofResolutionReason::MultipleBindings;
                target = None;
                evidence_chain.clear();
                None
            }
            Err(ExactCallsiteCorrelationFailure::Missing) => {
                status = ProofResolutionStatus::MissingBinding;
                reason = ProofResolutionReason::MissingBinding;
                target = None;
                evidence_chain.clear();
                None
            }
        }
    } else {
        None
    };
    let input = &claim.input;
    let mut dependency_ids = HashSet::from([NodeId(input.callsite.file_id.0)]);
    for node_id in evidence_chain
        .iter()
        .flat_map(ResolutionEvidence::node_ids)
        .chain(target)
    {
        if let Some(file_id) = nodes.get(&node_id).and_then(|node| node.file_node_id) {
            dependency_ids.insert(file_id);
        }
    }
    let mut dependency_file_hashes = dependency_ids
        .into_iter()
        .map(|file_id| {
            let source_sha256 = file_content_hashes
                .get(&file_id.0)
                .cloned()
                .ok_or_else(|| anyhow!("proof dependency file {} has no source hash", file_id.0))?;
            Ok(DependencyFileHash {
                file_id: FileId(file_id.0),
                source_sha256,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    dependency_file_hashes.sort();
    codestory_store::seal_call_resolution_fact(CallResolutionFact {
        fact_id: String::new(),
        edge_id: edge.map(|edge| edge.id),
        raw_edge_target: edge.map(|edge| edge.target),
        raw_callsite_identity: edge.and_then(|edge| edge.callsite_identity.clone()),
        callsite: input.callsite.clone(),
        caller: claim.caller,
        target,
        status,
        reason,
        evidence_chain,
        lookup_domain_complete: status != ProofResolutionStatus::IncompleteDomain,
        provenance: ResolutionProvenance {
            producer: INTERNAL_RESOLUTION_PRODUCER.to_string(),
            fact_schema_version: PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
            algorithm: EXACT_CALL_RESOLUTION_ALGORITHM.to_string(),
            language_adapter: input.language.clone(),
            language_adapter_version: input.adapter_version.clone(),
            parser_fingerprint: input.parser_fingerprint.clone(),
            dependency_file_hashes,
            evidence_sha256: String::new(),
        },
    })
    .map_err(Into::into)
}

pub fn build_funnel(facts: &[CallResolutionFact]) -> Vec<ProofResolutionFunnelRow> {
    let mut rows = BTreeMap::<
        (String, Option<CalleeForm>, Option<ResolutionEvidenceKind>),
        ProofResolutionFunnelCounts,
    >::new();
    for fact in facts {
        let evidence_kind = fact.evidence_chain.first().map(ResolutionEvidence::kind);
        let counts = rows
            .entry((
                fact.provenance.language_adapter.clone(),
                Some(fact.callsite.callee_form),
                evidence_kind,
            ))
            .or_default();
        counts.syntax_calls += 1;
        counts.adapter_supported += u64::from(fact.status != ProofResolutionStatus::Unsupported);
        match fact.status {
            ProofResolutionStatus::Exact => counts.exact += 1,
            ProofResolutionStatus::Ambiguous => counts.ambiguous += 1,
            ProofResolutionStatus::Unsupported => counts.unsupported += 1,
            ProofResolutionStatus::MissingBinding => counts.missing_binding += 1,
            ProofResolutionStatus::IncompleteDomain => counts.incomplete_domain += 1,
        }
        counts.exact_call_linked +=
            u64::from(fact.status == ProofResolutionStatus::Exact && fact.edge_id.is_some());
    }
    let mut result = rows
        .into_iter()
        .map(
            |((language, callee_form, evidence_kind), counts)| ProofResolutionFunnelRow {
                language,
                callee_form,
                evidence_kind,
                counts,
            },
        )
        .collect::<Vec<_>>();
    result.sort_by(|left, right| {
        (
            left.language.as_str(),
            left.callee_form.map(CalleeForm::as_str),
            left.evidence_kind.map(|kind| kind.as_str()),
        )
            .cmp(&(
                right.language.as_str(),
                right.callee_form.map(CalleeForm::as_str),
                right.evidence_kind.map(|kind| kind.as_str()),
            ))
    });
    result
}
