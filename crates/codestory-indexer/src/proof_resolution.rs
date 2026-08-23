use crate::cache::{
    CachedCallResolutionInput, CachedClassBinding, CachedClassDeclaration, CachedClassMethod,
    CachedDeclarationKind, CachedDirectExport, CachedIndexArtifact, CachedInherentMethod,
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

const ADAPTER_VERSION: &str = "reference-v7";
const RESOLUTION_INPUT_SCHEMA_VERSION: u32 = 5;
const INSTALLED_ADAPTERS: &[(&str, &str)] = &[
    ("javascript", ADAPTER_VERSION),
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
    let javascript_index = is_javascript_language(language)
        .then(|| JavascriptResolutionIndex::build(tree, source, file_id, nodes));
    let (direct_exports, export_poison_all, poisoned_export_names) =
        if let Some(index) = &javascript_index {
            index.collect_direct_exports(source)
        } else {
            (Vec::new(), false, Vec::new())
        };
    if language == "rust" && rust_file_has_item_domain_macro_invocation(tree.root_node()) {
        lookup_input_complete = false;
    }
    if language == "rust" && rust_file_has_attribute_domain(tree.root_node()) {
        lookup_input_complete = false;
    }
    let typescript_module = javascript_index
        .as_ref()
        .is_some_and(|index| index.ecmascript_module);
    let top_level_declarations = if let Some(index) = &javascript_index {
        index.cached_top_level_declarations()
    } else if language == "rust" {
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
    let mut emit_call = |callee: TsNode<'_>, form: CalleeForm, raw_target: String| {
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
        } else if let Some(index) = &javascript_index {
            index.resolve_syntax_claim(source, callee, form, &raw_target)
        } else {
            (None, CachedResolutionBinding::Unsupported)
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
    };
    if let Some(index) = &javascript_index {
        for call in &index.calls {
            emit_call(call.callee, call.form, call.raw_target.clone());
        }
    } else {
        collect_calls(tree.root_node(), source, &mut emit_call);
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
            classes: javascript_index
                .as_ref()
                .map_or_else(Vec::new, |index| index.cached_classes()),
            direct_exports,
            export_poison_all,
            poisoned_export_names,
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
        (matches.len() == 1).then_some(matches[0])
    }

    fn map_class_declaration(&self, declaration: TsNode<'_>, source: &str) -> Option<NodeId> {
        let name = declaration_name(declaration, source)?.to_string();
        let line = declaration.start_position().row as u32 + 1;
        let matches = self.class_nodes.get(&(line, name))?;
        (matches.len() == 1).then_some(matches[0])
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
    (matches.len() == 1).then_some(matches[0])
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
    let name = if declaration.kind() == "arrow_function" {
        crate::js_like_callable_source_name(declaration, source)?
    } else {
        declaration_name(declaration, source)?.to_string()
    };
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

fn top_level_rust_functions(root: TsNode<'_>) -> Vec<TsNode<'_>> {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .filter(|child| child.kind() == "function_item")
        .collect()
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

fn collect_top_level_declarations(
    tree: &Tree,
    source: &str,
    language: &str,
    file_id: NodeId,
    nodes: &[Node],
) -> Option<Vec<CachedTopLevelDeclaration>> {
    debug_assert_eq!(language, "rust");
    let declarations = top_level_rust_functions(tree.root_node());
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
    let exact_evidence_validation = ExactEvidenceValidationIndex::prepare(&edges);
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
                && (raw.kind == NodeKind::METHOD || raw.file_node_id != edge.file_node_id);
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

#[derive(Clone, Copy)]
enum RelativeImportResolution<'a> {
    Unique(&'a ResolutionCacheRecord),
    Missing,
    Incomplete,
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
}

struct ExactEvidenceValidationIndex {
    import_relation_counts: HashMap<(NodeId, NodeId, NodeId), usize>,
    member_relation_counts: HashMap<(NodeId, NodeId), usize>,
}

impl ExactEvidenceValidationIndex {
    fn prepare(edges: &[Edge]) -> Self {
        let mut import_relation_counts = HashMap::new();
        let mut member_relation_counts = HashMap::new();
        for edge in edges {
            if edge.kind == EdgeKind::IMPORT
                && let (Some(file_id), Some(target)) = (edge.file_node_id, edge.resolved_target)
            {
                *import_relation_counts
                    .entry((file_id, edge.source, target))
                    .or_default() += 1;
            }
            if edge.kind == EdgeKind::MEMBER {
                *member_relation_counts
                    .entry((edge.effective_source(), edge.effective_target()))
                    .or_default() += 1;
            }
        }
        Self {
            import_relation_counts,
            member_relation_counts,
        }
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
        let source_file = NodeId(claim.input.callsite.file_id.0);
        match (
            claim.input.callsite.callee_form,
            claim.evidence_chain.as_slice(),
        ) {
            (CalleeForm::Identifier, [ResolutionEvidence::SameFileDeclaration { declaration }]) => {
                *declaration == target && target_node.file_node_id == Some(source_file)
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
                    && self
                        .import_relation_counts
                        .get(&(source_file, *import, target))
                        .copied()
                        == Some(1)
            }
            (
                CalleeForm::ImplicitReceiver,
                [
                    ResolutionEvidence::ImplicitReceiver { owner },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) => {
                *declaration == target
                    && target_node.kind == NodeKind::METHOD
                    && target_node.file_node_id == Some(source_file)
                    && nodes.get(owner).is_some_and(|owner_node| {
                        matches!(owner_node.kind, NodeKind::STRUCT | NodeKind::CLASS)
                            && owner_node.file_node_id == Some(source_file)
                    })
                    && self
                        .member_relation_counts
                        .get(&(*owner, claim.caller))
                        .copied()
                        == Some(1)
                    && self.member_relation_counts.get(&(*owner, target)).copied() == Some(1)
            }
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::ConstructorBinding { constructor },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) if *constructor == *receiver_type => self.local_receiver_is_correlated(
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
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) if *owner == *constructor && *owner == *receiver_type && *declaration == target => {
                self.imported_receiver_is_correlated(
                    source_file,
                    *import,
                    *owner,
                    target,
                    target_node,
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
                    source_file,
                    *import,
                    *owner,
                    target,
                    target_node,
                    nodes,
                ),
            _ => false,
        }
    }

    fn local_receiver_is_correlated(
        &self,
        source_file: NodeId,
        owner: NodeId,
        declaration: NodeId,
        target: NodeId,
        target_node: &Node,
        nodes: &HashMap<NodeId, &Node>,
    ) -> bool {
        declaration == target
            && target_node.kind == NodeKind::METHOD
            && nodes.get(&owner).is_some_and(|owner_node| {
                owner_node.kind == NodeKind::CLASS
                    && owner_node.file_node_id == Some(source_file)
                    && owner_node.file_node_id == target_node.file_node_id
            })
            && self.member_relation_counts.get(&(owner, target)).copied() == Some(1)
    }

    fn imported_receiver_is_correlated(
        &self,
        source_file: NodeId,
        import: NodeId,
        owner: NodeId,
        target: NodeId,
        target_node: &Node,
        nodes: &HashMap<NodeId, &Node>,
    ) -> bool {
        target_node.kind == NodeKind::METHOD
            && nodes
                .get(&import)
                .is_some_and(|import_node| import_node.file_node_id == Some(source_file))
            && nodes.get(&owner).is_some_and(|owner_node| {
                owner_node.kind == NodeKind::CLASS
                    && owner_node.file_node_id == target_node.file_node_id
            })
            && self
                .import_relation_counts
                .get(&(source_file, import, owner))
                .copied()
                == Some(1)
            && self.member_relation_counts.get(&(owner, target)).copied() == Some(1)
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
        if !validation.claim_has_literal_corroboration(claim, nodes) {
            claim.status = ProofResolutionStatus::IncompleteDomain;
            claim.reason = ProofResolutionReason::LookupDomainIncomplete;
            claim.target = None;
            claim.evidence_chain.clear();
        }
    }
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
            let matching_method_count = if source_record.file.language == "rust" {
                all_records
                    .iter()
                    .filter(|record| record.file.language == "rust")
                    .flat_map(|record| record.file.inherent_methods.iter())
                    .filter(|method| {
                        method.owner_name == *owner_name
                            && method.method_name == input.callsite.raw_target
                            && method.declaration == *declaration
                    })
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
            if source_record.file.language == "rust"
                && all_records
                    .iter()
                    .filter(|record| record.file.language == "rust")
                    .any(|record| !record.file.lookup_input_complete)
            {
                status = ProofResolutionStatus::IncompleteDomain;
                reason = ProofResolutionReason::LookupDomainIncomplete;
            } else if matching_method_count != 1 {
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
            let target_resolution =
                resolve_relative_import(source_record, module_specifier, records)?;
            let target_record = match target_resolution {
                RelativeImportResolution::Unique(record) => Some(record),
                RelativeImportResolution::Missing | RelativeImportResolution::Incomplete => None,
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
                    let matches = source_record
                        .file
                        .classes
                        .iter()
                        .filter(|class| class.declaration == *owner && class.name == *owner_name)
                        .collect::<Vec<_>>();
                    if matches.len() != 1 {
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
                    let target_record =
                        match resolve_relative_import(source_record, module_specifier, records)? {
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
                                });
                            }
                        };
                    if target_record.file.export_poison_all
                        || target_record
                            .file
                            .poisoned_export_names
                            .iter()
                            .any(|name| name == imported_name)
                        || !target_record.file.lookup_input_complete
                    {
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
                        });
                    }
                    let owners = target_record
                        .file
                        .direct_exports
                        .iter()
                        .filter(|export| {
                            export.is_default == *is_default
                                && export.exported_name == *imported_name
                                && export.declaration_kind == CachedDeclarationKind::Class
                        })
                        .collect::<Vec<_>>();
                    let [export] = owners.as_slice() else {
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
                        });
                    };
                    (target_record, Some(*import), export.declaration)
                }
            };
            let methods = target_record
                .file
                .classes
                .iter()
                .filter(|class| class.declaration == owner)
                .flat_map(|class| class.methods.iter())
                .filter(|method| method.name == *method_name)
                .collect::<Vec<_>>();
            if let [method] = methods.as_slice() {
                let target_file_id = FileId(target_record.file.file_id.0);
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(method.declaration);
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
                evidence_chain.push(ResolutionEvidence::SameFileDeclaration {
                    declaration: method.declaration,
                });
                exact_node_file_expectations.push((owner, target_file_id));
                exact_node_file_expectations.push((method.declaration, target_file_id));
            } else {
                status = if methods.is_empty() {
                    ProofResolutionStatus::MissingBinding
                } else {
                    ProofResolutionStatus::Ambiguous
                };
                reason = if methods.is_empty() {
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
