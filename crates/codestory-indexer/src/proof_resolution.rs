use crate::cache::{
    CachedCallResolutionInput, CachedDirectExport, CachedIndexArtifact, CachedInherentMethod,
    CachedResolutionBinding, CachedResolutionFile, CachedTopLevelDeclaration,
};
use crate::source_content_hash;
use anyhow::{Context, Result, anyhow};
use codestory_contracts::graph::{Edge, EdgeKind, Node, NodeId, NodeKind};
use codestory_contracts::proof_resolution::{
    CallResolutionFact, CalleeForm, DependencyFileHash, EXACT_CALL_RESOLUTION_ALGORITHM,
    ExactCallsite, FileId, INTERNAL_RESOLUTION_PRODUCER, PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
    ProofResolutionAdapter, ProofResolutionFunnelCounts, ProofResolutionFunnelRow,
    ProofResolutionProjection, ProofResolutionReason, ProofResolutionStatus, ResolutionEvidence,
    ResolutionEvidenceKind, ResolutionProvenance,
};
use codestory_store::{IndexPublicationRecord, ProofResolutionPublication, Store};
use codestory_workspace::{WorkspacePathIdentity, workspace_path_identity};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use tree_sitter::{Node as TsNode, Tree};

const ADAPTER_VERSION: &str = "reference-v3";
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
    let typescript_module =
        matches!(language, "typescript" | "tsx") && typescript_file_is_module(tree.root_node());
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
    if contains_dynamic_construct(tree.root_node(), source)
        || callable_has_shadow_or_write("typescript", callable, callee, raw_target, source)
        || root_has_write(tree.root_node(), raw_target, source)
        || typescript_root_has_competing_value_binding(tree.root_node(), raw_target, source)
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

