use crate::cache::{
    CachedCallResolutionInput, CachedClassBinding, CachedClassDeclaration, CachedClassMethod,
    CachedDeclarationKind, CachedDirectExport, CachedGoMethod, CachedGoPackage, CachedGoType,
    CachedIndexArtifact, CachedInherentMethod, CachedResolutionBinding, CachedResolutionFile,
    CachedRustFileModule, CachedRustModule, CachedRustType, CachedRustUseBinding,
    CachedTopLevelDeclaration,
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

#[cfg(test)]
thread_local! {
    static RUST_RESOLUTION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static GO_RESOLUTION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PYTHON_RESOLUTION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline]
fn count_rust_resolution_work(amount: usize) {
    #[cfg(test)]
    RUST_RESOLUTION_WORK.with(|work| work.set(work.get().saturating_add(amount)));
    #[cfg(not(test))]
    let _ = amount;
}

#[cfg(test)]
fn reset_rust_resolution_work() {
    RUST_RESOLUTION_WORK.with(|work| work.set(0));
}

#[cfg(test)]
fn rust_resolution_work() -> usize {
    RUST_RESOLUTION_WORK.with(std::cell::Cell::get)
}

#[inline]
fn count_go_resolution_work(amount: usize) {
    #[cfg(test)]
    GO_RESOLUTION_WORK.with(|work| work.set(work.get().saturating_add(amount)));
    #[cfg(not(test))]
    let _ = amount;
}

#[cfg(test)]
fn reset_go_resolution_work() {
    GO_RESOLUTION_WORK.with(|work| work.set(0));
}

#[cfg(test)]
fn go_resolution_work() -> usize {
    GO_RESOLUTION_WORK.with(std::cell::Cell::get)
}

#[inline]
fn count_python_resolution_work(amount: usize) {
    #[cfg(test)]
    PYTHON_RESOLUTION_WORK.with(|work| work.set(work.get().saturating_add(amount)));
    #[cfg(not(test))]
    let _ = amount;
}

#[cfg(test)]
fn reset_python_resolution_work() {
    PYTHON_RESOLUTION_WORK.with(|work| work.set(0));
}

#[cfg(test)]
fn python_resolution_work() -> usize {
    PYTHON_RESOLUTION_WORK.with(std::cell::Cell::get)
}

const ADAPTER_VERSION: &str = "reference-v14";
const RESOLUTION_INPUT_SCHEMA_VERSION: u32 = 12;
const INSTALLED_ADAPTERS: &[(&str, &str)] = &[
    ("go", ADAPTER_VERSION),
    ("javascript", ADAPTER_VERSION),
    ("python", ADAPTER_VERSION),
    ("rust", ADAPTER_VERSION),
    ("tsx", ADAPTER_VERSION),
    ("typescript", ADAPTER_VERSION),
];

pub fn current_proof_resolution_adapter_roster() -> Vec<ProofResolutionAdapter> {
    let mut roster = INSTALLED_ADAPTERS
        .iter()
        .map(|(language, adapter_version)| ProofResolutionAdapter {
            language: (*language).to_owned(),
            adapter_version: (*adapter_version).to_owned(),
        })
        .collect::<Vec<_>>();
    roster.sort();
    roster
}

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
    let lookup_input_complete = complete;
    let source_sha256 = source_content_hash(source.as_bytes());
    let javascript_index = is_javascript_language(language)
        .then(|| JavascriptResolutionIndex::build(tree, source, file_id, nodes));
    let rust_index =
        (language == "rust").then(|| RustResolutionIndex::build(tree, source, file_id, nodes));
    let go_index =
        (language == "go").then(|| GoResolutionIndex::build(tree, source, file_id, nodes));
    let python_index =
        (language == "python").then(|| PythonResolutionIndex::build(tree, source, file_id, nodes));
    let (direct_exports, export_poison_all, poisoned_export_names) =
        if let Some(index) = &javascript_index {
            index.collect_direct_exports(source)
        } else if let Some(index) = &python_index {
            (
                Vec::new(),
                index.module_dynamic,
                index.poisoned_export_names(),
            )
        } else {
            (Vec::new(), false, Vec::new())
        };
    let typescript_module = javascript_index
        .as_ref()
        .is_some_and(|index| index.ecmascript_module);
    let top_level_declarations = if let Some(index) = &javascript_index {
        index.cached_top_level_declarations()
    } else if let Some(index) = &rust_index {
        index.declarations.clone()
    } else if let Some(index) = &go_index {
        index.declarations.clone()
    } else if let Some(index) = &python_index {
        index.declarations.clone()
    } else {
        Vec::new()
    };
    let inherent_methods = if let Some(index) = &rust_index {
        index.methods.clone()
    } else {
        Vec::new()
    };
    let mut calls = Vec::new();
    let mut emit_call =
        |callee: TsNode<'_>, form: CalleeForm, raw_target: String, callable_id: Option<usize>| {
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
            let (caller, mut binding) = if let Some(index) = &rust_index {
                index.resolve_syntax_claim(source, callee, form, &raw_target, callable_id)
            } else if let Some(index) = &go_index {
                index.resolve_syntax_claim(source, callee, form, &raw_target, callable_id)
            } else if let Some(index) = &python_index {
                index.resolve_syntax_claim(source, callee, form, &raw_target)
            } else if let Some(index) = &javascript_index {
                index.resolve_syntax_claim(source, callee, form, &raw_target)
            } else {
                (None, CachedResolutionBinding::Unsupported)
            };
            if !lookup_input_complete {
                binding = CachedResolutionBinding::IncompleteDomain;
            }
            if matches!(binding, CachedResolutionBinding::GoImplicitReceiver { .. }) {
                callsite.callee_form = CalleeForm::ImplicitReceiver;
            }
            if matches!(binding, CachedResolutionBinding::StaticImport { .. })
                || matches!(
                    binding,
                    CachedResolutionBinding::RustPath {
                        import: Some(_),
                        ..
                    }
                ) && form == CalleeForm::Identifier
            {
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
        };
    if let Some(index) = &javascript_index {
        for call in &index.calls {
            emit_call(call.callee, call.form, call.raw_target.clone(), None);
        }
    } else if let Some(index) = &rust_index {
        for call in &index.calls {
            emit_call(
                call.callee,
                call.form,
                call.raw_target.clone(),
                call.callable_id,
            );
        }
    } else if let Some(index) = &go_index {
        for call in &index.calls {
            emit_call(
                call.callee,
                call.form,
                call.raw_target.clone(),
                call.callable_id,
            );
        }
    } else if let Some(index) = &python_index {
        for call in &index.calls {
            emit_call(call.callee, call.form, call.raw_target.clone(), None);
        }
    } else {
        collect_calls(tree.root_node(), source, &mut |callee, form, raw_target| {
            emit_call(callee, form, raw_target, None);
        });
    }
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
            classes: javascript_index.as_ref().map_or_else(
                || {
                    python_index
                        .as_ref()
                        .map_or_else(Vec::new, |index| index.classes.clone())
                },
                |index| index.cached_classes(),
            ),
            direct_exports,
            export_poison_all,
            poisoned_export_names,
            rust_modules: rust_index
                .as_ref()
                .map_or_else(Vec::new, |index| index.modules.clone()),
            rust_types: rust_index
                .as_ref()
                .map_or_else(Vec::new, |index| index.types.clone()),
            rust_uses: rust_index
                .as_ref()
                .map_or_else(Vec::new, |index| index.uses.clone()),
            go_package: go_index.as_ref().map(GoResolutionIndex::cached_package),
        }),
    }
}

fn is_installed_language(language: &str) -> bool {
    INSTALLED_ADAPTERS
        .iter()
        .any(|(installed, _)| *installed == language)
}

fn is_javascript_language(language: &str) -> bool {
    matches!(language, "javascript" | "typescript" | "tsx")
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
            let receiver = function.child_by_field_name("object")?;
            let form = if receiver.kind() == "this" {
                CalleeForm::ImplicitReceiver
            } else {
                CalleeForm::ExplicitReceiver
            };
            Some((property, form, text(property)?))
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

#[derive(Debug, Clone)]
struct IndexedGoCall<'tree> {
    callee: TsNode<'tree>,
    form: CalleeForm,
    raw_target: String,
    callable_id: Option<usize>,
}

#[derive(Debug, Clone)]
struct GoReceiverBinding {
    owner_name: String,
    pointer: bool,
    constructor: bool,
    constructor_uses_builtin_new: bool,
    imported: bool,
    implicit: bool,
}

struct GoResolutionIndex<'tree> {
    calls: Vec<IndexedGoCall<'tree>>,
    package_name: Option<String>,
    declarations: Vec<CachedTopLevelDeclaration>,
    types: Vec<CachedGoType>,
    methods: Vec<CachedGoMethod>,
    package_blockers: Vec<String>,
    import_names: HashSet<String>,
    import_domain_complete: bool,
    callable_nodes: HashMap<usize, NodeId>,
    binding_decisions: HashMap<(usize, usize, String), GoBindingDecision>,
    build_constrained: bool,
    generated: bool,
}

#[derive(Debug, Clone)]
enum GoBindingDecision {
    Receiver(GoReceiverBinding),
    Blocked,
}

#[derive(Debug, Clone)]
struct GoBindingInterval {
    name: String,
    callable_id: usize,
    start_byte: usize,
    end_byte: usize,
    scope_depth: usize,
    receiver: Option<GoReceiverBinding>,
}

impl<'tree> GoResolutionIndex<'tree> {
    fn build(tree: &'tree Tree, source: &str, file_id: NodeId, nodes: &[Node]) -> Self {
        let root = tree.root_node();
        let package_name = go_package_name(root, source);
        let graph_nodes = GoGraphNodeIndex::prepare(file_id, nodes);
        let import_domain = go_import_domain(root, source);
        let mut result = Self {
            calls: Vec::new(),
            package_name,
            declarations: Vec::new(),
            types: Vec::new(),
            methods: Vec::new(),
            package_blockers: Vec::new(),
            import_names: import_domain.names,
            import_domain_complete: import_domain.complete,
            callable_nodes: HashMap::new(),
            binding_decisions: HashMap::new(),
            build_constrained: go_source_has_build_constraint(source),
            generated: go_source_is_generated(root, source),
        };
        walk_nodes(root, &mut |node| {
            count_go_resolution_work(1);
            match node.kind() {
                "function_declaration" => {
                    let Some(name_node) = node.child_by_field_name("name") else {
                        return;
                    };
                    let Some(name) = node_text(name_node, source).map(str::to_string) else {
                        return;
                    };
                    if let Some(declaration) = graph_nodes.unique(
                        NodeKind::FUNCTION,
                        node.start_position().row as u32 + 1,
                        &name,
                    ) {
                        result.callable_nodes.insert(node.id(), declaration);
                        if node.parent().is_some_and(|parent| parent.id() == root.id()) {
                            result.declarations.push(CachedTopLevelDeclaration {
                                name,
                                declaration,
                                module_path: Vec::new(),
                                cross_module_visible: false,
                            });
                        }
                    }
                }
                "method_declaration" => {
                    let Some(name_node) = node.child_by_field_name("name") else {
                        return;
                    };
                    let Some(name) = node_text(name_node, source).map(str::to_string) else {
                        return;
                    };
                    let Some(receiver) = node.child_by_field_name("receiver") else {
                        return;
                    };
                    let Some((owner_name, pointer_receiver, _)) =
                        go_receiver_declaration(receiver, source)
                    else {
                        return;
                    };
                    if let Some(declaration) = graph_nodes.unique(
                        NodeKind::METHOD,
                        node.start_position().row as u32 + 1,
                        &name,
                    ) {
                        result.callable_nodes.insert(node.id(), declaration);
                        result.methods.push(CachedGoMethod {
                            owner_name,
                            method_name: name,
                            declaration,
                            pointer_receiver,
                        });
                    }
                }
                "type_spec" => {
                    if !go_spec_is_package_level(node, root) {
                        return;
                    }
                    let Some(name_node) = node.child_by_field_name("name") else {
                        return;
                    };
                    let Some(name) = node_text(name_node, source).map(str::to_string) else {
                        return;
                    };
                    let declaration_node = node
                        .parent()
                        .filter(|parent| parent.kind() == "type_declaration")
                        .unwrap_or(node);
                    if let Some(declaration) = graph_nodes.unique(
                        NodeKind::STRUCT,
                        declaration_node.start_position().row as u32 + 1,
                        &name,
                    ) {
                        let kind = node.child_by_field_name("type");
                        result.types.push(CachedGoType {
                            name,
                            declaration,
                            interface: kind.is_some_and(|kind| kind.kind() == "interface_type"),
                            generic: node.child_by_field_name("type_parameters").is_some(),
                        });
                    }
                }
                "var_spec" | "const_spec" if go_spec_is_package_level(node, root) => {
                    go_declared_names(node, source, &mut result.package_blockers);
                }
                "comment" => {
                    if let Some(name) = go_linkname_local_name(node, source) {
                        result.package_blockers.push(name);
                    }
                }
                "call_expression" => {
                    let Some(function) = node.child_by_field_name("function") else {
                        return;
                    };
                    let callable_id = go_enclosing_callable(node).map(|callable| callable.id());
                    match function.kind() {
                        "identifier" => {
                            if let Some(raw_target) = node_text(function, source) {
                                result.calls.push(IndexedGoCall {
                                    callee: function,
                                    form: CalleeForm::Identifier,
                                    raw_target: raw_target.to_string(),
                                    callable_id,
                                });
                            }
                        }
                        "selector_expression" => {
                            let Some(field) = function.child_by_field_name("field") else {
                                return;
                            };
                            let Some(raw_target) = node_text(field, source) else {
                                return;
                            };
                            result.calls.push(IndexedGoCall {
                                callee: field,
                                form: CalleeForm::ExplicitReceiver,
                                raw_target: raw_target.to_string(),
                                callable_id,
                            });
                        }
                        _ => {
                            let mut cursor = function.walk();
                            let leaf = function
                                .named_children(&mut cursor)
                                .last()
                                .unwrap_or(function);
                            result.calls.push(IndexedGoCall {
                                callee: leaf,
                                form: CalleeForm::DynamicAccess,
                                raw_target: node_text(leaf, source)
                                    .unwrap_or(function.kind())
                                    .to_string(),
                                callable_id,
                            });
                        }
                    }
                }
                _ => {}
            }
        });
        result.declarations.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.declaration.cmp(&right.declaration))
        });
        result.types.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.declaration.cmp(&right.declaration))
        });
        result.methods.sort_by(|left, right| {
            (&left.owner_name, &left.method_name, left.declaration).cmp(&(
                &right.owner_name,
                &right.method_name,
                right.declaration,
            ))
        });
        result.package_blockers.sort();
        result.package_blockers.dedup();
        result
            .calls
            .sort_by_key(|call| (call.callee.start_byte(), call.callee.end_byte()));
        result.binding_decisions =
            go_prepare_binding_decisions(root, source, &result.calls, &result.import_names);
        result
    }

    fn cached_package(&self) -> CachedGoPackage {
        CachedGoPackage {
            name: self.package_name.clone().unwrap_or_default(),
            build_constrained: self.build_constrained,
            generated: self.generated,
            package_blockers: self.package_blockers.clone(),
            types: self.types.clone(),
            methods: self.methods.clone(),
        }
    }

    fn resolve_syntax_claim(
        &self,
        source: &str,
        callee: TsNode<'tree>,
        form: CalleeForm,
        raw_target: &str,
        callable_id: Option<usize>,
    ) -> (Option<NodeId>, CachedResolutionBinding) {
        let Some(callable_id) = callable_id else {
            return (None, CachedResolutionBinding::MissingBinding);
        };
        let Some(caller) = self.callable_nodes.get(&callable_id).copied() else {
            return (None, CachedResolutionBinding::Ambiguous);
        };
        let Some(package_name) = self.package_name.clone() else {
            return (Some(caller), CachedResolutionBinding::IncompleteDomain);
        };
        if self.build_constrained {
            return (Some(caller), CachedResolutionBinding::IncompleteDomain);
        }
        if self.generated {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        if !self.import_domain_complete {
            return (Some(caller), CachedResolutionBinding::IncompleteDomain);
        }
        match form {
            CalleeForm::Identifier => {
                if self.import_names.contains(raw_target)
                    || self.binding_decisions.contains_key(&(
                        callable_id,
                        callee.start_byte(),
                        raw_target.to_string(),
                    ))
                {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                }
                (
                    Some(caller),
                    CachedResolutionBinding::GoPackageFunction {
                        package_name,
                        name: raw_target.to_string(),
                    },
                )
            }
            CalleeForm::ExplicitReceiver => {
                let Some(call) = callee.parent().and_then(|selector| selector.parent()) else {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                };
                let Some(selector) = call.child_by_field_name("function") else {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                };
                let Some(receiver) = selector.child_by_field_name("operand") else {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                };
                let Some(receiver_name) = go_simple_identifier(receiver, source) else {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                };
                if self.import_names.contains(receiver_name) {
                    return (Some(caller), CachedResolutionBinding::IncompleteDomain);
                }
                let binding = match self.binding_decisions.get(&(
                    callable_id,
                    callee.start_byte(),
                    receiver_name.to_string(),
                )) {
                    Some(GoBindingDecision::Receiver(binding)) => binding.clone(),
                    Some(GoBindingDecision::Blocked) | None => {
                        return (Some(caller), CachedResolutionBinding::Unsupported);
                    }
                };
                if binding.imported {
                    return (Some(caller), CachedResolutionBinding::IncompleteDomain);
                }
                if binding.implicit {
                    (
                        Some(caller),
                        CachedResolutionBinding::GoImplicitReceiver {
                            package_name,
                            owner_name: binding.owner_name,
                            receiver_is_pointer: binding.pointer,
                        },
                    )
                } else {
                    (
                        Some(caller),
                        CachedResolutionBinding::GoExplicitReceiver {
                            package_name,
                            owner_name: binding.owner_name,
                            receiver_is_pointer: binding.pointer,
                            constructor: binding.constructor,
                            constructor_uses_builtin_new: binding.constructor_uses_builtin_new,
                        },
                    )
                }
            }
            _ => (Some(caller), CachedResolutionBinding::Unsupported),
        }
    }
}

struct GoGraphNodeIndex {
    nodes: HashMap<(NodeKind, u32, String), Vec<NodeId>>,
}

impl GoGraphNodeIndex {
    fn prepare(file_id: NodeId, nodes: &[Node]) -> Self {
        let mut result = HashMap::<_, Vec<_>>::new();
        for node in nodes
            .iter()
            .filter(|node| node.file_node_id == Some(file_id))
        {
            if let Some(line) = node.start_line {
                result
                    .entry((
                        node.kind,
                        line,
                        graph_leaf_name(&node.serialized_name).to_string(),
                    ))
                    .or_default()
                    .push(node.id);
            }
        }
        Self { nodes: result }
    }

    fn unique(&self, kind: NodeKind, line: u32, name: &str) -> Option<NodeId> {
        let values = self.nodes.get(&(kind, line, name.to_string()))?;
        let [value] = values.as_slice() else {
            return None;
        };
        Some(*value)
    }
}

fn go_package_name(root: TsNode<'_>, source: &str) -> Option<String> {
    let mut cursor = root.walk();
    let clauses = root
        .named_children(&mut cursor)
        .filter(|node| node.kind() == "package_clause")
        .collect::<Vec<_>>();
    let [clause] = clauses.as_slice() else {
        return None;
    };
    let mut cursor = clause.walk();
    clause
        .named_children(&mut cursor)
        .find(|node| node.kind() == "package_identifier")
        .and_then(|node| node_text(node, source))
        .map(str::to_string)
}

fn go_spec_is_package_level(node: TsNode<'_>, root: TsNode<'_>) -> bool {
    node.parent()
        .and_then(|declaration| declaration.parent())
        .is_some_and(|parent| parent.id() == root.id())
}

fn go_enclosing_callable(mut node: TsNode<'_>) -> Option<TsNode<'_>> {
    loop {
        if matches!(node.kind(), "function_declaration" | "method_declaration") {
            return Some(node);
        }
        if node.kind() == "func_literal" {
            return None;
        }
        node = node.parent()?;
    }
}

fn go_receiver_declaration(receiver: TsNode<'_>, source: &str) -> Option<(String, bool, String)> {
    let mut cursor = receiver.walk();
    let parameters = receiver
        .named_children(&mut cursor)
        .filter(|node| node.kind() == "parameter_declaration")
        .collect::<Vec<_>>();
    let [parameter] = parameters.as_slice() else {
        return None;
    };
    let type_node = parameter.child_by_field_name("type")?;
    let binding = go_exact_type_node(type_node, source, &HashSet::new(), true)?;
    if binding.imported {
        return None;
    }
    let mut cursor = parameter.walk();
    let names = parameter
        .named_children(&mut cursor)
        .take_while(|node| node.start_byte() < type_node.start_byte())
        .filter_map(|node| go_simple_identifier(node, source).map(str::to_string))
        .collect::<Vec<_>>();
    let variable = match names.as_slice() {
        [] => String::new(),
        [name] => name.clone(),
        _ => return None,
    };
    Some((binding.owner_name, binding.pointer, variable))
}

fn go_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|character| character == '_' || character.is_alphabetic())
        && chars.all(|character| character == '_' || character.is_alphanumeric())
}

fn go_simple_identifier<'a>(node: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    (node.kind() == "identifier")
        .then(|| node_text(node, source))
        .flatten()
        .filter(|name| go_identifier(name))
}

fn go_source_has_build_constraint(source: &str) -> bool {
    source
        .lines()
        .take_while(|line| {
            let trimmed = line.trim();
            trimmed.is_empty() || trimmed.starts_with("//")
        })
        .any(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("//go:build") || trimmed.starts_with("// +build")
        })
}

fn go_source_is_generated(root: TsNode<'_>, source: &str) -> bool {
    let package_start = {
        let mut cursor = root.walk();
        root.named_children(&mut cursor)
            .find(|node| node.kind() == "package_clause")
            .map(|node| node.start_byte())
            .unwrap_or(usize::MAX)
    };
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .take_while(|node| node.start_byte() < package_start)
        .filter(|node| node.kind() == "comment")
        .filter_map(|node| node_text(node, source))
        .flat_map(str::lines)
        .any(|line| {
            let line = line
                .trim()
                .trim_start_matches("/*")
                .trim_start_matches('*')
                .trim_end_matches("*/")
                .trim();
            line.starts_with("// Code generated ") && line.ends_with(" DO NOT EDIT.")
        })
}

fn go_linkname_local_name(node: TsNode<'_>, source: &str) -> Option<String> {
    let text = node_text(node, source)?.trim();
    let mut components = text.strip_prefix("//go:linkname")?.split_whitespace();
    let local = components.next()?;
    let remote = components.next()?;
    (components.next().is_none() && go_identifier(local) && !remote.is_empty())
        .then(|| local.to_string())
}

fn go_declared_names(node: TsNode<'_>, source: &str, names: &mut Vec<String>) {
    let boundary = node
        .child_by_field_name("type")
        .or_else(|| node.child_by_field_name("value"))
        .map(|node| node.start_byte())
        .unwrap_or(usize::MAX);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.start_byte() >= boundary {
            break;
        }
        if child.kind() == "identifier"
            && let Some(name) = node_text(child, source)
        {
            names.push(name.to_string());
        }
    }
}

struct GoImportDomain {
    names: HashSet<String>,
    complete: bool,
}

fn go_import_domain(root: TsNode<'_>, source: &str) -> GoImportDomain {
    let mut values = HashSet::new();
    let mut complete = true;
    walk_nodes(root, &mut |node| {
        if node.kind() != "import_spec" {
            return;
        }
        let Some(path) = node.child_by_field_name("path") else {
            complete = false;
            return;
        };
        let Some(path) = node_text(path, source).and_then(go_string_literal) else {
            complete = false;
            return;
        };
        let explicit_alias = node
            .child_by_field_name("name")
            .and_then(|node| node_text(node, source))
            .map(str::to_string);
        if explicit_alias.as_deref() == Some(".") {
            complete = false;
            return;
        }
        if explicit_alias.as_deref() == Some("_") {
            return;
        }
        let alias = explicit_alias.or_else(|| path.rsplit('/').next().map(str::to_string));
        let Some(alias) = alias.filter(|alias| !alias.is_empty() && go_identifier(alias)) else {
            complete = false;
            return;
        };
        if !values.insert(alias) {
            complete = false;
        }
    });
    GoImportDomain {
        names: values,
        complete,
    }
}

fn go_string_literal(literal: &str) -> Option<&str> {
    let literal = literal.trim();
    let quote = literal.as_bytes().first().copied()?;
    if !matches!(quote, b'"' | b'`') || literal.as_bytes().last().copied()? != quote {
        return None;
    }
    let value = literal.get(1..literal.len().checked_sub(1)?)?;
    (quote == b'`' || !value.contains('\\')).then_some(value)
}

fn go_expression_names(node: TsNode<'_>, source: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut cursor = node.walk();
    if node.kind() == "expression_list" {
        for child in node.named_children(&mut cursor) {
            if let Some(name) = go_simple_identifier(child, source) {
                result.push(name.to_string());
            }
        }
    } else if let Some(name) = go_simple_identifier(node, source) {
        result.push(name.to_string());
    }
    result
}

fn go_prepare_binding_decisions(
    root: TsNode<'_>,
    source: &str,
    calls: &[IndexedGoCall<'_>],
    import_names: &HashSet<String>,
) -> HashMap<(usize, usize, String), GoBindingDecision> {
    let shadowed_new = go_callables_declaring_name(root, source, "new");
    let mut intervals = Vec::<GoBindingInterval>::new();
    walk_nodes(root, &mut |node| {
        count_go_resolution_work(1);
        let Some(callable) = go_enclosing_callable(node) else {
            return;
        };
        if node.id() == callable.id() {
            if callable.kind() == "method_declaration"
                && let Some(receiver) = callable.child_by_field_name("receiver")
                && let Some((owner_name, pointer, variable)) =
                    go_receiver_declaration(receiver, source)
                && !variable.is_empty()
            {
                intervals.push(GoBindingInterval {
                    name: variable,
                    callable_id: callable.id(),
                    start_byte: callable.start_byte(),
                    end_byte: callable.end_byte(),
                    scope_depth: 0,
                    receiver: Some(GoReceiverBinding {
                        owner_name,
                        pointer,
                        constructor: false,
                        constructor_uses_builtin_new: false,
                        imported: false,
                        implicit: true,
                    }),
                });
            }
            return;
        }
        let Some((scope_start, scope_end, depth)) = go_binding_scope(node, callable) else {
            return;
        };
        match node.kind() {
            "parameter_declaration" | "variadic_parameter_declaration" => {
                if callable.kind() == "method_declaration"
                    && callable
                        .child_by_field_name("receiver")
                        .is_some_and(|receiver| {
                            receiver.start_byte() <= node.start_byte()
                                && node.end_byte() <= receiver.end_byte()
                        })
                {
                    return;
                }
                let Some(type_node) = node.child_by_field_name("type") else {
                    return;
                };
                let receiver = go_typed_receiver_binding(type_node, source, import_names);
                let mut cursor = node.walk();
                for name_node in node.named_children(&mut cursor) {
                    if name_node.start_byte() >= type_node.start_byte() {
                        break;
                    }
                    if let Some(name) = go_simple_identifier(name_node, source) {
                        intervals.push(GoBindingInterval {
                            name: name.to_string(),
                            callable_id: callable.id(),
                            start_byte: callable.start_byte(),
                            end_byte: callable.end_byte(),
                            scope_depth: 0,
                            receiver: receiver.clone(),
                        });
                    }
                }
            }
            "short_var_declaration" | "assignment_statement" => {
                let names = node
                    .child_by_field_name("left")
                    .map(|left| go_expression_names(left, source))
                    .unwrap_or_default();
                let values = node
                    .child_by_field_name("right")
                    .map(go_expression_items)
                    .unwrap_or_default();
                for (index, name) in names.into_iter().enumerate() {
                    let assignment = node.kind() == "assignment_statement";
                    let receiver = (!assignment && !go_binding_uses_control_flow(node, callable))
                        .then(|| {
                            values.get(index).and_then(|value| {
                                go_direct_constructor_binding_prepared(
                                    *value,
                                    source,
                                    !shadowed_new.contains(&callable.id())
                                        && !import_names.contains("new"),
                                    import_names,
                                )
                            })
                        })
                        .flatten();
                    intervals.push(GoBindingInterval {
                        name,
                        callable_id: callable.id(),
                        start_byte: node.end_byte().max(scope_start),
                        end_byte: if assignment {
                            callable.end_byte()
                        } else {
                            scope_end
                        },
                        scope_depth: if assignment { usize::MAX - 1 } else { depth },
                        receiver,
                    });
                }
            }
            "var_spec" => {
                let Some(type_node) = node.child_by_field_name("type") else {
                    return;
                };
                let receiver = (!go_binding_uses_control_flow(node, callable))
                    .then(|| go_typed_receiver_binding(type_node, source, import_names))
                    .flatten();
                let mut names = Vec::new();
                go_declared_names(node, source, &mut names);
                for name in names {
                    intervals.push(GoBindingInterval {
                        name,
                        callable_id: callable.id(),
                        start_byte: node.end_byte().max(scope_start),
                        end_byte: scope_end,
                        scope_depth: depth,
                        receiver: receiver.clone(),
                    });
                }
            }
            "const_spec" | "type_spec" => {
                let mut names = Vec::new();
                go_declared_names(node, source, &mut names);
                if node.kind() == "type_spec"
                    && let Some(name) = node
                        .child_by_field_name("name")
                        .and_then(|name| node_text(name, source))
                {
                    names.push(name.to_string());
                }
                for name in names {
                    intervals.push(GoBindingInterval {
                        name,
                        callable_id: callable.id(),
                        start_byte: node.end_byte().max(scope_start),
                        end_byte: scope_end,
                        scope_depth: depth,
                        receiver: None,
                    });
                }
            }
            "range_clause" | "receive_statement" | "type_switch_guard" => {
                let Some(special) = go_special_binding(node, callable, source) else {
                    return;
                };
                for name in special.names {
                    intervals.push(GoBindingInterval {
                        name,
                        callable_id: callable.id(),
                        start_byte: node.end_byte(),
                        end_byte: special.end_byte,
                        scope_depth: special.scope_depth,
                        receiver: None,
                    });
                }
            }
            "type_switch_statement" => {
                let Some(special) = go_type_switch_binding(node, callable, source) else {
                    return;
                };
                for name in special.names {
                    intervals.push(GoBindingInterval {
                        name,
                        callable_id: callable.id(),
                        start_byte: node.start_byte(),
                        end_byte: special.end_byte,
                        scope_depth: special.scope_depth,
                        receiver: None,
                    });
                }
            }
            "unary_expression" => {
                let Some(surface) = node_text(node, source).map(str::trim) else {
                    return;
                };
                let Some(name) = surface.strip_prefix('&').map(str::trim) else {
                    return;
                };
                if go_identifier(name) {
                    intervals.push(GoBindingInterval {
                        name: name.to_string(),
                        callable_id: callable.id(),
                        start_byte: callable.start_byte(),
                        end_byte: callable.end_byte(),
                        scope_depth: usize::MAX,
                        receiver: None,
                    });
                }
            }
            _ => {}
        }
    });
    walk_nodes(root, &mut |node| {
        if node.kind() != "identifier" {
            return;
        }
        let Some(callable) = go_outer_callable_for_captured_node(node) else {
            return;
        };
        let Some(name) = node_text(node, source).filter(|name| go_identifier(name)) else {
            return;
        };
        intervals.push(GoBindingInterval {
            name: name.to_string(),
            callable_id: callable.id(),
            start_byte: callable.start_byte(),
            end_byte: callable.end_byte(),
            scope_depth: usize::MAX,
            receiver: None,
        });
        count_go_resolution_work(1);
    });
    let mut intervals_by_name = HashMap::<(usize, String), Vec<GoBindingInterval>>::new();
    for interval in intervals {
        if interval.start_byte < interval.end_byte {
            intervals_by_name
                .entry((interval.callable_id, interval.name.clone()))
                .or_default()
                .push(interval);
        }
    }
    let mut calls_by_name = HashMap::<(usize, String), Vec<usize>>::new();
    for call in calls {
        let Some(callable_id) = call.callable_id else {
            continue;
        };
        let name = if call.form == CalleeForm::Identifier {
            Some(call.raw_target.clone())
        } else {
            call.callee
                .parent()
                .and_then(|selector| selector.child_by_field_name("operand"))
                .and_then(|receiver| go_simple_identifier(receiver, source))
                .map(str::to_string)
        };
        if let Some(name) = name {
            calls_by_name
                .entry((callable_id, name))
                .or_default()
                .push(call.callee.start_byte());
        }
    }
    let mut decisions = HashMap::new();
    for (key, mut callsites) in calls_by_name {
        let bindings = intervals_by_name.remove(&key).unwrap_or_default();
        callsites.sort_unstable();
        let mut events = Vec::with_capacity(bindings.len() * 2 + callsites.len());
        for (index, binding) in bindings.iter().enumerate() {
            events.push((binding.start_byte, 1_u8, index));
            events.push((binding.end_byte, 0_u8, index));
            count_go_resolution_work(2);
        }
        for (index, callsite) in callsites.iter().copied().enumerate() {
            events.push((callsite, 2_u8, index));
            count_go_resolution_work(1);
        }
        events.sort_unstable();
        let mut active = BTreeMap::<usize, HashSet<usize>>::new();
        for (byte, kind, index) in events {
            count_go_resolution_work(1);
            match kind {
                0 => {
                    let depth = bindings[index].scope_depth;
                    if let Some(entries) = active.get_mut(&depth) {
                        entries.remove(&index);
                        if entries.is_empty() {
                            active.remove(&depth);
                        }
                    }
                }
                1 => {
                    active
                        .entry(bindings[index].scope_depth)
                        .or_default()
                        .insert(index);
                }
                _ => {
                    let Some((_, entries)) = active.last_key_value() else {
                        continue;
                    };
                    let decision = if entries.len() == 1 {
                        let binding = &bindings[*entries.iter().next().expect("one binding")];
                        binding
                            .receiver
                            .clone()
                            .map_or(GoBindingDecision::Blocked, GoBindingDecision::Receiver)
                    } else {
                        GoBindingDecision::Blocked
                    };
                    decisions.insert((key.0, byte, key.1.clone()), decision);
                }
            }
        }
    }
    decisions
}

fn go_callables_declaring_name(root: TsNode<'_>, source: &str, wanted: &str) -> HashSet<usize> {
    let mut result = HashSet::new();
    walk_nodes(root, &mut |node| {
        let Some(callable) = go_enclosing_callable(node) else {
            return;
        };
        let declared = match node.kind() {
            "parameter_declaration" | "variadic_parameter_declaration" => {
                let boundary = node
                    .child_by_field_name("type")
                    .map(|node| node.start_byte())
                    .unwrap_or(usize::MAX);
                let mut cursor = node.walk();
                node.named_children(&mut cursor).any(|child| {
                    child.start_byte() < boundary
                        && go_simple_identifier(child, source) == Some(wanted)
                })
            }
            "short_var_declaration" | "assignment_statement" => {
                node.child_by_field_name("left").is_some_and(|left| {
                    go_expression_names(left, source)
                        .iter()
                        .any(|name| name == wanted)
                })
            }
            "var_spec" | "const_spec" | "type_spec" => {
                let mut names = Vec::new();
                go_declared_names(node, source, &mut names);
                if node.kind() == "type_spec"
                    && let Some(name) = node
                        .child_by_field_name("name")
                        .and_then(|name| node_text(name, source))
                {
                    names.push(name.to_string());
                }
                names.iter().any(|name| name == wanted)
            }
            "range_clause" | "receive_statement" | "type_switch_guard" => {
                go_special_binding_names(node_text(node, source).unwrap_or_default())
                    .iter()
                    .any(|name| name == wanted)
            }
            "type_switch_statement" => go_type_switch_header(node, source)
                .map(go_special_binding_names)
                .unwrap_or_default()
                .iter()
                .any(|name| name == wanted),
            _ => false,
        };
        if declared {
            result.insert(callable.id());
        }
    });
    result
}

struct GoSpecialBinding {
    names: Vec<String>,
    end_byte: usize,
    scope_depth: usize,
}

fn go_special_binding(
    node: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
) -> Option<GoSpecialBinding> {
    let surface = node_text(node, source)?;
    let names = go_special_binding_names(surface);
    if names.is_empty() {
        return None;
    }
    let declaration = surface.contains(":=");
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
    Some(GoSpecialBinding {
        names,
        end_byte: if declaration {
            boundary.end_byte()
        } else {
            callable.end_byte()
        },
        scope_depth: if declaration {
            go_scope_depth(boundary, callable).saturating_add(1)
        } else {
            usize::MAX - 2
        },
    })
}

fn go_type_switch_binding(
    node: TsNode<'_>,
    callable: TsNode<'_>,
    source: &str,
) -> Option<GoSpecialBinding> {
    let names = go_type_switch_header(node, source).map(go_special_binding_names)?;
    (!names.is_empty()).then(|| GoSpecialBinding {
        names,
        end_byte: node.end_byte(),
        scope_depth: go_scope_depth(node, callable).saturating_add(1),
    })
}

fn go_type_switch_header<'a>(node: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    let (header, _) = node_text(node, source)?.split_once('{')?;
    header.trim().strip_prefix("switch").map(str::trim)
}

fn go_special_binding_names(surface: &str) -> Vec<String> {
    let left = surface
        .split_once(":=")
        .or_else(|| surface.split_once('='))
        .map(|(left, _)| left)
        .unwrap_or_default();
    left.rsplit([';', '{', ':'])
        .next()
        .unwrap_or(left)
        .split(',')
        .map(str::trim)
        .filter(|name| *name != "_" && go_identifier(name))
        .map(str::to_string)
        .collect()
}

fn go_scope_depth(mut node: TsNode<'_>, callable: TsNode<'_>) -> usize {
    let mut depth: usize = 0;
    while node.id() != callable.id() {
        depth = depth.saturating_add(usize::from(node.kind() == "block"));
        let Some(parent) = node.parent() else {
            break;
        };
        node = parent;
    }
    depth
}

fn go_binding_scope(node: TsNode<'_>, callable: TsNode<'_>) -> Option<(usize, usize, usize)> {
    let mut current = node;
    let mut depth = 0;
    loop {
        if current.kind() == "block" {
            depth += 1;
            return Some((current.start_byte(), current.end_byte(), depth));
        }
        if current.id() == callable.id() {
            return Some((callable.start_byte(), callable.end_byte(), 0));
        }
        current = current.parent()?;
    }
}

fn go_binding_uses_control_flow(mut node: TsNode<'_>, callable: TsNode<'_>) -> bool {
    while node.id() != callable.id() {
        if matches!(
            node.kind(),
            "if_statement"
                | "for_statement"
                | "expression_switch_statement"
                | "type_switch_statement"
                | "select_statement"
        ) {
            return true;
        }
        let Some(parent) = node.parent() else {
            return true;
        };
        node = parent;
    }
    false
}

fn go_outer_callable_for_captured_node(mut node: TsNode<'_>) -> Option<TsNode<'_>> {
    let mut crossed_closure = false;
    loop {
        crossed_closure |= node.kind() == "func_literal";
        if crossed_closure && matches!(node.kind(), "function_declaration" | "method_declaration") {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn go_typed_receiver_binding(
    type_node: TsNode<'_>,
    source: &str,
    import_names: &HashSet<String>,
) -> Option<GoReceiverBinding> {
    let mut binding = go_exact_type_node(type_node, source, import_names, true)?;
    binding.constructor = false;
    binding.constructor_uses_builtin_new = false;
    binding.implicit = false;
    Some(binding)
}

fn go_exact_type_node(
    node: TsNode<'_>,
    source: &str,
    import_names: &HashSet<String>,
    allow_pointer: bool,
) -> Option<GoReceiverBinding> {
    let (base, pointer) = if node.kind() == "pointer_type" && allow_pointer {
        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        let [base] = children.as_slice() else {
            return None;
        };
        if base.kind() == "pointer_type" {
            return None;
        }
        (*base, true)
    } else {
        (node, false)
    };
    let surface = node_text(base, source)?.trim();
    let (owner_name, imported) = match base.kind() {
        "type_identifier" => {
            if !go_identifier(surface) {
                return None;
            }
            (surface, false)
        }
        "qualified_type" => {
            let (qualifier, owner_name) = surface.split_once('.')?;
            if owner_name.contains('.')
                || !go_identifier(qualifier)
                || !go_identifier(owner_name)
                || !import_names.contains(qualifier)
            {
                return None;
            }
            (owner_name, true)
        }
        _ => return None,
    };
    Some(GoReceiverBinding {
        owner_name: owner_name.to_string(),
        pointer,
        constructor: false,
        constructor_uses_builtin_new: false,
        imported,
        implicit: false,
    })
}

fn go_direct_constructor_binding_prepared(
    value: TsNode<'_>,
    source: &str,
    builtin_new_unshadowed: bool,
    import_names: &HashSet<String>,
) -> Option<GoReceiverBinding> {
    let (type_node, pointer, constructor_uses_builtin_new) = match value.kind() {
        "composite_literal" => (value.child_by_field_name("type")?, false, false),
        "unary_expression" => {
            let surface = node_text(value, source)?.trim();
            if !surface.starts_with('&') {
                return None;
            }
            let mut cursor = value.walk();
            let children = value.named_children(&mut cursor).collect::<Vec<_>>();
            let [literal] = children.as_slice() else {
                return None;
            };
            if literal.kind() != "composite_literal" {
                return None;
            }
            (literal.child_by_field_name("type")?, true, false)
        }
        "call_expression" if builtin_new_unshadowed => {
            let function = value.child_by_field_name("function")?;
            if go_simple_identifier(function, source) != Some("new") {
                return None;
            }
            let arguments = value.child_by_field_name("arguments")?;
            let mut cursor = arguments.walk();
            let arguments = arguments.named_children(&mut cursor).collect::<Vec<_>>();
            let [type_node] = arguments.as_slice() else {
                return None;
            };
            (*type_node, true, true)
        }
        _ => return None,
    };
    let mut binding = go_exact_type_node(type_node, source, import_names, false)?;
    binding.pointer = pointer;
    binding.constructor = true;
    binding.constructor_uses_builtin_new = constructor_uses_builtin_new;
    Some(binding)
}

fn go_expression_items(node: TsNode<'_>) -> Vec<TsNode<'_>> {
    if node.kind() != "expression_list" {
        return vec![node];
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

#[derive(Debug, Clone)]
struct IndexedPythonCall<'tree> {
    callee: TsNode<'tree>,
    form: CalleeForm,
    raw_target: String,
}

#[derive(Debug, Clone)]
enum PythonNameBinding {
    Receiver {
        class_binding: CachedClassBinding,
        constructor: bool,
        type_name: String,
    },
    Other,
}

#[derive(Debug, Clone)]
struct PythonBindingEvent {
    at: usize,
    binding: PythonNameBinding,
}

#[derive(Debug, Clone)]
struct PythonFunctionInfo {
    graph_id: Option<NodeId>,
    direct_method: bool,
    owner: Option<(NodeId, String)>,
    plain_self: bool,
}

#[derive(Debug, Clone)]
struct PythonImportBinding {
    import: Option<NodeId>,
    module_specifier: String,
    imported_name: String,
}

struct PythonResolutionIndex<'tree> {
    calls: Vec<IndexedPythonCall<'tree>>,
    declarations: Vec<CachedTopLevelDeclaration>,
    classes: Vec<CachedClassDeclaration>,
    functions: HashMap<usize, PythonFunctionInfo>,
    bindings: HashMap<usize, HashMap<String, Vec<PythonBindingEvent>>>,
    imports: HashMap<String, Vec<PythonImportBinding>>,
    module_blockers: HashMap<String, usize>,
    module_dynamic: bool,
    dynamic_functions: HashSet<usize>,
    declarations_by_name: HashMap<String, Vec<NodeId>>,
    classes_by_name: HashMap<String, Vec<CachedClassDeclaration>>,
    class_owner_by_syntax_id: HashMap<usize, NodeId>,
    closed_class_owners: HashSet<NodeId>,
    simple_base_class_owners: HashSet<NodeId>,
    dynamic_class_owners: HashSet<NodeId>,
    methods_by_owner_and_name: HashMap<(NodeId, String), Vec<NodeId>>,
    method_blockers_by_owner_and_name: HashSet<(NodeId, String)>,
    global_names_by_function: HashMap<usize, HashSet<String>>,
    writes_by_function: HashMap<usize, HashSet<String>>,
}

impl<'tree> PythonResolutionIndex<'tree> {
    fn build(tree: &'tree Tree, source: &str, file_id: NodeId, nodes: &[Node]) -> Self {
        let root = tree.root_node();
        let mut callable_nodes = HashMap::<(u32, String), Vec<NodeId>>::new();
        let mut class_nodes = HashMap::<(u32, String), Vec<NodeId>>::new();
        let mut import_nodes = HashMap::<(u32, u32, String), Vec<NodeId>>::new();
        for node in nodes
            .iter()
            .filter(|node| node.file_node_id == Some(file_id))
        {
            count_python_resolution_work(1);
            let Some(line) = node.start_line else {
                continue;
            };
            let name = graph_leaf_name(&node.serialized_name).to_string();
            if matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD) {
                callable_nodes
                    .entry((line, name.clone()))
                    .or_default()
                    .push(node.id);
            } else if node.kind == NodeKind::CLASS {
                class_nodes
                    .entry((line, name.clone()))
                    .or_default()
                    .push(node.id);
            }
            if let Some(column) = node.start_col {
                import_nodes
                    .entry((line, column, name))
                    .or_default()
                    .push(node.id);
            }
        }

        let mut result = Self {
            calls: Vec::new(),
            declarations: Vec::new(),
            classes: Vec::new(),
            functions: HashMap::new(),
            bindings: HashMap::new(),
            imports: HashMap::new(),
            module_blockers: HashMap::new(),
            module_dynamic: false,
            dynamic_functions: HashSet::new(),
            declarations_by_name: HashMap::new(),
            classes_by_name: HashMap::new(),
            class_owner_by_syntax_id: HashMap::new(),
            closed_class_owners: HashSet::new(),
            simple_base_class_owners: HashSet::new(),
            dynamic_class_owners: HashSet::new(),
            methods_by_owner_and_name: HashMap::new(),
            method_blockers_by_owner_and_name: HashSet::new(),
            global_names_by_function: HashMap::new(),
            writes_by_function: HashMap::new(),
        };

        walk_nodes(root, &mut |node| {
            count_python_resolution_work(1);
            match node.kind() {
                "function_definition" => {
                    result.collect_function(node, root, source, &callable_nodes, &class_nodes);
                }
                "class_definition" => {
                    result.collect_class(node, root, source, &class_nodes, &callable_nodes);
                }
                "import_from_statement" => {
                    result.collect_import(node, root, source, &import_nodes);
                    if python_enclosing_function(node).is_some() {
                        result.collect_local_binding_node(node, source);
                    }
                }
                "call" => {
                    result.collect_call(node, source);
                    if python_direct_call_name(node, source)
                        .is_some_and(|name| matches!(name, "exec" | "eval" | "globals"))
                    {
                        if let Some(function) = python_enclosing_function(node) {
                            result.dynamic_functions.insert(function.id());
                        } else {
                            result.module_dynamic = true;
                        }
                    }
                }
                "assignment" | "augmented_assignment" => result.collect_assignment(node, source),
                "parameters" => result.collect_parameter_bindings(node, source),
                "for_statement"
                | "with_item"
                | "except_clause"
                | "named_expression"
                | "global_statement"
                | "nonlocal_statement"
                | "delete_statement"
                | "case_pattern"
                | "list_comprehension"
                | "set_comprehension"
                | "dictionary_comprehension"
                | "generator_expression"
                | "import_statement" => result.collect_local_binding_node(node, source),
                _ => {}
            }
        });
        result.collect_module_class_member_mutations(root, source);
        for (function, global_names) in &result.global_names_by_function {
            if let Some(writes) = result.writes_by_function.get(function) {
                for name in global_names.intersection(writes) {
                    *result.module_blockers.entry(name.clone()).or_default() += 1;
                }
            }
        }
        for events in result.bindings.values_mut().flat_map(HashMap::values_mut) {
            events.sort_by_key(|event| event.at);
        }
        result
            .calls
            .sort_by_key(|call| (call.callee.start_byte(), call.callee.end_byte()));
        result.declarations.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.declaration.cmp(&right.declaration))
        });
        result.classes.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.declaration.cmp(&right.declaration))
        });
        for declaration in &result.declarations {
            result
                .declarations_by_name
                .entry(declaration.name.clone())
                .or_default()
                .push(declaration.declaration);
        }
        for class in &result.classes {
            for method in &class.methods {
                result
                    .methods_by_owner_and_name
                    .entry((class.declaration, method.name.clone()))
                    .or_default()
                    .push(method.declaration);
            }
        }
        result
    }

    fn poisoned_export_names(&self) -> Vec<String> {
        let mut names = self.module_blockers.keys().cloned().collect::<HashSet<_>>();
        names.extend(self.imports.keys().cloned());
        for class in &self.classes {
            if self
                .method_blockers_by_owner_and_name
                .iter()
                .any(|(owner, _)| *owner == class.declaration)
            {
                names.insert(class.name.clone());
            }
        }
        let mut names = names.into_iter().collect::<Vec<_>>();
        names.sort();
        names
    }

    fn collect_function(
        &mut self,
        node: TsNode<'tree>,
        root: TsNode<'tree>,
        source: &str,
        callable_nodes: &HashMap<(u32, String), Vec<NodeId>>,
        class_nodes: &HashMap<(u32, String), Vec<NodeId>>,
    ) {
        let Some(name) = declaration_name(node, source).map(str::to_string) else {
            return;
        };
        let graph_id = python_unique_graph_node(callable_nodes, node, &name);
        let direct_module = python_direct_child_of(node, root)
            && node
                .parent()
                .is_some_and(|parent| parent.kind() != "decorated_definition");
        let owner_node = python_direct_enclosing_class(node);
        let owner = owner_node.and_then(|owner_node| {
            let owner_name = declaration_name(owner_node, source)?.to_string();
            let owner = python_unique_graph_node(class_nodes, owner_node, &owner_name)?;
            Some((owner, owner_name))
        });
        let direct_method = owner_node
            .is_some_and(|owner| python_direct_function_in_class(node, owner))
            && node
                .parent()
                .is_some_and(|parent| parent.kind() != "decorated_definition");
        if let Some((owner, _)) = &owner
            && (!direct_method || graph_id.is_none())
        {
            self.method_blockers_by_owner_and_name
                .insert((*owner, name.clone()));
        }
        if let Some((owner, _)) = &owner
            && matches!(
                name.as_str(),
                "__getattribute__" | "__getattr__" | "__setattr__" | "__delattr__"
            )
        {
            self.dynamic_class_owners.insert(*owner);
        }
        self.functions.insert(
            node.id(),
            PythonFunctionInfo {
                graph_id,
                direct_method,
                owner,
                plain_self: direct_method && python_plain_self_parameter(node, source),
            },
        );
        if direct_module {
            if let Some(declaration) = graph_id {
                self.declarations.push(CachedTopLevelDeclaration {
                    name: name.clone(),
                    declaration,
                    module_path: Vec::new(),
                    cross_module_visible: true,
                });
            } else {
                *self.module_blockers.entry(name.clone()).or_default() += 1;
            }
        } else if python_enclosing_function(node).is_none() && owner_node.is_none() {
            *self.module_blockers.entry(name.clone()).or_default() += 1;
        }
        if let Some(enclosing) = python_enclosing_function(node)
            && enclosing.id() != node.id()
        {
            self.push_binding(enclosing, name, node.start_byte(), PythonNameBinding::Other);
        }
    }

    fn collect_class(
        &mut self,
        node: TsNode<'tree>,
        root: TsNode<'tree>,
        source: &str,
        class_nodes: &HashMap<(u32, String), Vec<NodeId>>,
        callable_nodes: &HashMap<(u32, String), Vec<NodeId>>,
    ) {
        let Some(name) = declaration_name(node, source).map(str::to_string) else {
            return;
        };
        let direct_module = python_direct_child_of(node, root)
            && node
                .parent()
                .is_some_and(|parent| parent.kind() != "decorated_definition");
        let simple_base = python_single_simple_base(node, source);
        let header_supported = simple_base || node.child_by_field_name("superclasses").is_none();
        let closed = direct_module
            && node.child_by_field_name("type_parameters").is_none()
            && header_supported;
        if closed && let Some(owner) = python_unique_graph_node(class_nodes, node, &name) {
            self.class_owner_by_syntax_id.insert(node.id(), owner);
            if simple_base {
                self.simple_base_class_owners.insert(owner);
            }
            let mut methods = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cursor = body.walk();
                for method in body.named_children(&mut cursor) {
                    count_python_resolution_work(1);
                    if method.kind() != "function_definition"
                        || method
                            .parent()
                            .is_some_and(|parent| parent.kind() == "decorated_definition")
                    {
                        continue;
                    }
                    let Some(method_name) = declaration_name(method, source) else {
                        continue;
                    };
                    if let Some(declaration) =
                        python_unique_graph_node(callable_nodes, method, method_name)
                    {
                        methods.push(CachedClassMethod {
                            name: method_name.to_string(),
                            declaration,
                        });
                    }
                }
            }
            let class = CachedClassDeclaration {
                name: name.clone(),
                declaration: owner,
                methods,
            };
            self.classes_by_name
                .entry(name.clone())
                .or_default()
                .push(class.clone());
            self.closed_class_owners.insert(owner);
            self.classes.push(class);
        } else if python_enclosing_function(node).is_none() {
            *self.module_blockers.entry(name.clone()).or_default() += 1;
        }
        if let Some(function) = python_enclosing_function(node) {
            self.push_binding(function, name, node.start_byte(), PythonNameBinding::Other);
        }
    }

    fn collect_import(
        &mut self,
        node: TsNode<'tree>,
        root: TsNode<'tree>,
        source: &str,
        import_nodes: &HashMap<(u32, u32, String), Vec<NodeId>>,
    ) {
        let direct = python_direct_child_of(node, root);
        let module = node
            .child_by_field_name("module_name")
            .and_then(|module| node_text(module, source))
            .map(str::trim)
            .unwrap_or_default();
        let mut cursor = node.walk();
        let names = node
            .children_by_field_name("name", &mut cursor)
            .collect::<Vec<_>>();
        if !direct || !python_exact_relative_module(module) || names.len() != 1 {
            if direct
                && (names.iter().any(|name| name.kind() == "wildcard_import")
                    || node_text(node, source).is_some_and(|surface| surface.contains('*')))
            {
                self.module_dynamic = true;
            }
            let binding_names = python_binding_names(node, source);
            if let Some(owner) = self.enclosing_class_owner(node) {
                self.poison_class_members(owner, binding_names);
            } else {
                for name in binding_names {
                    *self.module_blockers.entry(name).or_default() += 1;
                }
            }
            return;
        }
        let imported = names[0];
        let (imported_name_node, local_node) = if imported.kind() == "aliased_import" {
            let Some(name) = imported.child_by_field_name("name") else {
                return;
            };
            let Some(alias) = imported.child_by_field_name("alias") else {
                return;
            };
            (name, alias)
        } else {
            (imported, imported)
        };
        let Some(imported_name) = node_text(imported_name_node, source)
            .map(str::trim)
            .filter(|name| python_identifier(name))
        else {
            return;
        };
        let Some(local_name) = node_text(local_node, source)
            .map(str::trim)
            .filter(|name| python_identifier(name))
        else {
            return;
        };
        let key = (
            local_node.start_position().row as u32 + 1,
            local_node.start_position().column as u32 + 1,
            local_name.to_string(),
        );
        let import = import_nodes
            .get(&key)
            .and_then(|nodes| match nodes.as_slice() {
                [node] => Some(*node),
                _ => None,
            });
        self.imports
            .entry(local_name.to_string())
            .or_default()
            .push(PythonImportBinding {
                import,
                module_specifier: module.to_string(),
                imported_name: imported_name.to_string(),
            });
    }

    fn collect_call(&mut self, node: TsNode<'tree>, source: &str) {
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        let (callee, form, raw_target) = match function.kind() {
            "identifier" => (
                function,
                CalleeForm::Identifier,
                node_text(function, source).unwrap_or_default().to_string(),
            ),
            "attribute" => {
                let Some(attribute) = function.child_by_field_name("attribute") else {
                    return;
                };
                let Some(object) = function.child_by_field_name("object") else {
                    return;
                };
                let form = if node_text(object, source).is_some_and(|value| value == "self") {
                    CalleeForm::ImplicitReceiver
                } else if object.kind() == "identifier" {
                    CalleeForm::ExplicitReceiver
                } else {
                    CalleeForm::DynamicAccess
                };
                (
                    attribute,
                    form,
                    node_text(attribute, source).unwrap_or_default().to_string(),
                )
            }
            _ => (
                function,
                CalleeForm::DynamicAccess,
                node_text(function, source)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            ),
        };
        self.calls.push(IndexedPythonCall {
            callee,
            form,
            raw_target,
        });
        if python_direct_call_name(node, source)
            .is_some_and(|name| matches!(name, "getattr" | "setattr" | "delattr"))
            && let Some(function) = python_enclosing_function(node)
        {
            self.dynamic_functions.insert(function.id());
        }
    }

    fn collect_assignment(&mut self, node: TsNode<'tree>, source: &str) {
        let Some(function) = python_enclosing_function(node) else {
            if let Some(left) = node.child_by_field_name("left") {
                let names = python_binding_names(left, source);
                if let Some(owner) = self.enclosing_class_owner(node) {
                    self.poison_class_members(owner, names);
                } else {
                    for name in names {
                        *self.module_blockers.entry(name).or_default() += 1;
                    }
                }
            }
            return;
        };
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        let names = python_binding_names(left, source);
        self.writes_by_function
            .entry(function.id())
            .or_default()
            .extend(names.iter().cloned());
        if let Some((owner, _)) = self
            .functions
            .get(&function.id())
            .and_then(|info| info.owner.as_ref())
            .cloned()
        {
            self.poison_class_members(owner, python_self_member_binding_names(left, source));
        }
        let direct_block = python_direct_statement_in_function(node, function);
        let receiver = if names.len() == 1 && left.kind() == "identifier" && direct_block {
            let class_name = match node.child_by_field_name("right") {
                Some(right) => {
                    python_direct_constructor_name(right, source).map(|name| (name, true))
                }
                None => node
                    .child_by_field_name("type")
                    .and_then(|annotation| python_exact_annotation(annotation, source))
                    .map(|name| (name, false)),
            };
            class_name.and_then(|(class_name, constructor)| {
                self.resolve_class_binding(&class_name)
                    .map(|class_binding| PythonNameBinding::Receiver {
                        class_binding,
                        constructor,
                        type_name: class_name,
                    })
            })
        } else {
            None
        };
        for name in names {
            self.push_binding(
                function,
                name,
                node.start_byte(),
                receiver.clone().unwrap_or(PythonNameBinding::Other),
            );
        }
    }

    fn collect_parameter_bindings(&mut self, node: TsNode<'tree>, source: &str) {
        let Some(function) = node
            .parent()
            .filter(|parent| parent.kind() == "function_definition")
        else {
            return;
        };
        let plain_self = self
            .functions
            .get(&function.id())
            .is_some_and(|info| info.plain_self);
        for name in python_binding_names(node, source) {
            if plain_self && name == "self" {
                continue;
            }
            self.push_binding(function, name, node.start_byte(), PythonNameBinding::Other);
        }
    }

    fn collect_module_class_member_mutations(&mut self, root: TsNode<'tree>, source: &str) {
        walk_nodes(root, &mut |node| {
            count_python_resolution_work(1);
            if !matches!(
                node.kind(),
                "assignment" | "augmented_assignment" | "delete_statement"
            ) {
                return;
            }
            let direct_module = node.parent().is_some_and(|parent| parent.id() == root.id())
                || node.parent().is_some_and(|parent| {
                    parent.kind() == "expression_statement"
                        && parent
                            .parent()
                            .is_some_and(|grandparent| grandparent.id() == root.id())
                });
            if !direct_module {
                return;
            }

            let mut targets = Vec::new();
            if node.kind() == "delete_statement" {
                let mut cursor = node.walk();
                targets.extend(node.named_children(&mut cursor));
            } else if let Some(left) = node.child_by_field_name("left") {
                targets.push(left);
            }
            for target in targets {
                if target.kind() != "attribute" {
                    continue;
                }
                let Some(class_name) = target
                    .child_by_field_name("object")
                    .filter(|object| object.kind() == "identifier")
                    .and_then(|object| node_text(object, source))
                    .filter(|name| python_identifier(name))
                else {
                    continue;
                };
                let Some(member) = target
                    .child_by_field_name("attribute")
                    .filter(|attribute| attribute.kind() == "identifier")
                    .and_then(|attribute| node_text(attribute, source))
                    .filter(|name| python_identifier(name))
                else {
                    continue;
                };
                let owner = match self
                    .classes_by_name
                    .get(class_name)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    [class] => class.declaration,
                    _ => continue,
                };
                self.method_blockers_by_owner_and_name
                    .insert((owner, member.to_string()));
            }
        });
    }

    fn collect_local_binding_node(&mut self, node: TsNode<'tree>, source: &str) {
        let names = python_binding_names(node, source);
        if let Some(function) = python_enclosing_function(node) {
            if node.kind() == "global_statement" {
                self.global_names_by_function
                    .entry(function.id())
                    .or_default()
                    .extend(names.iter().cloned());
            } else {
                self.writes_by_function
                    .entry(function.id())
                    .or_default()
                    .extend(names.iter().cloned());
            }
            if let Some((owner, _)) = self
                .functions
                .get(&function.id())
                .and_then(|info| info.owner.as_ref())
                .cloned()
            {
                self.poison_class_members(owner, python_self_member_binding_names(node, source));
            }
            for name in names {
                self.push_binding(function, name, node.start_byte(), PythonNameBinding::Other);
            }
        } else if let Some(owner) = self.enclosing_class_owner(node) {
            self.poison_class_members(owner, names);
        } else {
            for name in names {
                *self.module_blockers.entry(name).or_default() += 1;
            }
        }
    }

    fn enclosing_class_owner(&self, node: TsNode<'tree>) -> Option<NodeId> {
        python_direct_enclosing_class(node)
            .and_then(|class| self.class_owner_by_syntax_id.get(&class.id()).copied())
    }

    fn poison_class_members(&mut self, owner: NodeId, names: impl IntoIterator<Item = String>) {
        self.method_blockers_by_owner_and_name
            .extend(names.into_iter().map(|name| (owner, name)));
    }

    fn push_binding(
        &mut self,
        function: TsNode<'tree>,
        name: String,
        at: usize,
        binding: PythonNameBinding,
    ) {
        self.bindings
            .entry(function.id())
            .or_default()
            .entry(name)
            .or_default()
            .push(PythonBindingEvent { at, binding });
    }

    fn resolve_class_binding(&self, name: &str) -> Option<CachedClassBinding> {
        if self.module_dynamic || self.module_blockers.contains_key(name) {
            return None;
        }
        let classes = self
            .classes_by_name
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let [class] = classes {
            return Some(CachedClassBinding::SameFile {
                owner: class.declaration,
                owner_name: class.name.clone(),
            });
        }
        let imports = self.imports.get(name)?;
        let [import] = imports.as_slice() else {
            return None;
        };
        Some(CachedClassBinding::StaticImport {
            import: import.import?,
            module_specifier: import.module_specifier.clone(),
            imported_name: import.imported_name.clone(),
            is_default: false,
        })
    }

    fn resolve_syntax_claim(
        &self,
        source: &str,
        callee: TsNode<'tree>,
        form: CalleeForm,
        raw_target: &str,
    ) -> (Option<NodeId>, CachedResolutionBinding) {
        let Some(call) = callee.parent().and_then(|parent| {
            if parent.kind() == "call" {
                Some(parent)
            } else {
                parent
                    .parent()
                    .filter(|grandparent| grandparent.kind() == "call")
            }
        }) else {
            return (None, CachedResolutionBinding::Unsupported);
        };
        let Some(function) = python_enclosing_function(call) else {
            return (None, CachedResolutionBinding::Unsupported);
        };
        let Some(info) = self.functions.get(&function.id()) else {
            return (None, CachedResolutionBinding::Unsupported);
        };
        let Some(caller) = info.graph_id else {
            return (None, CachedResolutionBinding::IncompleteDomain);
        };
        if python_enclosing_function(function).is_some() {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        if python_has_enclosing_lambda(call, function) {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        if self.dynamic_functions.contains(&function.id()) {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        let binding = match form {
            CalleeForm::Identifier => {
                if self.module_dynamic
                    || self
                        .bindings
                        .get(&function.id())
                        .and_then(|bindings| bindings.get(raw_target))
                        .is_some()
                    || self
                        .module_blockers
                        .get(raw_target)
                        .copied()
                        .unwrap_or_default()
                        > 0
                {
                    CachedResolutionBinding::Unsupported
                } else if let Some(imports) = self.imports.get(raw_target) {
                    match imports.as_slice() {
                        [import] if import.import.is_some() => {
                            CachedResolutionBinding::StaticImport {
                                import: import.import.expect("checked"),
                                module_specifier: import.module_specifier.clone(),
                                imported_name: import.imported_name.clone(),
                                is_default: false,
                            }
                        }
                        [_] => CachedResolutionBinding::IncompleteDomain,
                        _ => CachedResolutionBinding::Ambiguous,
                    }
                } else {
                    let declarations = self
                        .declarations_by_name
                        .get(raw_target)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    if let [declaration] = declarations {
                        CachedResolutionBinding::SameFile {
                            declaration: *declaration,
                        }
                    } else if declarations.len() > 1 {
                        CachedResolutionBinding::Ambiguous
                    } else if self.classes_by_name.contains_key(raw_target) {
                        CachedResolutionBinding::Unsupported
                    } else {
                        CachedResolutionBinding::MissingBinding
                    }
                }
            }
            CalleeForm::ImplicitReceiver => {
                let owner = info.owner.as_ref();
                let valid = info.direct_method
                    && info.plain_self
                    && !self
                        .bindings
                        .get(&function.id())
                        .and_then(|bindings| bindings.get("self"))
                        .is_some_and(|events| {
                            events.iter().any(|event| event.at > function.start_byte())
                        });
                if !valid {
                    CachedResolutionBinding::Unsupported
                } else if let Some((owner, owner_name)) = owner {
                    if !self.closed_class_owners.contains(owner)
                        || self.dynamic_class_owners.contains(owner)
                        || self
                            .method_blockers_by_owner_and_name
                            .contains(&(*owner, raw_target.to_owned()))
                    {
                        return (Some(caller), CachedResolutionBinding::Unsupported);
                    }
                    let methods = self
                        .methods_by_owner_and_name
                        .get(&(*owner, raw_target.to_string()))
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    if let [method] = methods {
                        CachedResolutionBinding::ImplicitReceiver {
                            owner: *owner,
                            declaration: *method,
                            owner_name: owner_name.clone(),
                        }
                    } else if methods.len() > 1 {
                        CachedResolutionBinding::Ambiguous
                    } else if self.simple_base_class_owners.contains(owner) {
                        CachedResolutionBinding::Unsupported
                    } else {
                        CachedResolutionBinding::MissingBinding
                    }
                } else {
                    CachedResolutionBinding::Unsupported
                }
            }
            CalleeForm::ExplicitReceiver => {
                if !python_direct_statement_in_function(call, function) {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                }
                let receiver = callee
                    .parent()
                    .and_then(|attribute| attribute.child_by_field_name("object"))
                    .and_then(|receiver| (receiver.kind() == "identifier").then_some(receiver))
                    .and_then(|receiver| node_text(receiver, source));
                let events =
                    receiver.and_then(|receiver| self.bindings.get(&function.id())?.get(receiver));
                match events.map(Vec::as_slice) {
                    Some(
                        [
                            PythonBindingEvent {
                                at,
                                binding:
                                    PythonNameBinding::Receiver {
                                        class_binding,
                                        constructor,
                                        type_name,
                                    },
                            },
                        ],
                    ) if *at < call.start_byte()
                        && !self
                            .bindings
                            .get(&function.id())
                            .is_some_and(|bindings| bindings.contains_key(type_name)) =>
                    {
                        if let CachedClassBinding::SameFile { owner, .. } = class_binding
                            && (self.dynamic_class_owners.contains(owner)
                                || self
                                    .method_blockers_by_owner_and_name
                                    .contains(&(*owner, raw_target.to_owned())))
                        {
                            return (Some(caller), CachedResolutionBinding::Unsupported);
                        }
                        if *constructor {
                            CachedResolutionBinding::ConstructorBinding {
                                class_binding: class_binding.clone(),
                                method_name: raw_target.to_string(),
                            }
                        } else {
                            CachedResolutionBinding::ExplicitReceiverType {
                                class_binding: class_binding.clone(),
                                method_name: raw_target.to_string(),
                            }
                        }
                    }
                    Some(_) => CachedResolutionBinding::Unsupported,
                    None => CachedResolutionBinding::Unsupported,
                }
            }
            _ => CachedResolutionBinding::Unsupported,
        };
        (Some(caller), binding)
    }
}

fn python_unique_graph_node(
    nodes: &HashMap<(u32, String), Vec<NodeId>>,
    syntax: TsNode<'_>,
    name: &str,
) -> Option<NodeId> {
    nodes
        .get(&(syntax.start_position().row as u32 + 1, name.to_string()))
        .and_then(|matches| match matches.as_slice() {
            [node] => Some(*node),
            _ => None,
        })
}

fn python_direct_child_of(node: TsNode<'_>, owner: TsNode<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.id() == owner.id())
        || node.parent().is_some_and(|parent| {
            parent.kind() == "decorated_definition"
                && parent
                    .parent()
                    .is_some_and(|grandparent| grandparent.id() == owner.id())
        })
}

fn python_direct_function_in_class(function: TsNode<'_>, class: TsNode<'_>) -> bool {
    class.child_by_field_name("body").is_some_and(|body| {
        function
            .parent()
            .is_some_and(|parent| parent.id() == body.id())
            || function.parent().is_some_and(|parent| {
                parent.kind() == "decorated_definition"
                    && parent
                        .parent()
                        .is_some_and(|grandparent| grandparent.id() == body.id())
            })
    })
}

fn python_single_simple_base(class: TsNode<'_>, source: &str) -> bool {
    let Some(superclasses) = class.child_by_field_name("superclasses") else {
        return false;
    };
    let mut cursor = superclasses.walk();
    let bases = superclasses.named_children(&mut cursor).collect::<Vec<_>>();
    let [base] = bases.as_slice() else {
        return false;
    };
    let Some(base_name) = (base.kind() == "identifier")
        .then(|| node_text(*base, source))
        .flatten()
        .filter(|name| python_identifier(name))
    else {
        return false;
    };
    node_text(superclasses, source)
        .and_then(|surface| surface.strip_prefix('('))
        .and_then(|surface| surface.strip_suffix(')'))
        .is_some_and(|surface| surface.trim() == base_name)
}

fn python_direct_enclosing_class(mut node: TsNode<'_>) -> Option<TsNode<'_>> {
    while let Some(parent) = node.parent() {
        if parent.kind() == "function_definition" {
            return None;
        }
        if parent.kind() == "class_definition" {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn python_enclosing_function(mut node: TsNode<'_>) -> Option<TsNode<'_>> {
    while let Some(parent) = node.parent() {
        if parent.kind() == "function_definition" {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn python_has_enclosing_lambda(mut node: TsNode<'_>, function: TsNode<'_>) -> bool {
    while let Some(parent) = node.parent() {
        if parent.id() == function.id() {
            return false;
        }
        if parent.kind() == "lambda" {
            return true;
        }
        node = parent;
    }
    false
}

fn python_plain_self_parameter(function: TsNode<'_>, source: &str) -> bool {
    let Some(parameters) = function.child_by_field_name("parameters") else {
        return false;
    };
    let mut cursor = parameters.walk();
    let params = parameters.named_children(&mut cursor).collect::<Vec<_>>();
    params.first().is_some_and(|parameter| {
        parameter.kind() == "identifier" && node_text(*parameter, source) == Some("self")
    })
}

fn python_direct_statement_in_function(node: TsNode<'_>, function: TsNode<'_>) -> bool {
    function.child_by_field_name("body").is_some_and(|body| {
        node.parent().is_some_and(|parent| parent.id() == body.id())
            || node.parent().is_some_and(|parent| {
                parent.kind() == "expression_statement"
                    && parent
                        .parent()
                        .is_some_and(|grandparent| grandparent.id() == body.id())
            })
    })
}

fn python_direct_constructor_name(node: TsNode<'_>, source: &str) -> Option<String> {
    if node.kind() != "call" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    let name = (function.kind() == "identifier").then(|| node_text(function, source))??;
    python_identifier(name).then(|| name.to_string())
}

fn python_exact_annotation(node: TsNode<'_>, source: &str) -> Option<String> {
    let surface = node_text(node, source)?.trim();
    python_identifier(surface).then(|| surface.to_string())
}

fn python_direct_call_name<'a>(node: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    let function = node.child_by_field_name("function")?;
    (function.kind() == "identifier").then(|| node_text(function, source))?
}

fn python_exact_relative_module(module: &str) -> bool {
    module.starts_with('.')
        && !module.starts_with("..")
        && module[1..].split('.').all(python_identifier)
        && module.len() > 1
}

fn python_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn python_binding_names(node: TsNode<'_>, source: &str) -> Vec<String> {
    let mut names = Vec::new();
    match node.kind() {
        "assignment" | "augmented_assignment" => {
            if let Some(left) = node.child_by_field_name("left") {
                python_binding_target_names(left, source, &mut names);
            }
        }
        "for_statement"
        | "list_comprehension"
        | "set_comprehension"
        | "dictionary_comprehension"
        | "generator_expression" => {
            if let Some(left) = node.child_by_field_name("left") {
                python_binding_target_names(left, source, &mut names);
            }
        }
        "named_expression" => {
            if let Some(name) = node.child_by_field_name("name") {
                python_binding_target_names(name, source, &mut names);
            }
        }
        "with_item" => {
            if let Some(alias) = node.child_by_field_name("alias") {
                python_binding_target_names(alias, source, &mut names);
            }
        }
        "except_clause" => {
            if let Some(name) = node.child_by_field_name("name") {
                python_binding_target_names(name, source, &mut names);
            }
        }
        "parameters"
        | "global_statement"
        | "nonlocal_statement"
        | "delete_statement"
        | "case_pattern"
        | "import_statement"
        | "import_from_statement" => {
            python_binding_target_names(node, source, &mut names);
        }
        _ => python_binding_target_names(node, source, &mut names),
    }
    names.sort();
    names.dedup();
    names
}

fn python_binding_target_names(node: TsNode<'_>, source: &str, names: &mut Vec<String>) {
    count_python_resolution_work(1);
    if node.kind() == "identifier" {
        if let Some(name) = node_text(node, source).filter(|name| python_identifier(name)) {
            names.push(name.to_string());
        }
        return;
    }
    if node.kind() == "attribute" {
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        python_binding_target_names(child, source, names);
    }
}

fn python_self_member_binding_names(node: TsNode<'_>, source: &str) -> Vec<String> {
    let mut names = Vec::new();
    python_collect_self_member_binding_names(node, source, &mut names);
    names.sort();
    names.dedup();
    names
}

fn python_collect_self_member_binding_names(
    node: TsNode<'_>,
    source: &str,
    names: &mut Vec<String>,
) {
    count_python_resolution_work(1);
    if node.kind() == "attribute"
        && node
            .child_by_field_name("object")
            .and_then(|object| node_text(object, source))
            == Some("self")
    {
        if let Some(name) = node
            .child_by_field_name("attribute")
            .and_then(|attribute| node_text(attribute, source))
            .filter(|name| python_identifier(name))
        {
            names.push(name.to_string());
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        python_collect_self_member_binding_names(child, source, names);
    }
}

#[derive(Debug, Clone)]
struct IndexedJavascriptCall<'tree> {
    callee: TsNode<'tree>,
    form: CalleeForm,
    raw_target: String,
}

#[derive(Debug, Clone)]
enum JavascriptBindingKind {
    SameFile {
        declaration: NodeId,
    },
    Class {
        owner: NodeId,
    },
    StaticImport {
        import: NodeId,
        module_specifier: String,
        imported_name: String,
        is_default: bool,
    },
    Other,
}

#[derive(Debug, Clone)]
struct JavascriptBinding {
    name: String,
    scope_start: usize,
    scope_end: usize,
    scope_depth: usize,
    kind: JavascriptBindingKind,
}

#[derive(Debug, Clone)]
struct JavascriptWrite {
    name: String,
    scope_start: usize,
    scope_end: usize,
    scope_depth: usize,
}

#[derive(Debug, Clone, Copy)]
enum JavascriptReceiverKind {
    Constructor,
    ExplicitType,
}

#[derive(Debug, Clone)]
struct JavascriptReceiverBinding {
    class_name: String,
    scope_start: usize,
    scope_end: usize,
    scope_depth: usize,
    kind: JavascriptReceiverKind,
}

struct JavascriptResolutionIndex<'tree> {
    calls: Vec<IndexedJavascriptCall<'tree>>,
    bindings: HashMap<String, Vec<JavascriptBinding>>,
    writes: HashMap<String, Vec<JavascriptWrite>>,
    top_level_declarations: Vec<CachedTopLevelDeclaration>,
    classes: Vec<CachedClassDeclaration>,
    receiver_bindings: HashMap<String, Vec<JavascriptReceiverBinding>>,
    callable_nodes: HashMap<(u32, String), Vec<NodeId>>,
    class_nodes: HashMap<(u32, String), Vec<NodeId>>,
    import_nodes: HashMap<(u32, u32, String), Vec<NodeId>>,
    mutated_members: HashMap<(String, String), Vec<(usize, usize)>>,
    dynamically_mutated_owners: HashMap<String, Vec<(usize, usize)>>,
    dynamic_breaker_scopes: HashSet<(usize, usize)>,
    module_dynamic_breaker: bool,
    export_statements: Vec<TsNode<'tree>>,
    ecmascript_module: bool,
}

impl<'tree> JavascriptResolutionIndex<'tree> {
    fn build(tree: &'tree Tree, source: &str, file_id: NodeId, nodes: &[Node]) -> Self {
        let root = tree.root_node();
        let mut callable_nodes = HashMap::<(u32, String), Vec<NodeId>>::new();
        let mut class_nodes = HashMap::<(u32, String), Vec<NodeId>>::new();
        let mut import_nodes = HashMap::<(u32, u32, String), Vec<NodeId>>::new();
        for node in nodes
            .iter()
            .filter(|node| node.file_node_id == Some(file_id))
        {
            if let Some(line) = node.start_line {
                let name = graph_leaf_name(&node.serialized_name).to_string();
                if matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD) {
                    callable_nodes
                        .entry((line, name.clone()))
                        .or_default()
                        .push(node.id);
                } else if node.kind == NodeKind::CLASS {
                    class_nodes
                        .entry((line, name.clone()))
                        .or_default()
                        .push(node.id);
                }
                if let Some(column) = node.start_col {
                    import_nodes
                        .entry((line, column, node.serialized_name.clone()))
                        .or_default()
                        .push(node.id);
                }
            }
        }
        let mut result = Self {
            calls: Vec::new(),
            bindings: HashMap::new(),
            writes: HashMap::new(),
            top_level_declarations: Vec::new(),
            classes: Vec::new(),
            receiver_bindings: HashMap::new(),
            callable_nodes,
            class_nodes,
            import_nodes,
            mutated_members: HashMap::new(),
            dynamically_mutated_owners: HashMap::new(),
            dynamic_breaker_scopes: HashSet::new(),
            module_dynamic_breaker: false,
            export_statements: Vec::new(),
            ecmascript_module: typescript_file_is_module(root),
        };
        walk_nodes(root, &mut |node| {
            if node.kind() == "export_statement" {
                result.export_statements.push(node);
            }
            if node.kind() == "call_expression"
                && let Some(function) = node.child_by_field_name("function")
                && let Some((callee, mut form, raw_target)) = classify_callee(function, source)
            {
                if node.child_by_field_name("arguments").is_none()
                    || source.as_bytes().get(function.end_byte()) == Some(&b'`')
                    || has_direct_named_child_kind(node, "optional_chain")
                    || has_direct_named_child_kind(function, "optional_chain")
                {
                    form = CalleeForm::DynamicAccess;
                }
                result.calls.push(IndexedJavascriptCall {
                    callee,
                    form,
                    raw_target,
                });
            }
            result.collect_dynamic_breaker(node, root, source);
            result.collect_reflection_mutation(node, root, source);
            result.collect_binding(node, root, source);
            result.collect_write(node, root, source);
        });
        result
            .calls
            .sort_by_key(|call| (call.callee.start_byte(), call.callee.end_byte()));
        result.top_level_declarations.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.declaration.cmp(&right.declaration))
        });
        result
    }

    fn collect_binding(&mut self, node: TsNode<'tree>, root: TsNode<'tree>, source: &str) {
        if node.kind() == "import_alias" {
            let mut cursor = node.walk();
            if let Some(name) = node
                .named_children(&mut cursor)
                .find(|child| child.kind() == "identifier")
                .and_then(|name| node_text(name, source))
            {
                self.push_binding(name.to_string(), root, 0, JavascriptBindingKind::Other);
            }
            return;
        }
        if node.kind() == "import_statement" {
            let supported = typescript_import_bindings_for_statement(node, source);
            let mut supported_by_name = supported
                .unwrap_or_default()
                .into_iter()
                .map(|binding| (binding.local_name.clone(), binding))
                .collect::<HashMap<_, _>>();
            let mut names = Vec::new();
            collect_javascript_pattern_names(node, source, &mut names);
            names.sort();
            names.dedup();
            for name in names {
                let kind = if let Some(binding) = supported_by_name.remove(&name) {
                    let import_nodes = self.import_nodes.get(&(
                        binding.line,
                        binding.column,
                        binding.local_name.clone(),
                    ));
                    if let Some([import]) = import_nodes.map(Vec::as_slice) {
                        JavascriptBindingKind::StaticImport {
                            import: *import,
                            module_specifier: binding.module_specifier,
                            imported_name: binding.imported_name,
                            is_default: binding.is_default,
                        }
                    } else {
                        JavascriptBindingKind::Other
                    }
                } else {
                    JavascriptBindingKind::Other
                };
                self.push_binding(name, root, 0, kind);
            }
            return;
        }

        if matches!(
            node.kind(),
            "function_declaration"
                | "generator_function_declaration"
                | "function_signature"
                | "class_declaration"
                | "abstract_class_declaration"
                | "enum_declaration"
                | "interface_declaration"
                | "type_alias_declaration"
                | "internal_module"
                | "module"
        ) {
            let Some(name_node) = node.child_by_field_name("name") else {
                return;
            };
            let Some(name) = node_text(name_node, source).map(str::to_string) else {
                return;
            };
            let (scope, depth) = javascript_binding_scope(node, root, source);
            let direct_callable = node.kind() == "function_declaration"
                && javascript_declaration_is_direct_module(node)
                && !javascript_declaration_is_decorated(node)
                && self.map_callable_declaration(node, source).is_some();
            let direct_class = node.kind() == "class_declaration"
                && javascript_declaration_is_direct_module(node)
                && javascript_class_is_closed(node)
                && self.map_class_declaration(node, source).is_some();
            let kind = if direct_callable {
                let declaration = self
                    .map_callable_declaration(node, source)
                    .expect("direct callable mapping was checked");
                self.top_level_declarations.push(CachedTopLevelDeclaration {
                    name: name.clone(),
                    declaration,
                    module_path: Vec::new(),
                    cross_module_visible: false,
                });
                JavascriptBindingKind::SameFile { declaration }
            } else if direct_class {
                let owner = self
                    .map_class_declaration(node, source)
                    .expect("direct class mapping was checked");
                if let Some(methods) = javascript_class_methods(node, &self.callable_nodes, source)
                {
                    self.classes.push(CachedClassDeclaration {
                        name: name.clone(),
                        declaration: owner,
                        methods,
                    });
                }
                JavascriptBindingKind::Class { owner }
            } else {
                JavascriptBindingKind::Other
            };
            self.push_binding(name, scope, depth, kind);
            return;
        }

        if node.kind() == "variable_declarator" {
            let Some(pattern) = node.child_by_field_name("name") else {
                return;
            };
            let mut names = Vec::new();
            collect_javascript_pattern_names(pattern, source, &mut names);
            let (scope, depth) = javascript_binding_scope(node, root, source);
            let direct_arrow = javascript_const_arrow(node, source)
                .filter(|_arrow| javascript_declaration_is_direct_module(node) && names.len() == 1)
                .and_then(|arrow| {
                    self.map_callable_declaration(arrow, source)
                        .map(|declaration| (arrow, declaration))
                });
            for name in names {
                let kind = if let Some((_, declaration)) = direct_arrow {
                    self.top_level_declarations.push(CachedTopLevelDeclaration {
                        name: name.clone(),
                        declaration,
                        module_path: Vec::new(),
                        cross_module_visible: false,
                    });
                    JavascriptBindingKind::SameFile { declaration }
                } else {
                    JavascriptBindingKind::Other
                };
                self.push_binding(name, scope, depth, kind);
            }
            if pattern.kind() == "identifier"
                && let Some(receiver_name) = node_text(pattern, source)
            {
                if let Some(class_name) = javascript_direct_constructor_name(node, source) {
                    self.push_receiver_binding(
                        receiver_name.to_string(),
                        class_name,
                        scope,
                        depth,
                        JavascriptReceiverKind::Constructor,
                    );
                } else if let Some(class_name) = typescript_variable_annotation(node, source) {
                    self.push_receiver_binding(
                        receiver_name.to_string(),
                        class_name,
                        scope,
                        depth,
                        JavascriptReceiverKind::ExplicitType,
                    );
                }
            }
            return;
        }

        if node.kind() == "formal_parameters" {
            let Some(callable) = node.parent() else {
                return;
            };
            let mut names = Vec::new();
            collect_javascript_pattern_names(node, source, &mut names);
            let depth = javascript_scope_depth(callable);
            for name in names {
                self.push_binding(name, callable, depth, JavascriptBindingKind::Other);
            }
            collect_typescript_typed_parameters(node, source, &mut |name, class_name| {
                self.push_receiver_binding(
                    name,
                    class_name,
                    callable,
                    depth,
                    JavascriptReceiverKind::ExplicitType,
                );
            });
            return;
        }

        if node.kind() == "arrow_function"
            && let Some(parameter) = node.child_by_field_name("parameter")
        {
            let mut names = Vec::new();
            collect_javascript_pattern_names(parameter, source, &mut names);
            let depth = javascript_scope_depth(node);
            for name in names {
                self.push_binding(name, node, depth, JavascriptBindingKind::Other);
            }
        }

        if node.kind() == "catch_clause"
            && let Some(parameter) = node.child_by_field_name("parameter")
        {
            let mut names = Vec::new();
            collect_javascript_pattern_names(parameter, source, &mut names);
            let depth = javascript_scope_depth(node);
            for name in names {
                self.push_binding(name, node, depth, JavascriptBindingKind::Other);
            }
        }
    }

    fn collect_write(&mut self, node: TsNode<'tree>, root: TsNode<'tree>, source: &str) {
        let Some(target) = typescript_write_target(node) else {
            return;
        };
        let Some(target) = target else {
            let scope = javascript_governing_scope(node, root);
            self.dynamic_breaker_scopes
                .insert((scope.start_byte(), scope.end_byte()));
            self.module_dynamic_breaker |= scope.id() == root.id();
            return;
        };
        if matches!(target.kind(), "member_expression" | "subscript_expression") {
            self.collect_member_mutation(target, root, source);
            return;
        }
        let mut names = Vec::new();
        collect_javascript_pattern_names(target, source, &mut names);
        let (scope, depth) = javascript_binding_scope(node, root, source);
        for name in names {
            self.writes
                .entry(name.clone())
                .or_default()
                .push(JavascriptWrite {
                    name,
                    scope_start: scope.start_byte(),
                    scope_end: scope.end_byte(),
                    scope_depth: depth,
                });
        }
    }

    fn collect_member_mutation(
        &mut self,
        target: TsNode<'tree>,
        root: TsNode<'tree>,
        source: &str,
    ) {
        let Some(object) = target.child_by_field_name("object") else {
            return;
        };
        let Some(owner) = javascript_mutation_owner(object, source) else {
            return;
        };
        let property = target
            .child_by_field_name("property")
            .or_else(|| target.child_by_field_name("index"));
        let scope = javascript_governing_scope(target, root);
        let range = (scope.start_byte(), scope.end_byte());
        if target.kind() == "member_expression"
            && let Some(property) = property
            && matches!(property.kind(), "property_identifier" | "identifier")
            && let Some(member) = node_text(property, source)
        {
            if member == "prototype" {
                self.dynamically_mutated_owners
                    .entry(format!("{owner}.prototype"))
                    .or_default()
                    .push(range);
            } else if member == "__proto__" {
                self.dynamically_mutated_owners
                    .entry(owner)
                    .or_default()
                    .push(range);
            } else {
                self.mutated_members
                    .entry((owner, member.to_string()))
                    .or_default()
                    .push(range);
            }
        } else if let Some(member) =
            property.and_then(|property| simple_typescript_string(property, source))
        {
            self.mutated_members
                .entry((owner, member.to_string()))
                .or_default()
                .push(range);
        } else {
            self.dynamically_mutated_owners
                .entry(owner)
                .or_default()
                .push(range);
        }
    }

    fn collect_reflection_mutation(
        &mut self,
        node: TsNode<'tree>,
        root: TsNode<'tree>,
        source: &str,
    ) {
        if node.kind() != "call_expression" {
            return;
        }
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        let Some(function) = node_text(function, source) else {
            return;
        };
        let Some(arguments) = node.child_by_field_name("arguments") else {
            return;
        };
        let mut cursor = arguments.walk();
        let arguments = arguments.named_children(&mut cursor).collect::<Vec<_>>();
        let Some(owner) = arguments
            .first()
            .and_then(|target| javascript_mutation_owner(*target, source))
        else {
            return;
        };
        let scope = javascript_governing_scope(node, root);
        let range = (scope.start_byte(), scope.end_byte());
        match function {
            "Object.defineProperty" | "Reflect.defineProperty" => {
                if let Some(member) = arguments
                    .get(1)
                    .and_then(|property| simple_typescript_string(*property, source))
                {
                    if member == "prototype" {
                        self.dynamically_mutated_owners
                            .entry(format!("{owner}.prototype"))
                            .or_default()
                            .push(range);
                    } else if member == "__proto__" {
                        self.dynamically_mutated_owners
                            .entry(owner)
                            .or_default()
                            .push(range);
                    } else {
                        self.mutated_members
                            .entry((owner, member.to_string()))
                            .or_default()
                            .push(range);
                    }
                } else {
                    self.dynamically_mutated_owners
                        .entry(owner)
                        .or_default()
                        .push(range);
                }
            }
            "Object.assign" | "Object.setPrototypeOf" => {
                self.dynamically_mutated_owners
                    .entry(owner)
                    .or_default()
                    .push(range);
            }
            _ => {}
        }
    }

    fn collect_dynamic_breaker(&mut self, node: TsNode<'tree>, root: TsNode<'tree>, source: &str) {
        let is_eval = node.kind() == "call_expression"
            && node
                .child_by_field_name("function")
                .and_then(|function| node_text(function, source))
                == Some("eval");
        if node.kind() != "with_statement" && !is_eval {
            return;
        }
        let scope = javascript_governing_scope(node, root);
        self.dynamic_breaker_scopes
            .insert((scope.start_byte(), scope.end_byte()));
        self.module_dynamic_breaker |= scope.id() == root.id();
    }

    fn push_binding(
        &mut self,
        name: String,
        scope: TsNode<'tree>,
        scope_depth: usize,
        kind: JavascriptBindingKind,
    ) {
        self.bindings
            .entry(name.clone())
            .or_default()
            .push(JavascriptBinding {
                name,
                scope_start: scope.start_byte(),
                scope_end: scope.end_byte(),
                scope_depth,
                kind,
            });
    }

    fn push_receiver_binding(
        &mut self,
        name: String,
        class_name: String,
        scope: TsNode<'tree>,
        scope_depth: usize,
        kind: JavascriptReceiverKind,
    ) {
        self.receiver_bindings
            .entry(name)
            .or_default()
            .push(JavascriptReceiverBinding {
                class_name,
                scope_start: scope.start_byte(),
                scope_end: scope.end_byte(),
                scope_depth,
                kind,
            });
    }

    fn cached_top_level_declarations(&self) -> Vec<CachedTopLevelDeclaration> {
        self.top_level_declarations.clone()
    }

    fn cached_classes(&self) -> Vec<CachedClassDeclaration> {
        self.classes.clone()
    }

    fn map_callable_declaration(&self, declaration: TsNode<'_>, source: &str) -> Option<NodeId> {
        let name = if declaration.kind() == "arrow_function" {
            crate::js_like_callable_source_name(declaration, source)?
        } else {
            declaration_name(declaration, source)?.to_string()
        };
        let line = declaration.start_position().row as u32 + 1;
        let matches = self.callable_nodes.get(&(line, name))?;
        matches.first().copied().filter(|_| matches.len() == 1)
    }

    fn map_class_declaration(&self, declaration: TsNode<'_>, source: &str) -> Option<NodeId> {
        let name = declaration_name(declaration, source)?.to_string();
        let line = declaration.start_position().row as u32 + 1;
        let matches = self.class_nodes.get(&(line, name))?;
        matches.first().copied().filter(|_| matches.len() == 1)
    }

    fn resolve_syntax_claim(
        &self,
        source: &str,
        callee: TsNode<'tree>,
        form: CalleeForm,
        raw_target: &str,
    ) -> (Option<NodeId>, CachedResolutionBinding) {
        let Some(callable) = javascript_enclosing_callable(callee) else {
            return (None, CachedResolutionBinding::MissingBinding);
        };
        let Some(caller) =
            javascript_supported_caller(&self.callable_nodes, callable, callee, source)
        else {
            return (None, CachedResolutionBinding::Unsupported);
        };
        if !self.ecmascript_module {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        if javascript_ancestor_range_is_indexed(callee, &self.dynamic_breaker_scopes) {
            return (Some(caller), CachedResolutionBinding::IncompleteDomain);
        }
        if form == CalleeForm::ImplicitReceiver {
            let Some(class) = javascript_enclosing_class(callable).and_then(|owner| {
                let owner_id = self.map_class_declaration(owner, source)?;
                self.classes
                    .iter()
                    .find(|class| class.declaration == owner_id)
            }) else {
                return (Some(caller), CachedResolutionBinding::Unsupported);
            };
            let methods = class
                .methods
                .iter()
                .filter(|method| method.name == raw_target)
                .collect::<Vec<_>>();
            if self.class_member_domain_is_mutated(
                &class.name,
                "this",
                raw_target,
                callee.start_byte(),
            ) {
                return (Some(caller), CachedResolutionBinding::IncompleteDomain);
            }
            return match methods.as_slice() {
                [method] => (
                    Some(caller),
                    CachedResolutionBinding::ImplicitReceiver {
                        owner: class.declaration,
                        declaration: method.declaration,
                        owner_name: class.name.clone(),
                    },
                ),
                [] => (Some(caller), CachedResolutionBinding::MissingBinding),
                _ => (Some(caller), CachedResolutionBinding::Ambiguous),
            };
        }
        if form == CalleeForm::ExplicitReceiver {
            let Some(receiver_name) = javascript_member_receiver(callee, source) else {
                return (Some(caller), CachedResolutionBinding::Unsupported);
            };
            let mut receiver_bindings = self
                .receiver_bindings
                .get(&receiver_name)
                .into_iter()
                .flatten()
                .filter(|binding| {
                    binding.scope_start <= callee.start_byte()
                        && callee.start_byte() < binding.scope_end
                })
                .collect::<Vec<_>>();
            let Some(depth) = receiver_bindings
                .iter()
                .map(|binding| binding.scope_depth)
                .max()
            else {
                return (Some(caller), CachedResolutionBinding::Unsupported);
            };
            receiver_bindings.retain(|binding| binding.scope_depth == depth);
            let mut lexical_bindings = self
                .bindings
                .get(&receiver_name)
                .into_iter()
                .flatten()
                .filter(|binding| {
                    binding.scope_start <= callee.start_byte()
                        && callee.start_byte() < binding.scope_end
                })
                .collect::<Vec<_>>();
            let lexical_depth = lexical_bindings
                .iter()
                .map(|binding| binding.scope_depth)
                .max();
            if let Some(lexical_depth) = lexical_depth {
                lexical_bindings.retain(|binding| binding.scope_depth == lexical_depth);
            }
            if receiver_bindings.len() != 1
                || lexical_depth != Some(depth)
                || lexical_bindings.len() != 1
                || self.writes.get(&receiver_name).is_some_and(|writes| {
                    writes.iter().any(|write| {
                        write.scope_start <= callee.start_byte()
                            && callee.start_byte() < write.scope_end
                    })
                })
            {
                return (Some(caller), CachedResolutionBinding::Ambiguous);
            }
            let receiver = receiver_bindings[0];
            if self.class_member_domain_is_mutated(
                &receiver.class_name,
                &receiver_name,
                raw_target,
                callee.start_byte(),
            ) {
                return (Some(caller), CachedResolutionBinding::IncompleteDomain);
            }
            let Some(class_binding) =
                self.resolve_class_binding(&receiver.class_name, callee.start_byte())
            else {
                return (Some(caller), CachedResolutionBinding::Ambiguous);
            };
            let binding = match receiver.kind {
                JavascriptReceiverKind::Constructor => {
                    CachedResolutionBinding::ConstructorBinding {
                        class_binding,
                        method_name: raw_target.to_string(),
                    }
                }
                JavascriptReceiverKind::ExplicitType => {
                    CachedResolutionBinding::ExplicitReceiverType {
                        class_binding,
                        method_name: raw_target.to_string(),
                    }
                }
            };
            return (Some(caller), binding);
        }
        if form != CalleeForm::Identifier {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        let Some(candidates) = self.bindings.get(raw_target) else {
            return (Some(caller), CachedResolutionBinding::MissingBinding);
        };
        let mut visible = candidates
            .iter()
            .filter(|binding| {
                binding.scope_start <= callee.start_byte()
                    && callee.start_byte() < binding.scope_end
            })
            .collect::<Vec<_>>();
        let Some(depth) = visible.iter().map(|binding| binding.scope_depth).max() else {
            return (Some(caller), CachedResolutionBinding::MissingBinding);
        };
        visible.retain(|binding| binding.scope_depth == depth);
        if visible.len() != 1 {
            return (Some(caller), CachedResolutionBinding::Ambiguous);
        }
        let binding = visible[0];
        if self.writes.get(raw_target).is_some_and(|writes| {
            writes.iter().any(|write| {
                write.name == binding.name
                    && write.scope_start <= callee.start_byte()
                    && callee.start_byte() < write.scope_end
            })
        }) {
            return (Some(caller), CachedResolutionBinding::Ambiguous);
        }
        let binding = match &binding.kind {
            JavascriptBindingKind::SameFile { declaration } => CachedResolutionBinding::SameFile {
                declaration: *declaration,
            },
            JavascriptBindingKind::StaticImport {
                import,
                module_specifier,
                imported_name,
                is_default,
            } => CachedResolutionBinding::StaticImport {
                import: *import,
                module_specifier: module_specifier.clone(),
                imported_name: imported_name.clone(),
                is_default: *is_default,
            },
            JavascriptBindingKind::Class { .. } => CachedResolutionBinding::Unsupported,
            JavascriptBindingKind::Other => CachedResolutionBinding::Ambiguous,
        };
        (Some(caller), binding)
    }

    fn resolve_class_binding(
        &self,
        class_name: &str,
        call_byte: usize,
    ) -> Option<CachedClassBinding> {
        let candidates = self.bindings.get(class_name)?;
        let mut visible = candidates
            .iter()
            .filter(|binding| binding.scope_start <= call_byte && call_byte < binding.scope_end)
            .collect::<Vec<_>>();
        let depth = visible.iter().map(|binding| binding.scope_depth).max()?;
        visible.retain(|binding| binding.scope_depth == depth);
        let [binding] = visible.as_slice() else {
            return None;
        };
        if self.writes.get(class_name).is_some_and(|writes| {
            writes
                .iter()
                .any(|write| write.scope_start <= call_byte && call_byte < write.scope_end)
        }) {
            return None;
        }
        match &binding.kind {
            JavascriptBindingKind::Class { owner } => Some(CachedClassBinding::SameFile {
                owner: *owner,
                owner_name: class_name.to_string(),
            }),
            JavascriptBindingKind::StaticImport {
                import,
                module_specifier,
                imported_name,
                is_default,
            } => Some(CachedClassBinding::StaticImport {
                import: *import,
                module_specifier: module_specifier.clone(),
                imported_name: imported_name.clone(),
                is_default: *is_default,
            }),
            JavascriptBindingKind::SameFile { .. } | JavascriptBindingKind::Other => None,
        }
    }

    fn class_member_domain_is_mutated(
        &self,
        class_name: &str,
        receiver_name: &str,
        method_name: &str,
        call_byte: usize,
    ) -> bool {
        let prototype = format!("{class_name}.prototype");
        self.mutated_members
            .get(&(receiver_name.to_string(), method_name.to_string()))
            .is_some_and(|ranges| javascript_ranges_contain(ranges, call_byte))
            || self
                .mutated_members
                .get(&(prototype.clone(), method_name.to_string()))
                .is_some_and(|ranges| javascript_ranges_contain(ranges, call_byte))
            || self
                .dynamically_mutated_owners
                .get(receiver_name)
                .is_some_and(|ranges| javascript_ranges_contain(ranges, call_byte))
            || self
                .dynamically_mutated_owners
                .get(&prototype)
                .is_some_and(|ranges| javascript_ranges_contain(ranges, call_byte))
            || self
                .dynamically_mutated_owners
                .get(class_name)
                .is_some_and(|ranges| javascript_ranges_contain(ranges, call_byte))
    }

    fn collect_direct_exports(&self, source: &str) -> (Vec<CachedDirectExport>, bool, Vec<String>) {
        let mut exports = Vec::new();
        let mut poison_all = self.module_dynamic_breaker;
        let mut poisoned_names = HashSet::new();
        let assignment_names = self
            .export_statements
            .iter()
            .filter_map(|statement| javascript_export_assignment_name(*statement, source))
            .collect::<HashSet<_>>();
        poisoned_names.extend(assignment_names.iter().cloned());
        for statement in &self.export_statements {
            let Some((name, _declaration_node, is_default)) =
                javascript_direct_export(*statement, source)
            else {
                let (statement_poison_all, statement_names) =
                    javascript_export_poison(*statement, source);
                poison_all |= statement_poison_all;
                poisoned_names.extend(statement_names);
                continue;
            };
            if assignment_names.contains(name.as_str()) {
                continue;
            }
            let Some(bindings) = self.bindings.get(&name) else {
                continue;
            };
            let module_bindings = bindings
                .iter()
                .filter(|binding| binding.scope_depth == 0)
                .collect::<Vec<_>>();
            if module_bindings.len() != 1
                || self
                    .writes
                    .get(&name)
                    .is_some_and(|writes| writes.iter().any(|write| write.scope_depth == 0))
            {
                poisoned_names.insert(if is_default {
                    "default".to_string()
                } else {
                    name
                });
                continue;
            }
            let (declaration, declaration_kind) = match module_bindings[0].kind {
                JavascriptBindingKind::SameFile { declaration } => {
                    (declaration, CachedDeclarationKind::Callable)
                }
                JavascriptBindingKind::Class { owner } => (owner, CachedDeclarationKind::Class),
                _ => continue,
            };
            exports.push(CachedDirectExport {
                exported_name: if is_default { "default" } else { &name }.to_string(),
                declaration,
                is_default,
                declaration_kind,
            });
        }
        exports.sort_by(|left, right| {
            left.exported_name
                .cmp(&right.exported_name)
                .then(left.declaration.cmp(&right.declaration))
        });
        for pair in exports.windows(2) {
            if pair[0].exported_name == pair[1].exported_name
                && pair[0].is_default == pair[1].is_default
            {
                poisoned_names.insert(pair[0].exported_name.clone());
            }
        }
        exports.retain(|export| !poisoned_names.contains(&export.exported_name));
        let mut poisoned_names = poisoned_names.into_iter().collect::<Vec<_>>();
        poisoned_names.sort();
        (exports, poison_all, poisoned_names)
    }
}

fn javascript_export_assignment_name(statement: TsNode<'_>, source: &str) -> Option<String> {
    let surface = node_text(statement, source)?.trim();
    let name = surface
        .strip_prefix("export")?
        .trim_start()
        .strip_prefix('=')?;
    let name = name.trim().trim_end_matches(';').trim();
    (!name.is_empty()
        && name
            .chars()
            .all(|character| character == '_' || character == '$' || character.is_alphanumeric()))
    .then(|| name.to_string())
}

fn javascript_export_poison(statement: TsNode<'_>, source: &str) -> (bool, Vec<String>) {
    if has_direct_unnamed_token(statement, "*") {
        return (true, Vec::new());
    }
    if let Some(name) = javascript_export_assignment_name(statement, source) {
        return (false, vec![name]);
    }
    let mut names = Vec::new();
    let mut cursor = statement.walk();
    if let Some(clause) = statement
        .named_children(&mut cursor)
        .find(|child| child.kind() == "export_clause")
    {
        let mut clause_cursor = clause.walk();
        for specifier in clause
            .named_children(&mut clause_cursor)
            .filter(|child| child.kind() == "export_specifier")
        {
            let exported = specifier
                .child_by_field_name("alias")
                .or_else(|| specifier.child_by_field_name("name"));
            if let Some(name) = exported.and_then(|node| {
                simple_typescript_string(node, source).or_else(|| node_text(node, source))
            }) {
                names.push(name.to_string());
            } else {
                return (true, Vec::new());
            }
        }
        return (false, names);
    }
    if let Some(declaration) = statement.child_by_field_name("declaration") {
        if export_statement_has_default_token(statement) == Some(true) {
            return (false, vec!["default".to_string()]);
        }
        if let Some(name) = declaration_name(declaration, source) {
            return (false, vec![name.to_string()]);
        }
        if matches!(
            declaration.kind(),
            "lexical_declaration" | "variable_declaration"
        ) {
            let mut declaration_cursor = declaration.walk();
            for declarator in declaration
                .named_children(&mut declaration_cursor)
                .filter(|child| child.kind() == "variable_declarator")
            {
                if let Some(pattern) = declarator.child_by_field_name("name") {
                    collect_javascript_pattern_names(pattern, source, &mut names);
                }
            }
        }
        names.sort();
        names.dedup();
        return if names.is_empty() {
            (true, Vec::new())
        } else {
            (false, names)
        };
    }
    if export_statement_has_default_token(statement) == Some(true) {
        return (false, vec!["default".to_string()]);
    }
    (true, Vec::new())
}

fn has_direct_unnamed_token(node: TsNode<'_>, token: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| !child.is_named() && child.kind() == token)
}

fn javascript_variable_keyword(mut node: TsNode<'_>) -> Option<&'static str> {
    loop {
        if matches!(node.kind(), "lexical_declaration" | "variable_declaration") {
            return ["const", "let", "var"]
                .into_iter()
                .find(|keyword| has_direct_unnamed_token(node, keyword));
        }
        node = node.parent()?;
    }
}

fn javascript_method_has_unsupported_modifier(method: TsNode<'_>) -> bool {
    ["static", "get", "set", "*"]
        .into_iter()
        .any(|token| has_direct_unnamed_token(method, token))
}

fn javascript_mutation_owner(node: TsNode<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" | "this" => node_text(node, source).map(str::to_string),
        "member_expression" => {
            let object = node.child_by_field_name("object")?;
            let property = node.child_by_field_name("property")?;
            if !matches!(property.kind(), "property_identifier" | "identifier") {
                return None;
            }
            Some(format!(
                "{}.{}",
                javascript_mutation_owner(object, source)?,
                node_text(property, source)?
            ))
        }
        _ => None,
    }
}

fn javascript_governing_scope<'tree>(
    mut node: TsNode<'tree>,
    root: TsNode<'tree>,
) -> TsNode<'tree> {
    while let Some(parent) = node.parent() {
        if javascript_node_is_callable(parent) {
            return parent;
        }
        node = parent;
    }
    root
}

fn javascript_ancestor_range_is_indexed(
    mut node: TsNode<'_>,
    ranges: &HashSet<(usize, usize)>,
) -> bool {
    loop {
        if ranges.contains(&(node.start_byte(), node.end_byte())) {
            return true;
        }
        let Some(parent) = node.parent() else {
            return false;
        };
        node = parent;
    }
}

fn javascript_ranges_contain(ranges: &[(usize, usize)], byte: usize) -> bool {
    ranges
        .iter()
        .any(|(start, end)| *start <= byte && byte < *end)
}

fn collect_javascript_pattern_names(node: TsNode<'_>, source: &str, names: &mut Vec<String>) {
    if matches!(
        node.kind(),
        "identifier" | "shorthand_property_identifier_pattern"
    ) {
        if let Some(name) = node_text(node, source) {
            names.push(name.to_string());
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_javascript_pattern_names(child, source, names);
    }
}

fn javascript_binding_scope<'tree>(
    node: TsNode<'tree>,
    root: TsNode<'tree>,
    _source: &str,
) -> (TsNode<'tree>, usize) {
    let is_var = javascript_variable_keyword(node) == Some("var");
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.id() == root.id() {
            return (root, 0);
        }
        if is_var && javascript_node_is_callable(parent) {
            return (parent, javascript_scope_depth(parent));
        }
        if !is_var && matches!(parent.kind(), "statement_block" | "class_body") {
            return (parent, javascript_scope_depth(parent));
        }
        if javascript_node_is_callable(parent) {
            return (parent, javascript_scope_depth(parent));
        }
        current = parent;
    }
    (root, 0)
}

fn javascript_scope_depth(mut node: TsNode<'_>) -> usize {
    let mut depth = 0;
    while let Some(parent) = node.parent() {
        if matches!(parent.kind(), "statement_block" | "class_body")
            || javascript_node_is_callable(parent)
        {
            depth += 1;
        }
        node = parent;
    }
    depth
}

fn javascript_node_is_callable(node: TsNode<'_>) -> bool {
    matches!(
        node.kind(),
        "function_declaration"
            | "function_expression"
            | "generator_function_declaration"
            | "generator_function"
            | "arrow_function"
            | "method_definition"
    )
}

fn javascript_declaration_is_direct_module(mut node: TsNode<'_>) -> bool {
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "program" => return true,
            "export_statement"
            | "lexical_declaration"
            | "variable_declaration"
            | "variable_declarator" => {
                node = parent;
            }
            _ => return false,
        }
    }
    false
}

fn javascript_const_arrow<'tree>(
    declarator: TsNode<'tree>,
    _source: &str,
) -> Option<TsNode<'tree>> {
    if javascript_variable_keyword(declarator) != Some("const") {
        return None;
    }
    declarator
        .child_by_field_name("value")
        .filter(|value| value.kind() == "arrow_function")
}