fn typescript_root_has_competing_value_binding(root: TsNode<'_>, name: &str, source: &str) -> bool {
    let supported_import_lines = typescript_import_bindings(root, source)
        .into_iter()
        .filter(|binding| binding.local_name == name)
        .map(|binding| binding.line)
        .collect::<HashSet<_>>();
    let mut found = false;
    walk_nodes(root, &mut |node| {
        if found {
            return;
        }
        if node.kind() == "function_declaration" && typescript_callable_is_top_level(node) {
            return;
        }
        match typescript_binding_regions(node) {
            Err(()) => found = true,
            Ok(Some(regions)) => {
                if node.kind() == "import_statement"
                    && supported_import_lines.contains(&(node.start_position().row as u32 + 1))
                {
                    return;
                }
                found = regions
                    .into_iter()
                    .any(|region| subtree_binds(region, name, source));
            }
            Ok(None) => {}
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
        let Some(text) = node_text(statement, source) else {
            continue;
        };
        if text.contains("import type") || text.contains('*') {
            continue;
        }
        let Some((clause, module_tail)) = text
            .trim()
            .strip_prefix("import ")
            .and_then(|rest| rest.rsplit_once(" from "))
        else {
            continue;
        };
        let module_specifier = module_tail
            .trim()
            .trim_end_matches(';')
            .trim_matches(['\'', '"'])
            .to_string();
        if !module_specifier.starts_with("./") && !module_specifier.starts_with("../") {
            continue;
        }
        let clause = clause.trim();
        let mut parsed = Vec::<(String, String, bool)>::new();
        if clause.starts_with('{') && clause.ends_with('}') {
            for item in clause[1..clause.len() - 1].split(',') {
                let parts = item.split_whitespace().collect::<Vec<_>>();
                match parts.as_slice() {
                    [name] if !name.is_empty() => {
                        parsed.push(((*name).to_string(), (*name).to_string(), false))
                    }
                    [imported, "as", local] => {
                        parsed.push(((*local).to_string(), (*imported).to_string(), false))
                    }
                    _ => {}
                }
            }
        } else if !clause.contains(',')
            && clause
                .chars()
                .all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
        {
            parsed.push((clause.to_string(), "default".to_string(), true));
        }
        for (local_name, imported_name, is_default) in parsed {
            let statement_start = statement.start_byte();
            let Some(offset) = source[statement_start..statement.end_byte()].find(&local_name)
            else {
                continue;
            };
            let absolute = statement_start + offset;
            let prefix = &source[..absolute];
            let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
            let column = absolute
                .saturating_sub(prefix.rfind('\n').map(|index| index + 1).unwrap_or(0))
                as u32
                + 1;
            result.push(TypescriptImportBinding {
                local_name,
                imported_name,
                module_specifier: module_specifier.clone(),
                is_default,
                line,
                column,
            });
        }
    }
    result
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
        let declaration = statement.child_by_field_name("declaration")?;
        if declaration.kind() != "function_declaration"
            || statement.child_by_field_name("source").is_some()
            || statement.child_by_field_name("value").is_some()
        {
            return None;
        }
        let is_default = export_statement_has_default_token(statement)?;
        let name = declaration_name(declaration, source)?;
        if root_has_write(tree.root_node(), name, source) {
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
        let stored_hash = store
            .get_file_content_hash(indexed_file.id)?
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
            || record.file.source_sha256 != stored_hash
            || record.file.language != indexed_file.language
            || record.file.complete != indexed_file.complete
            || record.file.adapter_version != ADAPTER_VERSION
            || record.calls.iter().any(|call| {
                call.callsite.file_id != FileId(indexed_file.id)
                    || call.callsite.source_sha256 != stored_hash
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
    let mut facts = Vec::with_capacity(inputs.len());
    for (source_record, input) in inputs {
        facts.push(resolve_input(
            store,
            &file_by_id,
            &node_by_id,
            &edges,
            &record_by_path,
            &records,
            source_record,
            input,
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

fn resolve_input(
    store: &Store,
    files: &HashMap<i64, &codestory_store::FileInfo>,
    nodes: &HashMap<NodeId, &Node>,
    edges: &[Edge],
    records: &HashMap<WorkspacePathIdentity, &ResolutionCacheRecord>,
    all_records: &[ResolutionCacheRecord],
    source_record: &ResolutionCacheRecord,
    input: CachedCallResolutionInput,
) -> Result<CallResolutionFact> {
    let source_file = files
        .get(&input.callsite.file_id.0)
        .ok_or_else(|| anyhow!("proof callsite file is missing"))?;
    let mut status;
    let mut reason;
    let mut target = None;
    let mut evidence_chain = Vec::new();
    let caller = input.caller.unwrap_or(NodeId(input.callsite.file_id.0));
    match input.binding {
        CachedResolutionBinding::SameFile { declaration } => {
            let declaration_is_recorded =
                source_record
                    .file
                    .top_level_declarations
                    .iter()
                    .any(|binding| {
                        binding.name == input.callsite.raw_target
                            && binding.declaration == declaration
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
                target = Some(declaration);
                evidence_chain.push(ResolutionEvidence::SameFileDeclaration { declaration });
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
                    method.owner_name == owner_name
                        && method.method_name == input.callsite.raw_target
                })
                .collect::<Vec<_>>();
            if rust_records
                .iter()
                .any(|record| !record.file.lookup_input_complete)
            {
                status = ProofResolutionStatus::IncompleteDomain;
                reason = ProofResolutionReason::LookupDomainIncomplete;
            } else if matching_methods.len() != 1 || matching_methods[0].declaration != declaration
            {
                status = ProofResolutionStatus::Ambiguous;
                reason = ProofResolutionReason::MultipleBindings;
            } else {
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(declaration);
                evidence_chain.push(ResolutionEvidence::ImplicitReceiver { owner });
                evidence_chain.push(ResolutionEvidence::SameFileDeclaration { declaration });
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
                    export.is_default == is_default && export.exported_name == imported_name
                })
                .collect::<Vec<_>>();
            if let [declaration] = declarations.as_slice() {
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(declaration.declaration);
                evidence_chain.push(ResolutionEvidence::StaticImportBinding {
                    import,
                    declaration: declaration.declaration,
                });
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
    let mut edge = None;
    if status == ProofResolutionStatus::Exact {
        let exact_target = target.expect("exact syntax claim has a target");
        let matching = edges
            .iter()
            .filter(|candidate| {
                let raw_target_matches_span = nodes.get(&candidate.target).is_some_and(|raw| {
                    raw.file_node_id == Some(NodeId(input.callsite.file_id.0))
                        && raw.start_line == Some(input.callsite.line)
                        && raw.start_col == Some(input.callsite.column)
                        && graph_leaf_name(&raw.serialized_name) == input.callsite.raw_target
                });
                candidate.kind == EdgeKind::CALL
                    && candidate.file_node_id == Some(NodeId(input.callsite.file_id.0))
                    && candidate.line == Some(input.callsite.line)
                    && candidate.effective_source() == caller
                    && candidate.resolved_target == Some(exact_target)
                    && candidate.effective_target() == exact_target
                    && candidate.candidate_targets.is_empty()
                    && candidate
                        .callsite_identity
                        .as_deref()
                        .is_some_and(|identity| !identity.is_empty())
                    && raw_target_matches_span
            })
            .collect::<Vec<_>>();
        if matching.len() == 1 {
            edge = Some(matching[0]);
        } else {
            status = if matching.len() > 1 {
                ProofResolutionStatus::Ambiguous
            } else {
                ProofResolutionStatus::MissingBinding
            };
            reason = if matching.len() > 1 {
                ProofResolutionReason::MultipleBindings
            } else {
                ProofResolutionReason::MissingBinding
            };
            target = None;
            evidence_chain.clear();
        }
    }
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
            let source_sha256 = store
                .get_file_content_hash(file_id.0)?
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
        callsite: input.callsite,
        caller,
        target,
        status,
        reason,
        evidence_chain,
        lookup_domain_complete: status != ProofResolutionStatus::IncompleteDomain,
        provenance: ResolutionProvenance {
            producer: INTERNAL_RESOLUTION_PRODUCER.to_string(),
            fact_schema_version: PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
            algorithm: EXACT_CALL_RESOLUTION_ALGORITHM.to_string(),
            language_adapter: input.language,
            language_adapter_version: input.adapter_version,
            parser_fingerprint: input.parser_fingerprint,
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