fn javascript_enclosing_callable(mut node: TsNode<'_>) -> Option<TsNode<'_>> {
    while let Some(parent) = node.parent() {
        if javascript_node_is_callable(parent) {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn javascript_supported_caller(
    callable_nodes: &HashMap<(u32, String), Vec<NodeId>>,
    callable: TsNode<'_>,
    callee: TsNode<'_>,
    source: &str,
) -> Option<NodeId> {
    if let Some(body) = callable.child_by_field_name("body")
        && callee.start_byte() < body.start_byte()
    {
        return None;
    }
    let supported = match callable.kind() {
        "function_declaration" => javascript_declaration_is_direct_module(callable),
        "arrow_function" => {
            callable
                .parent()
                .is_some_and(|parent| javascript_const_arrow(parent, source) == Some(callable))
                && javascript_declaration_is_direct_module(callable)
        }
        "method_definition" => {
            javascript_enclosing_class(callable).is_some_and(|class| {
                javascript_declaration_is_direct_module(class) && javascript_class_is_closed(class)
            }) && !javascript_method_has_unsupported_modifier(callable)
                && !has_direct_named_child_kind(callable, "decorator")
        }
        _ => false,
    };
    if !supported {
        return None;
    }
    let name = if callable.kind() == "arrow_function" {
        crate::js_like_callable_source_name(callable, source)?
    } else {
        declaration_name(callable, source)?.to_string()
    };
    let line = callable.start_position().row as u32 + 1;
    let matches = callable_nodes.get(&(line, name))?;
    matches.first().copied().filter(|_| matches.len() == 1)
}

fn javascript_direct_export<'tree>(
    statement: TsNode<'tree>,
    source: &str,
) -> Option<(String, TsNode<'tree>, bool)> {
    if statement.child_by_field_name("source").is_some() {
        return None;
    }
    let declaration = statement.child_by_field_name("declaration").or_else(|| {
        let mut cursor = statement.walk();
        statement.named_children(&mut cursor).find(|child| {
            matches!(
                child.kind(),
                "function_declaration"
                    | "class_declaration"
                    | "lexical_declaration"
                    | "variable_declaration"
            )
        })
    })?;
    let is_default = export_statement_has_default_token(statement)?;
    match declaration.kind() {
        "function_declaration" => Some((
            declaration_name(declaration, source)?.to_string(),
            declaration,
            is_default,
        )),
        "class_declaration" => Some((
            declaration_name(declaration, source)?.to_string(),
            declaration,
            is_default,
        )),
        "lexical_declaration" => {
            if is_default {
                return None;
            }
            let mut cursor = declaration.walk();
            let declarators = declaration
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "variable_declarator")
                .collect::<Vec<_>>();
            let [declarator] = declarators.as_slice() else {
                return None;
            };
            let name = declarator.child_by_field_name("name")?;
            if name.kind() != "identifier" {
                return None;
            }
            Some((
                node_text(name, source)?.to_string(),
                javascript_const_arrow(*declarator, source)?,
                false,
            ))
        }
        _ => None,
    }
}

fn javascript_class_is_closed(class: TsNode<'_>) -> bool {
    !javascript_declaration_is_decorated(class)
        && !has_direct_named_child_kind(class, "class_heritage")
        && class.child_by_field_name("body").is_some()
}

fn javascript_declaration_is_decorated(declaration: TsNode<'_>) -> bool {
    has_direct_named_child_kind(declaration, "decorator")
        || declaration.parent().is_some_and(|parent| {
            parent.kind() == "export_statement" && has_direct_named_child_kind(parent, "decorator")
        })
}

fn javascript_class_methods(
    class: TsNode<'_>,
    callable_nodes: &HashMap<(u32, String), Vec<NodeId>>,
    source: &str,
) -> Option<Vec<CachedClassMethod>> {
    let body = class.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let mut methods = Vec::new();
    for child in body.named_children(&mut cursor) {
        if child.kind() != "method_definition" {
            continue;
        }
        let name_node = child.child_by_field_name("name")?;
        let name = node_text(name_node, source)?;
        if !matches!(name_node.kind(), "property_identifier" | "identifier")
            || name.starts_with('#')
            || javascript_method_has_unsupported_modifier(child)
            || has_direct_named_child_kind(child, "decorator")
            || child.child_by_field_name("body").is_none()
        {
            continue;
        }
        let line = child.start_position().row as u32 + 1;
        let declarations = callable_nodes.get(&(line, name.to_string()))?;
        let [declaration] = declarations.as_slice() else {
            return None;
        };
        methods.push(CachedClassMethod {
            name: name.to_string(),
            declaration: *declaration,
        });
    }
    methods.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.declaration.cmp(&right.declaration))
    });
    if methods.windows(2).any(|pair| pair[0].name == pair[1].name) {
        return None;
    }
    Some(methods)
}

fn javascript_direct_constructor_name(declarator: TsNode<'_>, source: &str) -> Option<String> {
    javascript_const_arrow_guard(declarator, source)?;
    let value = declarator.child_by_field_name("value")?;
    if value.kind() != "new_expression" {
        return None;
    }
    let constructor = value.child_by_field_name("constructor")?;
    (constructor.kind() == "identifier")
        .then(|| node_text(constructor, source).map(str::to_string))?
}

fn javascript_const_arrow_guard(declarator: TsNode<'_>, source: &str) -> Option<()> {
    let _ = source;
    (javascript_variable_keyword(declarator) == Some("const")).then_some(())
}

fn typescript_variable_annotation(declarator: TsNode<'_>, source: &str) -> Option<String> {
    javascript_const_arrow_guard(declarator, source)?;
    let head = node_text(declarator, source)?.split('=').next()?.trim();
    let (_, annotation) = head.split_once(':')?;
    simple_typescript_class_type(annotation)
}

fn collect_typescript_typed_parameters(
    parameters: TsNode<'_>,
    source: &str,
    emit: &mut impl FnMut(String, String),
) {
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if parameter.kind() != "required_parameter" {
            continue;
        }
        let Some(name_node) = parameter
            .child_by_field_name("pattern")
            .or_else(|| parameter.child_by_field_name("name"))
            .filter(|name| name.kind() == "identifier")
        else {
            continue;
        };
        let Some(type_node) = parameter.child_by_field_name("type") else {
            continue;
        };
        let Some(name) = node_text(name_node, source) else {
            continue;
        };
        let Some(class_name) = node_text(type_node, source).and_then(simple_typescript_class_type)
        else {
            continue;
        };
        emit(name.to_string(), class_name);
    }
}

fn simple_typescript_class_type(surface: &str) -> Option<String> {
    let name = surface.trim().trim_start_matches(':').trim();
    (!matches!(name, "any" | "unknown")
        && !name.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()))
    .then(|| name.to_string())
}

fn javascript_member_receiver(callee: TsNode<'_>, source: &str) -> Option<String> {
    let member = callee
        .parent()
        .filter(|parent| parent.kind() == "member_expression")?;
    if has_direct_named_child_kind(member, "optional_chain") {
        return None;
    }
    let object = member.child_by_field_name("object")?;
    (object.kind() == "identifier").then(|| node_text(object, source).map(str::to_string))?
}

fn has_direct_named_child_kind(node: TsNode<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind() == kind)
}

fn javascript_enclosing_class(mut node: TsNode<'_>) -> Option<TsNode<'_>> {
    while let Some(parent) = node.parent() {
        if parent.kind() == "class_declaration" {
            return Some(parent);
        }
        node = parent;
    }
    None
}

#[derive(Debug, Clone)]
struct IndexedRustCall<'tree> {
    callee: TsNode<'tree>,
    form: CalleeForm,
    raw_target: String,
    callable_id: Option<usize>,
}

#[derive(Debug, Clone)]
struct RustLexicalBinding {
    start_byte: usize,
    scope_start: usize,
    scope_end: usize,
    scope_depth: usize,
    receiver_owner: Option<String>,
    constructor: bool,
    constructor_record: bool,
    constructor_method: Option<String>,
}

#[derive(Debug, Clone)]
enum RustBindingDecision {
    Unique(RustLexicalBinding),
    Ambiguous,
}

#[derive(Debug, Clone, Copy)]
struct RustLexicalScope {
    start_byte: usize,
    end_byte: usize,
    depth: usize,
}

#[derive(Clone)]
struct RustInherentCallableContext {
    module_path: Vec<String>,
    owner_name: String,
    has_self: bool,
}

struct RustGraphNodeIndex {
    callables: HashMap<(u32, String), Vec<NodeId>>,
    structs: HashMap<(u32, String), Vec<NodeId>>,
    enums: HashMap<(u32, String), Vec<NodeId>>,
    modules: HashMap<(u32, u32, String), Vec<NodeId>>,
    imports_by_line: HashMap<u32, Vec<(u32, String, NodeId)>>,
}

impl RustGraphNodeIndex {
    fn prepare(file_id: NodeId, nodes: &[Node]) -> Self {
        let mut result = Self {
            callables: HashMap::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            modules: HashMap::new(),
            imports_by_line: HashMap::new(),
        };
        for node in nodes
            .iter()
            .filter(|node| node.file_node_id == Some(file_id))
        {
            count_rust_resolution_work(1);
            let Some(line) = node.start_line else {
                continue;
            };
            let name = graph_leaf_name(&node.serialized_name).to_string();
            match node.kind {
                NodeKind::FUNCTION | NodeKind::METHOD => result
                    .callables
                    .entry((line, name))
                    .or_default()
                    .push(node.id),
                NodeKind::STRUCT => result
                    .structs
                    .entry((line, name))
                    .or_default()
                    .push(node.id),
                NodeKind::ENUM => result.enums.entry((line, name)).or_default().push(node.id),
                NodeKind::MODULE => {
                    if let Some(column) = node.start_col {
                        result.imports_by_line.entry(line).or_default().push((
                            column,
                            graph_leaf_name(node.serialized_name.trim_end_matches(" (import)"))
                                .to_string(),
                            node.id,
                        ));
                        if !node.serialized_name.ends_with(" (import)") {
                            result
                                .modules
                                .entry((line, column, name))
                                .or_default()
                                .push(node.id);
                        }
                    }
                }
                _ => {}
            }
        }
        for imports in result.imports_by_line.values_mut() {
            imports.sort_by(|left, right| left.0.cmp(&right.0).then(left.2.cmp(&right.2)));
        }
        result
    }

    fn callable(&self, declaration: TsNode<'_>, source: &str) -> Option<NodeId> {
        let name = declaration_name(declaration, source)?;
        let line = declaration.start_position().row as u32 + 1;
        let matches = self.callables.get(&(line, name.to_string()))?;
        matches.first().copied().filter(|_| matches.len() == 1)
    }

    fn rust_type(&self, declaration: TsNode<'_>, source: &str) -> Option<NodeId> {
        let name = declaration_name(declaration, source)?;
        let line = declaration.start_position().row as u32 + 1;
        let matches = match declaration.kind() {
            "struct_item" => self.structs.get(&(line, name.to_string())),
            "enum_item" => self.enums.get(&(line, name.to_string())),
            _ => None,
        }?;
        matches.first().copied().filter(|_| matches.len() == 1)
    }

    fn module(&self, declaration: TsNode<'_>, name: &str) -> Option<NodeId> {
        let line = declaration.start_position().row as u32 + 1;
        let column = declaration.start_position().column as u32 + 1;
        let matches = self.modules.get(&(line, column, name.to_string()))?;
        matches.first().copied().filter(|_| matches.len() == 1)
    }

    fn imports_in_range(
        &self,
        line: u32,
        start_column: u32,
        end_column: u32,
    ) -> &[(u32, String, NodeId)] {
        let imports = self
            .imports_by_line
            .get(&line)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let start = imports.partition_point(|(column, _, _)| *column < start_column);
        let end = imports.partition_point(|(column, _, _)| *column <= end_column);
        &imports[start..end]
    }
}

struct RustResolutionIndex<'tree> {
    calls: Vec<IndexedRustCall<'tree>>,
    declarations: Vec<CachedTopLevelDeclaration>,
    methods: Vec<CachedInherentMethod>,
    modules: Vec<CachedRustModule>,
    types: Vec<CachedRustType>,
    uses: Vec<CachedRustUseBinding>,
    declarations_by_module_name: HashMap<(Vec<String>, String), Vec<NodeId>>,
    types_by_module_name: HashMap<(Vec<String>, String), Vec<NodeId>>,
    generic_types: HashSet<NodeId>,
    methods_by_owner_name: HashMap<(Vec<String>, String, String), Vec<CachedInherentMethod>>,
    uses_by_module_name: HashMap<(Vec<String>, String), Vec<CachedRustUseBinding>>,
    module_complete: HashMap<Vec<String>, bool>,
    identifier_module_complete: HashMap<Vec<String>, bool>,
    module_value_blockers: HashMap<Vec<String>, HashSet<String>>,
    module_incomplete_value_names: HashMap<Vec<String>, HashSet<String>>,
    module_unsupported_value_names: HashMap<Vec<String>, HashSet<String>>,
    incomplete_inherent_owners: HashSet<(Vec<String>, String)>,
    lexical_bindings: HashMap<usize, HashMap<String, Vec<RustLexicalBinding>>>,
    binding_decisions: HashMap<(usize, usize, String), RustBindingDecision>,
    callable_type_blockers: HashMap<usize, HashSet<String>>,
    inherent_callable_contexts: HashMap<usize, RustInherentCallableContext>,
    callable_complete: HashMap<usize, bool>,
    callable_poison_ranges: HashMap<usize, Vec<(usize, usize)>>,
    callable_nodes: HashMap<usize, NodeId>,
    callable_module_paths: HashMap<usize, Vec<String>>,
    callable_start_bytes: HashMap<usize, usize>,
    attributed_items: HashSet<usize>,
}

impl<'tree> RustResolutionIndex<'tree> {
    fn build(tree: &'tree Tree, source: &str, file_id: NodeId, nodes: &[Node]) -> Self {
        let graph_nodes = RustGraphNodeIndex::prepare(file_id, nodes);
        let mut result = Self {
            calls: Vec::new(),
            declarations: Vec::new(),
            methods: Vec::new(),
            modules: Vec::new(),
            types: Vec::new(),
            uses: Vec::new(),
            declarations_by_module_name: HashMap::new(),
            types_by_module_name: HashMap::new(),
            generic_types: HashSet::new(),
            methods_by_owner_name: HashMap::new(),
            uses_by_module_name: HashMap::new(),
            module_complete: HashMap::new(),
            identifier_module_complete: HashMap::new(),
            module_value_blockers: HashMap::new(),
            module_incomplete_value_names: HashMap::new(),
            module_unsupported_value_names: HashMap::new(),
            incomplete_inherent_owners: HashSet::new(),
            lexical_bindings: HashMap::new(),
            binding_decisions: HashMap::new(),
            callable_type_blockers: HashMap::new(),
            inherent_callable_contexts: HashMap::new(),
            callable_complete: HashMap::new(),
            callable_poison_ranges: HashMap::new(),
            callable_nodes: HashMap::new(),
            callable_module_paths: HashMap::new(),
            callable_start_bytes: HashMap::new(),
            attributed_items: HashSet::new(),
        };
        result.collect_module(tree.root_node(), Vec::new(), None, source, &graph_nodes);
        for method in &mut result.methods {
            if result
                .incomplete_inherent_owners
                .contains(&(method.module_path.clone(), method.owner_name.clone()))
            {
                method.domain_complete = false;
            }
        }
        for ((module_path, owner_name, _), methods) in &mut result.methods_by_owner_name {
            if result
                .incomplete_inherent_owners
                .contains(&(module_path.clone(), owner_name.clone()))
            {
                for method in methods {
                    method.domain_complete = false;
                }
            }
        }
        result.collect_execution_tree(tree.root_node(), None, None, &[], source, &graph_nodes);
        result
            .calls
            .sort_by_key(|call| (call.callee.start_byte(), call.callee.end_byte()));
        result.prepare_binding_decisions(source);
        result.lexical_bindings.clear();
        result.lexical_bindings.shrink_to_fit();
        result.declarations.sort_by(|left, right| {
            left.module_path
                .cmp(&right.module_path)
                .then(left.name.cmp(&right.name))
                .then(left.declaration.cmp(&right.declaration))
        });
        result.methods.sort_by(|left, right| {
            left.module_path
                .cmp(&right.module_path)
                .then(left.owner_name.cmp(&right.owner_name))
                .then(left.method_name.cmp(&right.method_name))
                .then(left.declaration.cmp(&right.declaration))
        });
        result
    }

    fn collect_module(
        &mut self,
        body: TsNode<'tree>,
        module_path: Vec<String>,
        declaration: Option<NodeId>,
        source: &str,
        graph_nodes: &RustGraphNodeIndex,
    ) {
        let mut domain_complete = true;
        let mut identifier_module_complete = true;
        let mut file_children = Vec::new();
        let mut value_blockers = HashSet::new();
        let mut incomplete_value_names = HashSet::new();
        let mut unsupported_value_names = HashSet::new();
        let mut cursor = body.walk();
        let items = body.named_children(&mut cursor).collect::<Vec<_>>();
        let attributed_items = rust_attributed_item_ids(&items);
        self.attributed_items
            .extend(attributed_items.iter().copied());
        for item in &items {
            let direct_domain_poison = item.kind() == "macro_invocation"
                || (item.kind() == "inner_attribute_item"
                    && !rust_inner_allow_preserves_module_bindings(*item, source))
                || (item.kind() == "attribute_item"
                    && !rust_attribute_is_bounded_item_metadata(*item, source))
                || (item.kind() == "expression_statement"
                    && contains_node_kind(*item, "macro_invocation"));
            if direct_domain_poison {
                domain_complete = false;
                identifier_module_complete = false;
            } else {
                let conservative_macro_container =
                    !matches!(item.kind(), "function_item" | "mod_item" | "impl_item");
                let identifier_macro_container = !matches!(
                    item.kind(),
                    "function_item"
                        | "mod_item"
                        | "impl_item"
                        | "const_item"
                        | "static_item"
                        | "enum_item"
                );
                if (conservative_macro_container || identifier_macro_container)
                    && contains_node_kind(*item, "macro_invocation")
                {
                    if conservative_macro_container {
                        domain_complete = false;
                    }
                    if identifier_macro_container {
                        identifier_module_complete = false;
                    }
                }
            }
            match item.kind() {
                "function_item" => {
                    let name = declaration_name(*item, source).map(str::to_string);
                    if attributed_items.contains(&item.id()) {
                        if let Some(name) = name {
                            incomplete_value_names.insert(name);
                        } else {
                            domain_complete = false;
                            identifier_module_complete = false;
                        }
                        continue;
                    }
                    let Some(name) = name else {
                        domain_complete = false;
                        identifier_module_complete = false;
                        continue;
                    };
                    let Some(declaration) = graph_nodes.callable(*item, source) else {
                        incomplete_value_names.insert(name);
                        continue;
                    };
                    let cached = CachedTopLevelDeclaration {
                        name: name.clone(),
                        declaration,
                        module_path: module_path.clone(),
                        cross_module_visible: rust_item_is_plain_pub(*item, source),
                    };
                    self.declarations_by_module_name
                        .entry((module_path.clone(), name))
                        .or_default()
                        .push(declaration);
                    self.declarations.push(cached);
                }
                "struct_item" | "enum_item" => {
                    if let Some(name) = declaration_name(*item, source) {
                        value_blockers.insert(name.to_string());
                    }
                    if attributed_items.contains(&item.id()) {
                        if let Some(name) = declaration_name(*item, source) {
                            incomplete_value_names.insert(name.to_string());
                        } else {
                            domain_complete = false;
                            identifier_module_complete = false;
                        }
                        continue;
                    }
                    let (Some(name), Some(declaration)) = (
                        declaration_name(*item, source).map(str::to_string),
                        graph_nodes.rust_type(*item, source),
                    ) else {
                        domain_complete = false;
                        identifier_module_complete = false;
                        continue;
                    };
                    self.types_by_module_name
                        .entry((module_path.clone(), name.clone()))
                        .or_default()
                        .push(declaration);
                    let (unit_constructor, record_constructor) =
                        rust_struct_constructor_capabilities(*item);
                    let generic = rust_item_is_generic(*item);
                    if generic {
                        self.generic_types.insert(declaration);
                    }
                    self.types.push(CachedRustType {
                        module_path: module_path.clone(),
                        name,
                        declaration,
                        generic,
                        cross_module_visible: rust_item_is_plain_pub(*item, source),
                        unit_constructor,
                        record_constructor,
                    });
                }
                "impl_item" => {
                    self.collect_impl(
                        *item,
                        &module_path,
                        attributed_items.contains(&item.id()),
                        source,
                        graph_nodes,
                    );
                }
                "use_declaration" => {
                    if attributed_items.contains(&item.id()) {
                        let names = rust_use_bound_names(*item, source);
                        if names.is_empty() {
                            domain_complete = false;
                            identifier_module_complete = false;
                        } else {
                            incomplete_value_names.extend(names);
                        }
                        continue;
                    }
                    if rust_use_is_renamed(*item, source) {
                        unsupported_value_names.extend(rust_use_bound_names(*item, source));
                    } else if let Some(bindings) =
                        rust_supported_use_bindings(*item, &module_path, source, graph_nodes)
                    {
                        for binding in bindings {
                            self.uses_by_module_name
                                .entry((module_path.clone(), binding.local_name.clone()))
                                .or_default()
                                .push(binding.clone());
                            self.uses.push(binding);
                        }
                    } else {
                        if node_text(*item, source).is_some_and(|surface| surface.contains('*')) {
                            domain_complete = false;
                            identifier_module_complete = false;
                        }
                        value_blockers.extend(rust_use_bound_names(*item, source));
                    }
                }
                "const_item" | "static_item" => {
                    if let Some(name) = declaration_name(*item, source) {
                        value_blockers.insert(name.to_string());
                    }
                }
                "mod_item" => {
                    let Some(name) = declaration_name(*item, source).map(str::to_string) else {
                        domain_complete = false;
                        identifier_module_complete = false;
                        continue;
                    };
                    if attributed_items.contains(&item.id()) {
                        incomplete_value_names.insert(name.clone());
                    }
                    let module_declaration = graph_nodes.module(*item, &name);
                    if let Some(child_body) = item.child_by_field_name("body") {
                        let mut child_path = module_path.clone();
                        child_path.push(name);
                        self.collect_module(
                            child_body,
                            child_path,
                            module_declaration,
                            source,
                            graph_nodes,
                        );
                    } else {
                        let Some(declaration) = module_declaration else {
                            domain_complete = false;
                            identifier_module_complete = false;
                            continue;
                        };
                        file_children.push(CachedRustFileModule { name, declaration });
                    }
                }
                _ => {}
            }
        }
        file_children.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.declaration.cmp(&right.declaration))
        });
        file_children.dedup();
        self.module_complete
            .insert(module_path.clone(), domain_complete);
        self.identifier_module_complete
            .insert(module_path.clone(), identifier_module_complete);
        self.module_value_blockers
            .insert(module_path.clone(), value_blockers.clone());
        self.module_incomplete_value_names
            .insert(module_path.clone(), incomplete_value_names.clone());
        self.module_unsupported_value_names
            .insert(module_path.clone(), unsupported_value_names);
        self.modules.push(CachedRustModule {
            module_path,
            declaration,
            domain_complete,
            value_blockers: {
                let mut blockers = value_blockers.into_iter().collect::<Vec<_>>();
                blockers.sort();
                blockers
            },
            incomplete_value_names: {
                let mut names = incomplete_value_names.into_iter().collect::<Vec<_>>();
                names.sort();
                names
            },
            file_children,
        });
    }

    fn collect_impl(
        &mut self,
        impl_item: TsNode<'tree>,
        module_path: &[String],
        impl_attributed: bool,
        source: &str,
        graph_nodes: &RustGraphNodeIndex,
    ) {
        let owner_domain = rust_inherent_impl_owner_domain(impl_item, module_path, source);
        let Some(owner_name) = simple_inherent_impl_owner(impl_item, source).map(str::to_string)
        else {
            if let Some(owner_domain) = owner_domain {
                self.incomplete_inherent_owners.insert(owner_domain);
            }
            return;
        };
        let owner_nodes = self
            .types_by_module_name
            .get(&(module_path.to_vec(), owner_name.clone()))
            .cloned()
            .unwrap_or_default();
        let owner = owner_nodes
            .first()
            .copied()
            .filter(|_| owner_nodes.len() == 1);
        let body_items = impl_item
            .child_by_field_name("body")
            .map(|body| {
                let mut cursor = body.walk();
                body.named_children(&mut cursor).collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let attributed_methods = rust_attributed_item_ids(&body_items);
        self.attributed_items
            .extend(attributed_methods.iter().copied());
        let impl_complete = !impl_attributed
            && attributed_methods.is_empty()
            && !rust_impl_has_direct_item_macro(impl_item);
        if !impl_complete {
            self.incomplete_inherent_owners
                .insert((module_path.to_vec(), owner_name.clone()));
        }
        for method in direct_impl_functions(impl_item) {
            let (Some(method_name), Some(declaration)) = (
                declaration_name(method, source).map(str::to_string),
                graph_nodes.callable(method, source),
            ) else {
                continue;
            };
            let cached = CachedInherentMethod {
                owner_name: owner_name.clone(),
                method_name: method_name.clone(),
                declaration,
                module_path: module_path.to_vec(),
                owner,
                has_self: rust_function_has_self_receiver(method),
                return_owner: rust_exact_return_owner(method, &owner_name, source),
                domain_complete: impl_complete,
                cross_module_visible: rust_item_is_plain_pub(method, source),
            };
            self.inherent_callable_contexts.insert(
                method.id(),
                RustInherentCallableContext {
                    module_path: module_path.to_vec(),
                    owner_name: owner_name.clone(),
                    has_self: cached.has_self,
                },
            );
            self.methods_by_owner_name
                .entry((module_path.to_vec(), owner_name.clone(), method_name))
                .or_default()
                .push(cached.clone());
            self.methods.push(cached);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_execution_tree(
        &mut self,
        node: TsNode<'tree>,
        current_callable: Option<usize>,
        current_scope: Option<RustLexicalScope>,
        module_path: &[String],
        source: &str,
        graph_nodes: &RustGraphNodeIndex,
    ) {
        count_rust_resolution_work(1);
        let mut callable_id = current_callable;
        let mut scope = current_scope;

        if node.kind() == "function_item" {
            if let (Some(parent_callable), Some(parent_scope)) = (current_callable, current_scope) {
                let callable_start = self
                    .callable_start_bytes
                    .get(&parent_callable)
                    .copied()
                    .unwrap_or(parent_scope.start_byte);
                rust_collect_lexical_bindings(
                    node,
                    callable_start,
                    parent_scope,
                    source,
                    self.lexical_bindings.entry(parent_callable).or_default(),
                );
            }

            callable_id = Some(node.id());
            scope = Some(RustLexicalScope {
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
                depth: 0,
            });
            self.callable_start_bytes
                .insert(node.id(), node.start_byte());
            self.callable_module_paths
                .insert(node.id(), module_path.to_vec());
            self.callable_complete
                .insert(node.id(), !self.attributed_items.contains(&node.id()));
            self.callable_poison_ranges
                .insert(node.id(), rust_callable_poison_ranges(node, source));
            self.callable_type_blockers
                .insert(node.id(), rust_callable_type_parameter_names(node, source));
            self.lexical_bindings.entry(node.id()).or_default();
            if let Some(caller) = graph_nodes.callable(node, source) {
                self.callable_nodes.insert(node.id(), caller);
            }
        } else if let Some(callable_id) = callable_id {
            let callable_start = self
                .callable_start_bytes
                .get(&callable_id)
                .copied()
                .unwrap_or(node.start_byte());
            let scope = if rust_node_starts_lexical_scope(node) {
                Some(RustLexicalScope {
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                    depth: scope.map_or(0, |scope| scope.depth + 1),
                })
            } else {
                scope
            };
            if matches!(node.kind(), "type_item" | "struct_item" | "enum_item") {
                if let Some(name) = declaration_name(node, source) {
                    self.callable_type_blockers
                        .entry(callable_id)
                        .or_default()
                        .insert(name.to_string());
                }
            } else if node.kind() == "use_declaration" {
                self.callable_type_blockers
                    .entry(callable_id)
                    .or_default()
                    .extend(rust_use_bound_names(node, source));
            }
            if let Some(scope) = scope {
                rust_collect_lexical_bindings(
                    node,
                    callable_start,
                    scope,
                    source,
                    self.lexical_bindings.entry(callable_id).or_default(),
                );
            }
            if node.kind() == "call_expression"
                && let Some(function) = node.child_by_field_name("function")
                && let Some((callee, form, raw_target)) = classify_callee(function, source)
            {
                self.calls.push(IndexedRustCall {
                    callee,
                    form,
                    raw_target,
                    callable_id: Some(callable_id),
                });
            }
            self.collect_execution_children(
                node,
                Some(callable_id),
                scope,
                module_path,
                source,
                graph_nodes,
            );
            return;
        } else if node.kind() == "call_expression"
            && let Some(function) = node.child_by_field_name("function")
            && let Some((callee, form, raw_target)) = classify_callee(function, source)
        {
            self.calls.push(IndexedRustCall {
                callee,
                form,
                raw_target,
                callable_id: None,
            });
        }

        self.collect_execution_children(node, callable_id, scope, module_path, source, graph_nodes);
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_execution_children(
        &mut self,
        node: TsNode<'tree>,
        callable_id: Option<usize>,
        scope: Option<RustLexicalScope>,
        module_path: &[String],
        source: &str,
        graph_nodes: &RustGraphNodeIndex,
    ) {
        let inline_module = (node.kind() == "mod_item")
            .then(|| {
                let body = node.child_by_field_name("body")?;
                let name = declaration_name(node, source)?;
                let mut path = module_path.to_vec();
                path.push(name.to_string());
                Some((body.id(), path))
            })
            .flatten();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            let child_module_path = inline_module
                .as_ref()
                .filter(|(body_id, _)| *body_id == child.id())
                .map_or(module_path, |(_, path)| path.as_slice());
            self.collect_execution_tree(
                child,
                callable_id,
                scope,
                child_module_path,
                source,
                graph_nodes,
            );
        }
    }

    fn prepare_binding_decisions(&mut self, source: &str) {
        let mut queries = HashMap::<(usize, String), Vec<usize>>::new();
        for call in &self.calls {
            let Some(callable_id) = call.callable_id else {
                continue;
            };
            let name = match call.form {
                CalleeForm::Identifier => Some(call.raw_target.clone()),
                CalleeForm::ExplicitReceiver => {
                    rust_field_receiver_name(call.callee, source).map(str::to_string)
                }
                _ => None,
            };
            if let Some(name) = name {
                queries
                    .entry((callable_id, name))
                    .or_default()
                    .push(call.callee.start_byte());
            }
        }
        for ((callable_id, name), mut callsites) in queries {
            callsites.sort_unstable();
            callsites.dedup();
            let bindings = self
                .lexical_bindings
                .get(&callable_id)
                .and_then(|bindings| bindings.get(&name))
                .map(Vec::as_slice)
                .unwrap_or_default();
            let mut events = Vec::with_capacity(bindings.len() * 2 + callsites.len());
            for (index, binding) in bindings.iter().enumerate() {
                let start = binding.start_byte.max(binding.scope_start);
                if start < binding.scope_end {
                    events.push((start, 1_u8, index));
                    events.push((binding.scope_end, 0_u8, index));
                    count_rust_resolution_work(2);
                }
            }
            for (index, callsite) in callsites.iter().copied().enumerate() {
                events.push((callsite, 2_u8, index));
                count_rust_resolution_work(1);
            }
            events.sort_unstable();
            let mut active = BTreeMap::<usize, HashSet<usize>>::new();
            for (byte, kind, index) in events {
                count_rust_resolution_work(1);
                match kind {
                    0 => {
                        let depth = bindings[index].scope_depth;
                        if let Some(entries) = active.get_mut(&depth) {
                            entries.remove(&index);
                            if entries.is_empty() {
                                active.remove(&depth);
                            }
                        }
                    }
                    1 => {
                        active
                            .entry(bindings[index].scope_depth)
                            .or_default()
                            .insert(index);
                    }
                    _ => {
                        let Some((_, entries)) = active.last_key_value() else {
                            continue;
                        };
                        let decision = if entries.len() == 1 {
                            RustBindingDecision::Unique(
                                bindings[*entries.iter().next().expect("one binding")].clone(),
                            )
                        } else {
                            RustBindingDecision::Ambiguous
                        };
                        self.binding_decisions
                            .insert((callable_id, byte, name.clone()), decision);
                    }
                }
            }
        }
    }

    fn resolve_syntax_claim(
        &self,
        source: &str,
        callee: TsNode<'tree>,
        form: CalleeForm,
        raw_target: &str,
        callable_id: Option<usize>,
    ) -> (Option<NodeId>, CachedResolutionBinding) {
        let Some(callable_id) = callable_id else {
            return (None, CachedResolutionBinding::MissingBinding);
        };
        let Some(caller) = self.callable_nodes.get(&callable_id).copied() else {
            return (None, CachedResolutionBinding::Ambiguous);
        };
        if !self.callsite_domain_complete(callable_id, callee.start_byte()) {
            return (Some(caller), CachedResolutionBinding::IncompleteDomain);
        }
        let Some(module_path) = self.callable_module_paths.get(&callable_id).cloned() else {
            return (Some(caller), CachedResolutionBinding::IncompleteDomain);
        };
        if self.module_complete.get(&module_path) != Some(&true) {
            let declarations = self
                .declarations_by_module_name
                .get(&(module_path.clone(), raw_target.to_owned()))
                .map(Vec::as_slice)
                .unwrap_or_default();
            let uses = self
                .uses_by_module_name
                .get(&(module_path.clone(), raw_target.to_owned()))
                .map(Vec::as_slice)
                .unwrap_or_default();
            if !matches!(form, CalleeForm::Identifier)
                || self.identifier_module_complete.get(&module_path) != Some(&true)
                || !matches!((declarations, uses), ([_], []))
            {
                return (Some(caller), CachedResolutionBinding::IncompleteDomain);
            }
        }
        match form {
            CalleeForm::Identifier => {
                if self
                    .module_unsupported_value_names
                    .get(&module_path)
                    .is_some_and(|names| names.contains(raw_target))
                {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                }
                if self
                    .module_incomplete_value_names
                    .get(&module_path)
                    .is_some_and(|names| names.contains(raw_target))
                {
                    return (Some(caller), CachedResolutionBinding::IncompleteDomain);
                }
                if self
                    .binding_decision(callable_id, callee.start_byte(), raw_target)
                    .is_some()
                {
                    return (Some(caller), CachedResolutionBinding::Ambiguous);
                }
                if self
                    .module_value_blockers
                    .get(&module_path)
                    .is_some_and(|blockers| blockers.contains(raw_target))
                {
                    return (Some(caller), CachedResolutionBinding::Ambiguous);
                }
                let declarations = self
                    .declarations_by_module_name
                    .get(&(module_path.clone(), raw_target.to_string()))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let uses = self
                    .uses_by_module_name
                    .get(&(module_path.clone(), raw_target.to_string()))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                match (declarations, uses) {
                    ([declaration], []) => (
                        Some(caller),
                        CachedResolutionBinding::SameFile {
                            declaration: *declaration,
                        },
                    ),
                    ([], [binding]) => (
                        Some(caller),
                        CachedResolutionBinding::RustPath {
                            module_path,
                            components: binding.components.clone(),
                            import: Some(binding.clone()),
                            associated_owner: None,
                        },
                    ),
                    ([], []) => (Some(caller), CachedResolutionBinding::MissingBinding),
                    _ => (Some(caller), CachedResolutionBinding::Ambiguous),
                }
            }
            CalleeForm::ImplicitReceiver => {
                let Some(context) = self.inherent_callable_contexts.get(&callable_id) else {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                };
                if !context.has_self || context.module_path != module_path {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                }
                let owner_name = &context.owner_name;
                let owner_nodes = self
                    .types_by_module_name
                    .get(&(module_path.clone(), owner_name.clone()))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let methods = self
                    .methods_by_owner_name
                    .get(&(
                        module_path.clone(),
                        owner_name.clone(),
                        raw_target.to_string(),
                    ))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                match (owner_nodes, methods) {
                    ([owner], [method])
                        if !self.generic_types.contains(owner)
                            && method.has_self
                            && method.domain_complete =>
                    {
                        (
                            Some(caller),
                            CachedResolutionBinding::ImplicitReceiver {
                                owner: *owner,
                                declaration: method.declaration,
                                owner_name: owner_name.clone(),
                            },
                        )
                    }
                    ([owner], [_]) if self.generic_types.contains(owner) => {
                        (Some(caller), CachedResolutionBinding::Unsupported)
                    }
                    ([_], [method]) if !method.domain_complete => {
                        (Some(caller), CachedResolutionBinding::IncompleteDomain)
                    }
                    ([_], [method]) if !method.has_self => {
                        (Some(caller), CachedResolutionBinding::Unsupported)
                    }
                    ([], [method]) if method.has_self && method.domain_complete => {
                        let imports = self
                            .uses_by_module_name
                            .get(&(module_path.clone(), owner_name.clone()))
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        match imports {
                            [import] => (
                                Some(caller),
                                CachedResolutionBinding::RustImplicitReceiver {
                                    module_path,
                                    owner_name: owner_name.clone(),
                                    import: import.clone(),
                                    declaration: method.declaration,
                                },
                            ),
                            [] => (Some(caller), CachedResolutionBinding::MissingBinding),
                            _ => (Some(caller), CachedResolutionBinding::Ambiguous),
                        }
                    }
                    ([], _) | (_, []) => (Some(caller), CachedResolutionBinding::MissingBinding),
                    _ => (Some(caller), CachedResolutionBinding::Ambiguous),
                }
            }
            CalleeForm::QualifiedPath => {
                let components = rust_scoped_call_components(callee, source);
                let Some(components) = components else {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                };
                if components
                    .first()
                    .is_some_and(|component| component == "super")
                {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                }
                if components
                    .first()
                    .is_some_and(|component| component == "Self")
                {
                    if components.len() != 2 {
                        return (Some(caller), CachedResolutionBinding::Unsupported);
                    }
                    let Some(context) = self.inherent_callable_contexts.get(&callable_id) else {
                        return (Some(caller), CachedResolutionBinding::Unsupported);
                    };
                    let owner_name = &context.owner_name;
                    let owners = self
                        .types_by_module_name
                        .get(&(module_path.clone(), owner_name.clone()))
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    return match owners {
                        [owner] if !self.generic_types.contains(owner) => (
                            Some(caller),
                            CachedResolutionBinding::RustPath {
                                module_path,
                                components: vec![owner_name.clone(), raw_target.to_string()],
                                import: None,
                                associated_owner: Some(*owner),
                            },
                        ),
                        [_] => (Some(caller), CachedResolutionBinding::Unsupported),
                        [] => (Some(caller), CachedResolutionBinding::MissingBinding),
                        _ => (Some(caller), CachedResolutionBinding::Ambiguous),
                    };
                }
                let import = components.first().and_then(|owner| {
                    self.uses_by_module_name
                        .get(&(module_path.clone(), owner.clone()))
                        .and_then(|uses| (uses.len() == 1).then(|| uses[0].clone()))
                });
                (
                    Some(caller),
                    CachedResolutionBinding::RustPath {
                        module_path,
                        components,
                        import,
                        associated_owner: None,
                    },
                )
            }
            CalleeForm::ExplicitReceiver => {
                let Some(receiver_name) = rust_field_receiver_name(callee, source) else {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                };
                let binding =
                    match self.binding_decision(callable_id, callee.start_byte(), receiver_name) {
                        Some(RustBindingDecision::Unique(binding)) => binding,
                        Some(RustBindingDecision::Ambiguous) => {
                            return (Some(caller), CachedResolutionBinding::Ambiguous);
                        }
                        None => return (Some(caller), CachedResolutionBinding::Unsupported),
                    };
                let Some(owner_name) = binding.receiver_owner.clone() else {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                };
                if binding.constructor_record
                    || self
                        .callable_type_blockers
                        .get(&callable_id)
                        .is_some_and(|blockers| blockers.contains(&owner_name))
                {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
                }
                let import = self
                    .uses_by_module_name
                    .get(&(module_path.clone(), owner_name.clone()))
                    .and_then(|uses| (uses.len() == 1).then(|| uses[0].clone()));
                (
                    Some(caller),
                    CachedResolutionBinding::RustExplicitReceiver {
                        module_path,
                        owner_name,
                        import,
                        constructor: binding.constructor,
                        constructor_record: binding.constructor_record,
                        constructor_method: binding.constructor_method.clone(),
                    },
                )
            }
            _ => (Some(caller), CachedResolutionBinding::Unsupported),
        }
    }

    fn binding_decision(
        &self,
        callable_id: usize,
        callsite_start: usize,
        name: &str,
    ) -> Option<&RustBindingDecision> {
        count_rust_resolution_work(1);
        self.binding_decisions
            .get(&(callable_id, callsite_start, name.to_string()))
    }

    fn callsite_domain_complete(&self, callable_id: usize, callsite_start: usize) -> bool {
        if self.callable_complete.get(&callable_id) != Some(&true) {
            return false;
        }
        let Some(ranges) = self.callable_poison_ranges.get(&callable_id) else {
            return true;
        };
        let insertion = ranges.partition_point(|(start, _)| *start <= callsite_start);
        insertion == 0 || ranges[insertion - 1].1 <= callsite_start
    }
}

fn rust_item_is_plain_pub(item: TsNode<'_>, source: &str) -> bool {
    node_text(item, source).is_some_and(|surface| surface.trim_start().starts_with("pub "))
}

fn rust_item_is_generic(item: TsNode<'_>) -> bool {
    item.child_by_field_name("type_parameters").is_some() || {
        let mut cursor = item.walk();
        item.named_children(&mut cursor)
            .any(|child| child.kind() == "type_parameters")
    }
}

fn rust_struct_constructor_capabilities(item: TsNode<'_>) -> (bool, bool) {
    if item.kind() != "struct_item" {
        return (false, false);
    }
    let mut cursor = item.walk();
    let children = item.named_children(&mut cursor).collect::<Vec<_>>();
    let record = children
        .iter()
        .any(|child| child.kind() == "field_declaration_list");
    let tuple = children
        .iter()
        .any(|child| child.kind() == "ordered_field_declaration_list");
    (!record && !tuple, record)
}

fn rust_attributed_item_ids(items: &[TsNode<'_>]) -> HashSet<usize> {
    let mut result = HashSet::new();
    let mut pending_attribute = false;
    for item in items {
        if item.kind() == "attribute_item" {
            pending_attribute = true;
        } else {
            if pending_attribute {
                result.insert(item.id());
            }
            pending_attribute = false;
        }
    }
    result
}

fn rust_attribute_is_bounded_item_metadata(attribute: TsNode<'_>, source: &str) -> bool {
    let Some(surface) = node_text(attribute, source) else {
        return false;
    };
    let Some(body) = surface.trim().strip_prefix("#[") else {
        return false;
    };
    let name = body
        .split(|character: char| {
            character == '(' || character == '=' || character == ']' || character.is_whitespace()
        })
        .next()
        .unwrap_or_default();
    matches!(name, "allow" | "cfg" | "derive" | "doc")
}

fn rust_inner_allow_preserves_module_bindings(attribute: TsNode<'_>, source: &str) -> bool {
    let Some(surface) = node_text(attribute, source) else {
        return false;
    };
    let Some(body) = surface.trim().strip_prefix("#![") else {
        return false;
    };
    body.split(|character: char| {
        character == '(' || character == '=' || character == ']' || character.is_whitespace()
    })
    .next()
        == Some("allow")
}

fn rust_callable_poison_ranges(function: TsNode<'_>, source: &str) -> Vec<(usize, usize)> {
    fn collect(
        node: TsNode<'_>,
        root_id: usize,
        scope: (usize, usize),
        source: &str,
        output: &mut Vec<(usize, usize)>,
    ) {
        count_rust_resolution_work(1);
        if node.id() != root_id && node.kind() == "function_item" {
            return;
        }
        let scope = if rust_node_starts_lexical_scope(node) {
            (node.start_byte(), node.end_byte())
        } else {
            scope
        };
        if node.kind() == "inner_attribute_item"
            || (node.kind() == "macro_invocation" && rust_macro_can_change_enclosing_bindings(node))
        {
            output.push(scope);
        }

        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        let mut pending_attributes = Vec::new();
        for child in children {
            if child.kind() == "attribute_item" {
                pending_attributes.push(child);
                continue;
            }
            if !pending_attributes.is_empty() {
                if pending_attributes
                    .iter()
                    .all(|attribute| rust_attribute_is_bounded_item_metadata(*attribute, source))
                {
                    output.push((child.start_byte(), child.end_byte()));
                } else {
                    output.push(scope);
                }
                pending_attributes.clear();
            }
            collect(child, root_id, scope, source, output);
        }
        if !pending_attributes.is_empty() {
            output.push(scope);
        }
    }

    let mut ranges = Vec::new();
    collect(
        function,
        function.id(),
        (function.start_byte(), function.end_byte()),
        source,
        &mut ranges,
    );
    ranges.sort_unstable();
    let mut merged = Vec::<(usize, usize)>::new();
    for (start, end) in ranges {
        if let Some((_, previous_end)) = merged.last_mut()
            && start <= *previous_end
        {
            *previous_end = (*previous_end).max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

fn rust_impl_has_direct_item_macro(item: TsNode<'_>) -> bool {
    let Some(body) = item.child_by_field_name("body") else {
        return true;
    };
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .any(|child| child.kind() == "macro_invocation")
}

fn rust_macro_can_change_enclosing_bindings(invocation: TsNode<'_>) -> bool {
    invocation
        .parent()
        .is_some_and(|parent| parent.kind() == "expression_statement")
}

fn rust_function_has_self_receiver(function: TsNode<'_>) -> bool {
    function
        .child_by_field_name("parameters")
        .is_some_and(|parameters| {
            let mut found = false;
            walk_nodes(parameters, &mut |node| {
                found |= matches!(node.kind(), "self_parameter" | "self")
            });
            found
        })
}

fn rust_callable_type_parameter_names(function: TsNode<'_>, source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let Some(parameters) = function.child_by_field_name("type_parameters").or_else(|| {
        let mut cursor = function.walk();
        function
            .named_children(&mut cursor)
            .find(|child| child.kind() == "type_parameters")
    }) else {
        return names;
    };
    walk_nodes(parameters, &mut |node| {
        if matches!(node.kind(), "type_identifier" | "identifier")
            && let Some(name) = node_text(node, source)
        {
            names.insert(name.to_string());
        }
    });
    names
}

fn rust_exact_return_owner(function: TsNode<'_>, owner_name: &str, source: &str) -> Option<String> {
    let return_type = function
        .child_by_field_name("return_type")
        .or_else(|| {
            let mut cursor = function.walk();
            function
                .named_children(&mut cursor)
                .find(|child| child.kind() == "return_type")
        })
        .and_then(|node| node_text(node, source))?
        .trim()
        .trim_start_matches("->")
        .trim();
    match return_type {
        "Self" => Some(owner_name.to_string()),
        value if value == owner_name => Some(owner_name.to_string()),
        _ => None,
    }
}

fn rust_supported_use_bindings(
    declaration: TsNode<'_>,
    module_path: &[String],
    source: &str,
    graph_nodes: &RustGraphNodeIndex,
) -> Option<Vec<CachedRustUseBinding>> {
    let surface = node_text(declaration, source)?.trim();
    if surface.starts_with("pub ") {
        return None;
    }
    let argument = surface.strip_prefix("use ")?.strip_suffix(';')?.trim();
    if argument.contains(" as ")
        || argument.contains('*')
        || argument.contains("::{self")
        || argument == "self"
    {
        return None;
    }
    let mut leaves = Vec::<(String, Vec<String>)>::new();
    if let Some((prefix, group)) = argument.split_once("::{") {
        let group = group.strip_suffix('}')?;
        if group.contains('{') || group.contains('}') {
            return None;
        }
        let prefix = rust_path_components(prefix)?;
        for leaf in group.split(',') {
            let leaf = leaf.trim();
            if leaf.is_empty() || leaf == "self" {
                return None;
            }
            let (path_leaf, local) = if let Some((target, alias)) = leaf.rsplit_once(" as ") {
                (target.trim(), alias.trim())
            } else {
                (leaf, leaf)
            };
            if !rust_simple_identifier(path_leaf) || !rust_simple_identifier(local) {
                return None;
            }
            let mut components = prefix.clone();
            components.push(path_leaf.to_string());
            leaves.push((local.to_string(), components));
        }
    } else {
        let (path, local) = if let Some((target, alias)) = argument.rsplit_once(" as ") {
            (target.trim(), alias.trim().to_string())
        } else {
            let local = rust_path_components(argument)?.last()?.clone();
            (argument, local)
        };
        if !rust_simple_identifier(&local) {
            return None;
        }
        leaves.push((local, rust_path_components(path)?));
    }
    if leaves.is_empty()
        || leaves.iter().any(|(_, components)| {
            !matches!(
                components.first().map(String::as_str),
                Some("crate" | "self" | "super")
            )
        })
    {
        return None;
    }
    let line = declaration.start_position().row as u32 + 1;
    let start_column = declaration.start_position().column as u32 + 1;
    let end_column = declaration.end_position().column as u32 + 1;
    let mut imports_by_name = HashMap::<String, Vec<NodeId>>::new();
    for (_, name, node_id) in graph_nodes.imports_in_range(line, start_column, end_column) {
        imports_by_name
            .entry(name.clone())
            .or_default()
            .push(*node_id);
    }
    let mut result = Vec::with_capacity(leaves.len());
    for (local_name, components) in leaves {
        let mut matches = imports_by_name.remove(&local_name).unwrap_or_default();
        matches.sort_unstable();
        matches.dedup();
        let [import] = matches.as_slice() else {
            return None;
        };
        result.push(CachedRustUseBinding {
            module_path: module_path.to_vec(),
            local_name,
            components,
            import: *import,
        });
    }
    Some(result)
}

fn rust_use_is_renamed(declaration: TsNode<'_>, source: &str) -> bool {
    node_text(declaration, source).is_some_and(|surface| surface.contains(" as "))
}

fn rust_use_bound_names(declaration: TsNode<'_>, source: &str) -> Vec<String> {
    let Some(surface) = node_text(declaration, source) else {
        return Vec::new();
    };
    let surface = surface
        .trim()
        .strip_prefix("pub ")
        .unwrap_or(surface.trim())
        .strip_prefix("use ")
        .unwrap_or(surface.trim())
        .trim_end_matches(';');
    let mut names = Vec::new();
    if let Some((_, group)) = surface.split_once("::{") {
        if let Some(group) = group.strip_suffix('}') {
            for leaf in group.split(',') {
                let leaf = leaf.trim();
                if leaf == "self" || leaf == "*" {
                    continue;
                }
                let local = leaf
                    .rsplit_once(" as ")
                    .map_or(leaf, |(_, alias)| alias)
                    .trim();
                if rust_simple_identifier(local) {
                    names.push(local.to_string());
                }
            }
        }
    } else {
        let local = surface
            .rsplit_once(" as ")
            .map_or_else(
                || surface.rsplit("::").next().unwrap_or(surface),
                |(_, alias)| alias,
            )
            .trim();
        if rust_simple_identifier(local) {
            names.push(local.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

fn rust_path_components(surface: &str) -> Option<Vec<String>> {
    let components = surface
        .split("::")
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();
    (!components.is_empty()
        && components.iter().all(|component| {
            component == "crate"
                || component == "self"
                || component == "super"
                || rust_simple_identifier(component)
        }))
    .then_some(components)
}

fn rust_simple_identifier(value: &str) -> bool {
    let value = value.strip_prefix("r#").unwrap_or(value);
    !value.is_empty()
        && value
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn rust_scoped_call_components(callee: TsNode<'_>, source: &str) -> Option<Vec<String>> {
    let scoped = callee
        .parent()
        .filter(|parent| parent.kind() == "scoped_identifier")?;
    rust_path_components(node_text(scoped, source)?)
}

fn rust_field_receiver_name<'a>(callee: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    let field = callee
        .parent()
        .filter(|parent| parent.kind() == "field_expression")?;
    let receiver = field.child_by_field_name("value")?;
    (receiver.kind() == "identifier")
        .then(|| node_text(receiver, source))
        .flatten()
}

fn rust_collect_lexical_bindings(
    node: TsNode<'_>,
    callable_start: usize,
    scope: RustLexicalScope,
    source: &str,
    output: &mut HashMap<String, Vec<RustLexicalBinding>>,
) {
    if node.kind() == "assignment_expression" || node.kind() == "compound_assignment_expr" {
        if let Some(left) = node.child_by_field_name("left")
            && left.kind() == "identifier"
            && let Some(name) = node_text(left, source)
        {
            rust_push_lexical_binding(
                output,
                name,
                node.start_byte(),
                scope,
                None,
                false,
                false,
                None,
            );
        }
        return;
    }
    let pattern = match node.kind() {
        "parameter" | "let_declaration" | "let_condition" | "for_expression" | "match_arm" => {
            node.child_by_field_name("pattern")
        }
        "const_parameter" => node.child_by_field_name("name"),
        "closure_parameters" => Some(node),
        "function_item" | "const_item" | "static_item" | "struct_item" => {
            node.child_by_field_name("name")
        }
        "use_declaration" => node.child_by_field_name("argument"),
        _ => None,
    };
    let Some(pattern) = pattern else {
        return;
    };
    let mut names = if matches!(
        node.kind(),
        "function_item" | "const_item" | "static_item" | "struct_item"
    ) {
        node_text(pattern, source)
            .map(|name| vec![name.to_string()])
            .unwrap_or_default()
    } else if node.kind() == "use_declaration" {
        rust_use_bound_names(node, source)
    } else {
        let mut names = Vec::new();
        rust_pattern_names(pattern, source, &mut names);
        names
    };
    names.sort();
    names.dedup();
    let receiver = if names.len() == 1 && matches!(node.kind(), "parameter" | "let_declaration") {
        rust_receiver_binding(node, source)
    } else {
        None
    };
    for name in names {
        let (owner, constructor, constructor_record, constructor_method) = receiver
            .as_ref()
            .map(|receiver| {
                (
                    Some(receiver.0.clone()),
                    receiver.1,
                    receiver.2,
                    receiver.3.clone(),
                )
            })
            .unwrap_or((None, false, false, None));
        let start_byte = match node.kind() {
            "parameter" | "const_parameter" => callable_start,
            "for_expression" => node
                .child_by_field_name("body")
                .map_or(node.start_byte(), |body| body.start_byte()),
            "match_arm" => node.start_byte(),
            "function_item" | "const_item" | "static_item" | "struct_item" | "use_declaration" => {
                scope.start_byte
            }
            _ => node.end_byte(),
        };
        rust_push_lexical_binding(
            output,
            &name,
            start_byte,
            scope,
            owner,
            constructor,
            constructor_record,
            constructor_method,
        );
    }
}

fn rust_pattern_names(node: TsNode<'_>, source: &str, output: &mut Vec<String>) {
    if matches!(node.kind(), "identifier" | "shorthand_field_identifier") {
        if let Some(name) = node_text(node, source) {
            output.push(name.to_string());
        }
        return;
    }
    if matches!(node.kind(), "type_identifier" | "primitive_type") {
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        rust_pattern_names(child, source, output);
    }
}

#[allow(clippy::too_many_arguments)]
fn rust_push_lexical_binding(
    output: &mut HashMap<String, Vec<RustLexicalBinding>>,
    name: &str,
    start_byte: usize,
    scope: RustLexicalScope,
    receiver_owner: Option<String>,
    constructor: bool,
    constructor_record: bool,
    constructor_method: Option<String>,
) {
    output
        .entry(name.to_string())
        .or_default()
        .push(RustLexicalBinding {
            start_byte,
            scope_start: scope.start_byte,
            scope_end: scope.end_byte,
            scope_depth: scope.depth,
            receiver_owner,
            constructor,
            constructor_record,
            constructor_method,
        });
}

fn rust_node_starts_lexical_scope(node: TsNode<'_>) -> bool {
    matches!(
        node.kind(),
        "block"
            | "for_expression"
            | "if_expression"
            | "while_expression"
            | "match_arm"
            | "closure_expression"
    )
}

fn rust_receiver_binding(
    binding: TsNode<'_>,
    source: &str,
) -> Option<(String, bool, bool, Option<String>)> {
    if let Some(type_node) = binding.child_by_field_name("type") {
        let owner = rust_exact_type_owner(node_text(type_node, source)?)?;
        return Some((owner, false, false, None));
    }
    let value = binding.child_by_field_name("value")?;
    if value.kind() == "identifier" {
        let owner = node_text(value, source)?;
        return rust_simple_identifier(owner).then(|| (owner.to_string(), true, false, None));
    }
    if value.kind() == "struct_expression" {
        let name = value.child_by_field_name("name")?;
        let owner = node_text(name, source)?;
        return rust_simple_identifier(owner).then(|| (owner.to_string(), true, true, None));
    }
    if value.kind() == "call_expression" {
        let function = value.child_by_field_name("function")?;
        if function.kind() != "scoped_identifier" {
            return None;
        }
        let components = rust_path_components(node_text(function, source)?)?;
        let [owner, method] = components.as_slice() else {
            return None;
        };
        return Some((owner.clone(), true, false, Some(method.clone())));
    }
    None
}

fn rust_exact_type_owner(surface: &str) -> Option<String> {
    let mut surface = surface.trim();
    loop {
        if surface.starts_with('(') && surface.ends_with(')') {
            surface = surface.get(1..surface.len().checked_sub(1)?)?.trim();
            continue;
        }
        if let Some(rest) = surface.strip_prefix("&mut ") {
            surface = rest.trim();
            continue;
        }
        if let Some(rest) = surface.strip_prefix('&') {
            surface = rest.trim();
            continue;
        }
        break;
    }
    rust_simple_identifier(surface).then(|| surface.to_string())
}

fn node_text<'a>(node: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

fn declaration_name<'a>(node: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    node.child_by_field_name("name")
        .and_then(|name| node_text(name, source))
}

fn graph_leaf_name(name: &str) -> &str {
    name.rsplit(['.', ':'])
        .find(|part| !part.is_empty())
        .unwrap_or(name)
}

fn typescript_file_is_module(root: TsNode<'_>) -> bool {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .any(|child| matches!(child.kind(), "import_statement" | "export_statement"))
}

fn contains_node_kind(root: TsNode<'_>, kind: &str) -> bool {
    let mut found = false;
    walk_nodes(root, &mut |node| found |= node.kind() == kind);
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

fn rust_inherent_impl_owner_domain(
    impl_item: TsNode<'_>,
    module_path: &[String],
    source: &str,
) -> Option<(Vec<String>, String)> {
    let header = node_text(impl_item, source)?.split_once('{')?.0;
    if header.contains(" for ") {
        return None;
    }
    let owner = impl_item.child_by_field_name("type")?;
    let owner = node_text(owner, source)?;
    if rust_simple_identifier(owner) {
        return Some((module_path.to_vec(), owner.to_string()));
    }
    let components = rust_path_components(owner)?;
    let (owner, path) = components.split_last()?;
    let mut resolved = module_path.to_vec();
    let mut path = path.iter();
    match path.next()?.as_str() {
        "crate" => resolved.clear(),
        "self" => {}
        "super" => {
            resolved.pop()?;
        }
        _ => return None,
    }
    for component in path {
        match component.as_str() {
            "super" => {
                resolved.pop()?;
            }
            "self" | "crate" => return None,
            _ => resolved.push(component.clone()),
        }
    }
    Some((resolved, owner.clone()))
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

fn walk_nodes<'tree>(node: TsNode<'tree>, visit: &mut impl FnMut(TsNode<'tree>)) {
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

fn export_statement_has_default_token(statement: TsNode<'_>) -> Option<bool> {
    let mut cursor = statement.walk();
    let defaults = statement
        .children(&mut cursor)
        .filter(|child| !child.is_named() && child.kind() == "default")
        .count();
    (defaults <= 1).then_some(defaults == 1)
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

struct PreparedGovernedCachePaths {
    relative_suffixes: HashSet<PathBuf>,
    absolute_identities: HashSet<WorkspacePathIdentity>,
}

impl PreparedGovernedCachePaths {
    fn prepare(
        governed: &[&codestory_store::FileInfo],
        governed_identities: &HashMap<i64, WorkspacePathIdentity>,
    ) -> Self {
        let mut relative_suffixes = HashSet::new();
        for file in governed {
            let components = file
                .path
                .components()
                .filter_map(|component| match component {
                    Component::Normal(component) => Some(component),
                    _ => None,
                })
                .collect::<Vec<_>>();
            for start in 0..components.len() {
                count_python_resolution_work(1);
                relative_suffixes.insert(components[start..].iter().copied().collect());
            }
        }
        Self {
            relative_suffixes,
            absolute_identities: governed_identities.values().cloned().collect(),
        }
    }

    fn contains(&self, cache_path: &Path) -> Result<bool> {
        count_python_resolution_work(1);
        if cache_path.is_absolute() {
            let observed = workspace_path_identity(cache_path).with_context(|| {
                format!(
                    "proof resolution native identity is unavailable for parser cache path {}",
                    cache_path.display()
                )
            })?;
            return Ok(self.absolute_identities.contains(&observed));
        }
        if cache_path.as_os_str().is_empty()
            || cache_path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(anyhow!(
                "proof resolution parser cache path is not a portable project path: {}",
                cache_path.display()
            ));
        }
        Ok(self.relative_suffixes.contains(cache_path))
    }
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
    let exact_evidence_validation = ExactEvidenceValidationIndex::prepare(&edges, &node_by_id);
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
    let governed_cache_paths = PreparedGovernedCachePaths::prepare(&governed, &governed_identities);
    let mut records_by_id = HashMap::<i64, Vec<ResolutionCacheRecord>>::new();
    for entry in store.get_index_artifact_cache_entries()? {
        let artifact: CachedIndexArtifact = match serde_json::from_slice(&entry.artifact_blob) {
            Ok(artifact) => artifact,
            Err(error) => {
                if governed_cache_paths.contains(&entry.file_path)? {
                    return Err(anyhow!(
                        "proof resolution parser cache is corrupt for {}: {error}",
                        entry.file_path.display()
                    ));
                }
                continue;
            }
        };
        let Some(file) = artifact.resolution_file else {
            if governed_cache_paths.contains(&entry.file_path)? {
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
    let rust_projection_index = RustProjectionIndex::prepare(&records)?;
    let go_projection_index = GoProjectionIndex::prepare(&records)?;
    let python_projection_index = PythonProjectionIndex::prepare(&records, &record_by_path)?;
    let mut claims = inputs
        .into_iter()
        .map(|(source_record, input)| {
            resolve_syntax_claim(
                &file_by_id,
                &record_by_path,
                &rust_projection_index,
                &go_projection_index,
                &python_projection_index,
                source_record,
                input,
            )
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
    enforce_exact_evidence_corroboration(&mut claims, &node_by_id, &exact_evidence_validation);
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
    let constructor_evidence_nodes = claims
        .iter()
        .flat_map(|claim| claim.evidence_chain.iter())
        .filter_map(|evidence| match evidence {
            ResolutionEvidence::ConstructorBinding { constructor } => Some(*constructor),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let ordinary_edge_indices = edges
        .iter()
        .enumerate()
        .filter_map(|(index, edge)| {
            (edge.kind == EdgeKind::CALL
                && node_by_id.contains_key(&edge.target)
                && !constructor_evidence_nodes.contains(&edge.effective_target()))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let edge_correlation_inputs = ordinary_edge_indices
        .iter()
        .map(|index| {
            let edge = &edges[*index];
            let raw = node_by_id[&edge.target];
            let direct_member_edge = edge.target == edge.effective_target()
                && (matches!(raw.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                    || raw.file_node_id != edge.file_node_id);
            OrdinaryCallEdgeCorrelationInput {
                file_id: edge.file_node_id.map(|file| FileId(file.0)),
                line: edge.line,
                caller: edge.effective_source(),
                target: edge.effective_target(),
                raw_edge_target: edge.target,
                raw_file_id: if direct_member_edge {
                    edge.file_node_id.map(|file| FileId(file.0))
                } else {
                    raw.file_node_id.map(|file| FileId(file.0))
                },
                raw_line: if direct_member_edge {
                    edge.line
                } else {
                    raw.start_line
                },
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
    for (claim_index, correlation) in claim_correlations.into_iter().enumerate() {
        facts.push(seal_resolved_claim(
            &file_content_hash_by_id,
            &node_by_id,
            &edges,
            &claims,
            claim_index,
            correlation,
        )?);
    }
    let funnel = build_funnel(&facts);
    store
        .replace_proof_resolution_projection(
            publication,
            &ProofResolutionProjection {
                adapter_roster: current_proof_resolution_adapter_roster(),
                facts,
                funnel,
            },
        )
        .map_err(Into::into)
}

#[derive(Clone, Copy)]
enum RelativeImportResolution<'a> {
    Unique(&'a ResolutionCacheRecord),
    Missing,
    Incomplete,
}

struct PythonRelativeImportResolution<'a> {
    target: RelativeImportResolution<'a>,
    dependencies: Vec<FileId>,
}

#[derive(Default)]
struct PythonProjectionIndex {
    complete_directories: HashSet<WorkspacePathIdentity>,
    declarations_by_file_and_name: HashMap<(i64, String), Vec<NodeId>>,
    classes_by_file_and_name: HashMap<(i64, String), Vec<NodeId>>,
    methods_by_file_owner_and_name: HashMap<(i64, NodeId, String), Vec<NodeId>>,
}

impl PythonProjectionIndex {
    fn prepare(
        records: &[ResolutionCacheRecord],
        records_by_path: &HashMap<WorkspacePathIdentity, &ResolutionCacheRecord>,
    ) -> Result<Self> {
        let mut index = Self::default();
        let mut directories = HashMap::<WorkspacePathIdentity, PathBuf>::new();
        for record in records
            .iter()
            .filter(|record| record.file.language == "python")
        {
            count_python_resolution_work(1);
            let directory = record.path.parent().ok_or_else(|| {
                anyhow!(
                    "Python proof resolution source has no package directory: {}",
                    record.path.display()
                )
            })?;
            let identity = workspace_path_identity(directory).map_err(|error| {
                anyhow!(
                    "Python proof resolution package directory has no native identity ({}): {error}",
                    directory.display()
                )
            })?;
            directories
                .entry(identity)
                .or_insert_with(|| directory.to_path_buf());
            for declaration in &record.file.top_level_declarations {
                count_python_resolution_work(1);
                index
                    .declarations_by_file_and_name
                    .entry((record.file.file_id.0, declaration.name.clone()))
                    .or_default()
                    .push(declaration.declaration);
            }
            for class in &record.file.classes {
                count_python_resolution_work(1);
                index
                    .classes_by_file_and_name
                    .entry((record.file.file_id.0, class.name.clone()))
                    .or_default()
                    .push(class.declaration);
                for method in &class.methods {
                    count_python_resolution_work(1);
                    index
                        .methods_by_file_owner_and_name
                        .entry((
                            record.file.file_id.0,
                            class.declaration,
                            method.name.clone(),
                        ))
                        .or_default()
                        .push(method.declaration);
                }
            }
        }
        for (identity, directory) in directories {
            let mut complete = true;
            let entries = match std::fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(_) => {
                    continue;
                }
            };
            for entry in entries {
                count_python_resolution_work(1);
                let Ok(entry) = entry else {
                    complete = false;
                    break;
                };
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("py") {
                    continue;
                }
                if entry.file_type().map_or(true, |kind| kind.is_symlink()) {
                    complete = false;
                    break;
                }
                let Ok(entry_identity) = workspace_path_identity(&path) else {
                    complete = false;
                    break;
                };
                if !records_by_path.contains_key(&entry_identity) {
                    complete = false;
                    break;
                }
            }
            if complete {
                index.complete_directories.insert(identity);
            }
        }
        Ok(index)
    }

    fn directory_is_complete(&self, directory: &Path) -> bool {
        count_python_resolution_work(1);
        workspace_path_identity(directory)
            .ok()
            .is_some_and(|identity| self.complete_directories.contains(&identity))
    }

    fn classes(&self, file_id: FileId, name: &str) -> &[NodeId] {
        count_python_resolution_work(1);
        self.classes_by_file_and_name
            .get(&(file_id.0, name.to_owned()))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn declarations(&self, file_id: FileId, name: &str) -> &[NodeId] {
        count_python_resolution_work(1);
        self.declarations_by_file_and_name
            .get(&(file_id.0, name.to_owned()))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn methods(&self, file_id: FileId, owner: NodeId, name: &str) -> &[NodeId] {
        count_python_resolution_work(1);
        self.methods_by_file_owner_and_name
            .get(&(file_id.0, owner, name.to_owned()))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

fn resolve_python_relative_import<'a>(
    source_record: &ResolutionCacheRecord,
    module_specifier: &str,
    records: &'a HashMap<WorkspacePathIdentity, &ResolutionCacheRecord>,
    python_index: &PythonProjectionIndex,
) -> Result<PythonRelativeImportResolution<'a>> {
    if !python_exact_relative_module(module_specifier) {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Missing,
            dependencies: Vec::new(),
        });
    }
    let Some(source_directory) = source_record.path.parent() else {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Missing,
            dependencies: Vec::new(),
        });
    };
    let mut dependency_records = Vec::new();
    let source_marker = source_directory.join("__init__.py");
    let source_marker_identity = match workspace_path_identity(&source_marker) {
        Ok(identity) => identity,
        Err(_) => {
            return Ok(PythonRelativeImportResolution {
                target: RelativeImportResolution::Incomplete,
                dependencies: Vec::new(),
            });
        }
    };
    let Some(source_marker_record) = records.get(&source_marker_identity).copied() else {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Incomplete,
            dependencies: Vec::new(),
        });
    };
    dependency_records.push(source_marker_record);

    let components = module_specifier[1..].split('.').collect::<Vec<_>>();
    let mut base = source_directory.to_path_buf();
    for component in &components[..components.len().saturating_sub(1)] {
        base.push(component);
        let marker = base.join("__init__.py");
        let identity = match workspace_path_identity(&marker) {
            Ok(identity) => identity,
            Err(_) => {
                return Ok(PythonRelativeImportResolution {
                    target: RelativeImportResolution::Incomplete,
                    dependencies: Vec::new(),
                });
            }
        };
        let Some(record) = records.get(&identity).copied() else {
            return Ok(PythonRelativeImportResolution {
                target: RelativeImportResolution::Incomplete,
                dependencies: Vec::new(),
            });
        };
        dependency_records.push(record);
    }
    let leaf = components
        .last()
        .expect("exact relative module has a component");
    let file_candidate = base.join(leaf).with_extension("py");
    let package_candidate = base.join(leaf).join("__init__.py");
    let mut matches: Vec<&ResolutionCacheRecord> = Vec::new();
    let mut uncovered = false;
    for candidate in [&file_candidate, &package_candidate] {
        match std::fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                uncovered = true;
                continue;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                uncovered = true;
                continue;
            }
        }
        let identity = match workspace_path_identity(candidate) {
            Ok(identity) => identity,
            Err(_) => {
                uncovered = true;
                continue;
            }
        };
        if let Some(record) = records.get(&identity) {
            matches.push(*record);
        } else {
            uncovered = true;
        }
    }
    matches.sort_by_key(|record| record.file.file_id);
    matches.dedup_by_key(|record| record.file.file_id);
    if uncovered || matches.len() != 1 {
        return Ok(PythonRelativeImportResolution {
            target: if matches.is_empty() && !uncovered {
                RelativeImportResolution::Missing
            } else {
                RelativeImportResolution::Incomplete
            },
            dependencies: Vec::new(),
        });
    }
    let target = matches[0];
    if package_candidate == target.path {
        dependency_records.push(target);
    }
    if [
        source_directory,
        target.path.parent().unwrap_or(source_directory),
    ]
    .into_iter()
    .any(|directory| !python_index.directory_is_complete(directory))
    {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Incomplete,
            dependencies: Vec::new(),
        });
    }
    let mut dependencies = dependency_records
        .into_iter()
        .map(|record| FileId(record.file.file_id.0))
        .chain(std::iter::once(FileId(target.file.file_id.0)))
        .collect::<Vec<_>>();
    dependencies.sort();
    dependencies.dedup();
    Ok(PythonRelativeImportResolution {
        target: RelativeImportResolution::Unique(target),
        dependencies,
    })
}

fn resolve_relative_import<'a>(
    source_record: &ResolutionCacheRecord,
    module_specifier: &str,
    records: &'a HashMap<WorkspacePathIdentity, &ResolutionCacheRecord>,
) -> Result<RelativeImportResolution<'a>> {
    let Some(parent) = source_record.path.parent() else {
        return Ok(RelativeImportResolution::Missing);
    };
    let base = parent.join(module_specifier);
    let candidates = if base.extension().is_some() {
        let supported = match source_record.file.language.as_str() {
            "typescript" | "tsx" => ["ts", "tsx", "mts", "cts"].as_slice(),
            "javascript" => ["js", "jsx", "mjs", "cjs"].as_slice(),
            _ => &[],
        };
        let extension = base.extension().and_then(|extension| extension.to_str());
        if !extension.is_some_and(|extension| supported.contains(&extension))
            || base
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".d.ts"))
        {
            return Ok(RelativeImportResolution::Missing);
        }
        vec![base]
    } else {
        match source_record.file.language.as_str() {
            "typescript" | "tsx" => vec![
                base.with_extension("ts"),
                base.with_extension("tsx"),
                base.join("index.ts"),
                base.join("index.tsx"),
            ],
            "javascript" => vec![
                base.with_extension("js"),
                base.with_extension("jsx"),
                base.join("index.js"),
                base.join("index.jsx"),
            ],
            _ => return Ok(RelativeImportResolution::Missing),
        }
    };
    let mut matches = Vec::new();
    let mut uncovered = false;
    for candidate in candidates {
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                uncovered = true;
                continue;
            }
        }
        let key = match workspace_path_identity(&candidate) {
            Ok(key) => key,
            Err(_) => {
                uncovered = true;
                continue;
            }
        };
        if let Some(record) = records.get(&key) {
            matches.push(*record);
        } else {
            uncovered = true;
        }
    }
    matches.sort_by_key(|record| record.file.file_id);
    matches.dedup_by_key(|record| record.file.file_id);
    Ok(if uncovered || matches.len() > 1 {
        RelativeImportResolution::Incomplete
    } else if let [record] = matches.as_slice() {
        RelativeImportResolution::Unique(record)
    } else {
        RelativeImportResolution::Missing
    })
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
    exact_dependency_files: Vec<FileId>,
}

#[derive(Default)]
struct ProofRelationState {
    admissible: usize,
    conflicting: usize,
}

impl ProofRelationState {
    fn is_unique(&self) -> bool {
        self.admissible == 1 && self.conflicting == 0
    }
}

struct ExactEvidenceValidationIndex {
    import_relations: HashMap<(NodeId, NodeId, NodeId), ProofRelationState>,
    member_relations: HashMap<(NodeId, NodeId), ProofRelationState>,
    python_import_paths: HashMap<(NodeId, NodeId, String), Vec<Vec<NodeId>>>,
    python_import_path_counts: HashMap<(NodeId, NodeId, Vec<NodeId>), usize>,
}

fn python_raw_import_marker_is_admissible(edge: &Edge, nodes: &HashMap<NodeId, &Node>) -> bool {
    if edge.kind != EdgeKind::IMPORT
        || edge.effective_source() != edge.source
        || !edge.candidate_targets.is_empty()
    {
        return false;
    }
    let Some(file_id) = edge.file_node_id else {
        return false;
    };
    let raw_markers_are_local = [edge.source, edge.target].into_iter().all(|node_id| {
        nodes.get(&node_id).is_some_and(|node| {
            node.file_node_id == Some(file_id)
                && matches!(node.kind, NodeKind::UNKNOWN | NodeKind::MODULE)
        })
    });
    let target_relation_is_consistent = match edge.resolved_target {
        None => edge.effective_target() == edge.target,
        Some(resolved) => {
            edge.effective_target() == resolved
                && resolved != edge.source
                && resolved != edge.target
                && nodes.contains_key(&resolved)
        }
    };
    raw_markers_are_local && target_relation_is_consistent
}

impl ExactEvidenceValidationIndex {
    fn prepare(edges: &[Edge], nodes: &HashMap<NodeId, &Node>) -> Self {
        let mut import_relations = HashMap::<_, ProofRelationState>::new();
        let mut member_relations = HashMap::<_, ProofRelationState>::new();
        let mut python_import_edges = HashMap::<NodeId, Vec<NodeId>>::new();
        for edge in edges {
            if edge.kind == EdgeKind::IMPORT
                && let (Some(file_id), Some(target)) = (edge.file_node_id, edge.resolved_target)
            {
                let admissible = edge.effective_source() == edge.source
                    && edge.effective_target() == target
                    && edge.candidate_targets.is_empty()
                    && nodes.get(&edge.source).is_some_and(|node| {
                        matches!(node.kind, NodeKind::MODULE | NodeKind::UNKNOWN)
                            && node.file_node_id == Some(file_id)
                    })
                    && nodes.get(&target).is_some_and(|node| {
                        matches!(
                            node.kind,
                            NodeKind::FUNCTION
                                | NodeKind::METHOD
                                | NodeKind::STRUCT
                                | NodeKind::CLASS
                                | NodeKind::ENUM
                        ) && node.file_node_id.is_some()
                    });
                let state = import_relations
                    .entry((file_id, edge.source, target))
                    .or_default();
                if admissible {
                    state.admissible += 1;
                } else {
                    state.conflicting += 1;
                }
            }
            if python_raw_import_marker_is_admissible(edge, nodes) {
                python_import_edges
                    .entry(edge.source)
                    .or_default()
                    .push(edge.target);
            }
            if edge.kind == EdgeKind::MEMBER {
                let owner = nodes.get(&edge.source);
                let member = nodes.get(&edge.target);
                let admissible = edge.effective_source() == edge.source
                    && edge.effective_target() == edge.target
                    && edge.candidate_targets.is_empty()
                    && owner.is_some_and(|owner| {
                        matches!(
                            (owner.kind, member.map(|member| member.kind)),
                            (
                                NodeKind::MODULE,
                                Some(
                                    NodeKind::MODULE
                                        | NodeKind::FUNCTION
                                        | NodeKind::STRUCT
                                        | NodeKind::CLASS
                                        | NodeKind::ENUM
                                )
                            ) | (NodeKind::STRUCT | NodeKind::ENUM, Some(NodeKind::METHOD))
                                | (NodeKind::CLASS, Some(NodeKind::METHOD | NodeKind::FUNCTION))
                        )
                    })
                    && member.is_some_and(|member| {
                        member.file_node_id.is_some() && edge.file_node_id == member.file_node_id
                    });
                let state = member_relations
                    .entry((edge.source, edge.target))
                    .or_default();
                if admissible {
                    state.admissible += 1;
                } else {
                    state.conflicting += 1;
                }
            }
        }
        for targets in python_import_edges.values_mut() {
            targets.sort();
            targets.dedup();
        }
        let python_import_targets = python_import_edges
            .values()
            .flatten()
            .copied()
            .collect::<HashSet<_>>();
        let mut python_import_paths = HashMap::<_, Vec<Vec<NodeId>>>::new();
        let mut python_import_path_counts = HashMap::<_, usize>::new();
        for &source in python_import_edges
            .keys()
            .filter(|source| !python_import_targets.contains(source))
        {
            let Some(file_id) = nodes.get(&source).and_then(|node| node.file_node_id) else {
                continue;
            };
            let mut path = vec![source];
            let mut visited = HashSet::from([source]);
            let mut current = source;
            while let Some([next]) = python_import_edges.get(&current).map(Vec::as_slice) {
                if !visited.insert(*next) {
                    break;
                }
                path.push(*next);
                let Some(node) = nodes.get(next) else {
                    break;
                };
                if node.kind == NodeKind::MODULE {
                    python_import_paths
                        .entry((file_id, source, node.serialized_name.clone()))
                        .or_default()
                        .push(path.clone());
                    *python_import_path_counts
                        .entry((file_id, source, path))
                        .or_default() += 1;
                    break;
                }
                current = *next;
            }
        }
        Self {
            import_relations,
            member_relations,
            python_import_paths,
            python_import_path_counts,
        }
    }

    fn has_import(&self, file: NodeId, import: NodeId, target: NodeId) -> bool {
        self.import_relations
            .get(&(file, import, target))
            .is_some_and(ProofRelationState::is_unique)
    }

    fn has_member(&self, owner: NodeId, member: NodeId) -> bool {
        self.member_relations
            .get(&(owner, member))
            .is_some_and(ProofRelationState::is_unique)
    }

    fn python_import_path(
        &self,
        file: NodeId,
        import: NodeId,
        module_specifier: &str,
    ) -> Option<&[NodeId]> {
        self.python_import_paths
            .get(&(file, import, module_specifier.to_string()))
            .and_then(|paths| match paths.as_slice() {
                [path] => Some(path.as_slice()),
                _ => None,
            })
    }

    fn has_python_import_path(&self, file: NodeId, import: NodeId, components: &[NodeId]) -> bool {
        self.python_import_path_counts
            .get(&(file, import, components.to_vec()))
            .copied()
            == Some(1)
    }

    fn claim_has_literal_corroboration(
        &self,
        claim: &ResolvedSyntaxClaim,
        nodes: &HashMap<NodeId, &Node>,
    ) -> bool {
        let Some(target) = claim.target else {
            return false;
        };
        let Some(target_node) = nodes.get(&target) else {
            return false;
        };
        if claim.evidence_chain.iter().any(|evidence| {
            let ResolutionEvidence::StaticImportBinding { import, .. } = evidence else {
                return false;
            };
            nodes.get(import).is_none_or(|node| {
                !proof_import_node_kind_is_literal(&claim.input.language, node.kind)
            })
        }) {
            return false;
        }
        let source_file = NodeId(claim.input.callsite.file_id.0);
        match (
            claim.input.callsite.callee_form,
            claim.evidence_chain.as_slice(),
        ) {
            (CalleeForm::Identifier, [ResolutionEvidence::SameFileDeclaration { declaration }]) => {
                *declaration == target && target_node.file_node_id == Some(source_file)
            }
            (
                CalleeForm::Identifier,
                [ResolutionEvidence::SamePackageDeclaration { declaration }],
            ) => {
                claim.input.language == "go"
                    && *declaration == target
                    && target_node.file_node_id.is_some()
                    && target_node.file_node_id != Some(source_file)
            }
            (
                CalleeForm::NamedImport,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration,
                    },
                ],
            ) => {
                *declaration == target
                    && nodes.get(import).is_some_and(|import_node| {
                        import_node.file_node_id == Some(source_file)
                            && graph_leaf_name(&import_node.serialized_name)
                                == claim.input.callsite.raw_target
                    })
                    && self.has_import(source_file, *import, target)
            }
            (
                CalleeForm::NamedImport,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration,
                    },
                    ResolutionEvidence::QualifiedPath { components },
                ],
            ) => {
                let python_relative = claim.input.language == "python"
                    && components.first() == Some(import)
                    && components.last() == Some(&target)
                    && nodes.get(import).is_some_and(|import_node| {
                        import_node.kind == NodeKind::UNKNOWN
                            && import_node.file_node_id == Some(source_file)
                            && graph_leaf_name(&import_node.serialized_name)
                                == claim.input.callsite.raw_target
                    })
                    && components.len() >= 3
                    && nodes
                        .get(&components[components.len() - 2])
                        .is_some_and(|node| node.kind == NodeKind::MODULE);
                python_relative
                    || (*declaration == target
                        && components.last() == Some(&target)
                        && components.len() >= 2
                        && nodes.get(import).is_some_and(|import_node| {
                            import_node.kind == NodeKind::MODULE
                                && import_node.file_node_id == Some(source_file)
                                && graph_leaf_name(&import_node.serialized_name)
                                    == claim.input.callsite.raw_target
                        })
                        && self.has_import(source_file, *import, target)
                        && components
                            .windows(2)
                            .all(|pair| self.has_member(pair[0], pair[1])))
            }
            (
                CalleeForm::ImplicitReceiver,
                [
                    ResolutionEvidence::ImplicitReceiver { owner },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) => {
                let local_shape = *declaration == target
                    && if claim.input.language == "python" {
                        target_node.kind == NodeKind::FUNCTION
                    } else {
                        target_node.kind == NodeKind::METHOD
                    }
                    && nodes.get(owner).is_some_and(|owner_node| {
                        matches!(
                            owner_node.kind,
                            NodeKind::STRUCT | NodeKind::CLASS | NodeKind::ENUM
                        ) && if claim.input.language == "go" {
                            owner_node.file_node_id.is_some() && target_node.file_node_id.is_some()
                        } else {
                            owner_node.file_node_id == Some(source_file)
                                && target_node.file_node_id == Some(source_file)
                        }
                    });
                local_shape
                    && self.has_member(*owner, claim.caller)
                    && self.has_member(*owner, target)
            }
            (
                CalleeForm::ImplicitReceiver,
                [
                    ResolutionEvidence::ImplicitReceiver { owner },
                    ResolutionEvidence::SamePackageDeclaration { declaration },
                ],
            ) => {
                claim.input.language == "go"
                    && *declaration == target
                    && target_node.kind == NodeKind::METHOD
                    && target_node.file_node_id.is_some()
                    && nodes.get(owner).is_some_and(|owner_node| {
                        owner_node.kind == NodeKind::STRUCT && owner_node.file_node_id.is_some()
                    })
                    && self.has_member(*owner, claim.caller)
                    && self.has_member(*owner, target)
            }
            (
                CalleeForm::ImplicitReceiver,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: imported_owner,
                    },
                    ResolutionEvidence::ImplicitReceiver { owner },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) => {
                *imported_owner == *owner
                    && *declaration == target
                    && target_node.kind == NodeKind::METHOD
                    && target_node.file_node_id == Some(source_file)
                    && nodes.get(import).is_some_and(|node| {
                        node.kind == NodeKind::MODULE && node.file_node_id == Some(source_file)
                    })
                    && nodes.get(owner).is_some_and(|node| {
                        matches!(node.kind, NodeKind::STRUCT | NodeKind::ENUM)
                            && node.file_node_id.is_some()
                            && node.file_node_id != Some(source_file)
                    })
                    && nodes.get(&claim.caller).is_some_and(|node| {
                        node.kind == NodeKind::METHOD && node.file_node_id == Some(source_file)
                    })
                    && self.has_import(source_file, *import, *owner)
                    && self.has_member(*owner, claim.caller)
                    && self.has_member(*owner, target)
            }
            (CalleeForm::QualifiedPath, [ResolutionEvidence::QualifiedPath { components }]) => {
                components.last() == Some(&target)
                    && components.len() >= 2
                    && components
                        .windows(2)
                        .all(|pair| self.has_member(pair[0], pair[1]))
            }
            (
                CalleeForm::QualifiedPath,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: owner,
                    },
                    ResolutionEvidence::QualifiedPath { components },
                ],
            ) => {
                components.first() == Some(owner)
                    && components.last() == Some(&target)
                    && nodes
                        .get(import)
                        .is_some_and(|node| node.file_node_id == Some(source_file))
                    && self.has_import(source_file, *import, *owner)
                    && components
                        .windows(2)
                        .all(|pair| self.has_member(pair[0], pair[1]))
            }
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::ConstructorBinding { constructor },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) if *constructor == *receiver_type => self.local_receiver_is_correlated(
                &claim.input.language,
                source_file,
                *constructor,
                *declaration,
                target,
                target_node,
                nodes,
            ),
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) => self.local_receiver_is_correlated(
                &claim.input.language,
                source_file,
                *receiver_type,
                *declaration,
                target,
                target_node,
                nodes,
            ),
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: owner,
                    },
                    ResolutionEvidence::ConstructorBinding { constructor },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::QualifiedPath { components },
                ],
            ) if *owner == *constructor && *owner == *receiver_type => self
                .imported_receiver_is_correlated(
                    &claim.input.language,
                    source_file,
                    *import,
                    *owner,
                    target,
                    target_node,
                    components,
                    nodes,
                ),
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: owner,
                    },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::QualifiedPath { components },
                ],
            ) if *owner == *receiver_type => self.imported_receiver_is_correlated(
                &claim.input.language,
                source_file,
                *import,
                *owner,
                target,
                target_node,
                components,
                nodes,
            ),
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: owner,
                    },
                    ResolutionEvidence::ConstructorBinding { constructor },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) if *owner == *constructor && *owner == *receiver_type && *declaration == target => {
                self.imported_receiver_is_correlated(
                    &claim.input.language,
                    source_file,
                    *import,
                    *owner,
                    target,
                    target_node,
                    &[],
                    nodes,
                )
            }
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: owner,
                    },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) if *owner == *receiver_type && *declaration == target => self
                .imported_receiver_is_correlated(
                    &claim.input.language,
                    source_file,
                    *import,
                    *owner,
                    target,
                    target_node,
                    &[],
                    nodes,
                ),
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::ConstructorBinding { constructor },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SamePackageDeclaration { declaration },
                ],
            ) if *constructor == *receiver_type => self.local_receiver_is_correlated(
                &claim.input.language,
                source_file,
                *constructor,
                *declaration,
                target,
                target_node,
                nodes,
            ),
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SamePackageDeclaration { declaration },
                ],
            ) => self.local_receiver_is_correlated(
                &claim.input.language,
                source_file,
                *receiver_type,
                *declaration,
                target,
                target_node,
                nodes,
            ),
            _ => false,
        }
    }

    // Keeping every proof-correlation input explicit makes accidental endpoint
    // substitution visible at the call sites.
    #[allow(clippy::too_many_arguments)]
    fn local_receiver_is_correlated(
        &self,
        language: &str,
        source_file: NodeId,
        owner: NodeId,
        declaration: NodeId,
        target: NodeId,
        target_node: &Node,
        nodes: &HashMap<NodeId, &Node>,
    ) -> bool {
        declaration == target
            && if language == "python" {
                target_node.kind == NodeKind::FUNCTION
            } else {
                target_node.kind == NodeKind::METHOD
            }
            && nodes.get(&owner).is_some_and(|owner_node| {
                matches!(
                    owner_node.kind,
                    NodeKind::CLASS | NodeKind::STRUCT | NodeKind::ENUM
                ) && if language == "go" {
                    owner_node.file_node_id.is_some() && target_node.file_node_id.is_some()
                } else {
                    owner_node.file_node_id == Some(source_file)
                        && owner_node.file_node_id == target_node.file_node_id
                }
            })
            && self.has_member(owner, target)
    }

    #[allow(clippy::too_many_arguments)]
    fn imported_receiver_is_correlated(
        &self,
        language: &str,
        source_file: NodeId,
        import: NodeId,
        owner: NodeId,
        target: NodeId,
        target_node: &Node,
        components: &[NodeId],
        nodes: &HashMap<NodeId, &Node>,
    ) -> bool {
        let python_path = language == "python"
            && components.len() >= 4
            && components[components.len() - 2] == owner
            && components.last() == Some(&target)
            && self.has_python_import_path(
                source_file,
                import,
                &components[..components.len() - 2],
            );
        (if language == "python" {
            target_node.kind == NodeKind::FUNCTION && python_path
        } else {
            target_node.kind == NodeKind::METHOD
                && (components.is_empty() || components == [owner, target])
        }) && nodes
            .get(&import)
            .is_some_and(|import_node| import_node.file_node_id == Some(source_file))
            && nodes.get(&owner).is_some_and(|owner_node| {
                matches!(
                    owner_node.kind,
                    NodeKind::CLASS | NodeKind::STRUCT | NodeKind::ENUM
                ) && owner_node.file_node_id == target_node.file_node_id
            })
            && (python_path || self.has_import(source_file, import, owner))
            && self.has_member(owner, target)
    }
}

fn proof_import_node_kind_is_literal(language: &str, kind: NodeKind) -> bool {
    match language {
        "go" | "rust" => kind == NodeKind::MODULE,
        "javascript" | "typescript" | "tsx" | "python" => {
            matches!(kind, NodeKind::MODULE | NodeKind::UNKNOWN)
        }
        _ => false,
    }
}

fn enforce_exact_evidence_corroboration(
    claims: &mut [ResolvedSyntaxClaim],
    nodes: &HashMap<NodeId, &Node>,
    validation: &ExactEvidenceValidationIndex,
) {
    for claim in claims
        .iter_mut()
        .filter(|claim| claim.status == ProofResolutionStatus::Exact)
    {
        let python_import = match &claim.input.binding {
            CachedResolutionBinding::StaticImport {
                import,
                module_specifier,
                ..
            } => Some((*import, module_specifier.as_str())),
            CachedResolutionBinding::ConstructorBinding {
                class_binding:
                    CachedClassBinding::StaticImport {
                        import,
                        module_specifier,
                        ..
                    },
                ..
            }
            | CachedResolutionBinding::ExplicitReceiverType {
                class_binding:
                    CachedClassBinding::StaticImport {
                        import,
                        module_specifier,
                        ..
                    },
                ..
            } => Some((*import, module_specifier.as_str())),
            _ => None,
        };
        if claim.input.language == "python"
            && let Some((import, module_specifier)) = python_import
            && let Some(path) = validation.python_import_path(
                NodeId(claim.input.callsite.file_id.0),
                import,
                module_specifier,
            )
        {
            let mut components = path.to_vec();
            for evidence in &claim.evidence_chain {
                match evidence {
                    ResolutionEvidence::StaticImportBinding { declaration, .. }
                        if components.last() != Some(declaration) =>
                    {
                        components.push(*declaration);
                    }
                    ResolutionEvidence::ExplicitReceiverType { receiver_type }
                        if components.last() != Some(receiver_type) =>
                    {
                        components.push(*receiver_type);
                    }
                    _ => {}
                }
            }
            if let Some(target) = claim.target
                && components.last() != Some(&target)
            {
                components.push(target);
            }
            if let Some(ResolutionEvidence::QualifiedPath {
                components: evidence_components,
            }) = claim
                .evidence_chain
                .iter_mut()
                .find(|evidence| matches!(evidence, ResolutionEvidence::QualifiedPath { .. }))
            {
                *evidence_components = components;
            }
        }
        if !validation.claim_has_literal_corroboration(claim, nodes) {
            claim.status = ProofResolutionStatus::IncompleteDomain;
            claim.reason = ProofResolutionReason::LookupDomainIncomplete;
            claim.target = None;
            claim.evidence_chain.clear();
        }
    }
}

enum RustPathResolution {
    Function {
        target: NodeId,
        target_file: FileId,
        path_components: Vec<NodeId>,
    },
    Associated {
        owner: NodeId,
        owner_file: FileId,
        target: NodeId,
        target_file: FileId,
    },
    Missing,
    Ambiguous,
    Incomplete,
    Unsupported,
}

enum RustReceiverResolution {
    Exact {
        owner: NodeId,
        owner_file: FileId,
        declaration: NodeId,
        declaration_file: FileId,
    },
    Missing,
    Ambiguous,
    Incomplete,
    Unsupported,
}

enum RustImplicitReceiverResolution {
    Exact {
        owner: NodeId,
        owner_file: FileId,
        declaration: NodeId,
    },
    Missing,
    Ambiguous,
    Incomplete,
    Unsupported,
}

#[derive(Clone)]
struct RustModuleMatch<'a> {
    record: &'a ResolutionCacheRecord,
    relative_module: Vec<String>,
}

#[derive(Clone)]
struct RustRecordOrigin {
    root: PathBuf,
    base_module: Vec<String>,
}

#[derive(Clone)]
struct RustParentClaim {
    parent_file_id: i64,
    relative_child_module: Vec<String>,
}

struct RustProjectionIndex<'a> {
    origins: HashMap<i64, RustRecordOrigin>,
    modules: HashMap<(PathBuf, Vec<String>), Vec<RustModuleMatch<'a>>>,
    module_declarations: HashMap<(PathBuf, Vec<String>), Vec<NodeId>>,
    node_files: HashMap<NodeId, i64>,
    module_inputs: HashMap<(i64, Vec<String>), &'a CachedRustModule>,
    declarations: HashMap<(i64, Vec<String>, String), Vec<&'a CachedTopLevelDeclaration>>,
    types: HashMap<(i64, Vec<String>, String), Vec<&'a CachedRustType>>,
    methods: HashMap<(i64, Vec<String>, String, String), Vec<&'a CachedInherentMethod>>,
}

impl<'a> RustProjectionIndex<'a> {
    fn prepare(records: &'a [ResolutionCacheRecord]) -> Result<Self> {
        let rust_records = records
            .iter()
            .filter(|record| record.file.language == "rust")
            .collect::<Vec<_>>();
        let record_by_id = rust_records
            .iter()
            .map(|record| (record.file.file_id.0, *record))
            .collect::<HashMap<_, _>>();
        let mut record_by_identity = HashMap::new();
        for record in &rust_records {
            count_rust_resolution_work(1);
            record_by_identity.insert(workspace_path_identity(&record.path)?, *record);
        }
        let mut roots = HashMap::<PathBuf, Vec<&ResolutionCacheRecord>>::new();
        for record in &rust_records {
            count_rust_resolution_work(1);
            if matches!(
                record.path.file_name().and_then(|name| name.to_str()),
                Some("lib.rs" | "main.rs")
            ) && let Some(root) = record.path.parent()
            {
                roots.entry(root.to_path_buf()).or_default().push(record);
            }
        }
        let valid_roots = roots
            .into_iter()
            .filter_map(|(root, records)| {
                let [record] = records.as_slice() else {
                    return None;
                };
                Some((record.file.file_id.0, root))
            })
            .collect::<HashMap<_, _>>();
        let mut parent_claims = HashMap::<i64, Vec<RustParentClaim>>::new();
        for parent in &rust_records {
            count_rust_resolution_work(1);
            for module in &parent.file.rust_modules {
                count_rust_resolution_work(1);
                for child in &module.file_children {
                    count_rust_resolution_work(1);
                    let candidates = standard_rust_module_candidates(
                        &parent.path,
                        &module.module_path,
                        &child.name,
                    );
                    let mut matches = Vec::new();
                    for candidate in candidates {
                        count_rust_resolution_work(1);
                        if let Ok(identity) = workspace_path_identity(&candidate)
                            && let Some(record) = record_by_identity.get(&identity)
                        {
                            matches.push(*record);
                        }
                    }
                    matches.sort_by_key(|record| record.file.file_id);
                    matches.dedup_by_key(|record| record.file.file_id);
                    let [record] = matches.as_slice() else {
                        continue;
                    };
                    let mut relative_child_module = module.module_path.clone();
                    relative_child_module.push(child.name.clone());
                    parent_claims
                        .entry(record.file.file_id.0)
                        .or_default()
                        .push(RustParentClaim {
                            parent_file_id: parent.file.file_id.0,
                            relative_child_module,
                        });
                }
            }
        }
        let mut origins = HashMap::<i64, Option<RustRecordOrigin>>::new();
        let mut visiting = HashSet::new();
        for record in &rust_records {
            count_rust_resolution_work(1);
            resolve_rust_record_origin(
                record.file.file_id.0,
                &record_by_id,
                &valid_roots,
                &parent_claims,
                &mut origins,
                &mut visiting,
            );
        }
        let origins = origins
            .into_iter()
            .filter_map(|(file_id, origin)| origin.map(|origin| (file_id, origin)))
            .collect::<HashMap<_, _>>();
        let mut modules = HashMap::<(PathBuf, Vec<String>), Vec<RustModuleMatch<'a>>>::new();
        let mut module_declarations = HashMap::<(PathBuf, Vec<String>), Vec<NodeId>>::new();
        let mut node_files = HashMap::new();
        let mut module_inputs = HashMap::new();
        let mut declarations = HashMap::<_, Vec<_>>::new();
        let mut types = HashMap::<_, Vec<_>>::new();
        let mut methods = HashMap::<_, Vec<_>>::new();
        for record in rust_records {
            count_rust_resolution_work(1);
            let Some(origin) = origins.get(&record.file.file_id.0) else {
                continue;
            };
            for module in &record.file.rust_modules {
                count_rust_resolution_work(1);
                module_inputs.insert((record.file.file_id.0, module.module_path.clone()), module);
                let mut absolute = origin.base_module.clone();
                absolute.extend(module.module_path.clone());
                modules
                    .entry((origin.root.clone(), absolute.clone()))
                    .or_default()
                    .push(RustModuleMatch {
                        record,
                        relative_module: module.module_path.clone(),
                    });
                if let Some(declaration) = module.declaration {
                    node_files.insert(declaration, record.file.file_id.0);
                    module_declarations
                        .entry((origin.root.clone(), absolute.clone()))
                        .or_default()
                        .push(declaration);
                }
                for child in &module.file_children {
                    count_rust_resolution_work(1);
                    node_files.insert(child.declaration, record.file.file_id.0);
                    let mut child_path = absolute.clone();
                    child_path.push(child.name.clone());
                    module_declarations
                        .entry((origin.root.clone(), child_path))
                        .or_default()
                        .push(child.declaration);
                }
            }
            for declaration in &record.file.top_level_declarations {
                count_rust_resolution_work(1);
                node_files.insert(declaration.declaration, record.file.file_id.0);
                declarations
                    .entry((
                        record.file.file_id.0,
                        declaration.module_path.clone(),
                        declaration.name.clone(),
                    ))
                    .or_default()
                    .push(declaration);
            }
            for rust_type in &record.file.rust_types {
                count_rust_resolution_work(1);
                node_files.insert(rust_type.declaration, record.file.file_id.0);
                types
                    .entry((
                        record.file.file_id.0,
                        rust_type.module_path.clone(),
                        rust_type.name.clone(),
                    ))
                    .or_default()
                    .push(rust_type);
            }
            for method in &record.file.inherent_methods {
                count_rust_resolution_work(1);
                node_files.insert(method.declaration, record.file.file_id.0);
                methods
                    .entry((
                        record.file.file_id.0,
                        method.module_path.clone(),
                        method.owner_name.clone(),
                        method.method_name.clone(),
                    ))
                    .or_default()
                    .push(method);
            }
            for import in &record.file.rust_uses {
                count_rust_resolution_work(1);
                node_files.insert(import.import, record.file.file_id.0);
            }
        }
        Ok(Self {
            origins,
            modules,
            module_declarations,
            node_files,
            module_inputs,
            declarations,
            types,
            methods,
        })
    }

    fn node_file(&self, node: NodeId) -> Option<i64> {
        count_rust_resolution_work(1);
        self.node_files.get(&node).copied()
    }

    fn module(&self, record: &ResolutionCacheRecord, path: &[String]) -> Option<&CachedRustModule> {
        count_rust_resolution_work(1);
        self.module_inputs
            .get(&(record.file.file_id.0, path.to_vec()))
            .copied()
    }

    fn declarations(
        &self,
        record: &ResolutionCacheRecord,
        path: &[String],
        name: &str,
    ) -> &[&CachedTopLevelDeclaration] {
        count_rust_resolution_work(1);
        self.declarations
            .get(&(record.file.file_id.0, path.to_vec(), name.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn types(
        &self,
        record: &ResolutionCacheRecord,
        path: &[String],
        name: &str,
    ) -> &[&CachedRustType] {
        count_rust_resolution_work(1);
        self.types
            .get(&(record.file.file_id.0, path.to_vec(), name.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn methods(
        &self,
        record: &ResolutionCacheRecord,
        path: &[String],
        owner: &str,
        method: &str,
    ) -> &[&CachedInherentMethod] {
        count_rust_resolution_work(1);
        self.methods
            .get(&(
                record.file.file_id.0,
                path.to_vec(),
                owner.to_string(),
                method.to_string(),
            ))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn resolve_module_path(
        &self,
        source_record: &ResolutionCacheRecord,
        relative_module: &[String],
        components: &[String],
    ) -> Option<(RustModuleMatch<'a>, String, Vec<NodeId>, bool)> {
        let (target_name, path_components) = components.split_last()?;
        let source_origin = self.origins.get(&source_record.file.file_id.0)?;
        let mut absolute_module = source_origin.base_module.clone();
        absolute_module.extend_from_slice(relative_module);
        let source_absolute_module = absolute_module.clone();
        let mut iter = path_components.iter();
        match iter.next()?.as_str() {
            "crate" => absolute_module.clear(),
            "self" => {}
            "super" => {
                absolute_module.pop()?;
            }
            _ => return None,
        }
        for component in iter {
            if component == "super" {
                absolute_module.pop()?;
            } else if component == "self" || component == "crate" {
                return None;
            } else {
                absolute_module.push(component.clone());
            }
        }
        let modules = self
            .modules
            .get(&(source_origin.root.clone(), absolute_module.clone()))?;
        let [module_match] = modules.as_slice() else {
            return None;
        };
        let mut path_nodes = Vec::with_capacity(absolute_module.len());
        for length in 1..=absolute_module.len() {
            let declarations = self.module_declarations.get(&(
                source_origin.root.clone(),
                absolute_module[..length].to_vec(),
            ))?;
            let [declaration] = declarations.as_slice() else {
                return None;
            };
            path_nodes.push(*declaration);
        }
        let private_visible = absolute_module.len() <= source_absolute_module.len()
            && absolute_module
                .iter()
                .zip(&source_absolute_module)
                .all(|(target, source)| target == source);
        Some((
            module_match.clone(),
            target_name.clone(),
            path_nodes,
            private_visible,
        ))
    }
}

fn standard_rust_module_candidates(
    parent_path: &Path,
    inline_module_path: &[String],
    child_name: &str,
) -> Vec<PathBuf> {
    let Some(parent) = parent_path.parent() else {
        return Vec::new();
    };
    let Some(file_name) = parent_path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let mut base = match file_name {
        "lib.rs" | "main.rs" | "mod.rs" => parent.to_path_buf(),
        file if file.ends_with(".rs") => {
            let Some(stem) = file.strip_suffix(".rs") else {
                return Vec::new();
            };
            parent.join(stem)
        }
        _ => return Vec::new(),
    };
    for component in inline_module_path {
        base.push(component);
    }
    vec![
        base.join(format!("{child_name}.rs")),
        base.join(child_name).join("mod.rs"),
    ]
}

fn resolve_rust_record_origin(
    file_id: i64,
    records: &HashMap<i64, &ResolutionCacheRecord>,
    roots: &HashMap<i64, PathBuf>,
    parent_claims: &HashMap<i64, Vec<RustParentClaim>>,
    memo: &mut HashMap<i64, Option<RustRecordOrigin>>,
    visiting: &mut HashSet<i64>,
) -> Option<RustRecordOrigin> {
    if let Some(origin) = memo.get(&file_id) {
        return origin.clone();
    }
    if !visiting.insert(file_id) {
        memo.insert(file_id, None);
        return None;
    }
    let claims = parent_claims
        .get(&file_id)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let result = match (roots.get(&file_id), claims) {
        (Some(root), []) => Some(RustRecordOrigin {
            root: root.clone(),
            base_module: Vec::new(),
        }),
        (None, [claim]) if records.contains_key(&claim.parent_file_id) => {
            resolve_rust_record_origin(
                claim.parent_file_id,
                records,
                roots,
                parent_claims,
                memo,
                visiting,
            )
            .map(|parent| {
                let mut base_module = parent.base_module;
                base_module.extend(claim.relative_child_module.clone());
                RustRecordOrigin {
                    root: parent.root,
                    base_module,
                }
            })
        }
        _ => None,
    };
    visiting.remove(&file_id);
    memo.insert(file_id, result.clone());
    result
}

fn resolve_rust_path_binding(
    rust_index: &RustProjectionIndex<'_>,
    source_record: &ResolutionCacheRecord,
    module_path: &[String],
    components: &[String],
    import: Option<&CachedRustUseBinding>,
    associated_owner: Option<NodeId>,
    raw_target: &str,
) -> RustPathResolution {
    if let Some(owner) = associated_owner {
        let Some(owner_name) = components.first() else {
            return RustPathResolution::Unsupported;
        };
        let methods = rust_index
            .methods(source_record, module_path, owner_name, raw_target)
            .iter()
            .copied()
            .filter(|method| method.owner == Some(owner) && !method.has_self)
            .collect::<Vec<_>>();
        return match methods.as_slice() {
            [method] if method.domain_complete => RustPathResolution::Associated {
                owner,
                owner_file: FileId(source_record.file.file_id.0),
                target: method.declaration,
                target_file: FileId(source_record.file.file_id.0),
            },
            [] => RustPathResolution::Missing,
            [method] if !method.domain_complete => RustPathResolution::Incomplete,
            _ => RustPathResolution::Ambiguous,
        };
    }

    if import.is_none()
        && components.len() == 2
        && !matches!(components[0].as_str(), "crate" | "self" | "super")
    {
        let module_complete = rust_index
            .module(source_record, module_path)
            .is_some_and(|module| module.domain_complete);
        if !module_complete {
            return RustPathResolution::Incomplete;
        }
        let owners = rust_index.types(source_record, module_path, &components[0]);
        let methods = rust_index
            .methods(source_record, module_path, &components[0], raw_target)
            .iter()
            .copied()
            .filter(|method| !method.has_self)
            .collect::<Vec<_>>();
        return match (owners, methods.as_slice()) {
            ([owner], [method]) if !owner.generic && method.domain_complete => {
                RustPathResolution::Associated {
                    owner: owner.declaration,
                    owner_file: FileId(source_record.file.file_id.0),
                    target: method.declaration,
                    target_file: FileId(source_record.file.file_id.0),
                }
            }
            ([owner], [_]) if owner.generic => RustPathResolution::Unsupported,
            ([], _) | (_, []) => RustPathResolution::Missing,
            (_, [method]) if !method.domain_complete => RustPathResolution::Incomplete,
            _ => RustPathResolution::Ambiguous,
        };
    }

    if let Some(import) = import
        && components.len() == 2
        && components[0] == import.local_name
    {
        let Some((module_match, owner_name, _, private_visible)) =
            rust_resolve_module_path(rust_index, source_record, module_path, &import.components)
        else {
            return RustPathResolution::Incomplete;
        };
        let owners = rust_index.types(
            module_match.record,
            &module_match.relative_module,
            &owner_name,
        );
        let methods = rust_index
            .methods(
                module_match.record,
                &module_match.relative_module,
                &owner_name,
                raw_target,
            )
            .iter()
            .copied()
            .filter(|method| !method.has_self)
            .collect::<Vec<_>>();
        return match (owners, methods.as_slice()) {
            ([owner], [method])
                if !owner.generic
                    && (owner.cross_module_visible || private_visible)
                    && (method.cross_module_visible || private_visible)
                    && method.domain_complete =>
            {
                RustPathResolution::Associated {
                    owner: owner.declaration,
                    owner_file: FileId(module_match.record.file.file_id.0),
                    target: method.declaration,
                    target_file: FileId(module_match.record.file.file_id.0),
                }
            }
            ([owner], [_]) if owner.generic => RustPathResolution::Unsupported,
            ([owner], [_]) if !owner.cross_module_visible && !private_visible => {
                RustPathResolution::Unsupported
            }
            ([_], [method]) if !method.cross_module_visible && !private_visible => {
                RustPathResolution::Unsupported
            }
            ([], _) | (_, []) => RustPathResolution::Missing,
            (_, [method]) if !method.domain_complete => RustPathResolution::Incomplete,
            _ => RustPathResolution::Ambiguous,
        };
    }

    let path = import.map_or(components, |binding| binding.components.as_slice());
    let Some((target_module, target_name, path_components, private_visible)) =
        rust_resolve_module_path(rust_index, source_record, module_path, path)
    else {
        return if path
            .first()
            .is_some_and(|root| matches!(root.as_str(), "crate" | "self" | "super"))
        {
            RustPathResolution::Incomplete
        } else {
            RustPathResolution::Unsupported
        };
    };
    let declarations = rust_index.declarations(
        target_module.record,
        &target_module.relative_module,
        &target_name,
    );
    let target_module_input =
        rust_index.module(target_module.record, &target_module.relative_module);
    let module_complete = target_module_input.is_some_and(|module| module.domain_complete);
    let target_blocked = target_module_input.is_some_and(|module| {
        module
            .value_blockers
            .iter()
            .any(|name| name == &target_name)
    });
    let target_incomplete = target_module_input.is_some_and(|module| {
        module
            .incomplete_value_names
            .iter()
            .any(|name| name == &target_name)
    });
    match declarations {
        [declaration]
            if module_complete
                && !target_blocked
                && !target_incomplete
                && (declaration.cross_module_visible || private_visible) =>
        {
            RustPathResolution::Function {
                target: declaration.declaration,
                target_file: FileId(target_module.record.file.file_id.0),
                path_components,
            }
        }
        [_] if target_incomplete || !module_complete => RustPathResolution::Incomplete,
        [_] if target_blocked => RustPathResolution::Ambiguous,
        [_] if module_complete => RustPathResolution::Unsupported,
        [] if target_incomplete || !module_complete => RustPathResolution::Incomplete,
        [] if target_blocked => RustPathResolution::Ambiguous,
        [] if module_complete => RustPathResolution::Missing,
        _ => RustPathResolution::Ambiguous,
    }
}

struct RustReceiverQuery<'a> {
    module_path: &'a [String],
    owner_name: &'a str,
    import: Option<&'a CachedRustUseBinding>,
    method_name: &'a str,
    constructor: bool,
    constructor_record: bool,
    constructor_method: Option<&'a str>,
}

fn resolve_rust_receiver_binding(
    rust_index: &RustProjectionIndex<'_>,
    source_record: &ResolutionCacheRecord,
    query: RustReceiverQuery<'_>,
) -> RustReceiverResolution {
    let RustReceiverQuery {
        module_path,
        owner_name,
        import,
        method_name,
        constructor,
        constructor_record,
        constructor_method,
    } = query;
    let (record, relative_module, owner) = if let Some(import) = import {
        let Some((module_match, target_name, _, private_visible)) =
            rust_resolve_module_path(rust_index, source_record, module_path, &import.components)
        else {
            return RustReceiverResolution::Incomplete;
        };
        if target_name != owner_name {
            return RustReceiverResolution::Ambiguous;
        }
        let owners = rust_index.types(
            module_match.record,
            &module_match.relative_module,
            owner_name,
        );
        let [owner] = owners else {
            return if owners.is_empty() {
                RustReceiverResolution::Missing
            } else {
                RustReceiverResolution::Ambiguous
            };
        };
        if owner.generic {
            return RustReceiverResolution::Unsupported;
        }
        if !owner.cross_module_visible && !private_visible {
            return RustReceiverResolution::Unsupported;
        }
        (module_match.record, module_match.relative_module, *owner)
    } else {
        let owners = rust_index.types(source_record, module_path, owner_name);
        let [owner] = owners else {
            return if owners.is_empty() {
                RustReceiverResolution::Missing
            } else {
                RustReceiverResolution::Ambiguous
            };
        };
        if owner.generic {
            return RustReceiverResolution::Unsupported;
        }
        (source_record, module_path.to_vec(), *owner)
    };
    let module_complete = rust_index
        .module(record, &relative_module)
        .is_some_and(|module| module.domain_complete);
    if !module_complete {
        return RustReceiverResolution::Incomplete;
    }
    let methods = rust_index
        .methods(record, &relative_module, owner_name, method_name)
        .iter()
        .copied()
        .filter(|method| method.owner == Some(owner.declaration) && method.has_self)
        .collect::<Vec<_>>();
    let [method] = methods.as_slice() else {
        return if methods.is_empty() {
            RustReceiverResolution::Missing
        } else {
            RustReceiverResolution::Ambiguous
        };
    };
    if !method.domain_complete {
        return RustReceiverResolution::Incomplete;
    }
    if import.is_some() && !method.cross_module_visible {
        return RustReceiverResolution::Unsupported;
    }
    if constructor && let Some(constructor_method) = constructor_method {
        let constructors = rust_index
            .methods(record, &relative_module, owner_name, constructor_method)
            .iter()
            .copied()
            .filter(|candidate| {
                candidate.owner == Some(owner.declaration)
                    && !candidate.has_self
                    && candidate.return_owner.as_deref() == Some(owner_name)
                    && candidate.domain_complete
                    && (import.is_none() || candidate.cross_module_visible)
            })
            .count();
        if constructors != 1 {
            return if constructors == 0 {
                RustReceiverResolution::Unsupported
            } else {
                RustReceiverResolution::Ambiguous
            };
        }
    } else if constructor
        && ((constructor_record && !owner.record_constructor)
            || (!constructor_record && !owner.unit_constructor))
    {
        return RustReceiverResolution::Unsupported;
    }
    RustReceiverResolution::Exact {
        owner: owner.declaration,
        owner_file: FileId(record.file.file_id.0),
        declaration: method.declaration,
        declaration_file: FileId(record.file.file_id.0),
    }
}

fn resolve_rust_imported_implicit_receiver(
    rust_index: &RustProjectionIndex<'_>,
    source_record: &ResolutionCacheRecord,
    module_path: &[String],
    owner_name: &str,
    import: &CachedRustUseBinding,
    declaration: NodeId,
    method_name: &str,
) -> RustImplicitReceiverResolution {
    let Some((module_match, resolved_owner_name, path_nodes, private_visible)) =
        rust_resolve_module_path(rust_index, source_record, module_path, &import.components)
    else {
        return RustImplicitReceiverResolution::Incomplete;
    };
    if resolved_owner_name != owner_name {
        return RustImplicitReceiverResolution::Ambiguous;
    }
    let owners = rust_index.types(
        module_match.record,
        &module_match.relative_module,
        owner_name,
    );
    let methods = rust_index
        .methods(source_record, module_path, owner_name, method_name)
        .iter()
        .copied()
        .filter(|method| method.declaration == declaration && method.has_self)
        .collect::<Vec<_>>();
    match (owners, methods.as_slice()) {
        ([owner], [method])
            if !owner.generic
                && (owner.cross_module_visible || private_visible)
                && method.domain_complete
                && path_nodes.iter().all(|node| {
                    rust_index.node_file(*node).is_some_and(|file| {
                        file == source_record.file.file_id.0
                            || file == module_match.record.file.file_id.0
                    })
                }) =>
        {
            RustImplicitReceiverResolution::Exact {
                owner: owner.declaration,
                owner_file: FileId(module_match.record.file.file_id.0),
                declaration,
            }
        }
        ([owner], [_]) if owner.generic => RustImplicitReceiverResolution::Unsupported,
        ([_], [method]) if !method.domain_complete => RustImplicitReceiverResolution::Incomplete,
        ([_], [_])
            if !path_nodes
                .iter()
                .all(|node| rust_index.node_file(*node).is_some()) =>
        {
            RustImplicitReceiverResolution::Incomplete
        }
        ([], _) | (_, []) => RustImplicitReceiverResolution::Missing,
        _ => RustImplicitReceiverResolution::Ambiguous,
    }
}

fn rust_resolve_module_path<'a>(
    rust_index: &'a RustProjectionIndex<'a>,
    source_record: &'a ResolutionCacheRecord,
    relative_module: &[String],
    components: &[String],
) -> Option<(RustModuleMatch<'a>, String, Vec<NodeId>, bool)> {
    rust_index.resolve_module_path(source_record, relative_module, components)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct GoPackageKey {
    directory: WorkspacePathIdentity,
    package_name: String,
}

struct GoPackageDeclaration<'a> {
    declaration: NodeId,
    record: &'a ResolutionCacheRecord,
}

struct GoPackageType<'a> {
    value: &'a CachedGoType,
    record: &'a ResolutionCacheRecord,
}

struct GoPackageMethod<'a> {
    value: &'a CachedGoMethod,
    record: &'a ResolutionCacheRecord,
}

struct GoCandidateSet<T> {
    exact: Option<T>,
    ambiguous: bool,
    conditional: bool,
    generated: bool,
}

impl<T> Default for GoCandidateSet<T> {
    fn default() -> Self {
        Self {
            exact: None,
            ambiguous: false,
            conditional: false,
            generated: false,
        }
    }
}

impl<T> GoCandidateSet<T> {
    fn add(&mut self, value: T, conditional: bool, generated: bool) {
        self.conditional |= conditional;
        if conditional {
            return;
        }
        if generated {
            self.ambiguous |= self.generated || self.exact.is_some();
            self.generated = true;
            return;
        }
        self.ambiguous |= self.generated;
        if self.exact.replace(value).is_some() {
            self.ambiguous = true;
        }
    }
}

struct GoPackageDomain<'a> {
    dependencies: Vec<FileId>,
    complete: bool,
    functions: HashMap<String, GoCandidateSet<GoPackageDeclaration<'a>>>,
    blockers: HashMap<String, usize>,
    conditional_blockers: HashSet<String>,
    types: HashMap<String, GoCandidateSet<GoPackageType<'a>>>,
    methods: HashMap<(String, String), GoCandidateSet<GoPackageMethod<'a>>>,
}

struct GoProjectionIndex<'a> {
    keys_by_file: HashMap<i64, GoPackageKey>,
    domains: HashMap<GoPackageKey, GoPackageDomain<'a>>,
}

enum GoFunctionResolution {
    Exact {
        declaration: NodeId,
        declaration_file: FileId,
        dependencies: Vec<FileId>,
    },
    Missing,
    Ambiguous,
    Incomplete,
    Unsupported,
}

enum GoReceiverResolution {
    Exact {
        owner: NodeId,
        owner_file: FileId,
        declaration: NodeId,
        declaration_file: FileId,
        dependencies: Vec<FileId>,
    },
    Missing,
    Ambiguous,
    Incomplete,
    Unsupported,
}

impl<'a> GoProjectionIndex<'a> {
    fn prepare(records: &'a [ResolutionCacheRecord]) -> Result<Self> {
        let go_records = records
            .iter()
            .filter(|record| record.file.language == "go")
            .collect::<Vec<_>>();
        let mut records_by_directory = HashMap::<WorkspacePathIdentity, Vec<_>>::new();
        let mut records_by_identity = HashMap::<WorkspacePathIdentity, Vec<_>>::new();
        for record in &go_records {
            count_go_resolution_work(1);
            let identity = workspace_path_identity(&record.path)?;
            records_by_identity
                .entry(identity)
                .or_default()
                .push(*record);
            let directory = record
                .path
                .parent()
                .ok_or_else(|| anyhow!("Go proof input has no package directory"))?;
            records_by_directory
                .entry(workspace_path_identity(directory)?)
                .or_default()
                .push(*record);
        }
        let mut directory_inventory_complete = HashMap::new();
        for (identity, directory_records) in &records_by_directory {
            count_go_resolution_work(1);
            let Some(directory) = directory_records
                .first()
                .and_then(|record| record.path.parent())
            else {
                directory_inventory_complete.insert(identity.clone(), false);
                continue;
            };
            let mut complete = true;
            let entries = match std::fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(_) => {
                    directory_inventory_complete.insert(identity.clone(), false);
                    continue;
                }
            };
            for entry in entries {
                count_go_resolution_work(1);
                let Ok(entry) = entry else {
                    complete = false;
                    continue;
                };
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("go") {
                    continue;
                }
                let Ok(file_type) = entry.file_type() else {
                    complete = false;
                    continue;
                };
                if !file_type.is_file() {
                    complete = false;
                    continue;
                }
                let Ok(file_identity) = workspace_path_identity(&path) else {
                    complete = false;
                    continue;
                };
                if records_by_identity.get(&file_identity).map(Vec::len) != Some(1) {
                    complete = false;
                }
            }
            directory_inventory_complete.insert(identity.clone(), complete);
        }
        let mut keys_by_file = HashMap::new();
        let mut grouped = HashMap::<GoPackageKey, Vec<&ResolutionCacheRecord>>::new();
        for record in go_records {
            count_go_resolution_work(1);
            let Some(package) = &record.file.go_package else {
                continue;
            };
            if package.name.is_empty() {
                continue;
            }
            let directory = workspace_path_identity(
                record
                    .path
                    .parent()
                    .ok_or_else(|| anyhow!("Go proof input has no package directory"))?,
            )?;
            let key = GoPackageKey {
                directory,
                package_name: package.name.clone(),
            };
            keys_by_file.insert(record.file.file_id.0, key.clone());
            grouped.entry(key).or_default().push(record);
        }
        let package_names_by_directory = grouped.keys().fold(
            HashMap::<WorkspacePathIdentity, Vec<String>>::new(),
            |mut names, key| {
                names
                    .entry(key.directory.clone())
                    .or_default()
                    .push(key.package_name.clone());
                names
            },
        );
        let mut domains = HashMap::new();
        for (key, mut package_records) in grouped {
            count_go_resolution_work(1);
            package_records.sort_by_key(|record| record.file.file_id);
            let names = package_names_by_directory
                .get(&key.directory)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let allowed_test_split = names.iter().all(|name| {
                name == &key.package_name
                    || name == &format!("{}_test", key.package_name)
                    || key.package_name == format!("{name}_test")
            });
            let mut complete = directory_inventory_complete
                .get(&key.directory)
                .copied()
                .unwrap_or(false)
                && allowed_test_split;
            let mut functions = HashMap::<String, GoCandidateSet<GoPackageDeclaration<'_>>>::new();
            let mut blockers = HashMap::<String, usize>::new();
            let mut conditional_blockers = HashSet::<String>::new();
            let mut types = HashMap::<String, GoCandidateSet<GoPackageType<'_>>>::new();
            let mut methods =
                HashMap::<(String, String), GoCandidateSet<GoPackageMethod<'_>>>::new();
            for record in &package_records {
                count_go_resolution_work(1);
                if go_test_file(&record.path) {
                    continue;
                }
                let Some(package) = &record.file.go_package else {
                    complete = false;
                    continue;
                };
                let conditional = go_file_is_conditional(&record.path, package);
                complete &= record.file.complete && record.file.lookup_input_complete;
                for declaration in &record.file.top_level_declarations {
                    count_go_resolution_work(1);
                    functions.entry(declaration.name.clone()).or_default().add(
                        GoPackageDeclaration {
                            declaration: declaration.declaration,
                            record,
                        },
                        conditional,
                        package.generated,
                    );
                }
                for blocker in &package.package_blockers {
                    count_go_resolution_work(1);
                    *blockers.entry(blocker.clone()).or_default() += 1;
                    if conditional {
                        conditional_blockers.insert(blocker.clone());
                    }
                }
                for value in &package.types {
                    count_go_resolution_work(1);
                    types.entry(value.name.clone()).or_default().add(
                        GoPackageType { value, record },
                        conditional,
                        package.generated,
                    );
                }
                for value in &package.methods {
                    count_go_resolution_work(1);
                    methods
                        .entry((value.owner_name.clone(), value.method_name.clone()))
                        .or_default()
                        .add(
                            GoPackageMethod { value, record },
                            conditional,
                            package.generated,
                        );
                }
            }
            let dependencies = package_records
                .iter()
                .filter(|record| !go_test_file(&record.path))
                .map(|record| {
                    count_go_resolution_work(1);
                    FileId(record.file.file_id.0)
                })
                .collect();
            domains.insert(
                key,
                GoPackageDomain {
                    dependencies,
                    complete,
                    functions,
                    blockers,
                    conditional_blockers,
                    types,
                    methods,
                },
            );
        }
        Ok(Self {
            keys_by_file,
            domains,
        })
    }

    fn domain(
        &self,
        source_record: &ResolutionCacheRecord,
        package_name: &str,
    ) -> Option<&GoPackageDomain<'a>> {
        count_go_resolution_work(1);
        let key = self.keys_by_file.get(&source_record.file.file_id.0)?;
        (key.package_name == package_name)
            .then(|| self.domains.get(key))
            .flatten()
    }

    fn resolve_function(
        &self,
        source_record: &ResolutionCacheRecord,
        package_name: &str,
        name: &str,
    ) -> GoFunctionResolution {
        let Some(domain) = self.domain(source_record, package_name) else {
            return GoFunctionResolution::Incomplete;
        };
        if !domain.complete
            || go_test_file(&source_record.path)
            || source_record
                .file
                .go_package
                .as_ref()
                .is_none_or(|package| go_file_is_conditional(&source_record.path, package))
        {
            return GoFunctionResolution::Incomplete;
        }
        if domain.conditional_blockers.contains(name) {
            return GoFunctionResolution::Incomplete;
        }
        if domain.blockers.get(name).copied().unwrap_or_default() > 0 {
            return GoFunctionResolution::Ambiguous;
        }
        if let Some(types) = domain.types.get(name) {
            if types.conditional {
                return GoFunctionResolution::Incomplete;
            }
            return if types.ambiguous || types.exact.is_some() {
                GoFunctionResolution::Ambiguous
            } else {
                GoFunctionResolution::Unsupported
            };
        }
        let Some(declarations) = domain.functions.get(name) else {
            return GoFunctionResolution::Missing;
        };
        count_go_resolution_work(1);
        if declarations.conditional {
            return GoFunctionResolution::Incomplete;
        }
        if declarations.ambiguous {
            return GoFunctionResolution::Ambiguous;
        }
        match declarations.exact.as_ref() {
            Some(declaration) => GoFunctionResolution::Exact {
                declaration: declaration.declaration,
                declaration_file: FileId(declaration.record.file.file_id.0),
                dependencies: go_domain_dependencies(domain),
            },
            None if declarations.generated => GoFunctionResolution::Unsupported,
            None => GoFunctionResolution::Missing,
        }
    }

    fn resolve_receiver(
        &self,
        source_record: &ResolutionCacheRecord,
        package_name: &str,
        owner_name: &str,
        method_name: &str,
        receiver_is_pointer: bool,
        constructor_uses_builtin_new: bool,
    ) -> GoReceiverResolution {
        let Some(domain) = self.domain(source_record, package_name) else {
            return GoReceiverResolution::Incomplete;
        };
        if !domain.complete
            || go_test_file(&source_record.path)
            || source_record
                .file
                .go_package
                .as_ref()
                .is_none_or(|package| go_file_is_conditional(&source_record.path, package))
        {
            return GoReceiverResolution::Incomplete;
        }
        if constructor_uses_builtin_new {
            let new_functions = domain.functions.get("new");
            let new_types = domain.types.get("new");
            if domain.conditional_blockers.contains("new")
                || new_functions.is_some_and(|declarations| declarations.conditional)
                || new_types.is_some_and(|declarations| declarations.conditional)
            {
                return GoReceiverResolution::Incomplete;
            }
            if domain.blockers.get("new").copied().unwrap_or_default() > 0
                || new_functions.is_some()
                || new_types.is_some()
            {
                return GoReceiverResolution::Unsupported;
            }
        }
        if domain.conditional_blockers.contains(owner_name) {
            return GoReceiverResolution::Incomplete;
        }
        if domain.blockers.get(owner_name).copied().unwrap_or_default() > 0 {
            return GoReceiverResolution::Ambiguous;
        }
        let owners = domain.types.get(owner_name);
        let methods = domain
            .methods
            .get(&(owner_name.to_string(), method_name.to_string()));
        count_go_resolution_work(2);
        if owners.is_some_and(|owners| owners.conditional)
            || methods.is_some_and(|methods| methods.conditional)
        {
            return GoReceiverResolution::Incomplete;
        }
        if owners.is_some_and(|owners| owners.ambiguous)
            || methods.is_some_and(|methods| methods.ambiguous)
        {
            return GoReceiverResolution::Ambiguous;
        }
        let (Some(owner), Some(method)) = (
            owners.and_then(|owners| owners.exact.as_ref()),
            methods.and_then(|methods| methods.exact.as_ref()),
        ) else {
            if owners.is_some_and(|owners| owners.generated)
                || methods.is_some_and(|methods| methods.generated)
            {
                return GoReceiverResolution::Unsupported;
            }
            return GoReceiverResolution::Missing;
        };
        if owner.value.interface || owner.value.generic {
            return GoReceiverResolution::Unsupported;
        }
        if !receiver_is_pointer && method.value.pointer_receiver {
            return GoReceiverResolution::Unsupported;
        }
        GoReceiverResolution::Exact {
            owner: owner.value.declaration,
            owner_file: FileId(owner.record.file.file_id.0),
            declaration: method.value.declaration,
            declaration_file: FileId(method.record.file.file_id.0),
            dependencies: go_domain_dependencies(domain),
        }
    }
}

fn go_test_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("_test.go"))
}

fn go_file_is_conditional(path: &Path, package: &CachedGoPackage) -> bool {
    package.build_constrained || go_filename_has_build_constraint(path)
}

fn go_filename_has_build_constraint(path: &Path) -> bool {
    const GOOS: &[&str] = &[
        "aix",
        "android",
        "darwin",
        "dragonfly",
        "freebsd",
        "illumos",
        "ios",
        "js",
        "linux",
        "netbsd",
        "openbsd",
        "plan9",
        "solaris",
        "wasip1",
        "windows",
    ];
    const GOARCH: &[&str] = &[
        "386", "amd64", "arm", "arm64", "loong64", "mips", "mips64", "mips64le", "mipsle", "ppc64",
        "ppc64le", "riscv64", "s390x", "wasm",
    ];
    let Some(mut stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return true;
    };
    if stem.starts_with('.') || stem.starts_with('_') {
        return true;
    }
    if let Some(without_test) = stem.strip_suffix("_test") {
        stem = without_test;
    }
    let components = stem.split('_').collect::<Vec<_>>();
    let Some(last) = components.last().copied() else {
        return false;
    };
    GOOS.contains(&last)
        || GOARCH.contains(&last)
        || components.len() >= 2
            && GOOS.contains(&components[components.len() - 2])
            && GOARCH.contains(&last)
}

fn go_domain_dependencies(domain: &GoPackageDomain<'_>) -> Vec<FileId> {
    count_go_resolution_work(1);
    domain.dependencies.clone()
}

fn resolve_syntax_claim(
    files: &HashMap<i64, &codestory_store::FileInfo>,
    records: &HashMap<WorkspacePathIdentity, &ResolutionCacheRecord>,
    rust_index: &RustProjectionIndex<'_>,
    go_index: &GoProjectionIndex<'_>,
    python_index: &PythonProjectionIndex,
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
    let mut exact_dependency_files = vec![input.callsite.file_id];
    match &input.binding {
        CachedResolutionBinding::SameFile { declaration } => {
            let declaration_is_recorded = if source_record.file.language == "python" {
                python_index
                    .declarations(input.callsite.file_id, &input.callsite.raw_target)
                    .iter()
                    .any(|candidate| candidate == declaration)
            } else {
                source_record
                    .file
                    .top_level_declarations
                    .iter()
                    .any(|binding| {
                        binding.name == input.callsite.raw_target
                            && binding.declaration == *declaration
                    })
            };
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
            let matching_method_count = if source_record.file.language == "rust" {
                source_record
                    .file
                    .inherent_methods
                    .iter()
                    .filter(|method| {
                        method.owner_name == *owner_name
                            && method.method_name == input.callsite.raw_target
                            && method.declaration == *declaration
                    })
                    .count()
            } else if source_record.file.language == "python" {
                python_index
                    .methods(input.callsite.file_id, *owner, &input.callsite.raw_target)
                    .iter()
                    .filter(|method| **method == *declaration)
                    .count()
            } else {
                source_record
                    .file
                    .classes
                    .iter()
                    .filter(|class| class.name == *owner_name && class.declaration == *owner)
                    .flat_map(|class| class.methods.iter())
                    .filter(|method| {
                        method.name == input.callsite.raw_target
                            && method.declaration == *declaration
                    })
                    .count()
            };
            if matching_method_count != 1 {
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
            if source_record.file.language == "python" {
                let resolution = resolve_python_relative_import(
                    source_record,
                    module_specifier,
                    records,
                    python_index,
                )?;
                let target_record = match resolution.target {
                    RelativeImportResolution::Unique(record) => Some(record),
                    RelativeImportResolution::Missing | RelativeImportResolution::Incomplete => {
                        None
                    }
                };
                let target_domain_unsupported = target_record.is_some_and(|record| {
                    record.file.export_poison_all
                        || record
                            .file
                            .poisoned_export_names
                            .iter()
                            .any(|name| name == imported_name)
                });
                let declarations = target_record
                    .filter(|record| {
                        record.file.lookup_input_complete && !target_domain_unsupported
                    })
                    .map(|record| {
                        python_index.declarations(FileId(record.file.file_id.0), imported_name)
                    })
                    .unwrap_or_default();
                if target_domain_unsupported {
                    status = ProofResolutionStatus::Unsupported;
                    reason = ProofResolutionReason::UnsupportedConstruct;
                } else if let [declaration] = declarations {
                    let target_file_id = FileId(
                        target_record
                            .expect("one Python declaration requires a target record")
                            .file
                            .file_id
                            .0,
                    );
                    status = ProofResolutionStatus::Exact;
                    reason = ProofResolutionReason::ExactResolution;
                    target = Some(*declaration);
                    evidence_chain.push(ResolutionEvidence::StaticImportBinding {
                        import: *import,
                        declaration: *declaration,
                    });
                    evidence_chain.push(ResolutionEvidence::QualifiedPath {
                        components: vec![*import, *declaration],
                    });
                    exact_node_file_expectations.push((*import, input.callsite.file_id));
                    exact_node_file_expectations.push((*declaration, target_file_id));
                    exact_dependency_files = resolution.dependencies;
                } else if matches!(resolution.target, RelativeImportResolution::Incomplete)
                    || target_record.is_some_and(|record| !record.file.lookup_input_complete)
                {
                    status = ProofResolutionStatus::IncompleteDomain;
                    reason = ProofResolutionReason::LookupDomainIncomplete;
                } else if declarations.len() > 1 {
                    status = ProofResolutionStatus::Ambiguous;
                    reason = ProofResolutionReason::MultipleBindings;
                } else {
                    status = ProofResolutionStatus::MissingBinding;
                    reason = ProofResolutionReason::MissingBinding;
                }
            } else {
                let target_resolution =
                    resolve_relative_import(source_record, module_specifier, records)?;
                let target_record = match target_resolution {
                    RelativeImportResolution::Unique(record) => Some(record),
                    RelativeImportResolution::Missing | RelativeImportResolution::Incomplete => {
                        None
                    }
                };
                let target_domain_poisoned = target_record.is_some_and(|record| {
                    record.file.export_poison_all
                        || record
                            .file
                            .poisoned_export_names
                            .iter()
                            .any(|name| name == imported_name)
                });
                let declarations = target_record
                    .filter(|record| record.file.lookup_input_complete && !target_domain_poisoned)
                    .into_iter()
                    .flat_map(|record| record.file.direct_exports.iter())
                    .filter(|export| {
                        export.is_default == *is_default
                            && export.exported_name == *imported_name
                            && export.declaration_kind == CachedDeclarationKind::Callable
                    })
                    .collect::<Vec<_>>();
                if target_domain_poisoned {
                    status = ProofResolutionStatus::IncompleteDomain;
                    reason = ProofResolutionReason::LookupDomainIncomplete;
                } else if let [declaration] = declarations.as_slice() {
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
                } else if matches!(target_resolution, RelativeImportResolution::Incomplete)
                    || target_record.is_some_and(|record| !record.file.lookup_input_complete)
                {
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
        }
        CachedResolutionBinding::RustPath {
            module_path,
            components,
            import,
            associated_owner,
        } => {
            let resolution = resolve_rust_path_binding(
                rust_index,
                source_record,
                module_path,
                components,
                import.as_ref(),
                *associated_owner,
                &input.callsite.raw_target,
            );
            match resolution {
                RustPathResolution::Function {
                    target: declaration,
                    target_file,
                    path_components,
                } => {
                    status = ProofResolutionStatus::Exact;
                    reason = ProofResolutionReason::ExactResolution;
                    target = Some(declaration);
                    if let Some(import) = import {
                        evidence_chain.push(ResolutionEvidence::StaticImportBinding {
                            import: import.import,
                            declaration,
                        });
                        exact_node_file_expectations.push((import.import, input.callsite.file_id));
                        let carries_intermediate_file = path_components.iter().any(|component| {
                            rust_index.node_file(*component).is_some_and(|file_id| {
                                file_id != input.callsite.file_id.0 && file_id != target_file.0
                            })
                        });
                        if carries_intermediate_file {
                            let mut components = path_components;
                            let mut path_complete = true;
                            for component in &components {
                                let Some(file_id) = rust_index.node_file(*component) else {
                                    path_complete = false;
                                    break;
                                };
                                exact_node_file_expectations.push((*component, FileId(file_id)));
                            }
                            if path_complete {
                                components.push(declaration);
                                evidence_chain
                                    .push(ResolutionEvidence::QualifiedPath { components });
                            } else {
                                status = ProofResolutionStatus::IncompleteDomain;
                                reason = ProofResolutionReason::LookupDomainIncomplete;
                                target = None;
                                evidence_chain.clear();
                            }
                        }
                    } else if !path_components.is_empty() {
                        let mut components = path_components;
                        for component in &components {
                            if let Some(file_id) = rust_index.node_file(*component) {
                                exact_node_file_expectations.push((*component, FileId(file_id)));
                            }
                        }
                        components.push(declaration);
                        evidence_chain.push(ResolutionEvidence::QualifiedPath { components });
                    } else if target_file == input.callsite.file_id {
                        evidence_chain
                            .push(ResolutionEvidence::SameFileDeclaration { declaration });
                    } else {
                        status = ProofResolutionStatus::IncompleteDomain;
                        reason = ProofResolutionReason::LookupDomainIncomplete;
                        target = None;
                    }
                    if status == ProofResolutionStatus::Exact {
                        exact_node_file_expectations.push((declaration, target_file));
                    }
                }
                RustPathResolution::Associated {
                    owner,
                    owner_file,
                    target: declaration,
                    target_file,
                } => {
                    status = ProofResolutionStatus::Exact;
                    reason = ProofResolutionReason::ExactResolution;
                    target = Some(declaration);
                    if let Some(import) = import {
                        evidence_chain.push(ResolutionEvidence::StaticImportBinding {
                            import: import.import,
                            declaration: owner,
                        });
                        exact_node_file_expectations.push((import.import, input.callsite.file_id));
                    }
                    evidence_chain.push(ResolutionEvidence::QualifiedPath {
                        components: vec![owner, declaration],
                    });
                    exact_node_file_expectations.push((owner, owner_file));
                    exact_node_file_expectations.push((declaration, target_file));
                }
                RustPathResolution::Missing => {
                    status = ProofResolutionStatus::MissingBinding;
                    reason = ProofResolutionReason::MissingBinding;
                }
                RustPathResolution::Ambiguous => {
                    status = ProofResolutionStatus::Ambiguous;
                    reason = ProofResolutionReason::MultipleBindings;
                }
                RustPathResolution::Incomplete => {
                    status = ProofResolutionStatus::IncompleteDomain;
                    reason = ProofResolutionReason::LookupDomainIncomplete;
                }
                RustPathResolution::Unsupported => {
                    status = ProofResolutionStatus::Unsupported;
                    reason = ProofResolutionReason::UnsupportedConstruct;
                }
            }
        }
        CachedResolutionBinding::RustImplicitReceiver {
            module_path,
            owner_name,
            import,
            declaration,
        } => match resolve_rust_imported_implicit_receiver(
            rust_index,
            source_record,
            module_path,
            owner_name,
            import,
            *declaration,
            &input.callsite.raw_target,
        ) {
            RustImplicitReceiverResolution::Exact {
                owner,
                owner_file,
                declaration,
            } => {
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(declaration);
                evidence_chain.push(ResolutionEvidence::StaticImportBinding {
                    import: import.import,
                    declaration: owner,
                });
                evidence_chain.push(ResolutionEvidence::ImplicitReceiver { owner });
                evidence_chain.push(ResolutionEvidence::SameFileDeclaration { declaration });
                exact_node_file_expectations.push((import.import, input.callsite.file_id));
                exact_node_file_expectations.push((owner, owner_file));
                exact_node_file_expectations.push((declaration, input.callsite.file_id));
            }
            RustImplicitReceiverResolution::Missing => {
                status = ProofResolutionStatus::MissingBinding;
                reason = ProofResolutionReason::MissingBinding;
            }
            RustImplicitReceiverResolution::Ambiguous => {
                status = ProofResolutionStatus::Ambiguous;
                reason = ProofResolutionReason::MultipleBindings;
            }
            RustImplicitReceiverResolution::Incomplete => {
                status = ProofResolutionStatus::IncompleteDomain;
                reason = ProofResolutionReason::LookupDomainIncomplete;
            }
            RustImplicitReceiverResolution::Unsupported => {
                status = ProofResolutionStatus::Unsupported;
                reason = ProofResolutionReason::UnsupportedConstruct;
            }
        },
        CachedResolutionBinding::RustExplicitReceiver {
            module_path,
            owner_name,
            import,
            constructor,
            constructor_record,
            constructor_method,
        } => {
            match resolve_rust_receiver_binding(
                rust_index,
                source_record,
                RustReceiverQuery {
                    module_path,
                    owner_name,
                    import: import.as_ref(),
                    method_name: &input.callsite.raw_target,
                    constructor: *constructor,
                    constructor_record: *constructor_record,
                    constructor_method: constructor_method.as_deref(),
                },
            ) {
                RustReceiverResolution::Exact {
                    owner,
                    owner_file,
                    declaration,
                    declaration_file,
                } => {
                    status = ProofResolutionStatus::Exact;
                    reason = ProofResolutionReason::ExactResolution;
                    target = Some(declaration);
                    if let Some(import) = import {
                        evidence_chain.push(ResolutionEvidence::StaticImportBinding {
                            import: import.import,
                            declaration: owner,
                        });
                        exact_node_file_expectations.push((import.import, input.callsite.file_id));
                    }
                    if *constructor {
                        evidence_chain
                            .push(ResolutionEvidence::ConstructorBinding { constructor: owner });
                    }
                    evidence_chain.push(ResolutionEvidence::ExplicitReceiverType {
                        receiver_type: owner,
                    });
                    if declaration_file == input.callsite.file_id {
                        evidence_chain
                            .push(ResolutionEvidence::SameFileDeclaration { declaration });
                    } else {
                        evidence_chain.push(ResolutionEvidence::QualifiedPath {
                            components: vec![owner, declaration],
                        });
                    }
                    exact_node_file_expectations.push((owner, owner_file));
                    exact_node_file_expectations.push((declaration, declaration_file));
                }
                RustReceiverResolution::Missing => {
                    status = ProofResolutionStatus::MissingBinding;
                    reason = ProofResolutionReason::MissingBinding;
                }
                RustReceiverResolution::Ambiguous => {
                    status = ProofResolutionStatus::Ambiguous;
                    reason = ProofResolutionReason::MultipleBindings;
                }
                RustReceiverResolution::Incomplete => {
                    status = ProofResolutionStatus::IncompleteDomain;
                    reason = ProofResolutionReason::LookupDomainIncomplete;
                }
                RustReceiverResolution::Unsupported => {
                    status = ProofResolutionStatus::Unsupported;
                    reason = ProofResolutionReason::UnsupportedConstruct;
                }
            }
        }
        CachedResolutionBinding::GoPackageFunction { package_name, name } => {
            match go_index.resolve_function(source_record, package_name, name) {
                GoFunctionResolution::Exact {
                    declaration,
                    declaration_file,
                    dependencies,
                } => {
                    status = ProofResolutionStatus::Exact;
                    reason = ProofResolutionReason::ExactResolution;
                    target = Some(declaration);
                    if declaration_file == input.callsite.file_id {
                        evidence_chain
                            .push(ResolutionEvidence::SameFileDeclaration { declaration });
                    } else {
                        evidence_chain
                            .push(ResolutionEvidence::SamePackageDeclaration { declaration });
                    }
                    exact_node_file_expectations.push((declaration, declaration_file));
                    exact_dependency_files = dependencies;
                }
                GoFunctionResolution::Missing => {
                    status = ProofResolutionStatus::MissingBinding;
                    reason = ProofResolutionReason::MissingBinding;
                }
                GoFunctionResolution::Ambiguous => {
                    status = ProofResolutionStatus::Ambiguous;
                    reason = ProofResolutionReason::MultipleBindings;
                }
                GoFunctionResolution::Incomplete => {
                    status = ProofResolutionStatus::IncompleteDomain;
                    reason = ProofResolutionReason::LookupDomainIncomplete;
                }
                GoFunctionResolution::Unsupported => {
                    status = ProofResolutionStatus::Unsupported;
                    reason = ProofResolutionReason::UnsupportedConstruct;
                }
            }
        }
        CachedResolutionBinding::GoImplicitReceiver {
            package_name,
            owner_name,
            receiver_is_pointer,
        } => {
            match go_index.resolve_receiver(
                source_record,
                package_name,
                owner_name,
                &input.callsite.raw_target,
                *receiver_is_pointer,
                false,
            ) {
                GoReceiverResolution::Exact {
                    owner,
                    owner_file,
                    declaration,
                    declaration_file,
                    dependencies,
                } => {
                    status = ProofResolutionStatus::Exact;
                    reason = ProofResolutionReason::ExactResolution;
                    target = Some(declaration);
                    evidence_chain.push(ResolutionEvidence::ImplicitReceiver { owner });
                    if declaration_file == input.callsite.file_id {
                        evidence_chain
                            .push(ResolutionEvidence::SameFileDeclaration { declaration });
                    } else {
                        evidence_chain
                            .push(ResolutionEvidence::SamePackageDeclaration { declaration });
                    }
                    exact_node_file_expectations.push((owner, owner_file));
                    exact_node_file_expectations.push((declaration, declaration_file));
                    exact_dependency_files = dependencies;
                }
                GoReceiverResolution::Missing => {
                    status = ProofResolutionStatus::MissingBinding;
                    reason = ProofResolutionReason::MissingBinding;
                }
                GoReceiverResolution::Ambiguous => {
                    status = ProofResolutionStatus::Ambiguous;
                    reason = ProofResolutionReason::MultipleBindings;
                }
                GoReceiverResolution::Incomplete => {
                    status = ProofResolutionStatus::IncompleteDomain;
                    reason = ProofResolutionReason::LookupDomainIncomplete;
                }
                GoReceiverResolution::Unsupported => {
                    status = ProofResolutionStatus::Unsupported;
                    reason = ProofResolutionReason::UnsupportedConstruct;
                }
            }
        }
        CachedResolutionBinding::GoExplicitReceiver {
            package_name,
            owner_name,
            receiver_is_pointer,
            constructor,
            constructor_uses_builtin_new,
        } => {
            match go_index.resolve_receiver(
                source_record,
                package_name,
                owner_name,
                &input.callsite.raw_target,
                *receiver_is_pointer,
                *constructor_uses_builtin_new,
            ) {
                GoReceiverResolution::Exact {
                    owner,
                    owner_file,
                    declaration,
                    declaration_file,
                    dependencies,
                } => {
                    status = ProofResolutionStatus::Exact;
                    reason = ProofResolutionReason::ExactResolution;
                    target = Some(declaration);
                    if *constructor {
                        evidence_chain
                            .push(ResolutionEvidence::ConstructorBinding { constructor: owner });
                    }
                    evidence_chain.push(ResolutionEvidence::ExplicitReceiverType {
                        receiver_type: owner,
                    });
                    if declaration_file == input.callsite.file_id {
                        evidence_chain
                            .push(ResolutionEvidence::SameFileDeclaration { declaration });
                    } else {
                        evidence_chain
                            .push(ResolutionEvidence::SamePackageDeclaration { declaration });
                    }
                    exact_node_file_expectations.push((owner, owner_file));
                    exact_node_file_expectations.push((declaration, declaration_file));
                    exact_dependency_files = dependencies;
                }
                GoReceiverResolution::Missing => {
                    status = ProofResolutionStatus::MissingBinding;
                    reason = ProofResolutionReason::MissingBinding;
                }
                GoReceiverResolution::Ambiguous => {
                    status = ProofResolutionStatus::Ambiguous;
                    reason = ProofResolutionReason::MultipleBindings;
                }
                GoReceiverResolution::Incomplete => {
                    status = ProofResolutionStatus::IncompleteDomain;
                    reason = ProofResolutionReason::LookupDomainIncomplete;
                }
                GoReceiverResolution::Unsupported => {
                    status = ProofResolutionStatus::Unsupported;
                    reason = ProofResolutionReason::UnsupportedConstruct;
                }
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
        CachedResolutionBinding::ConstructorBinding {
            class_binding,
            method_name,
        }
        | CachedResolutionBinding::ExplicitReceiverType {
            class_binding,
            method_name,
        } => {
            let constructor_binding = matches!(
                &input.binding,
                CachedResolutionBinding::ConstructorBinding { .. }
            );
            let (target_record, import, owner) = match class_binding {
                CachedClassBinding::SameFile { owner, owner_name } => {
                    let match_count = if source_record.file.language == "python" {
                        python_index
                            .classes(input.callsite.file_id, owner_name)
                            .iter()
                            .filter(|candidate| **candidate == *owner)
                            .count()
                    } else {
                        source_record
                            .file
                            .classes
                            .iter()
                            .filter(|class| {
                                class.declaration == *owner && class.name == *owner_name
                            })
                            .count()
                    };
                    if match_count != 1 {
                        status = ProofResolutionStatus::Ambiguous;
                        reason = ProofResolutionReason::MultipleBindings;
                        return Ok(ResolvedSyntaxClaim {
                            input,
                            caller,
                            target,
                            status,
                            reason,
                            evidence_chain,
                            exact_node_file_expectations,
                            exact_dependency_files,
                        });
                    }
                    (source_record, None, *owner)
                }
                CachedClassBinding::StaticImport {
                    import,
                    module_specifier,
                    imported_name,
                    is_default,
                } => {
                    let python_resolution = (source_record.file.language == "python")
                        .then(|| {
                            resolve_python_relative_import(
                                source_record,
                                module_specifier,
                                records,
                                python_index,
                            )
                        })
                        .transpose()?;
                    let target_resolution = if let Some(resolution) = &python_resolution {
                        resolution.target
                    } else {
                        resolve_relative_import(source_record, module_specifier, records)?
                    };
                    let target_record = match target_resolution {
                        RelativeImportResolution::Unique(record) => record,
                        RelativeImportResolution::Missing => {
                            status = ProofResolutionStatus::MissingBinding;
                            reason = ProofResolutionReason::MissingBinding;
                            return Ok(ResolvedSyntaxClaim {
                                input,
                                caller,
                                target,
                                status,
                                reason,
                                evidence_chain,
                                exact_node_file_expectations,
                                exact_dependency_files,
                            });
                        }
                        RelativeImportResolution::Incomplete => {
                            status = ProofResolutionStatus::IncompleteDomain;
                            reason = ProofResolutionReason::LookupDomainIncomplete;
                            return Ok(ResolvedSyntaxClaim {
                                input,
                                caller,
                                target,
                                status,
                                reason,
                                evidence_chain,
                                exact_node_file_expectations,
                                exact_dependency_files,
                            });
                        }
                    };
                    let target_domain_poisoned = target_record.file.export_poison_all
                        || target_record
                            .file
                            .poisoned_export_names
                            .iter()
                            .any(|name| name == imported_name)
                        || !target_record.file.lookup_input_complete;
                    if target_domain_poisoned {
                        if source_record.file.language == "python"
                            && target_record.file.lookup_input_complete
                        {
                            status = ProofResolutionStatus::Unsupported;
                            reason = ProofResolutionReason::UnsupportedConstruct;
                        } else {
                            status = ProofResolutionStatus::IncompleteDomain;
                            reason = ProofResolutionReason::LookupDomainIncomplete;
                        }
                        return Ok(ResolvedSyntaxClaim {
                            input,
                            caller,
                            target,
                            status,
                            reason,
                            evidence_chain,
                            exact_node_file_expectations,
                            exact_dependency_files,
                        });
                    }
                    let owners = if source_record.file.language == "python" {
                        python_index
                            .classes(FileId(target_record.file.file_id.0), imported_name)
                            .to_vec()
                    } else {
                        target_record
                            .file
                            .direct_exports
                            .iter()
                            .filter(|export| {
                                export.is_default == *is_default
                                    && export.exported_name == *imported_name
                                    && export.declaration_kind == CachedDeclarationKind::Class
                            })
                            .map(|export| export.declaration)
                            .collect::<Vec<_>>()
                    };
                    let [owner] = owners.as_slice() else {
                        status = if owners.is_empty() {
                            ProofResolutionStatus::MissingBinding
                        } else {
                            ProofResolutionStatus::Ambiguous
                        };
                        reason = if owners.is_empty() {
                            ProofResolutionReason::MissingBinding
                        } else {
                            ProofResolutionReason::MultipleBindings
                        };
                        return Ok(ResolvedSyntaxClaim {
                            input,
                            caller,
                            target,
                            status,
                            reason,
                            evidence_chain,
                            exact_node_file_expectations,
                            exact_dependency_files,
                        });
                    };
                    if let Some(resolution) = python_resolution {
                        exact_dependency_files = resolution.dependencies;
                    }
                    (target_record, Some(*import), *owner)
                }
            };
            let python_methods = (source_record.file.language == "python").then(|| {
                python_index.methods(FileId(target_record.file.file_id.0), owner, method_name)
            });
            let methods = target_record
                .file
                .classes
                .iter()
                .filter(|class| class.declaration == owner)
                .flat_map(|class| class.methods.iter())
                .filter(|method| method.name == *method_name)
                .collect::<Vec<_>>();
            let method_declarations = python_methods
                .map(|methods| methods.to_vec())
                .unwrap_or_else(|| methods.iter().map(|method| method.declaration).collect());
            if let [method] = method_declarations.as_slice() {
                let target_file_id = FileId(target_record.file.file_id.0);
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(*method);
                if let Some(import) = import {
                    evidence_chain.push(ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: owner,
                    });
                    exact_node_file_expectations.push((import, input.callsite.file_id));
                }
                if constructor_binding {
                    evidence_chain
                        .push(ResolutionEvidence::ConstructorBinding { constructor: owner });
                }
                evidence_chain.push(ResolutionEvidence::ExplicitReceiverType {
                    receiver_type: owner,
                });
                if import.is_some() && source_record.file.language == "python" {
                    evidence_chain.push(ResolutionEvidence::QualifiedPath {
                        components: vec![owner, *method],
                    });
                } else {
                    evidence_chain.push(ResolutionEvidence::SameFileDeclaration {
                        declaration: *method,
                    });
                }
                exact_node_file_expectations.push((owner, target_file_id));
                exact_node_file_expectations.push((*method, target_file_id));
            } else {
                status = if method_declarations.is_empty() {
                    ProofResolutionStatus::MissingBinding
                } else {
                    ProofResolutionStatus::Ambiguous
                };
                reason = if method_declarations.is_empty() {
                    ProofResolutionReason::MissingBinding
                } else {
                    ProofResolutionReason::MultipleBindings
                };
            }
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
        exact_dependency_files,
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
        let mut expected_file_ids = claim
            .exact_dependency_files
            .iter()
            .map(|file| file.0)
            .collect::<HashSet<_>>();
        expected_file_ids.insert(claim.input.callsite.file_id.0);
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
    if status == ProofResolutionStatus::Exact {
        dependency_ids.extend(
            claim
                .exact_dependency_files
                .iter()
                .map(|file| NodeId(file.0)),
        );
    }
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

#[cfg(test)]
mod rust_complexity_tests {
    use super::*;
    use tree_sitter::Parser;

    fn measured_source_work(source: &str) -> usize {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .expect("Rust grammar must load");
        let tree = parser.parse(source, None).expect("source must parse");
        reset_rust_resolution_work();
        let _ = RustResolutionIndex::build(&tree, source, NodeId(1), &[]);
        rust_resolution_work()
    }

    fn repeated_binding_source(count: usize) -> String {
        let mut source =
            String::from("struct Owner; impl Owner { fn target(&self) {} } fn caller() {\n");
        for _ in 0..count {
            source.push_str("{ let value: Owner = Owner; value.target(); }\n");
        }
        source.push_str("}\n");
        source
    }

    fn nested_callable_source(depth: usize) -> String {
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!("fn f{index}() {{\n"));
        }
        source.push_str("target();\n");
        for _ in 0..depth {
            source.push_str("}\n");
        }
        source
    }

    fn measured_projection_work(count: usize) -> usize {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("lib.rs");
        std::fs::write(&path, "").expect("write root");
        let modules = (0..count)
            .map(|index| CachedRustModule {
                module_path: vec![format!("module_{index}")],
                declaration: Some(NodeId(10_000 + index as i64)),
                domain_complete: true,
                value_blockers: Vec::new(),
                incomplete_value_names: Vec::new(),
                file_children: Vec::new(),
            })
            .collect::<Vec<_>>();
        let declarations = (0..count)
            .map(|index| CachedTopLevelDeclaration {
                name: format!("function_{index}"),
                declaration: NodeId(20_000 + index as i64),
                module_path: vec![format!("module_{index}")],
                cross_module_visible: true,
            })
            .collect::<Vec<_>>();
        let types = (0..count)
            .map(|index| CachedRustType {
                module_path: vec![format!("module_{index}")],
                name: format!("Owner{index}"),
                declaration: NodeId(30_000 + index as i64),
                generic: false,
                cross_module_visible: true,
                unit_constructor: true,
                record_constructor: false,
            })
            .collect::<Vec<_>>();
        let methods = (0..count)
            .map(|index| CachedInherentMethod {
                owner_name: format!("Owner{index}"),
                method_name: format!("method_{index}"),
                declaration: NodeId(40_000 + index as i64),
                module_path: vec![format!("module_{index}")],
                owner: Some(NodeId(30_000 + index as i64)),
                has_self: true,
                return_owner: None,
                domain_complete: true,
                cross_module_visible: true,
            })
            .collect::<Vec<_>>();
        let imports = (0..count)
            .map(|index| CachedRustUseBinding {
                module_path: Vec::new(),
                local_name: format!("Owner{index}"),
                components: vec!["crate".to_string(), format!("Owner{index}")],
                import: NodeId(50_000 + index as i64),
            })
            .collect::<Vec<_>>();
        let record = ResolutionCacheRecord {
            path,
            file: CachedResolutionFile {
                file_id: NodeId(1),
                source_sha256: "0".repeat(64),
                language: "rust".to_string(),
                adapter_version: ADAPTER_VERSION.to_string(),
                parser_fingerprint: "parser".to_string(),
                complete: true,
                lookup_input_complete: true,
                typescript_module: false,
                top_level_declarations: declarations,
                inherent_methods: methods,
                classes: Vec::new(),
                direct_exports: Vec::new(),
                export_poison_all: false,
                poisoned_export_names: Vec::new(),
                rust_modules: modules,
                rust_types: types,
                rust_uses: imports,
                go_package: None,
            },
            calls: Vec::new(),
        };
        reset_rust_resolution_work();
        let records = [record];
        let index = RustProjectionIndex::prepare(&records).expect("projection index");
        for item in 0..count {
            let module = vec![format!("module_{item}")];
            let _ = index.module(&records[0], &module);
            let _ = index.declarations(&records[0], &module, &format!("function_{item}"));
            let _ = index.types(&records[0], &module, &format!("Owner{item}"));
            let _ = index.methods(
                &records[0],
                &module,
                &format!("Owner{item}"),
                &format!("method_{item}"),
            );
            let _ = index.node_file(NodeId(50_000 + item as i64));
        }
        rust_resolution_work()
    }

    #[test]
    fn rust_source_resolution_work_is_linear_for_binding_histories_and_nested_callables() {
        let small_bindings = measured_source_work(&repeated_binding_source(64));
        let large_bindings = measured_source_work(&repeated_binding_source(128));
        assert!(
            large_bindings <= small_bindings * 2 + 64,
            "binding work grew superlinearly: {small_bindings} -> {large_bindings}"
        );

        let small_nested = measured_source_work(&nested_callable_source(64));
        let large_nested = measured_source_work(&nested_callable_source(128));
        assert!(
            large_nested <= small_nested * 2 + 64,
            "nested-callable work grew superlinearly: {small_nested} -> {large_nested}"
        );
    }

    #[test]
    fn rust_projection_preparation_and_lookups_are_linear() {
        let small = measured_projection_work(64);
        let large = measured_projection_work(128);
        assert!(
            small >= 64 * 5,
            "projection work was not instrumented: {small}"
        );
        assert!(
            large <= small * 2 + 128,
            "projection work grew superlinearly: {small} -> {large}"
        );
    }
}

#[cfg(test)]
mod go_complexity_tests {
    use super::*;
    use tree_sitter::Parser;

    fn go_source(count: usize) -> String {
        let mut source = String::from(
            "package proof\ntype Worker struct{}\nfunc (w *Worker) Run() {}\nfunc caller() {\n",
        );
        for index in 0..count {
            source.push_str(&format!(
                "  worker{index} := &Worker{{}}\n  worker{index}.Run()\n"
            ));
        }
        source.push_str("}\n");
        source
    }

    fn measured_source_work(count: usize) -> usize {
        let source = go_source(count);
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .expect("Go grammar must load");
        let tree = parser.parse(&source, None).expect("source must parse");
        reset_go_resolution_work();
        let _ = GoResolutionIndex::build(&tree, &source, NodeId(1), &[]);
        go_resolution_work()
    }

    fn measured_package_work(file_count: usize, lookup_count: usize) -> usize {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut records = Vec::new();
        for index in 0..file_count {
            let path = temp.path().join(format!("file_{index}.go"));
            std::fs::write(&path, "package proof\n").expect("write Go source");
            records.push(ResolutionCacheRecord {
                path,
                file: CachedResolutionFile {
                    file_id: NodeId(index as i64 + 1),
                    source_sha256: "0".repeat(64),
                    language: "go".to_string(),
                    adapter_version: ADAPTER_VERSION.to_string(),
                    parser_fingerprint: "parser".to_string(),
                    complete: true,
                    lookup_input_complete: true,
                    typescript_module: false,
                    top_level_declarations: vec![CachedTopLevelDeclaration {
                        name: "Target".to_string(),
                        declaration: NodeId(10_000 + index as i64),
                        module_path: Vec::new(),
                        cross_module_visible: false,
                    }],
                    inherent_methods: Vec::new(),
                    classes: Vec::new(),
                    direct_exports: Vec::new(),
                    export_poison_all: false,
                    poisoned_export_names: Vec::new(),
                    rust_modules: Vec::new(),
                    rust_types: Vec::new(),
                    rust_uses: Vec::new(),
                    go_package: Some(CachedGoPackage {
                        name: "proof".to_string(),
                        build_constrained: false,
                        generated: false,
                        package_blockers: Vec::new(),
                        types: Vec::new(),
                        methods: Vec::new(),
                    }),
                },
                calls: Vec::new(),
            });
        }
        reset_go_resolution_work();
        let index = GoProjectionIndex::prepare(&records).expect("Go package projection");
        for _ in 0..lookup_count {
            let _ = index.resolve_function(&records[0], "proof", "Target");
        }
        go_resolution_work()
    }

    #[test]
    fn go_source_resolution_work_is_linear_for_bindings_and_calls() {
        let small = measured_source_work(64);
        let large = measured_source_work(128);
        assert!(small > 0, "Go source work was not instrumented");
        assert!(
            large <= small * 2 + 128,
            "Go source work grew superlinearly: {small} -> {large}"
        );
    }

    #[test]
    fn go_package_projection_work_is_linear() {
        let small = measured_package_work(64, 64);
        let large = measured_package_work(128, 128);
        assert!(small >= 64, "Go package work was not instrumented: {small}");
        assert!(
            large <= small * 2 + 128,
            "Go package work grew superlinearly: {small} -> {large}"
        );
    }

    #[test]
    fn go_package_files_and_call_lookups_are_independently_counted_and_linear() {
        let baseline = measured_package_work(64, 64);
        assert!(
            baseline >= 64 * 8,
            "Go package dependency preparation/lookups were not fully counted: {baseline}"
        );
        let more_files = measured_package_work(128, 64);
        let more_calls = measured_package_work(64, 128);
        let combined = measured_package_work(128, 128);
        assert!(
            more_files <= baseline * 2 + 128,
            "Go file preparation grew superlinearly: {baseline} -> {more_files}"
        );
        assert!(
            more_calls <= baseline * 2 + 128,
            "Go call lookup work grew superlinearly: {baseline} -> {more_calls}"
        );
        assert!(
            combined <= baseline * 2 + 256,
            "combined Go package/call work grew superlinearly: {baseline} -> {combined}"
        );
    }
}

#[cfg(test)]
mod python_complexity_tests {
    use super::*;
    use tree_sitter::Parser;

    fn python_record(path: PathBuf, file_id: i64) -> ResolutionCacheRecord {
        ResolutionCacheRecord {
            path,
            file: CachedResolutionFile {
                file_id: NodeId(file_id),
                source_sha256: "0".repeat(64),
                language: "python".to_owned(),
                adapter_version: ADAPTER_VERSION.to_owned(),
                parser_fingerprint: "parser".to_owned(),
                complete: true,
                lookup_input_complete: true,
                typescript_module: false,
                top_level_declarations: vec![CachedTopLevelDeclaration {
                    name: "target".to_owned(),
                    declaration: NodeId(10_000 + file_id),
                    module_path: Vec::new(),
                    cross_module_visible: false,
                }],
                inherent_methods: Vec::new(),
                classes: vec![CachedClassDeclaration {
                    name: "Worker".to_owned(),
                    declaration: NodeId(20_000 + file_id),
                    methods: vec![CachedClassMethod {
                        name: "run".to_owned(),
                        declaration: NodeId(30_000 + file_id),
                    }],
                }],
                direct_exports: Vec::new(),
                export_poison_all: false,
                poisoned_export_names: Vec::new(),
                rust_modules: Vec::new(),
                rust_types: Vec::new(),
                rust_uses: Vec::new(),
                go_package: None,
            },
            calls: Vec::new(),
        }
    }

    fn measured_source_work(call_count: usize) -> usize {
        let mut source = String::from(
            "class Worker:\n    def run(self):\n        pass\n\ndef target():\n    pass\n\ndef caller():\n",
        );
        for index in 0..call_count {
            source.push_str(&format!(
                "    worker_{index} = Worker()\n    worker_{index}.run()\n    target()\n"
            ));
        }
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("Python grammar must load");
        let tree = parser.parse(&source, None).expect("source must parse");
        reset_python_resolution_work();
        let _ = PythonResolutionIndex::build(&tree, &source, NodeId(1), &[]);
        python_resolution_work()
    }

    fn measured_projection_work(file_count: usize, lookup_count: usize) -> usize {
        let temp = tempfile::tempdir().expect("Python projection tempdir");
        let package = temp.path().join("proof");
        std::fs::create_dir(&package).expect("create Python package");
        let mut records = Vec::new();
        for index in 0..file_count {
            let name = if index == 0 {
                "__init__.py".to_owned()
            } else {
                format!("module_{index}.py")
            };
            let path = package.join(name);
            std::fs::write(&path, "def target():\n    pass\n").expect("write Python source");
            records.push(python_record(path, index as i64 + 1));
        }
        let source = &records[0];
        let records_by_path = records
            .iter()
            .map(|record| {
                (
                    workspace_path_identity(&record.path).expect("Python file identity"),
                    record,
                )
            })
            .collect::<HashMap<_, _>>();
        reset_python_resolution_work();
        let index = PythonProjectionIndex::prepare(&records, &records_by_path)
            .expect("Python projection index");
        for _ in 0..lookup_count {
            let resolution =
                resolve_python_relative_import(source, ".module_1", &records_by_path, &index)
                    .expect("relative import resolution");
            assert!(matches!(
                resolution.target,
                RelativeImportResolution::Unique(_)
            ));
            let _ = index.declarations(FileId(2), "target");
            let owners = index.classes(FileId(2), "Worker");
            assert_eq!(owners.len(), 1);
            let _ = index.methods(FileId(2), owners[0], "run");
        }
        python_resolution_work()
    }

    #[test]
    fn python_source_index_work_is_counted_and_linear() {
        let small = measured_source_work(64);
        let large = measured_source_work(128);
        assert!(small > 0, "Python source work was not counted");
        assert!(
            large <= small * 2 + 256,
            "Python source work grew superlinearly: {small} -> {large}"
        );
    }

    #[test]
    fn python_package_projection_preparation_and_lookups_are_independently_linear() {
        let baseline = measured_projection_work(32, 32);
        let more_files = measured_projection_work(64, 32);
        let more_calls = measured_projection_work(32, 64);
        let combined = measured_projection_work(64, 64);
        assert!(
            baseline >= 64,
            "Python projection work was not counted: {baseline}"
        );
        assert!(
            more_files <= baseline * 2 + 64,
            "Python package preparation grew superlinearly: {baseline} -> {more_files}"
        );
        assert!(
            more_calls <= baseline * 2 + 64,
            "Python projection lookup grew superlinearly: {baseline} -> {more_calls}"
        );
        assert!(
            combined <= baseline * 2 + 128,
            "combined Python package/lookup work grew superlinearly: {baseline} -> {combined}"
        );
    }

    fn measured_hostile_cache_lookup_work(file_count: usize, lookup_count: usize) -> usize {
        let temp = tempfile::tempdir().expect("hostile cache tempdir");
        let files = (0..file_count)
            .map(|index| {
                let path = temp.path().join(format!("pkg_{index}/module.py"));
                std::fs::create_dir_all(path.parent().expect("cache parent"))
                    .expect("create cache parent");
                std::fs::write(&path, "def target():\n    pass\n").expect("write cache source");
                codestory_store::FileInfo {
                    id: index as i64 + 1,
                    path,
                    language: "python".to_owned(),
                    modification_time: 0,
                    indexed: true,
                    complete: true,
                    line_count: 2,
                    file_role: codestory_store::FileRole::Source,
                }
            })
            .collect::<Vec<_>>();
        let governed = files.iter().collect::<Vec<_>>();
        let identities = files
            .iter()
            .map(|file| {
                (
                    file.id,
                    workspace_path_identity(&file.path).expect("cache identity"),
                )
            })
            .collect::<HashMap<_, _>>();
        reset_python_resolution_work();
        let prepared = PreparedGovernedCachePaths::prepare(&governed, &identities);
        for index in 0..lookup_count {
            assert!(
                prepared
                    .contains(Path::new(&format!("pkg_{}/module.py", index % file_count)))
                    .expect("hostile cache lookup")
            );
        }
        python_resolution_work()
    }

    #[test]
    fn hostile_cache_preparation_and_lookup_work_are_independently_linear() {
        let baseline = measured_hostile_cache_lookup_work(32, 32);
        let more_files = measured_hostile_cache_lookup_work(64, 32);
        let more_lookups = measured_hostile_cache_lookup_work(32, 64);
        let combined = measured_hostile_cache_lookup_work(64, 64);
        assert!(baseline > 0, "hostile cache work was not counted");
        assert!(
            more_files <= baseline * 2 + 64,
            "cache files: {baseline} -> {more_files}"
        );
        assert!(
            more_lookups <= baseline * 2 + 64,
            "cache lookups: {baseline} -> {more_lookups}"
        );
        assert!(
            combined <= baseline * 2 + 128,
            "cache combined: {baseline} -> {combined}"
        );
    }
}
