use crate::cache::{
    CachedCCppFile, CachedCCppNamespace, CachedCCppSourceRole, CachedCallResolutionInput,
    CachedClassBinding, CachedClassDeclaration, CachedClassMethod, CachedDeclarationKind,
    CachedDirectExport, CachedGoMethod, CachedGoPackage, CachedGoType, CachedIndexArtifact,
    CachedInherentMethod, CachedPhpNamespace, CachedResolutionBinding, CachedResolutionFile,
    CachedRustFileModule, CachedRustModule, CachedRustType, CachedRustUseBinding,
    CachedTopLevelDeclaration,
};
use crate::source_content_hash;
use anyhow::{Context, Result, anyhow};
use codestory_contracts::graph::{Edge, EdgeId, EdgeKind, Node, NodeId, NodeKind};
use codestory_contracts::proof_resolution::{
    CallResolutionFact, CalleeForm, DependencyFileHash, EXACT_CALL_RESOLUTION_ALGORITHM,
    ExactCallsite, ExactCallsiteCorrelationFailure, ExactSyntaxCallsiteCorrelationInput, FileId,
    INTERNAL_RESOLUTION_PRODUCER, OrdinaryCallEdgeCorrelationInput,
    PROOF_RESOLUTION_FACT_SCHEMA_VERSION, ProofResolutionAdapter, ProofResolutionFunnelCounts,
    ProofResolutionFunnelRow, ProofResolutionProjection, ProofResolutionReason,
    ProofResolutionStatus, ResolutionEvidence, ResolutionEvidenceKind, ResolutionProvenance,
    correlate_exact_syntax_callsites,
};
use codestory_store::{
    ExactCallEdgeProjection, IndexPublicationRecord, ProofResolutionPublication, Store,
};
use codestory_workspace::{WorkspacePathIdentity, workspace_path_identity};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use tree_sitter::{Node as TsNode, Parser, Tree};

#[cfg(test)]
thread_local! {
    static RUST_RESOLUTION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static GO_RESOLUTION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PYTHON_RESOLUTION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static C_CPP_RESOLUTION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RUBY_PHP_RESOLUTION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BashResolutionWork {
    pub preparation: usize,
    pub cache_reauthentication: usize,
    pub projection: usize,
    pub graph_correlation: usize,
}

#[cfg(debug_assertions)]
thread_local! {
    static BASH_RESOLUTION_WORK: std::cell::Cell<BashResolutionWork> = const {
        std::cell::Cell::new(BashResolutionWork {
            preparation: 0,
            cache_reauthentication: 0,
            projection: 0,
            graph_correlation: 0,
        })
    };
}

#[derive(Clone, Copy)]
enum BashResolutionPhase {
    Preparation,
    CacheReauthentication,
    Projection,
    GraphCorrelation,
}

#[inline]
fn count_bash_resolution_work(phase: BashResolutionPhase, amount: usize) {
    #[cfg(debug_assertions)]
    BASH_RESOLUTION_WORK.with(|work| {
        let mut value = work.get();
        let slot = match phase {
            BashResolutionPhase::Preparation => &mut value.preparation,
            BashResolutionPhase::CacheReauthentication => &mut value.cache_reauthentication,
            BashResolutionPhase::Projection => &mut value.projection,
            BashResolutionPhase::GraphCorrelation => &mut value.graph_correlation,
        };
        *slot = slot.saturating_add(amount);
        work.set(value);
    });
    #[cfg(not(debug_assertions))]
    let _ = (phase, amount);
}

#[cfg(debug_assertions)]
pub fn reset_bash_resolution_work() {
    BASH_RESOLUTION_WORK.set(BashResolutionWork::default());
}

#[cfg(debug_assertions)]
pub fn bash_resolution_work() -> BashResolutionWork {
    BASH_RESOLUTION_WORK.get()
}

#[cfg(test)]
static JAVA_KOTLIN_RESOLUTION_WORK: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

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

#[inline]
fn count_java_kotlin_resolution_work(amount: usize) {
    #[cfg(test)]
    let _ = JAVA_KOTLIN_RESOLUTION_WORK.fetch_add(amount, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(test))]
    let _ = amount;
}

#[cfg(test)]
fn reset_java_kotlin_resolution_work() {
    JAVA_KOTLIN_RESOLUTION_WORK.store(0, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
fn java_kotlin_resolution_work() -> usize {
    JAVA_KOTLIN_RESOLUTION_WORK.load(std::sync::atomic::Ordering::Relaxed)
}

#[inline]
fn count_c_cpp_resolution_work(amount: usize) {
    #[cfg(test)]
    C_CPP_RESOLUTION_WORK.with(|work| work.set(work.get().saturating_add(amount)));
    #[cfg(not(test))]
    let _ = amount;
}

#[cfg(test)]
fn reset_c_cpp_resolution_work() {
    C_CPP_RESOLUTION_WORK.with(|work| work.set(0));
}

#[cfg(test)]
fn c_cpp_resolution_work() -> usize {
    C_CPP_RESOLUTION_WORK.with(std::cell::Cell::get)
}

#[inline]
fn count_ruby_php_resolution_work(amount: usize) {
    #[cfg(test)]
    RUBY_PHP_RESOLUTION_WORK.with(|work| work.set(work.get().saturating_add(amount)));
    #[cfg(not(test))]
    let _ = amount;
}

#[cfg(test)]
fn reset_ruby_php_resolution_work() {
    RUBY_PHP_RESOLUTION_WORK.with(|work| work.set(0));
}

#[cfg(test)]
fn ruby_php_resolution_work() -> usize {
    RUBY_PHP_RESOLUTION_WORK.with(std::cell::Cell::get)
}

const ADAPTER_VERSION: &str = "reference-v15";
const GO_ADAPTER_VERSION: &str = "reference-v19";
const PYTHON_ADAPTER_VERSION: &str = "reference-v17";
const RUST_ADAPTER_VERSION: &str = "reference-v19";
const TYPESCRIPT_ADAPTER_VERSION: &str = "reference-v17";
const JAVA_ADAPTER_VERSION: &str = "reference-v2";
const KOTLIN_ADAPTER_VERSION: &str = "reference-v2";
const C_ADAPTER_VERSION: &str = "reference-v2";
const CPP_ADAPTER_VERSION: &str = "reference-v3";
const RUBY_ADAPTER_VERSION: &str = "reference-v3";
const PHP_ADAPTER_VERSION: &str = "reference-v2";
const CSHARP_ADAPTER_VERSION: &str = "reference-v2";
const SWIFT_ADAPTER_VERSION: &str = "reference-v2";
const DART_ADAPTER_VERSION: &str = "reference-v2";
const BASH_ADAPTER_VERSION: &str = "reference-v1";
const RESOLUTION_INPUT_SCHEMA_VERSION: u32 = 26;
const INSTALLED_ADAPTERS: &[(&str, &str)] = &[
    ("bash", BASH_ADAPTER_VERSION),
    ("go", GO_ADAPTER_VERSION),
    ("javascript", ADAPTER_VERSION),
    ("c", C_ADAPTER_VERSION),
    ("cpp", CPP_ADAPTER_VERSION),
    ("csharp", CSHARP_ADAPTER_VERSION),
    ("dart", DART_ADAPTER_VERSION),
    ("java", JAVA_ADAPTER_VERSION),
    ("kotlin", KOTLIN_ADAPTER_VERSION),
    ("python", PYTHON_ADAPTER_VERSION),
    ("php", PHP_ADAPTER_VERSION),
    ("ruby", RUBY_ADAPTER_VERSION),
    ("rust", RUST_ADAPTER_VERSION),
    ("swift", SWIFT_ADAPTER_VERSION),
    ("tsx", TYPESCRIPT_ADAPTER_VERSION),
    ("typescript", TYPESCRIPT_ADAPTER_VERSION),
];

fn adapter_version(language: &str) -> &str {
    INSTALLED_ADAPTERS
        .iter()
        .find_map(|(installed, version)| (*installed == language).then_some(*version))
        .unwrap_or(ADAPTER_VERSION)
}

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
    source_path: &Path,
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
    let javascript_index = is_javascript_language(language).then(|| {
        JavascriptResolutionIndex::build(tree, source, source_path, language, file_id, nodes)
    });
    let rust_index =
        (language == "rust").then(|| RustResolutionIndex::build(tree, source, file_id, nodes));
    let go_index =
        (language == "go").then(|| GoResolutionIndex::build(tree, source, file_id, nodes));
    let python_index =
        (language == "python").then(|| PythonResolutionIndex::build(tree, source, file_id, nodes));
    let java_kotlin_index = is_nominal_language(language).then(|| {
        JavaKotlinResolutionIndex::build(tree, source, source_path, language, file_id, nodes)
    });
    let c_cpp_index = is_c_cpp_language(language)
        .then(|| CCppResolutionIndex::build(tree, source, source_path, language, file_id, nodes));
    let ruby_index =
        (language == "ruby").then(|| RubyResolutionIndex::build(tree, source, file_id, nodes));
    let php_index =
        (language == "php").then(|| PhpResolutionIndex::build(tree, source, file_id, nodes));
    let bash_index =
        (language == "bash").then(|| BashResolutionIndex::build(tree, source, file_id, nodes));
    let (direct_exports, export_poison_all, poisoned_export_names) =
        if let Some(index) = &javascript_index {
            index.collect_direct_exports(source)
        } else if let Some(index) = &python_index {
            (
                Vec::new(),
                index.module_dynamic,
                index.poisoned_export_names(),
            )
        } else if let Some(index) = &java_kotlin_index {
            (
                Vec::new(),
                index.has_annotated_declaration || index.domain_poisoned,
                Vec::new(),
            )
        } else if let Some(index) = &ruby_index {
            (index.direct_exports.clone(), index.poisoned, Vec::new())
        } else if let Some(index) = &php_index {
            (Vec::new(), index.poisoned, Vec::new())
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
    } else if let Some(index) = &java_kotlin_index {
        index.declarations.clone()
    } else if let Some(index) = &c_cpp_index {
        index.declarations.clone()
    } else if let Some(index) = &ruby_index {
        index.declarations.clone()
    } else if let Some(index) = &php_index {
        index.declarations.clone()
    } else if let Some(index) = &bash_index {
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
            } else if let Some(index) = &java_kotlin_index {
                index.resolve_syntax_claim(source, callee, form, &raw_target)
            } else if let Some(index) = &c_cpp_index {
                index.resolve_syntax_claim(callee, form, &raw_target)
            } else if let Some(index) = &ruby_index {
                index.resolve_syntax_claim(callee, form, &raw_target)
            } else if let Some(index) = &php_index {
                index.resolve_syntax_claim(callee, form, &raw_target)
            } else if let Some(index) = &bash_index {
                index.resolve_syntax_claim(callee, form, &raw_target)
            } else {
                (None, CachedResolutionBinding::Unsupported)
            };
            if !lookup_input_complete {
                binding = CachedResolutionBinding::IncompleteDomain;
            }
            if matches!(
                binding,
                CachedResolutionBinding::GoImplicitReceiver { .. }
                    | CachedResolutionBinding::ImplicitReceiver { .. }
            ) {
                callsite.callee_form = CalleeForm::ImplicitReceiver;
            }
            if matches!(binding, CachedResolutionBinding::StaticImport { .. })
                || matches!(
                    binding,
                    CachedResolutionBinding::JavaKotlinImportedFunction { .. }
                )
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
                adapter_version: adapter_version(language).to_string(),
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
    } else if let Some(index) = &java_kotlin_index {
        for call in &index.calls {
            emit_call(call.callee, call.form, call.raw_target.clone(), None);
        }
    } else if let Some(index) = &c_cpp_index {
        for call in &index.calls {
            emit_call(call.callee, call.form, call.raw_target.clone(), None);
        }
    } else if let Some(index) = &ruby_index {
        for call in &index.calls {
            emit_call(call.callee, call.form, call.raw_target.clone(), None);
        }
    } else if let Some(index) = &php_index {
        for call in &index.calls {
            emit_call(call.callee, call.form, call.raw_target.clone(), None);
        }
    } else if let Some(index) = &bash_index {
        for call in &index.calls {
            emit_call(call.callee, call.form, call.raw_target.clone(), None);
        }
    } else {
        collect_calls(tree.root_node(), source, &mut |callee, form, raw_target| {
            emit_call(callee, form, raw_target, None);
        });
    }
    if c_cpp_index.is_none()
        && ruby_index.is_none()
        && php_index.is_none()
        && bash_index.is_none()
        && !is_csharp_swift_dart_language(language)
    {
        calls.sort_by_key(|input| (input.callsite.start_byte, input.callsite.end_byte_exclusive));
    } else {
        debug_assert!(calls.windows(2).all(|pair| {
            (
                pair[0].callsite.start_byte,
                pair[0].callsite.end_byte_exclusive,
            ) <= (
                pair[1].callsite.start_byte,
                pair[1].callsite.end_byte_exclusive,
            )
        }));
    }
    CollectedResolutionInputs {
        calls,
        file: Some(CachedResolutionFile {
            file_id,
            source_sha256,
            language: language.to_string(),
            adapter_version: adapter_version(language).to_string(),
            parser_fingerprint: parser_fingerprint.to_string(),
            complete,
            lookup_input_complete,
            typescript_module,
            top_level_declarations,
            inherent_methods,
            classes: javascript_index.as_ref().map_or_else(
                || {
                    python_index.as_ref().map_or_else(
                        || {
                            java_kotlin_index.as_ref().map_or_else(
                                || {
                                    c_cpp_index.as_ref().map_or_else(
                                        || {
                                            ruby_index.as_ref().map_or_else(
                                                || {
                                                    php_index
                                                        .as_ref()
                                                        .map_or_else(Vec::new, |index| {
                                                            index.classes.clone()
                                                        })
                                                },
                                                |index| index.classes.clone(),
                                            )
                                        },
                                        |index| index.classes.clone(),
                                    )
                                },
                                |index| index.classes.clone(),
                            )
                        },
                        |index| index.classes.clone(),
                    )
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
            java_kotlin_package: java_kotlin_index
                .as_ref()
                .and_then(|index| index.package_name.clone()),
            php_namespace: php_index
                .as_ref()
                .map_or(CachedPhpNamespace::Invalid, |index| index.namespace.clone()),
            c_cpp_file: c_cpp_index.as_ref().map(|index| CachedCCppFile {
                source_path: source_path.to_path_buf(),
                source_role: index.source_role,
                namespaces: index.namespaces.clone(),
            }),
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

fn is_java_kotlin_language(language: &str) -> bool {
    matches!(language, "java" | "kotlin")
}

fn is_csharp_swift_dart_language(language: &str) -> bool {
    matches!(language, "csharp" | "swift" | "dart")
}

fn semantic_cache_requires_source_reauthentication(language: &str) -> bool {
    is_csharp_swift_dart_language(language) || language == "bash"
}

fn is_nominal_language(language: &str) -> bool {
    is_java_kotlin_language(language) || is_csharp_swift_dart_language(language)
}

fn csd_source_domain(language: &str, source_path: &Path) -> Option<String> {
    match language {
        "csharp" => Some("csharp:global".to_string()),
        "swift" => swift_source_domain(source_path),
        "dart" => dart_source_domain(source_path),
        _ => None,
    }
}

fn path_normal_components(path: &Path) -> Vec<&str> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect()
}

fn swift_source_domain(path: &Path) -> Option<String> {
    let components = path_normal_components(path);
    if let Some(module) = components
        .windows(2)
        .find_map(|pair| (pair[0] == "Sources").then_some(pair[1]))
        .filter(|module| java_kotlin_simple_identifier(module))
    {
        return Some(format!("swift:Sources/{module}"));
    }
    components
        .contains(&"Source")
        .then(|| "swift:Source".to_string())
}

fn dart_source_domain(path: &Path) -> Option<String> {
    let components = path_normal_components(path);
    let lib = components
        .iter()
        .rposition(|component| *component == "lib")?;
    if lib >= 2 && matches!(components[lib - 2], "pkgs" | "packages") {
        Some(format!(
            "dart:{}/{}/lib",
            components[lib - 2],
            components[lib - 1]
        ))
    } else {
        Some("dart:lib".to_string())
    }
}

fn swift_project_module(path: &Path) -> Option<&str> {
    let mut components = path.components().filter_map(|component| match component {
        Component::Normal(value) => value.to_str(),
        _ => None,
    });
    while let Some(component) = components.next() {
        if component == "Sources" {
            return components
                .next()
                .filter(|module| java_kotlin_simple_identifier(module));
        }
    }
    None
}

fn dart_literal_import_target_is_authenticated(
    source_file: NodeId,
    import: NodeId,
    target_file: NodeId,
    dependencies: &[FileId],
    nodes: &HashMap<NodeId, &Node>,
    files: &HashMap<i64, &codestory_store::FileInfo>,
) -> bool {
    let (Some(source), Some(target), Some(import_node)) = (
        files.get(&source_file.0),
        files.get(&target_file.0),
        nodes.get(&import),
    ) else {
        return false;
    };
    let Some(uri) = quoted_literal(&import_node.serialized_name) else {
        return false;
    };
    let relative = Path::new(uri);
    let Some(source_directory) = source.path.parent() else {
        return false;
    };
    let expected_path = source_directory.join(relative);
    let exact_native_target = std::fs::symlink_metadata(&expected_path)
        .ok()
        .is_some_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
        && workspace_path_identity(&expected_path).ok()
            == workspace_path_identity(&target.path).ok();
    let same_library = dart_library_root(&source.path)
        .zip(dart_library_root(&target.path))
        .is_some_and(|(source_root, target_root)| {
            workspace_path_identity(source_root).ok() == workspace_path_identity(target_root).ok()
        });
    let source_library_identity =
        dart_library_root(&source.path).and_then(|root| workspace_path_identity(root).ok());
    let expected_dependencies = files
        .values()
        .filter(|file| {
            file.indexed
                && file.language == "dart"
                && dart_library_root(&file.path).and_then(|root| workspace_path_identity(root).ok())
                    == source_library_identity
        })
        .map(|file| FileId(file.id))
        .collect::<HashSet<_>>();
    import_node.kind == NodeKind::MODULE
        && import_node.file_node_id == Some(source_file)
        && relative
            .extension()
            .and_then(|extension| extension.to_str())
            == Some("dart")
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && source_file != target_file
        && target.language == "dart"
        && target.indexed
        && exact_native_target
        && same_library
        && source_library_identity.is_some()
        && dependencies.iter().copied().collect::<HashSet<_>>() == expected_dependencies
}

fn generated_source_marker(source: &str) -> bool {
    let prefix = source.get(..source.len().min(512)).unwrap_or(source);
    let lowercase = prefix.to_ascii_lowercase();
    lowercase.contains("generated code")
        || lowercase.contains("@generated")
        || lowercase.contains("<auto-generated")
        || lowercase.contains("generatedcode(")
}

fn quoted_literal(surface: &str) -> Option<&str> {
    let start = surface.find(['\'', '"'])?;
    let quote = surface.as_bytes()[start] as char;
    let rest = &surface[start + quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(&rest[..end])
}

fn import_alias(surface: &str) -> Option<String> {
    let tail = surface.trim_end_matches(';').trim();
    let (_, alias) = tail.rsplit_once(" as ")?;
    java_kotlin_simple_identifier(alias.trim()).then(|| alias.trim().to_string())
}

fn import_show_names(surface: &str) -> Option<Vec<String>> {
    let (_, shown) = surface.split_once(" show ")?;
    let names = shown
        .trim_end_matches(';')
        .split(',')
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();
    (!names.is_empty() && names.iter().all(|name| java_kotlin_simple_identifier(name)))
        .then_some(names)
}

fn import_hide_names(surface: &str) -> Option<Vec<String>> {
    let (_, hidden) = surface.split_once(" hide ")?;
    let names = hidden
        .trim_end_matches(';')
        .split(',')
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();
    (!names.is_empty() && names.iter().all(|name| java_kotlin_simple_identifier(name)))
        .then_some(names)
}

fn is_c_cpp_language(language: &str) -> bool {
    matches!(language, "c" | "cpp")
}

#[derive(Clone)]
struct IndexedRubyPhpCall<'tree> {
    callee: TsNode<'tree>,
    form: CalleeForm,
    raw_target: String,
    caller: Option<NodeId>,
    binding: CachedResolutionBinding,
}

type RubyPhpGraphDeclarations = HashMap<(u32, String, NodeKind), Vec<NodeId>>;
type RubyPhpGraphImports = HashMap<(u32, NodeKind), Vec<NodeId>>;

fn ruby_php_graph_maps(
    nodes: &[Node],
    file_id: NodeId,
) -> (RubyPhpGraphDeclarations, RubyPhpGraphImports) {
    let mut declarations = HashMap::new();
    let mut imports = HashMap::new();
    for node in nodes
        .iter()
        .filter(|node| node.file_node_id == Some(file_id))
    {
        count_ruby_php_resolution_work(1);
        let Some(line) = node.start_line else {
            continue;
        };
        declarations
            .entry((
                line,
                graph_leaf_name(&node.serialized_name).to_string(),
                node.kind,
            ))
            .or_insert_with(Vec::new)
            .push(node.id);
        count_ruby_php_resolution_work(1);
        if matches!(node.kind, NodeKind::MODULE | NodeKind::UNKNOWN) {
            imports
                .entry((line, node.kind))
                .or_insert_with(Vec::new)
                .push(node.id);
            count_ruby_php_resolution_work(1);
        }
    }
    (declarations, imports)
}

fn unique_graph_declaration(
    declarations: &RubyPhpGraphDeclarations,
    line: u32,
    name: &str,
    kinds: &[NodeKind],
) -> Option<NodeId> {
    count_ruby_php_resolution_work(kinds.len());
    let matches = kinds
        .iter()
        .flat_map(|kind| {
            declarations
                .get(&(line, name.to_string(), *kind))
                .into_iter()
                .flatten()
                .copied()
        })
        .collect::<Vec<_>>();
    matches.first().copied().filter(|_| matches.len() == 1)
}

fn unique_import_node(
    imports: &HashMap<(u32, NodeKind), Vec<NodeId>>,
    line: u32,
) -> Option<NodeId> {
    for kind in [NodeKind::MODULE, NodeKind::UNKNOWN] {
        count_ruby_php_resolution_work(1);
        let matches = imports
            .get(&(line, kind))
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let [node] = matches {
            return Some(*node);
        }
        if !matches.is_empty() {
            return None;
        }
    }
    None
}

#[derive(Clone)]
struct IndexedBashCall<'tree> {
    callee: TsNode<'tree>,
    form: CalleeForm,
    raw_target: String,
    caller: Option<NodeId>,
    binding: CachedResolutionBinding,
}

#[derive(Clone, Copy)]
struct BashDeclaration {
    declaration: NodeId,
    start_byte: usize,
}

struct BashResolutionIndex<'tree> {
    calls: Vec<IndexedBashCall<'tree>>,
    call_indices_by_span: HashMap<(usize, usize), usize>,
    declarations: Vec<CachedTopLevelDeclaration>,
}

struct BashResolutionProducer<'a, 'tree> {
    source: &'a str,
    graph_functions: HashMap<(u32, String), Vec<NodeId>>,
    calls: Vec<IndexedBashCall<'tree>>,
    declarations: Vec<CachedTopLevelDeclaration>,
    declarations_by_name: HashMap<String, Vec<BashDeclaration>>,
    poisoned_definition_names: HashSet<String>,
    source_domain_incomplete: bool,
    dynamic_domain_unsupported: bool,
}

enum BashCommandEffect {
    None,
    IncompleteDomain,
    Unsupported,
    InvalidatesFunctions(Vec<String>),
}

impl<'tree> BashResolutionIndex<'tree> {
    fn build(tree: &'tree Tree, source: &str, file_id: NodeId, nodes: &[Node]) -> Self {
        let mut graph_functions = HashMap::<(u32, String), Vec<NodeId>>::new();
        for node in nodes
            .iter()
            .filter(|node| node.file_node_id == Some(file_id) && node.kind == NodeKind::FUNCTION)
        {
            count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
            if let Some(line) = node.start_line {
                graph_functions
                    .entry((line, graph_leaf_name(&node.serialized_name).to_string()))
                    .or_default()
                    .push(node.id);
                count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
            }
        }
        let mut producer = BashResolutionProducer {
            source,
            graph_functions,
            calls: Vec::new(),
            declarations: Vec::new(),
            declarations_by_name: HashMap::new(),
            poisoned_definition_names: HashSet::new(),
            source_domain_incomplete: false,
            dynamic_domain_unsupported: false,
        };
        producer.visit(tree.root_node(), None, true);
        producer.finish()
    }

    fn resolve_syntax_claim(
        &self,
        callee: TsNode<'tree>,
        form: CalleeForm,
        raw_target: &str,
    ) -> (Option<NodeId>, CachedResolutionBinding) {
        let Some(call) = self
            .call_indices_by_span
            .get(&(callee.start_byte(), callee.end_byte()))
            .and_then(|index| self.calls.get(*index))
        else {
            return (None, CachedResolutionBinding::Unsupported);
        };
        if call.form != form || call.raw_target != raw_target {
            return (call.caller, CachedResolutionBinding::Unsupported);
        }
        (call.caller, call.binding.clone())
    }
}

impl<'a, 'tree> BashResolutionProducer<'a, 'tree> {
    fn visit(&mut self, node: TsNode<'tree>, caller: Option<NodeId>, file_scope: bool) {
        count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
        let mut child_caller = caller;
        if node.kind() == "function_definition" {
            let name = node
                .child_by_field_name("name")
                .and_then(|name| node_text(name, self.source))
                .map(str::to_string);
            child_caller = name.as_deref().and_then(|name| {
                self.unique_graph_function(node.start_position().row as u32 + 1, name)
            });
            if let Some(name) = name {
                if file_scope {
                    if let Some(declaration) = child_caller {
                        self.declarations.push(CachedTopLevelDeclaration {
                            name: name.clone(),
                            declaration,
                            module_path: Vec::new(),
                            cross_module_visible: false,
                        });
                        count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
                        self.declarations_by_name
                            .entry(name)
                            .or_default()
                            .push(BashDeclaration {
                                declaration,
                                start_byte: node.start_byte(),
                            });
                        count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
                    } else {
                        self.poisoned_definition_names.insert(name);
                        count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
                    }
                } else {
                    self.poisoned_definition_names.insert(name);
                    count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
                }
            }
        } else if matches!(node.kind(), "command" | "unset_command") {
            match bash_command_effect(node, self.source) {
                BashCommandEffect::None => {}
                BashCommandEffect::IncompleteDomain => self.source_domain_incomplete = true,
                BashCommandEffect::Unsupported => self.dynamic_domain_unsupported = true,
                BashCommandEffect::InvalidatesFunctions(names) => {
                    for name in names {
                        count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
                        self.poisoned_definition_names.insert(name);
                    }
                }
            }
        }
        if node.kind() == "command"
            && let Some(callee) = bash_command_callee(node)
            && let Some(raw_target) = node_text(callee, self.source).map(str::to_string)
        {
            let literal = bash_literal_command_name(callee, &raw_target);
            self.calls.push(IndexedBashCall {
                callee,
                form: if literal.is_some() {
                    CalleeForm::Identifier
                } else {
                    CalleeForm::DynamicAccess
                },
                raw_target,
                caller,
                binding: CachedResolutionBinding::MissingBinding,
            });
            count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
        }

        let direct_program_child = node.kind() == "program";
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.visit(child, child_caller, direct_program_child);
        }
    }

    fn unique_graph_function(&self, line: u32, name: &str) -> Option<NodeId> {
        count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
        let candidates = self
            .graph_functions
            .get(&(line, name.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let [declaration] = candidates else {
            return None;
        };
        Some(*declaration)
    }

    fn finish(mut self) -> BashResolutionIndex<'tree> {
        for call in &mut self.calls {
            count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
            call.binding = if call.caller.is_none()
                || call.form != CalleeForm::Identifier
                || self.dynamic_domain_unsupported
            {
                CachedResolutionBinding::Unsupported
            } else if self.source_domain_incomplete
                || self.poisoned_definition_names.contains(&call.raw_target)
            {
                CachedResolutionBinding::IncompleteDomain
            } else {
                match self
                    .declarations_by_name
                    .get(&call.raw_target)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    [declaration] if declaration.start_byte < call.callee.start_byte() => {
                        CachedResolutionBinding::SameFile {
                            declaration: declaration.declaration,
                            rust_glob_local_module: None,
                        }
                    }
                    [_] => CachedResolutionBinding::IncompleteDomain,
                    [] => CachedResolutionBinding::MissingBinding,
                    _ => CachedResolutionBinding::Ambiguous,
                }
            };
        }
        debug_assert!(self.calls.windows(2).all(|pair| {
            (pair[0].callee.start_byte(), pair[0].callee.end_byte())
                <= (pair[1].callee.start_byte(), pair[1].callee.end_byte())
        }));
        let call_indices_by_span = self
            .calls
            .iter()
            .enumerate()
            .map(|(index, call)| {
                count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
                ((call.callee.start_byte(), call.callee.end_byte()), index)
            })
            .collect();
        BashResolutionIndex {
            calls: self.calls,
            call_indices_by_span,
            declarations: self.declarations,
        }
    }
}

fn bash_command_callee(command: TsNode<'_>) -> Option<TsNode<'_>> {
    let name = command.child_by_field_name("name")?;
    if name.kind() == "command_name" {
        name.named_child(0).or(Some(name))
    } else {
        Some(name)
    }
}

fn bash_literal_command_name<'a>(callee: TsNode<'_>, raw_target: &'a str) -> Option<&'a str> {
    (callee.kind() == "word" && callee.named_child_count() == 0 && !raw_target.is_empty())
        .then_some(raw_target)
}

fn bash_command_effect(command: TsNode<'_>, source: &str) -> BashCommandEffect {
    if command.kind() == "unset_command" {
        let mut cursor = command.walk();
        let arguments = command.named_children(&mut cursor).collect::<Vec<_>>();
        return bash_unset_effect(&arguments, source);
    }
    if command.kind() != "command" {
        return BashCommandEffect::None;
    }
    let Some(callee) = bash_command_callee(command) else {
        return BashCommandEffect::None;
    };
    let Some(mut effective) = bash_literal_node_text(callee, source) else {
        return BashCommandEffect::None;
    };
    let mut cursor = command.walk();
    let arguments = command
        .children_by_field_name("argument", &mut cursor)
        .collect::<Vec<_>>();
    let mut argument_index = 0;
    while matches!(effective, "builtin" | "command") {
        let Some(argument) = arguments.get(argument_index).copied() else {
            return BashCommandEffect::Unsupported;
        };
        let Some(next) = bash_literal_node_text(argument, source) else {
            return BashCommandEffect::Unsupported;
        };
        argument_index += 1;
        if next == "--" {
            let Some(argument) = arguments.get(argument_index).copied() else {
                return BashCommandEffect::Unsupported;
            };
            let Some(next_after_options) = bash_literal_node_text(argument, source) else {
                return BashCommandEffect::Unsupported;
            };
            argument_index += 1;
            effective = next_after_options;
        } else if next.starts_with('-') {
            return BashCommandEffect::Unsupported;
        } else {
            effective = next;
        }
        count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
    }
    match effective {
        "source" | "." => BashCommandEffect::IncompleteDomain,
        "eval" | "alias" => BashCommandEffect::Unsupported,
        "unset" => bash_unset_effect(&arguments[argument_index..], source),
        _ => BashCommandEffect::None,
    }
}

fn bash_unset_effect(arguments: &[TsNode<'_>], source: &str) -> BashCommandEffect {
    let mut function_mode = false;
    let mut names_started = false;
    let mut names = Vec::new();
    for argument in arguments {
        count_bash_resolution_work(BashResolutionPhase::Preparation, 1);
        let literal = bash_literal_node_text(*argument, source);
        if !names_started {
            match literal {
                Some("--") => {
                    names_started = true;
                    continue;
                }
                Some("--function") => {
                    function_mode = true;
                    continue;
                }
                Some(option) if option.starts_with('-') && option.len() > 1 => {
                    if option[1..]
                        .chars()
                        .all(|flag| matches!(flag, 'f' | 'v' | 'n'))
                    {
                        function_mode |= option[1..].contains('f');
                        continue;
                    }
                    return BashCommandEffect::Unsupported;
                }
                _ => names_started = true,
            }
        }
        if !function_mode {
            continue;
        }
        let Some(name) = literal.filter(|name| bash_function_identifier(name)) else {
            return BashCommandEffect::Unsupported;
        };
        names.push(name.to_owned());
    }
    if !function_mode {
        BashCommandEffect::None
    } else if names.is_empty() {
        BashCommandEffect::Unsupported
    } else {
        BashCommandEffect::InvalidatesFunctions(names)
    }
}

fn bash_literal_node_text<'a>(node: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    matches!(node.kind(), "word" | "variable_name")
        .then(|| node_text(node, source))
        .flatten()
        .filter(|text| !text.is_empty())
}

fn bash_function_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

struct RubyResolutionIndex<'tree> {
    calls: Vec<IndexedRubyPhpCall<'tree>>,
    call_indices_by_span: HashMap<(usize, usize), usize>,
    declarations: Vec<CachedTopLevelDeclaration>,
    classes: Vec<CachedClassDeclaration>,
    direct_exports: Vec<CachedDirectExport>,
    declarations_by_name: HashMap<String, Vec<NodeId>>,
    classes_by_name: HashMap<String, Vec<usize>>,
    methods_by_owner_and_name: HashMap<(usize, String), Vec<NodeId>>,
    poisoned: bool,
}

#[derive(Clone)]
enum RubyReceiverBinding {
    Exact {
        owner_name: String,
        constructor: bool,
    },
    Ambiguous,
}

impl<'tree> RubyResolutionIndex<'tree> {
    fn build(tree: &'tree Tree, source: &str, file_id: NodeId, nodes: &[Node]) -> Self {
        let (graph_declarations, graph_imports) = ruby_php_graph_maps(nodes, file_id);
        let mut index = Self {
            calls: Vec::new(),
            call_indices_by_span: HashMap::new(),
            declarations: Vec::new(),
            classes: Vec::new(),
            direct_exports: Vec::new(),
            declarations_by_name: HashMap::new(),
            classes_by_name: HashMap::new(),
            methods_by_owner_and_name: HashMap::new(),
            poisoned: false,
        };
        let mut producer = RubyResolutionProducer {
            index: &mut index,
            source,
            graph_declarations,
            graph_imports,
            class_indices: HashMap::new(),
            bindings: HashMap::new(),
            require_relative: None,
        };
        producer.visit(tree.root_node(), None, None, true, false, false);
        for class_indices in producer.class_indices.values() {
            if class_indices.len() != 1 {
                producer.index.poisoned = true;
            }
        }
        for call in &mut producer.index.calls {
            if call.form != CalleeForm::Identifier
                || !matches!(call.binding, CachedResolutionBinding::MissingBinding)
            {
                continue;
            }
            count_ruby_php_resolution_work(1);
            let candidates = producer
                .index
                .declarations_by_name
                .get(&call.raw_target)
                .map(Vec::as_slice)
                .unwrap_or_default();
            call.binding = match candidates {
                [declaration] => CachedResolutionBinding::SameFile {
                    declaration: *declaration,
                    rust_glob_local_module: None,
                },
                [] => CachedResolutionBinding::MissingBinding,
                _ => CachedResolutionBinding::Ambiguous,
            };
        }
        debug_assert!(producer.index.calls.windows(2).all(|pair| {
            (pair[0].callee.start_byte(), pair[0].callee.end_byte())
                <= (pair[1].callee.start_byte(), pair[1].callee.end_byte())
        }));
        for (call_index, call) in producer.index.calls.iter().enumerate() {
            count_ruby_php_resolution_work(1);
            producer.index.call_indices_by_span.insert(
                (call.callee.start_byte(), call.callee.end_byte()),
                call_index,
            );
        }
        index
    }

    fn resolve_syntax_claim(
        &self,
        callee: TsNode<'tree>,
        form: CalleeForm,
        raw_target: &str,
    ) -> (Option<NodeId>, CachedResolutionBinding) {
        count_ruby_php_resolution_work(1);
        let Some(call) = self
            .call_indices_by_span
            .get(&(callee.start_byte(), callee.end_byte()))
            .and_then(|index| self.calls.get(*index))
        else {
            return (None, CachedResolutionBinding::Unsupported);
        };
        if self.poisoned || call.form != form || call.raw_target != raw_target {
            return (call.caller, CachedResolutionBinding::Unsupported);
        }
        (call.caller, call.binding.clone())
    }
}

struct RubyResolutionProducer<'index, 'tree> {
    index: &'index mut RubyResolutionIndex<'tree>,
    source: &'index str,
    graph_declarations: RubyPhpGraphDeclarations,
    graph_imports: HashMap<(u32, NodeKind), Vec<NodeId>>,
    class_indices: HashMap<String, Vec<usize>>,
    bindings: HashMap<(NodeId, String), RubyReceiverBinding>,
    require_relative: Option<(String, NodeId)>,
}

impl<'index, 'tree> RubyResolutionProducer<'index, 'tree> {
    fn visit(
        &mut self,
        node: TsNode<'tree>,
        mut caller: Option<NodeId>,
        mut owner_index: Option<usize>,
        declarations_static: bool,
        declaration_body_entry: bool,
        file_scope_entry: bool,
    ) {
        count_ruby_php_resolution_work(1);
        let supported_declaration = matches!(node.kind(), "method" | "class" | "comment");
        let supported_file_import = file_scope_entry
            && node.kind() == "call"
            && ruby_literal_require_relative(node_text(node, self.source).unwrap_or_default())
                .is_some();
        if declaration_body_entry
            && !(supported_declaration
                || supported_file_import
                || ruby_inert_declaration_syntax(node))
        {
            self.index.poisoned = true;
        }
        if node.kind() == "class" {
            let Some(name) = declaration_name(node, self.source).map(str::to_string) else {
                self.index.poisoned = true;
                return;
            };
            let Some(declaration) = unique_graph_declaration(
                &self.graph_declarations,
                node.start_position().row as u32 + 1,
                &name,
                &[NodeKind::CLASS],
            ) else {
                self.index.poisoned = true;
                return;
            };
            if declarations_static {
                owner_index = Some(self.index.classes.len());
                self.index.classes.push(CachedClassDeclaration {
                    name: name.clone(),
                    declaration,
                    methods: Vec::new(),
                    cross_module_visible: false,
                    runtime_closed: false,
                    super_name: None,
                });
                self.index.direct_exports.push(CachedDirectExport {
                    exported_name: name.clone(),
                    declaration,
                    is_default: false,
                    declaration_kind: CachedDeclarationKind::Class,
                });
                self.class_indices
                    .entry(name)
                    .or_default()
                    .push(owner_index.expect("Ruby class index"));
                count_ruby_php_resolution_work(1);
                count_ruby_php_resolution_work(1);
                self.index
                    .classes_by_name
                    .entry(
                        self.index.classes[owner_index.expect("Ruby class index")]
                            .name
                            .clone(),
                    )
                    .or_default()
                    .push(owner_index.expect("Ruby class index"));
            } else {
                self.index.poisoned = true;
                owner_index = None;
            }
        } else if matches!(node.kind(), "module" | "singleton_method") {
            self.index.poisoned = true;
        }

        if node.kind() == "method" {
            let Some(name) = declaration_name(node, self.source).map(str::to_string) else {
                self.index.poisoned = true;
                return;
            };
            let Some(declaration) = unique_graph_declaration(
                &self.graph_declarations,
                node.start_position().row as u32 + 1,
                &name,
                &[NodeKind::FUNCTION, NodeKind::METHOD],
            ) else {
                self.index.poisoned = true;
                return;
            };
            caller = Some(declaration);
            if ruby_magic_method_declaration(&name) || !declarations_static {
                self.index.poisoned = true;
            }
            if declarations_static {
                if let Some(owner_index) = owner_index {
                    self.index.classes[owner_index]
                        .methods
                        .push(CachedClassMethod {
                            name: name.clone(),
                            declaration,
                            cross_module_visible: false,
                        });
                    count_ruby_php_resolution_work(1);
                    self.index
                        .methods_by_owner_and_name
                        .entry((owner_index, name))
                        .or_default()
                        .push(declaration);
                } else {
                    self.index.declarations.push(CachedTopLevelDeclaration {
                        name: name.clone(),
                        declaration,
                        module_path: Vec::new(),
                        cross_module_visible: false,
                    });
                    self.index.direct_exports.push(CachedDirectExport {
                        exported_name: name,
                        declaration,
                        is_default: false,
                        declaration_kind: CachedDeclarationKind::Callable,
                    });
                    count_ruby_php_resolution_work(1);
                    self.index
                        .declarations_by_name
                        .entry(
                            self.index
                                .declarations
                                .last()
                                .expect("Ruby declaration")
                                .name
                                .clone(),
                        )
                        .or_default()
                        .push(declaration);
                }
            }
        }

        self.observe_poison(node, file_scope_entry);
        if node.kind() == "assignment" {
            self.observe_assignment(node, caller);
        }
        if matches!(node.kind(), "identifier" | "constant")
            && crate::is_ruby_bare_call_site(node)
            && caller.is_some()
        {
            let raw_target = node_text(node, self.source).unwrap_or_default().to_string();
            let binding = self.same_file_function_binding(&raw_target);
            self.push_call(node, CalleeForm::Identifier, raw_target, caller, binding);
        }

        let mut cursor = node.walk();
        let call_method = (node.kind() == "call")
            .then(|| node.child_by_field_name("method"))
            .flatten()
            .map(|method| method.id());
        let child_declarations_static =
            declarations_static && matches!(node.kind(), "program" | "body_statement" | "class");
        for child in node.named_children(&mut cursor) {
            if call_method == Some(child.id()) {
                self.observe_call(node, caller, owner_index);
            }
            let child_declaration_body_entry = declarations_static
                && caller.is_none()
                && matches!(node.kind(), "program" | "body_statement");
            let child_file_scope_entry =
                child_declaration_body_entry && owner_index.is_none() && node.kind() == "program";
            self.visit(
                child,
                caller,
                owner_index,
                child_declarations_static,
                child_declaration_body_entry,
                child_file_scope_entry,
            );
        }
    }

    fn observe_poison(&mut self, node: TsNode<'_>, file_scope_entry: bool) {
        if node.kind() == "comment"
            && node_text(node, self.source).is_some_and(|comment| {
                let comment = comment.to_ascii_lowercase();
                comment.contains("@generated") || comment.contains("do not edit")
            })
        {
            self.index.poisoned = true;
        }
        if matches!(node.kind(), "alias" | "undef" | "singleton_class") {
            self.index.poisoned = true;
        }
        if node.kind() != "call" {
            return;
        }
        let method = node
            .child_by_field_name("method")
            .and_then(|method| node_text(method, self.source))
            .unwrap_or_default();
        if ruby_method_table_mutator(method) {
            self.index.poisoned = true;
        }
        if method == "require_relative" {
            let line = node.start_position().row as u32 + 1;
            let literal =
                ruby_literal_require_relative(node_text(node, self.source).unwrap_or_default());
            let import = unique_import_node(&self.graph_imports, line);
            match (
                literal,
                import,
                self.require_relative.is_none() && file_scope_entry,
            ) {
                (Some(module), Some(import), true) => {
                    self.require_relative = Some((module, import))
                }
                _ => self.index.poisoned = true,
            }
        }
    }

    fn observe_assignment(&mut self, node: TsNode<'_>, caller: Option<NodeId>) {
        let (Some(caller), Some(left), Some(right)) = (
            caller,
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            return;
        };
        let Some(name) = node_text(left, self.source)
            .map(str::trim)
            .map(str::to_string)
        else {
            return;
        };
        if name.starts_with(|ch: char| ch.is_ascii_uppercase()) {
            self.index.poisoned = true;
            return;
        }
        let binding = ruby_constructor_owner(right, self.source)
            .map(|owner_name| RubyReceiverBinding::Exact {
                owner_name,
                constructor: true,
            })
            .unwrap_or(RubyReceiverBinding::Ambiguous);
        let key = (caller, name);
        self.bindings
            .entry(key)
            .and_modify(|existing| *existing = RubyReceiverBinding::Ambiguous)
            .or_insert(binding);
        count_ruby_php_resolution_work(1);
    }

    fn observe_call(
        &mut self,
        node: TsNode<'tree>,
        caller: Option<NodeId>,
        owner_index: Option<usize>,
    ) {
        let Some(method) = node.child_by_field_name("method") else {
            return;
        };
        let raw_target = node_text(method, self.source)
            .unwrap_or_default()
            .to_string();
        if raw_target == "new" || raw_target == "require_relative" {
            return;
        }
        let Some(receiver) = node.child_by_field_name("receiver") else {
            if caller.is_some() {
                let binding = self.same_file_function_binding(&raw_target);
                self.push_call(method, CalleeForm::Identifier, raw_target, caller, binding);
            }
            return;
        };
        let receiver_surface = node_text(receiver, self.source).unwrap_or_default().trim();
        let (form, binding) = if receiver.kind() == "self" {
            let binding = owner_index
                .and_then(|owner| self.implicit_method_binding(owner, &raw_target))
                .unwrap_or(CachedResolutionBinding::MissingBinding);
            (CalleeForm::ImplicitReceiver, binding)
        } else if let Some(owner_name) = ruby_constructor_owner(receiver, self.source) {
            (
                CalleeForm::ExplicitReceiver,
                self.receiver_binding(&owner_name, &raw_target, true),
            )
        } else {
            let binding = caller
                .and_then(|caller| {
                    count_ruby_php_resolution_work(1);
                    self.bindings
                        .get(&(caller, receiver_surface.to_string()))
                        .cloned()
                })
                .map(|binding| match binding {
                    RubyReceiverBinding::Exact {
                        owner_name,
                        constructor,
                    } => self.receiver_binding(&owner_name, &raw_target, constructor),
                    RubyReceiverBinding::Ambiguous => CachedResolutionBinding::Ambiguous,
                })
                .unwrap_or(CachedResolutionBinding::Unsupported);
            (CalleeForm::ExplicitReceiver, binding)
        };
        self.push_call(method, form, raw_target, caller, binding);
    }

    fn same_file_function_binding(&self, name: &str) -> CachedResolutionBinding {
        count_ruby_php_resolution_work(1);
        let candidates = self
            .index
            .declarations_by_name
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        match candidates {
            [declaration] => CachedResolutionBinding::SameFile {
                declaration: *declaration,
                rust_glob_local_module: None,
            },
            [] => CachedResolutionBinding::MissingBinding,
            _ => CachedResolutionBinding::Ambiguous,
        }
    }

    fn implicit_method_binding(
        &self,
        owner_index: usize,
        method_name: &str,
    ) -> Option<CachedResolutionBinding> {
        let class = self.index.classes.get(owner_index)?;
        count_ruby_php_resolution_work(1);
        let methods = self
            .index
            .methods_by_owner_and_name
            .get(&(owner_index, method_name.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        match methods {
            [method] => Some(CachedResolutionBinding::ImplicitReceiver {
                owner: class.declaration,
                declaration: *method,
                owner_name: class.name.clone(),
            }),
            [] => None,
            _ => Some(CachedResolutionBinding::Ambiguous),
        }
    }

    fn receiver_binding(
        &self,
        owner_name: &str,
        method_name: &str,
        constructor: bool,
    ) -> CachedResolutionBinding {
        count_ruby_php_resolution_work(1);
        let classes = self
            .index
            .classes_by_name
            .get(owner_name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let class_binding = match classes {
            [class_index] => CachedClassBinding::SameFile {
                owner: self.index.classes[*class_index].declaration,
                owner_name: owner_name.to_string(),
            },
            [] => {
                let Some((module_specifier, import)) = &self.require_relative else {
                    return CachedResolutionBinding::MissingBinding;
                };
                CachedClassBinding::StaticImport {
                    import: *import,
                    module_specifier: module_specifier.clone(),
                    imported_name: owner_name.to_string(),
                    is_default: false,
                }
            }
            _ => return CachedResolutionBinding::Ambiguous,
        };
        if constructor {
            CachedResolutionBinding::ConstructorBinding {
                class_binding,
                method_name: method_name.to_string(),
            }
        } else {
            CachedResolutionBinding::ExplicitReceiverType {
                class_binding,
                method_name: method_name.to_string(),
            }
        }
    }

    fn push_call(
        &mut self,
        callee: TsNode<'tree>,
        form: CalleeForm,
        raw_target: String,
        caller: Option<NodeId>,
        binding: CachedResolutionBinding,
    ) {
        self.index.calls.push(IndexedRubyPhpCall {
            callee,
            form,
            raw_target,
            caller,
            binding,
        });
        count_ruby_php_resolution_work(1);
    }
}

fn ruby_magic_method_declaration(name: &str) -> bool {
    matches!(
        name,
        "method_missing"
            | "respond_to_missing?"
            | "method_added"
            | "singleton_method_added"
            | "inherited"
            | "included"
            | "extended"
            | "prepended"
            | "append_features"
            | "extend_object"
            | "prepend_features"
            | "const_missing"
    )
}

fn ruby_method_table_mutator(name: &str) -> bool {
    matches!(
        name,
        "remove_method"
            | "undef_method"
            | "alias_method"
            | "define_method"
            | "define_singleton_method"
            | "attr"
            | "attr_reader"
            | "attr_writer"
            | "attr_accessor"
            | "include"
            | "prepend"
            | "extend"
            | "eval"
            | "class_eval"
            | "module_eval"
            | "instance_eval"
            | "class_exec"
            | "module_exec"
            | "instance_exec"
            | "send"
            | "public_send"
            | "__send__"
            | "method"
            | "public_method"
            | "singleton_method"
            | "instance_method"
            | "const_set"
            | "remove_const"
            | "autoload"
            | "method_missing"
            | "respond_to_missing?"
    )
}

fn ruby_inert_declaration_syntax(node: TsNode<'_>) -> bool {
    count_ruby_php_resolution_work(1);
    if matches!(
        node.kind(),
        "comment"
            | "integer"
            | "float"
            | "nil"
            | "true"
            | "false"
            | "string_content"
            | "escape_sequence"
    ) {
        return true;
    }
    if !matches!(
        node.kind(),
        "string"
            | "symbol"
            | "simple_symbol"
            | "bare_symbol"
            | "array"
            | "hash"
            | "pair"
            | "parenthesized_statements"
            | "unary"
    ) {
        return false;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .all(ruby_inert_declaration_syntax)
}

fn ruby_literal_require_relative(surface: &str) -> Option<String> {
    let rest = surface.trim().strip_prefix("require_relative")?.trim();
    let rest = rest.trim_start_matches('(').trim_end_matches(')').trim();
    let quote = rest.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"') || rest.as_bytes().last().copied() != Some(quote) {
        return None;
    }
    let value = rest.get(1..rest.len().checked_sub(1)?)?;
    if value.is_empty() || value.contains(['\0', '\n', '\r']) || value.contains("#{") {
        return None;
    }
    Some(if value.starts_with("./") || value.starts_with("../") {
        value.to_string()
    } else {
        format!("./{value}")
    })
}

fn ruby_constructor_owner(node: TsNode<'_>, source: &str) -> Option<String> {
    if node.kind() != "call" {
        return None;
    }
    let method = node.child_by_field_name("method")?;
    (node_text(method, source)?.trim() == "new").then_some(())?;
    let receiver = node.child_by_field_name("receiver")?;
    let owner = node_text(receiver, source)?.trim();
    (!owner.is_empty()
        && owner.split("::").all(|part| {
            part.chars()
                .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        }))
    .then(|| owner.rsplit("::").next().unwrap_or(owner).to_string())
}

#[derive(Clone)]
enum PhpImportBinding {
    Function {
        namespace: String,
        name: String,
        import: NodeId,
    },
    Type {
        namespace: String,
        name: String,
        import: NodeId,
    },
    Ambiguous,
}

#[derive(Clone)]
enum PhpReceiverBinding {
    Exact {
        owner_name: String,
        constructor: bool,
    },
    Ambiguous,
}

struct PhpResolutionIndex<'tree> {
    calls: Vec<IndexedRubyPhpCall<'tree>>,
    call_indices_by_span: HashMap<(usize, usize), usize>,
    declarations: Vec<CachedTopLevelDeclaration>,
    classes: Vec<CachedClassDeclaration>,
    declarations_by_name: HashMap<String, Vec<NodeId>>,
    classes_by_name: HashMap<String, Vec<usize>>,
    methods_by_owner_and_name: HashMap<(usize, String), Vec<NodeId>>,
    namespace: CachedPhpNamespace,
    poisoned: bool,
}

impl<'tree> PhpResolutionIndex<'tree> {
    fn build(tree: &'tree Tree, source: &str, file_id: NodeId, nodes: &[Node]) -> Self {
        let (graph_declarations, graph_imports) = ruby_php_graph_maps(nodes, file_id);
        let mut index = Self {
            calls: Vec::new(),
            call_indices_by_span: HashMap::new(),
            declarations: Vec::new(),
            classes: Vec::new(),
            declarations_by_name: HashMap::new(),
            classes_by_name: HashMap::new(),
            methods_by_owner_and_name: HashMap::new(),
            namespace: CachedPhpNamespace::Invalid,
            poisoned: false,
        };
        let mut producer = PhpResolutionProducer {
            index: &mut index,
            source,
            graph_declarations,
            graph_imports,
            class_indices: HashMap::new(),
            imports: HashMap::new(),
            bindings: HashMap::new(),
            namespace: None,
            namespace_invalid: false,
        };
        producer.visit(tree.root_node(), None, None, true);
        producer.index.namespace = if producer.namespace_invalid {
            producer.index.poisoned = true;
            CachedPhpNamespace::Invalid
        } else {
            producer
                .namespace
                .take()
                .map_or(CachedPhpNamespace::Global, CachedPhpNamespace::Named)
        };
        if producer
            .class_indices
            .values()
            .any(|indices| indices.len() != 1)
        {
            producer.index.poisoned = true;
        }
        for call in &mut producer.index.calls {
            if call.form != CalleeForm::Identifier
                || !matches!(call.binding, CachedResolutionBinding::MissingBinding)
            {
                continue;
            }
            count_ruby_php_resolution_work(1);
            let candidates = producer
                .index
                .declarations_by_name
                .get(&call.raw_target)
                .map(Vec::as_slice)
                .unwrap_or_default();
            call.binding = match candidates {
                [declaration] => CachedResolutionBinding::SameFile {
                    declaration: *declaration,
                    rust_glob_local_module: None,
                },
                [] => CachedResolutionBinding::MissingBinding,
                _ => CachedResolutionBinding::Ambiguous,
            };
        }
        debug_assert!(producer.index.calls.windows(2).all(|pair| {
            (pair[0].callee.start_byte(), pair[0].callee.end_byte())
                <= (pair[1].callee.start_byte(), pair[1].callee.end_byte())
        }));
        for (call_index, call) in producer.index.calls.iter().enumerate() {
            count_ruby_php_resolution_work(1);
            producer.index.call_indices_by_span.insert(
                (call.callee.start_byte(), call.callee.end_byte()),
                call_index,
            );
        }
        index
    }

    fn resolve_syntax_claim(
        &self,
        callee: TsNode<'tree>,
        form: CalleeForm,
        raw_target: &str,
    ) -> (Option<NodeId>, CachedResolutionBinding) {
        count_ruby_php_resolution_work(1);
        let Some(call) = self
            .call_indices_by_span
            .get(&(callee.start_byte(), callee.end_byte()))
            .and_then(|index| self.calls.get(*index))
        else {
            return (None, CachedResolutionBinding::Unsupported);
        };
        if self.poisoned || call.form != form || call.raw_target != raw_target {
            return (call.caller, CachedResolutionBinding::Unsupported);
        }
        (call.caller, call.binding.clone())
    }
}

struct PhpResolutionProducer<'index, 'tree> {
    index: &'index mut PhpResolutionIndex<'tree>,
    source: &'index str,
    graph_declarations: RubyPhpGraphDeclarations,
    graph_imports: HashMap<(u32, NodeKind), Vec<NodeId>>,
    class_indices: HashMap<String, Vec<usize>>,
    imports: HashMap<String, PhpImportBinding>,
    bindings: HashMap<(NodeId, String), PhpReceiverBinding>,
    namespace: Option<String>,
    namespace_invalid: bool,
}

impl<'index, 'tree> PhpResolutionProducer<'index, 'tree> {
    fn visit(
        &mut self,
        node: TsNode<'tree>,
        mut caller: Option<NodeId>,
        mut owner_index: Option<usize>,
        declarations_static: bool,
    ) {
        count_ruby_php_resolution_work(1);
        if node.kind() == "namespace_definition" {
            let name = declaration_name(node, self.source).and_then(canonical_php_namespace_name);
            if self.namespace.is_some() || name.is_none() {
                self.namespace_invalid = true;
            } else {
                self.namespace = name;
            }
            count_ruby_php_resolution_work(1);
        }
        if node.kind() == "namespace_use_declaration" {
            self.observe_import(node);
        }
        if matches!(node.kind(), "interface_declaration" | "trait_declaration") {
            self.index.poisoned = true;
        }
        if node.kind() == "class_declaration" {
            let Some(name) = declaration_name(node, self.source).map(str::to_string) else {
                self.index.poisoned = true;
                return;
            };
            let Some(declaration) = unique_graph_declaration(
                &self.graph_declarations,
                node.start_position().row as u32 + 1,
                &name,
                &[NodeKind::CLASS],
            ) else {
                self.index.poisoned = true;
                return;
            };
            if declarations_static {
                owner_index = Some(self.index.classes.len());
                self.index.classes.push(CachedClassDeclaration {
                    name: name.clone(),
                    declaration,
                    methods: Vec::new(),
                    cross_module_visible: false,
                    runtime_closed: false,
                    super_name: None,
                });
                self.class_indices
                    .entry(name.clone())
                    .or_default()
                    .push(owner_index.expect("PHP class index"));
                count_ruby_php_resolution_work(1);
                count_ruby_php_resolution_work(1);
                self.index
                    .classes_by_name
                    .entry(name)
                    .or_default()
                    .push(owner_index.expect("PHP class index"));
            } else {
                self.index.poisoned = true;
                owner_index = None;
            }
        }
        if matches!(node.kind(), "function_definition" | "method_declaration") {
            let Some(name) = declaration_name(node, self.source).map(str::to_string) else {
                self.index.poisoned = true;
                return;
            };
            let Some(declaration) = unique_graph_declaration(
                &self.graph_declarations,
                node.start_position().row as u32 + 1,
                &name,
                &[NodeKind::FUNCTION, NodeKind::METHOD],
            ) else {
                self.index.poisoned = true;
                return;
            };
            caller = Some(declaration);
            if name.starts_with("__") || !declarations_static {
                self.index.poisoned = true;
            }
            if declarations_static {
                if let Some(owner_index) = owner_index {
                    self.index.classes[owner_index]
                        .methods
                        .push(CachedClassMethod {
                            name: name.clone(),
                            declaration,
                            cross_module_visible: false,
                        });
                    count_ruby_php_resolution_work(1);
                    self.index
                        .methods_by_owner_and_name
                        .entry((owner_index, name))
                        .or_default()
                        .push(declaration);
                } else {
                    self.index.declarations.push(CachedTopLevelDeclaration {
                        name: name.clone(),
                        declaration,
                        module_path: Vec::new(),
                        cross_module_visible: true,
                    });
                    count_ruby_php_resolution_work(1);
                    self.index
                        .declarations_by_name
                        .entry(name)
                        .or_default()
                        .push(declaration);
                }
                self.observe_parameters(node, declaration);
            }
        }

        self.observe_poison(node);
        if node.kind() == "assignment_expression" {
            self.observe_assignment(node, caller);
        }
        match node.kind() {
            "function_call_expression" => self.observe_function_call(node, caller),
            "member_call_expression" | "nullsafe_member_call_expression" => {
                self.observe_member_call(node, caller, owner_index)
            }
            "scoped_call_expression" => {
                let callee = node.child_by_field_name("name").unwrap_or(node);
                let raw_target = node_text(callee, self.source)
                    .unwrap_or_default()
                    .to_string();
                self.push_call(
                    callee,
                    CalleeForm::DynamicAccess,
                    raw_target,
                    caller,
                    CachedResolutionBinding::Unsupported,
                );
            }
            _ => {}
        }

        let mut cursor = node.walk();
        let child_declarations_static = declarations_static
            && matches!(
                node.kind(),
                "program"
                    | "namespace_definition"
                    | "compound_statement"
                    | "class_declaration"
                    | "declaration_list"
            );
        for child in node.named_children(&mut cursor) {
            self.visit(child, caller, owner_index, child_declarations_static);
        }
    }

    fn observe_import(&mut self, node: TsNode<'_>) {
        let Some(surface) = node_text(node, self.source) else {
            self.index.poisoned = true;
            return;
        };
        let Some(binding) = php_use_binding(
            surface,
            unique_import_node(&self.graph_imports, node.start_position().row as u32 + 1),
        ) else {
            self.index.poisoned = true;
            return;
        };
        let local_name = match &binding {
            PhpImportBinding::Function { name, .. } | PhpImportBinding::Type { name, .. } => {
                php_use_local_name(surface).unwrap_or_else(|| name.clone())
            }
            PhpImportBinding::Ambiguous => return,
        };
        self.imports
            .entry(local_name)
            .and_modify(|existing| *existing = PhpImportBinding::Ambiguous)
            .or_insert(binding);
        count_ruby_php_resolution_work(1);
    }

    fn observe_parameters(&mut self, callable: TsNode<'_>, caller: NodeId) {
        let Some(parameters) = callable.child_by_field_name("parameters") else {
            return;
        };
        let mut cursor = parameters.walk();
        for parameter in parameters.named_children(&mut cursor) {
            let Some(type_node) = parameter.child_by_field_name("type") else {
                continue;
            };
            let Some(name_node) = parameter.child_by_field_name("name") else {
                continue;
            };
            let Some(owner_name) =
                php_simple_type(node_text(type_node, self.source).unwrap_or_default())
            else {
                self.index.poisoned = true;
                continue;
            };
            let name = node_text(name_node, self.source)
                .unwrap_or_default()
                .trim_start_matches('$')
                .to_string();
            self.bindings.insert(
                (caller, name),
                PhpReceiverBinding::Exact {
                    owner_name,
                    constructor: false,
                },
            );
            count_ruby_php_resolution_work(1);
        }
    }

    fn observe_assignment(&mut self, node: TsNode<'_>, caller: Option<NodeId>) {
        let (Some(caller), Some(left), Some(right)) = (
            caller,
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            return;
        };
        let name = node_text(left, self.source)
            .unwrap_or_default()
            .trim_start_matches('$')
            .to_string();
        let binding = php_object_creation_owner(right, self.source)
            .map(|owner_name| PhpReceiverBinding::Exact {
                owner_name,
                constructor: true,
            })
            .unwrap_or(PhpReceiverBinding::Ambiguous);
        self.bindings
            .entry((caller, name))
            .and_modify(|existing| *existing = PhpReceiverBinding::Ambiguous)
            .or_insert(binding);
        count_ruby_php_resolution_work(1);
    }

    fn observe_poison(&mut self, node: TsNode<'_>) {
        if node.kind() == "comment"
            && node_text(node, self.source).is_some_and(|comment| {
                let comment = comment.to_ascii_lowercase();
                comment.contains("@generated") || comment.contains("do not edit")
            })
        {
            self.index.poisoned = true;
        }
        if matches!(
            node.kind(),
            "include_expression"
                | "include_once_expression"
                | "require_expression"
                | "require_once_expression"
                | "trait_use_clause"
        ) {
            self.index.poisoned = true;
        }
        if node.kind() == "method_declaration"
            && declaration_name(node, self.source)
                .is_some_and(|name| matches!(name, "__call" | "__callStatic" | "__get" | "__set"))
        {
            self.index.poisoned = true;
        }
        if node.kind() == "function_call_expression" {
            let name = node
                .child_by_field_name("function")
                .and_then(|function| node_text(function, self.source))
                .unwrap_or_default();
            if matches!(name, "spl_autoload_register" | "call_user_func" | "eval") {
                self.index.poisoned = true;
            }
        }
        if node.kind() == "object_creation_expression"
            && node
                .named_child(0)
                .is_some_and(|name| name.kind() == "variable_name")
        {
            self.index.poisoned = true;
        }
    }

    fn observe_function_call(&mut self, node: TsNode<'tree>, caller: Option<NodeId>) {
        let Some(callee) = node.child_by_field_name("function") else {
            return;
        };
        let raw_target = node_text(callee, self.source)
            .unwrap_or_default()
            .to_string();
        count_ruby_php_resolution_work(1);
        let binding = if callee.kind() == "variable_name" {
            CachedResolutionBinding::Unsupported
        } else if let Some(import) = self.imports.get(&raw_target) {
            match import {
                PhpImportBinding::Function {
                    namespace,
                    name,
                    import,
                } => CachedResolutionBinding::JavaKotlinImportedFunction {
                    package_name: namespace.clone(),
                    owner_name: None,
                    name: name.clone(),
                    import: *import,
                },
                PhpImportBinding::Ambiguous => CachedResolutionBinding::Ambiguous,
                PhpImportBinding::Type { .. } => CachedResolutionBinding::Unsupported,
            }
        } else {
            count_ruby_php_resolution_work(1);
            let candidates = self
                .index
                .declarations_by_name
                .get(&raw_target)
                .map(Vec::as_slice)
                .unwrap_or_default();
            match candidates {
                [declaration] => CachedResolutionBinding::SameFile {
                    declaration: *declaration,
                    rust_glob_local_module: None,
                },
                [] => CachedResolutionBinding::MissingBinding,
                _ => CachedResolutionBinding::Ambiguous,
            }
        };
        let form = if matches!(
            binding,
            CachedResolutionBinding::JavaKotlinImportedFunction { .. }
        ) {
            CalleeForm::NamedImport
        } else if callee.kind() == "variable_name" {
            CalleeForm::DynamicAccess
        } else {
            CalleeForm::Identifier
        };
        self.push_call(callee, form, raw_target, caller, binding);
    }

    fn observe_member_call(
        &mut self,
        node: TsNode<'tree>,
        caller: Option<NodeId>,
        owner_index: Option<usize>,
    ) {
        let Some(callee) = node.child_by_field_name("name") else {
            return;
        };
        let Some(object) = node.child_by_field_name("object") else {
            return;
        };
        let raw_target = node_text(callee, self.source)
            .unwrap_or_default()
            .to_string();
        let object_surface = node_text(object, self.source).unwrap_or_default().trim();
        let (form, binding) = if object_surface == "$this" {
            let binding = owner_index
                .and_then(|owner| self.implicit_method_binding(owner, &raw_target))
                .unwrap_or(CachedResolutionBinding::MissingBinding);
            (CalleeForm::ImplicitReceiver, binding)
        } else if let Some(owner_name) = php_object_creation_owner(object, self.source) {
            (
                CalleeForm::ExplicitReceiver,
                self.receiver_binding(&owner_name, &raw_target, true),
            )
        } else if object.kind() == "variable_name" {
            let receiver_name = object_surface.trim_start_matches('$');
            let binding = caller
                .and_then(|caller| {
                    count_ruby_php_resolution_work(1);
                    self.bindings
                        .get(&(caller, receiver_name.to_string()))
                        .cloned()
                })
                .map(|binding| match binding {
                    PhpReceiverBinding::Exact {
                        owner_name,
                        constructor,
                    } => self.receiver_binding(&owner_name, &raw_target, constructor),
                    PhpReceiverBinding::Ambiguous => CachedResolutionBinding::Ambiguous,
                })
                .unwrap_or(CachedResolutionBinding::Unsupported);
            (CalleeForm::ExplicitReceiver, binding)
        } else {
            (
                CalleeForm::DynamicAccess,
                CachedResolutionBinding::Unsupported,
            )
        };
        self.push_call(callee, form, raw_target, caller, binding);
    }

    fn implicit_method_binding(
        &self,
        owner_index: usize,
        method_name: &str,
    ) -> Option<CachedResolutionBinding> {
        let class = self.index.classes.get(owner_index)?;
        count_ruby_php_resolution_work(1);
        let methods = self
            .index
            .methods_by_owner_and_name
            .get(&(owner_index, method_name.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        match methods {
            [method] => Some(CachedResolutionBinding::ImplicitReceiver {
                owner: class.declaration,
                declaration: *method,
                owner_name: class.name.clone(),
            }),
            [] => None,
            _ => Some(CachedResolutionBinding::Ambiguous),
        }
    }

    fn receiver_binding(
        &self,
        owner_name: &str,
        method_name: &str,
        constructor: bool,
    ) -> CachedResolutionBinding {
        count_ruby_php_resolution_work(1);
        let classes = self
            .index
            .classes_by_name
            .get(owner_name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let [class_index] = classes {
            let class_binding = CachedClassBinding::SameFile {
                owner: self.index.classes[*class_index].declaration,
                owner_name: owner_name.to_string(),
            };
            return if constructor {
                CachedResolutionBinding::ConstructorBinding {
                    class_binding,
                    method_name: method_name.to_string(),
                }
            } else {
                CachedResolutionBinding::ExplicitReceiverType {
                    class_binding,
                    method_name: method_name.to_string(),
                }
            };
        }
        if classes.len() > 1 {
            return CachedResolutionBinding::Ambiguous;
        }
        match self.imports.get(owner_name) {
            Some(PhpImportBinding::Type {
                namespace,
                name,
                import,
            }) => CachedResolutionBinding::JavaKotlinImportedReceiver {
                package_name: namespace.clone(),
                owner_name: name.clone(),
                method_name: method_name.to_string(),
                import: *import,
                constructor,
            },
            Some(PhpImportBinding::Ambiguous) => CachedResolutionBinding::Ambiguous,
            _ => CachedResolutionBinding::MissingBinding,
        }
    }

    fn push_call(
        &mut self,
        callee: TsNode<'tree>,
        form: CalleeForm,
        raw_target: String,
        caller: Option<NodeId>,
        binding: CachedResolutionBinding,
    ) {
        self.index.calls.push(IndexedRubyPhpCall {
            callee,
            form,
            raw_target,
            caller,
            binding,
        });
        count_ruby_php_resolution_work(1);
    }
}

fn canonical_php_namespace_name(name: &str) -> Option<String> {
    let components = name.split('\\').collect::<Vec<_>>();
    count_ruby_php_resolution_work(components.len());
    (!components.is_empty()
        && components.iter().all(|component| {
            let mut chars = component.chars();
            chars
                .next()
                .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
                && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        }))
    .then(|| components.join("."))
}

fn php_simple_type(surface: &str) -> Option<String> {
    let surface = surface.trim().trim_start_matches('?');
    (!surface.is_empty()
        && !surface.contains(['\\', '|', '&'])
        && surface
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric()))
    .then(|| surface.to_string())
}

fn php_object_creation_owner(node: TsNode<'_>, source: &str) -> Option<String> {
    let mut node = node;
    if node.kind() == "parenthesized_expression" {
        node = node.named_child(0)?;
    }
    if node.kind() != "object_creation_expression" {
        return None;
    }
    let name = node.named_child(0)?;
    (name.kind() != "variable_name")
        .then(|| node_text(name, source))
        .flatten()
        .and_then(php_simple_type)
}

fn php_use_local_name(surface: &str) -> Option<String> {
    let body = surface
        .trim()
        .strip_prefix("use ")?
        .strip_prefix("function ")
        .unwrap_or_else(|| surface.trim().strip_prefix("use ").unwrap_or_default())
        .trim_end_matches(';')
        .trim();
    let mut words = body.split_whitespace();
    let path = words.next()?;
    match (words.next(), words.next(), words.next()) {
        (Some(keyword), Some(alias), None) if keyword.eq_ignore_ascii_case("as") => {
            Some(alias.to_string())
        }
        (None, None, None) => path.rsplit('\\').next().map(str::to_string),
        _ => None,
    }
}

fn php_use_binding(surface: &str, import: Option<NodeId>) -> Option<PhpImportBinding> {
    let import = import?;
    let surface = surface.trim();
    let (function, body) = if let Some(body) = surface.strip_prefix("use function ") {
        (true, body)
    } else {
        (false, surface.strip_prefix("use ")?)
    };
    let path = body.trim_end_matches(';').split_whitespace().next()?;
    if path.contains(['{', '}', ',']) || path.starts_with("const ") {
        return None;
    }
    let (namespace, name) = path.trim_start_matches('\\').rsplit_once('\\')?;
    if namespace.is_empty() || name.is_empty() {
        return None;
    }
    Some(if function {
        PhpImportBinding::Function {
            namespace: namespace.replace('\\', "."),
            name: name.to_string(),
            import,
        }
    } else {
        PhpImportBinding::Type {
            namespace: namespace.replace('\\', "."),
            name: name.to_string(),
            import,
        }
    })
}

#[derive(Clone)]
struct IndexedCCppCall<'tree> {
    callee: TsNode<'tree>,
    form: CalleeForm,
    raw_target: String,
    caller: Option<NodeId>,
    callable_id: Option<usize>,
    namespace_path: Vec<String>,
    owner_index: Option<usize>,
    unsupported: bool,
    identifier_shadowed: bool,
    receiver: CCppCallReceiver,
}

#[derive(Clone)]
enum CCppCallReceiver {
    None,
    Implicit,
    ExactType {
        owner_name: String,
        constructor: bool,
        receiver_name: Option<String>,
    },
    Qualified(Vec<String>),
    Blocked,
}

#[derive(Clone)]
enum CCppLexicalBinding {
    Other,
    Receiver { owner_name: String },
}

struct CCppScope {
    names: HashSet<String>,
    insertions: Vec<String>,
}

struct CCppCallableRecord {
    namespace_path: Vec<String>,
    owner_index: Option<usize>,
    name: String,
    signature: Option<String>,
    declaration: Option<NodeId>,
    defined: bool,
    is_virtual: bool,
    is_static: bool,
}

#[derive(Clone, Copy)]
struct CCppWalkContext {
    callable_id: Option<usize>,
    caller: Option<NodeId>,
    owner_index: Option<usize>,
    unsupported: bool,
}

struct CCppResolutionIndex<'tree> {
    language: &'tree str,
    source_role: CachedCCppSourceRole,
    calls: Vec<IndexedCCppCall<'tree>>,
    call_indices_by_span: HashMap<(usize, usize), usize>,
    declarations: Vec<CachedTopLevelDeclaration>,
    declaration_indices_by_key: HashMap<(Vec<String>, String), Vec<usize>>,
    classes: Vec<CachedClassDeclaration>,
    class_namespace_paths: Vec<Vec<String>>,
    class_indices_by_name: HashMap<String, Vec<usize>>,
    class_method_indices_by_name: HashMap<(usize, String), Vec<usize>>,
    namespaces: Vec<CachedCCppNamespace>,
    namespace_nodes_by_path: HashMap<Vec<String>, Vec<NodeId>>,
    callable_nodes: HashMap<(u32, String), Vec<NodeId>>,
    class_nodes: HashMap<(u32, String), Vec<NodeId>>,
    namespace_nodes: HashMap<(u32, String), Vec<NodeId>>,
    owner_bindings: HashMap<(usize, String), Vec<CCppLexicalBinding>>,
    callable_records: Vec<CCppCallableRecord>,
    callable_namespace_paths: HashMap<usize, Vec<String>>,
    rebound_receivers: HashSet<(usize, String)>,
    declaration_signatures: HashMap<(Vec<String>, Option<String>, String), HashSet<String>>,
    unsupported_declarations: HashSet<(Vec<String>, Option<String>, String)>,
    global_shadow_names: HashSet<(Vec<String>, String)>,
    poisoned_callables: HashSet<usize>,
    poisoned_owners: HashSet<usize>,
    poisoned_namespaces: HashSet<Vec<String>>,
    virtual_methods: HashSet<(usize, String)>,
    static_methods: HashSet<(usize, String)>,
    macro_names: HashSet<String>,
    generated: bool,
}

impl<'tree> CCppResolutionIndex<'tree> {
    fn build(
        tree: &'tree Tree,
        source: &str,
        source_path: &Path,
        language: &'tree str,
        file_id: NodeId,
        nodes: &[Node],
    ) -> Self {
        let mut callable_nodes = HashMap::<(u32, String), Vec<NodeId>>::new();
        let mut class_nodes = HashMap::<(u32, String), Vec<NodeId>>::new();
        let mut namespace_nodes = HashMap::<(u32, String), Vec<NodeId>>::new();
        for graph in nodes
            .iter()
            .filter(|node| node.file_node_id == Some(file_id))
        {
            count_c_cpp_resolution_work(1);
            let Some(line) = graph.start_line else {
                continue;
            };
            let name = graph_leaf_name(&graph.serialized_name).to_string();
            match graph.kind {
                NodeKind::FUNCTION | NodeKind::METHOD => {
                    count_c_cpp_resolution_work(1);
                    callable_nodes
                        .entry((line, name))
                        .or_default()
                        .push(graph.id);
                }
                NodeKind::CLASS | NodeKind::STRUCT => {
                    count_c_cpp_resolution_work(1);
                    class_nodes.entry((line, name)).or_default().push(graph.id);
                }
                NodeKind::MODULE => {
                    count_c_cpp_resolution_work(1);
                    namespace_nodes
                        .entry((line, name))
                        .or_default()
                        .push(graph.id);
                }
                _ => {}
            }
        }
        let mut result = Self {
            language,
            source_role: c_cpp_source_role(source_path),
            calls: Vec::new(),
            call_indices_by_span: HashMap::new(),
            declarations: Vec::new(),
            declaration_indices_by_key: HashMap::new(),
            classes: Vec::new(),
            class_namespace_paths: Vec::new(),
            class_indices_by_name: HashMap::new(),
            class_method_indices_by_name: HashMap::new(),
            namespaces: Vec::new(),
            namespace_nodes_by_path: HashMap::new(),
            callable_nodes,
            class_nodes,
            namespace_nodes,
            owner_bindings: HashMap::new(),
            callable_records: Vec::new(),
            callable_namespace_paths: HashMap::new(),
            rebound_receivers: HashSet::new(),
            declaration_signatures: HashMap::new(),
            unsupported_declarations: HashSet::new(),
            global_shadow_names: HashSet::new(),
            poisoned_callables: HashSet::new(),
            poisoned_owners: HashSet::new(),
            poisoned_namespaces: HashSet::new(),
            virtual_methods: HashSet::new(),
            static_methods: HashSet::new(),
            macro_names: HashSet::new(),
            generated: c_cpp_generated_source(source),
        };
        CCppProducer::new(&mut result, source).visit(
            tree.root_node(),
            CCppWalkContext {
                callable_id: None,
                caller: None,
                owner_index: None,
                unsupported: false,
            },
        );

        result.finalize_callable_domain();
        c_cpp_sort_calls_by_span(&mut result.calls);
        for (index, call) in result.calls.iter().enumerate() {
            count_c_cpp_resolution_work(1);
            result
                .call_indices_by_span
                .insert((call.callee.start_byte(), call.callee.end_byte()), index);
        }
        for (index, declaration) in result.declarations.iter().enumerate() {
            count_c_cpp_resolution_work(1);
            result
                .declaration_indices_by_key
                .entry((declaration.module_path.clone(), declaration.name.clone()))
                .or_default()
                .push(index);
        }
        for (class_index, class) in result.classes.iter().enumerate() {
            count_c_cpp_resolution_work(2);
            for (method_index, method) in class.methods.iter().enumerate() {
                count_c_cpp_resolution_work(1);
                result
                    .class_method_indices_by_name
                    .entry((class_index, method.name.clone()))
                    .or_default()
                    .push(method_index);
            }
        }
        result
    }

    fn finalize_callable_domain(&mut self) {
        let mut records_by_signature =
            HashMap::<(Vec<String>, Option<usize>, String, String), Vec<usize>>::new();
        for (record_index, record) in self.callable_records.iter().enumerate() {
            let owner_name = record
                .owner_index
                .map(|owner| self.classes[owner].name.clone());
            let lookup_key = (
                record.namespace_path.clone(),
                owner_name,
                record.name.clone(),
            );
            let Some(signature) = &record.signature else {
                count_c_cpp_resolution_work(1);
                self.unsupported_declarations.insert(lookup_key);
                continue;
            };
            count_c_cpp_resolution_work(2);
            self.declaration_signatures
                .entry(lookup_key)
                .or_default()
                .insert(signature.clone());
            records_by_signature
                .entry((
                    record.namespace_path.clone(),
                    record.owner_index,
                    record.name.clone(),
                    signature.clone(),
                ))
                .or_default()
                .push(record_index);
        }

        for ((namespace_path, owner_index, name, _signature), record_indices) in
            records_by_signature
        {
            let records = record_indices
                .iter()
                .map(|index| &self.callable_records[*index])
                .collect::<Vec<_>>();
            let mut defined = records
                .iter()
                .filter(|record| record.defined)
                .filter_map(|record| record.declaration)
                .collect::<Vec<_>>();
            defined.sort_unstable();
            defined.dedup();
            let mut declared = records
                .iter()
                .filter(|record| !record.defined)
                .filter_map(|record| record.declaration)
                .collect::<Vec<_>>();
            declared.sort_unstable();
            declared.dedup();
            let target = match (defined.as_slice(), declared.as_slice(), self.language) {
                ([definition], [], _) => Some(*definition),
                ([_definition], [declaration], _) => Some(*declaration),
                ([], [declaration], "c") => Some(*declaration),
                _ => None,
            };
            let owner_name = owner_index.map(|owner| self.classes[owner].name.clone());
            let lookup_key = (namespace_path.clone(), owner_name, name.clone());
            if records.iter().any(|record| record.is_virtual) {
                if let Some(owner) = owner_index {
                    count_c_cpp_resolution_work(1);
                    self.virtual_methods.insert((owner, name.clone()));
                } else {
                    self.unsupported_declarations.insert(lookup_key.clone());
                }
            }
            if records.iter().any(|record| record.is_static)
                && let Some(owner) = owner_index
            {
                count_c_cpp_resolution_work(1);
                self.static_methods.insert((owner, name.clone()));
            }
            let Some(target) = target else {
                self.unsupported_declarations.insert(lookup_key);
                continue;
            };
            count_c_cpp_resolution_work(1);
            if let Some(owner) = owner_index {
                self.classes[owner].methods.push(CachedClassMethod {
                    name,
                    declaration: target,
                    cross_module_visible: false,
                });
            } else {
                self.declarations.push(CachedTopLevelDeclaration {
                    name,
                    declaration: target,
                    module_path: namespace_path,
                    cross_module_visible: false,
                });
            }
        }
    }

    fn resolve_syntax_claim(
        &self,
        callee: TsNode<'_>,
        form: CalleeForm,
        target: &str,
    ) -> (Option<NodeId>, CachedResolutionBinding) {
        count_c_cpp_resolution_work(1);
        let Some(call_index) = self
            .call_indices_by_span
            .get(&(callee.start_byte(), callee.end_byte()))
        else {
            return (None, CachedResolutionBinding::Unsupported);
        };
        let call = &self.calls[*call_index];
        let Some(caller) = call.caller else {
            return (None, CachedResolutionBinding::Unsupported);
        };
        if call.form != form
            || call.raw_target != target
            || call.unsupported
            || self.generated
            || self.source_role == CachedCCppSourceRole::Header
            || call
                .callable_id
                .is_some_and(|callable| self.poisoned_callables.contains(&callable))
            || call
                .owner_index
                .is_some_and(|owner| self.poisoned_owners.contains(&owner))
            || self.poisoned_namespaces.contains(&call.namespace_path)
        {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        if self.language == "c" && form != CalleeForm::Identifier {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        count_c_cpp_resolution_work(4);
        match &call.receiver {
            CCppCallReceiver::Qualified(scope) => {
                return (Some(caller), self.resolve_qualified(scope, target));
            }
            CCppCallReceiver::ExactType {
                owner_name,
                constructor,
                receiver_name,
            } => {
                if let (Some(callable_id), Some(receiver_name)) = (call.callable_id, receiver_name)
                    && self
                        .rebound_receivers
                        .contains(&(callable_id, receiver_name.clone()))
                {
                    return (Some(caller), CachedResolutionBinding::Ambiguous);
                }
                return (
                    Some(caller),
                    self.resolve_explicit_receiver(owner_name, target, *constructor),
                );
            }
            CCppCallReceiver::Implicit => {
                return (
                    Some(caller),
                    self.resolve_implicit_receiver(call.owner_index, target),
                );
            }
            CCppCallReceiver::Blocked => {
                return (Some(caller), CachedResolutionBinding::Unsupported);
            }
            CCppCallReceiver::None => {}
        }
        if form != CalleeForm::Identifier || call.identifier_shadowed {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        if self.macro_names.contains(target) {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        if self
            .global_shadow_names
            .contains(&(call.namespace_path.clone(), target.to_string()))
        {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        if let Some(owner_index) = call.owner_index {
            let method = self.resolve_implicit_receiver(Some(owner_index), target);
            if !matches!(method, CachedResolutionBinding::Unsupported) {
                return (Some(caller), method);
            }
        }
        let count_key = (call.namespace_path.clone(), None, target.to_string());
        if self
            .declaration_signatures
            .get(&count_key)
            .is_some_and(|signatures| signatures.len() > 1)
        {
            return (Some(caller), CachedResolutionBinding::Ambiguous);
        }
        if self.unsupported_declarations.contains(&count_key) {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        count_c_cpp_resolution_work(1);
        let declarations = self
            .declaration_indices_by_key
            .get(&(call.namespace_path.clone(), target.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        match declarations {
            [declaration] => (
                Some(caller),
                CachedResolutionBinding::SameFile {
                    declaration: self.declarations[*declaration].declaration,
                    rust_glob_local_module: None,
                },
            ),
            [] => (Some(caller), CachedResolutionBinding::Unsupported),
            _ => (Some(caller), CachedResolutionBinding::Ambiguous),
        }
    }

    fn resolve_implicit_receiver(
        &self,
        owner_index: Option<usize>,
        target: &str,
    ) -> CachedResolutionBinding {
        let Some(owner_index) = owner_index else {
            return CachedResolutionBinding::Unsupported;
        };
        let class = &self.classes[owner_index];
        let count_key = (
            self.class_namespace_paths[owner_index].clone(),
            Some(class.name.clone()),
            target.to_string(),
        );
        if self
            .declaration_signatures
            .get(&count_key)
            .is_some_and(|signatures| signatures.len() > 1)
        {
            return CachedResolutionBinding::Ambiguous;
        }
        if self.unsupported_declarations.contains(&count_key) {
            return CachedResolutionBinding::Unsupported;
        }
        if self
            .virtual_methods
            .contains(&(owner_index, target.to_string()))
        {
            return CachedResolutionBinding::Unsupported;
        }
        count_c_cpp_resolution_work(1);
        let methods = self
            .class_method_indices_by_name
            .get(&(owner_index, target.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        match methods {
            [method] => CachedResolutionBinding::ImplicitReceiver {
                owner: class.declaration,
                declaration: class.methods[*method].declaration,
                owner_name: class.name.clone(),
            },
            [] => CachedResolutionBinding::Unsupported,
            _ => CachedResolutionBinding::Ambiguous,
        }
    }

    fn resolve_explicit_receiver(
        &self,
        owner_name: &str,
        target: &str,
        constructor: bool,
    ) -> CachedResolutionBinding {
        count_c_cpp_resolution_work(2);
        let classes = self
            .class_indices_by_name
            .get(owner_name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let [class_index] = classes else {
            return if classes.is_empty() {
                CachedResolutionBinding::Unsupported
            } else {
                CachedResolutionBinding::Ambiguous
            };
        };
        let class = &self.classes[*class_index];
        let declaration_key = (
            self.class_namespace_paths[*class_index].clone(),
            Some(class.name.clone()),
            target.to_string(),
        );
        if self.unsupported_declarations.contains(&declaration_key) {
            return CachedResolutionBinding::Unsupported;
        }
        if self
            .virtual_methods
            .contains(&(*class_index, target.to_string()))
        {
            return CachedResolutionBinding::Unsupported;
        }
        let methods = self
            .class_method_indices_by_name
            .get(&(*class_index, target.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let [_method] = methods else {
            return if methods.is_empty() {
                CachedResolutionBinding::Unsupported
            } else {
                CachedResolutionBinding::Ambiguous
            };
        };
        let class_binding = CachedClassBinding::SameFile {
            owner: class.declaration,
            owner_name: class.name.clone(),
        };
        if constructor {
            CachedResolutionBinding::ConstructorBinding {
                class_binding,
                method_name: target.to_string(),
            }
        } else {
            CachedResolutionBinding::ExplicitReceiverType {
                class_binding,
                method_name: target.to_string(),
            }
        }
    }

    fn resolve_qualified(&self, scope: &[String], target: &str) -> CachedResolutionBinding {
        if scope.len() == 1 {
            count_c_cpp_resolution_work(1);
            if let Some(classes) = self.class_indices_by_name.get(&scope[0]) {
                let [class_index] = classes.as_slice() else {
                    return CachedResolutionBinding::Ambiguous;
                };
                let declaration_key = (
                    self.class_namespace_paths[*class_index].clone(),
                    Some(self.classes[*class_index].name.clone()),
                    target.to_string(),
                );
                if self.unsupported_declarations.contains(&declaration_key)
                    || !self
                        .static_methods
                        .contains(&(*class_index, target.to_string()))
                {
                    return CachedResolutionBinding::Unsupported;
                }
                let methods = self
                    .class_method_indices_by_name
                    .get(&(*class_index, target.to_string()))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                return match methods {
                    [method] => CachedResolutionBinding::CCppQualified {
                        components: vec![
                            self.classes[*class_index].declaration,
                            self.classes[*class_index].methods[*method].declaration,
                        ],
                    },
                    [] => CachedResolutionBinding::Unsupported,
                    _ => CachedResolutionBinding::Ambiguous,
                };
            }
        }
        count_c_cpp_resolution_work(2);
        let namespaces = self
            .namespace_nodes_by_path
            .get(scope)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let declarations = self
            .declaration_indices_by_key
            .get(&(scope.to_vec(), target.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        match (namespaces, declarations) {
            ([namespace], [declaration]) => CachedResolutionBinding::CCppQualified {
                components: vec![*namespace, self.declarations[*declaration].declaration],
            },
            ([], _) | (_, []) => CachedResolutionBinding::Unsupported,
            _ => CachedResolutionBinding::Ambiguous,
        }
    }
}

struct CCppProducer<'index, 'tree> {
    index: &'index mut CCppResolutionIndex<'tree>,
    source: &'index str,
    namespace_path: Vec<String>,
    active_bindings: HashMap<String, Vec<CCppLexicalBinding>>,
    scopes: Vec<CCppScope>,
}

impl<'index, 'tree> CCppProducer<'index, 'tree> {
    fn new(index: &'index mut CCppResolutionIndex<'tree>, source: &'index str) -> Self {
        Self {
            index,
            source,
            namespace_path: Vec::new(),
            active_bindings: HashMap::new(),
            scopes: vec![CCppScope {
                names: HashSet::new(),
                insertions: Vec::new(),
            }],
        }
    }

    fn visit(&mut self, node: TsNode<'tree>, context: CCppWalkContext) {
        count_c_cpp_resolution_work(1);
        let namespace_name = (node.kind() == "namespace_definition")
            .then(|| node.child_by_field_name("name"))
            .flatten()
            .and_then(|name| node_text(name, self.source))
            .map(str::to_string);
        if let Some(name) = &namespace_name {
            self.namespace_path.push(name.clone());
            self.collect_namespace(node, name);
        }

        let is_callable = node.kind() == "function_definition";
        let is_scope = is_callable || node.kind() == "compound_statement";
        if is_scope {
            count_c_cpp_resolution_work(1);
            self.scopes.push(CCppScope {
                names: HashSet::new(),
                insertions: Vec::new(),
            });
        }
        let mut context = context;
        if matches!(
            node.kind(),
            "template_declaration"
                | "preproc_if"
                | "preproc_ifdef"
                | "preproc_elif"
                | "preproc_else"
                | "lambda_expression"
        ) {
            context.unsupported = true;
        }
        if matches!(node.kind(), "class_specifier" | "struct_specifier") {
            context = self.enter_class(node, context);
        }
        if is_callable {
            context = self.enter_callable(node, context);
        }
        self.collect_declaration(node, context);
        self.collect_parameter(node, context);
        self.collect_write(node, context);
        self.collect_macro(node);
        self.collect_call(node, context);

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.visit(child, context);
        }
        if is_scope {
            self.leave_scope();
        }
        if namespace_name.is_some() {
            self.namespace_path.pop();
        }
    }

    fn collect_namespace(&mut self, node: TsNode<'tree>, name: &str) {
        count_c_cpp_resolution_work(1);
        let candidates = self
            .index
            .namespace_nodes
            .get(&(node.start_position().row as u32 + 1, name.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let [declaration] = candidates {
            count_c_cpp_resolution_work(2);
            self.index.namespaces.push(CachedCCppNamespace {
                path: self.namespace_path.clone(),
                declaration: *declaration,
            });
            self.index
                .namespace_nodes_by_path
                .entry(self.namespace_path.clone())
                .or_default()
                .push(*declaration);
        }
    }

    fn enter_class(
        &mut self,
        node: TsNode<'tree>,
        mut context: CCppWalkContext,
    ) -> CCppWalkContext {
        context.owner_index = None;
        context.unsupported |= node.child_by_field_name("name").is_none()
            || c_cpp_has_direct_kind(node, "base_class_clause")
            || c_cpp_has_direct_kind(node, "template_type");
        let Some(name) = node
            .child_by_field_name("name")
            .and_then(|name| node_text(name, self.source))
            .map(str::to_string)
        else {
            return context;
        };
        count_c_cpp_resolution_work(1);
        let candidates = self
            .index
            .class_nodes
            .get(&(node.start_position().row as u32 + 1, name.clone()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let [declaration] = candidates else {
            return context;
        };
        count_c_cpp_resolution_work(2);
        self.index.classes.push(CachedClassDeclaration {
            name,
            declaration: *declaration,
            methods: Vec::new(),
            cross_module_visible: false,
            runtime_closed: false,
            super_name: None,
        });
        self.index
            .class_namespace_paths
            .push(self.namespace_path.clone());
        let owner_index = self.index.classes.len() - 1;
        count_c_cpp_resolution_work(1);
        self.index
            .class_indices_by_name
            .entry(self.index.classes[owner_index].name.clone())
            .or_default()
            .push(owner_index);
        context.owner_index = Some(owner_index);
        context
    }

    fn enter_callable(
        &mut self,
        node: TsNode<'tree>,
        mut context: CCppWalkContext,
    ) -> CCppWalkContext {
        let Some(declarator) = node.child_by_field_name("declarator") else {
            context.callable_id = Some(node.id());
            context.caller = None;
            context.unsupported = true;
            return context;
        };
        let Some((name, qualifier, signature)) =
            c_cpp_callable_shape(declarator, self.source, self.index.language)
        else {
            context.callable_id = Some(node.id());
            context.caller = None;
            context.unsupported = true;
            return context;
        };
        let (namespace_path, owner_index, qualified_supported) =
            self.resolve_declaration_owner(qualifier.as_deref(), context.owner_index);
        context.owner_index = owner_index;
        context.unsupported |= !qualified_supported;
        let caller = self.map_callable(node, &name);
        let is_virtual = c_cpp_function_is_virtual(node, self.source);
        if is_virtual {
            context.unsupported = true;
        }
        let domain_supported = !context.unsupported && qualified_supported;
        self.record_callable(CCppCallableRecord {
            namespace_path: namespace_path.clone(),
            owner_index,
            name,
            signature: domain_supported.then_some(signature),
            declaration: caller,
            defined: true,
            is_virtual,
            is_static: c_cpp_function_is_static(node, self.source),
        });
        count_c_cpp_resolution_work(1);
        self.index
            .callable_namespace_paths
            .insert(node.id(), namespace_path);
        context.callable_id = Some(node.id());
        context.caller = caller;
        context
    }

    fn map_callable(&self, node: TsNode<'tree>, name: &str) -> Option<NodeId> {
        count_c_cpp_resolution_work(1);
        let candidates = self
            .index
            .callable_nodes
            .get(&(node.start_position().row as u32 + 1, name.to_string()))?;
        candidates
            .first()
            .copied()
            .filter(|_| candidates.len() == 1)
    }

    fn collect_parameter(&mut self, node: TsNode<'tree>, context: CCppWalkContext) {
        if node.kind() != "parameter_declaration" || context.callable_id.is_none() {
            return;
        }
        if node.child_by_field_name("declarator").is_none()
            && node
                .child_by_field_name("type")
                .and_then(|ty| node_text(ty, self.source))
                .is_some_and(|ty| ty.trim() == "void")
        {
            return;
        }
        let Some((name, binding)) = c_cpp_typed_binding(node, self.source) else {
            self.poison_scope(context);
            return;
        };
        self.insert_active(name, binding);
    }

    fn collect_declaration(&mut self, node: TsNode<'tree>, context: CCppWalkContext) {
        if node.kind() == "expression_statement"
            && context.callable_id.is_some()
            && c_cpp_untrusted_declaration_expression(node)
        {
            self.poison_scope(context);
            return;
        }
        if !matches!(node.kind(), "declaration" | "field_declaration") {
            return;
        }
        let Some(ty) = node.child_by_field_name("type") else {
            self.poison_scope(context);
            return;
        };
        let mut cursor = node.walk();
        let declarators = node
            .children_by_field_name("declarator", &mut cursor)
            .collect::<Vec<_>>();
        if declarators.is_empty() {
            self.poison_scope(context);
            return;
        }
        for declarator in declarators {
            count_c_cpp_resolution_work(1);
            let Some(bound_names) = c_cpp_declarator_bound_names(declarator, self.source) else {
                self.poison_scope(context);
                continue;
            };
            if bound_names.is_empty() {
                self.poison_scope(context);
                continue;
            }
            if context.callable_id.is_some() {
                let binding = c_cpp_binding_for_declarator(ty, declarator, self.source);
                for name in bound_names {
                    self.insert_active(name, binding.clone());
                }
                continue;
            }
            if let Some((name, qualifier, signature)) =
                c_cpp_callable_shape(declarator, self.source, self.index.language)
            {
                let (namespace_path, owner_index, qualified_supported) =
                    self.resolve_declaration_owner(qualifier.as_deref(), context.owner_index);
                let declaration = self.map_callable(node, &name);
                self.record_callable(CCppCallableRecord {
                    namespace_path,
                    owner_index,
                    name,
                    signature: (qualified_supported && !context.unsupported).then_some(signature),
                    declaration,
                    defined: false,
                    is_virtual: c_cpp_function_is_virtual(node, self.source),
                    is_static: c_cpp_function_is_static(node, self.source),
                });
                continue;
            }
            let binding = c_cpp_binding_for_declarator(ty, declarator, self.source);
            for name in bound_names {
                if let Some(owner_index) = context.owner_index {
                    count_c_cpp_resolution_work(1);
                    self.index
                        .owner_bindings
                        .entry((owner_index, name))
                        .or_default()
                        .push(binding.clone());
                } else {
                    count_c_cpp_resolution_work(1);
                    self.index
                        .global_shadow_names
                        .insert((self.namespace_path.clone(), name));
                }
            }
        }
    }

    fn record_callable(&mut self, record: CCppCallableRecord) {
        count_c_cpp_resolution_work(1);
        self.index.callable_records.push(record);
    }

    fn resolve_declaration_owner(
        &self,
        qualifier: Option<&[String]>,
        lexical_owner: Option<usize>,
    ) -> (Vec<String>, Option<usize>, bool) {
        let Some(qualifier) = qualifier else {
            return (self.namespace_path.clone(), lexical_owner, true);
        };
        count_c_cpp_resolution_work(1);
        if let [owner_name] = qualifier
            && let Some(owners) = self.index.class_indices_by_name.get(owner_name)
            && let [owner] = owners.as_slice()
        {
            return (
                self.index.class_namespace_paths[*owner].clone(),
                Some(*owner),
                true,
            );
        }
        let namespace = qualifier.to_vec();
        let supported = self
            .index
            .namespace_nodes_by_path
            .get(&namespace)
            .is_some_and(|nodes| nodes.len() == 1);
        (namespace, None, supported)
    }

    fn poison_scope(&mut self, context: CCppWalkContext) {
        count_c_cpp_resolution_work(1);
        if let Some(callable) = context.callable_id {
            self.index.poisoned_callables.insert(callable);
        } else if let Some(owner) = context.owner_index {
            self.index.poisoned_owners.insert(owner);
        } else {
            self.index
                .poisoned_namespaces
                .insert(self.namespace_path.clone());
        }
    }

    fn collect_write(&mut self, node: TsNode<'tree>, context: CCppWalkContext) {
        if node.kind() != "assignment_expression" {
            return;
        }
        let Some(callable_id) = context.callable_id else {
            return;
        };
        let Some(name) = node
            .child_by_field_name("left")
            .filter(|left| left.kind() == "identifier")
            .and_then(|left| node_text(left, self.source))
        else {
            return;
        };
        count_c_cpp_resolution_work(1);
        self.index
            .rebound_receivers
            .insert((callable_id, name.to_string()));
    }

    fn collect_macro(&mut self, node: TsNode<'tree>) {
        if !matches!(node.kind(), "preproc_def" | "preproc_function_def") {
            return;
        }
        let Some(name) = node
            .child_by_field_name("name")
            .and_then(|name| node_text(name, self.source))
        else {
            return;
        };
        count_c_cpp_resolution_work(1);
        self.index.macro_names.insert(name.to_string());
    }

    fn collect_call(&mut self, node: TsNode<'tree>, context: CCppWalkContext) {
        if node.kind() != "call_expression" {
            return;
        }
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        let Some((callee, form, raw_target, receiver)) = self.classify_call(function, context)
        else {
            return;
        };
        let identifier_shadowed =
            form == CalleeForm::Identifier && self.active_bindings.contains_key(&raw_target);
        let namespace_path = context
            .callable_id
            .and_then(|callable| self.index.callable_namespace_paths.get(&callable))
            .cloned()
            .unwrap_or_else(|| self.namespace_path.clone());
        count_c_cpp_resolution_work(2);
        self.index.calls.push(IndexedCCppCall {
            callee,
            form,
            raw_target,
            caller: context.caller,
            callable_id: context.callable_id,
            namespace_path,
            owner_index: context.owner_index,
            unsupported: context.unsupported,
            identifier_shadowed,
            receiver,
        });
    }

    fn classify_call(
        &self,
        function: TsNode<'tree>,
        context: CCppWalkContext,
    ) -> Option<(TsNode<'tree>, CalleeForm, String, CCppCallReceiver)> {
        match function.kind() {
            "identifier" | "type_identifier" => {
                let target = node_text(function, self.source)?.to_string();
                Some((
                    function,
                    CalleeForm::Identifier,
                    target,
                    CCppCallReceiver::None,
                ))
            }
            "qualified_identifier" => {
                let name = function.child_by_field_name("name")?;
                let target = node_text(name, self.source)?.to_string();
                let surface = node_text(function, self.source)?;
                let mut components = surface
                    .split("::")
                    .map(str::trim)
                    .filter(|component| !component.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                components.pop();
                (!components.is_empty()).then_some((
                    name,
                    CalleeForm::QualifiedPath,
                    target,
                    CCppCallReceiver::Qualified(components),
                ))
            }
            "field_expression" => {
                let receiver = function.child_by_field_name("argument")?;
                let field = function.child_by_field_name("field")?;
                let target = node_text(field, self.source)?.to_string();
                let operator = function
                    .child_by_field_name("operator")
                    .and_then(|operator| node_text(operator, self.source))
                    .unwrap_or_default();
                if receiver.kind() == "this" {
                    return Some((
                        field,
                        CalleeForm::ImplicitReceiver,
                        target,
                        CCppCallReceiver::Implicit,
                    ));
                }
                if operator != "." {
                    return Some((
                        field,
                        CalleeForm::ExplicitReceiver,
                        target,
                        CCppCallReceiver::Blocked,
                    ));
                }
                if let Some(owner_name) = c_cpp_direct_constructor_type(receiver, self.source) {
                    return Some((
                        field,
                        CalleeForm::ExplicitReceiver,
                        target,
                        CCppCallReceiver::ExactType {
                            owner_name,
                            constructor: true,
                            receiver_name: None,
                        },
                    ));
                }
                let receiver_name = node_text(receiver, self.source)?.trim();
                if !c_cpp_simple_identifier(receiver_name) {
                    return Some((
                        field,
                        CalleeForm::ExplicitReceiver,
                        target,
                        CCppCallReceiver::Blocked,
                    ));
                }
                let binding = self
                    .active_bindings
                    .get(receiver_name)
                    .and_then(|bindings| bindings.last())
                    .or_else(|| {
                        context.owner_index.and_then(|owner| {
                            self.index
                                .owner_bindings
                                .get(&(owner, receiver_name.to_string()))
                                .and_then(|bindings| {
                                    let [binding] = bindings.as_slice() else {
                                        return None;
                                    };
                                    Some(binding)
                                })
                        })
                    });
                let receiver = match binding {
                    Some(CCppLexicalBinding::Receiver { owner_name }) => {
                        CCppCallReceiver::ExactType {
                            owner_name: owner_name.clone(),
                            constructor: false,
                            receiver_name: Some(receiver_name.to_string()),
                        }
                    }
                    _ => CCppCallReceiver::Blocked,
                };
                Some((field, CalleeForm::ExplicitReceiver, target, receiver))
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
                    node_text(leaf, self.source)?.to_string(),
                    CCppCallReceiver::Blocked,
                ))
            }
        }
    }

    fn insert_active(&mut self, name: String, binding: CCppLexicalBinding) {
        count_c_cpp_resolution_work(1);
        let scope = self
            .scopes
            .last_mut()
            .expect("C/C++ producer always has one lexical scope");
        let duplicate_in_scope = scope.names.contains(&name);
        count_c_cpp_resolution_work(1);
        scope.names.insert(name.clone());
        count_c_cpp_resolution_work(1);
        scope.insertions.push(name.clone());
        count_c_cpp_resolution_work(1);
        self.active_bindings
            .entry(name.clone())
            .or_default()
            .push(if duplicate_in_scope {
                CCppLexicalBinding::Other
            } else {
                binding
            });
    }

    fn leave_scope(&mut self) {
        let scope = self
            .scopes
            .pop()
            .expect("C/C++ producer scope stack is balanced");
        for name in scope.insertions {
            count_c_cpp_resolution_work(1);
            if let Some(bindings) = self.active_bindings.get_mut(&name) {
                bindings.pop();
                if bindings.is_empty() {
                    self.active_bindings.remove(&name);
                }
            }
        }
    }
}

#[cfg(test)]
fn c_cpp_function_name<'a>(node: TsNode<'a>, source: &'a str) -> Option<&'a str> {
    c_cpp_declarator_identifier(node.child_by_field_name("declarator")?, source)
}

fn c_cpp_callable_shape(
    declarator: TsNode<'_>,
    source: &str,
    language: &str,
) -> Option<(String, Option<Vec<String>>, String)> {
    let function = c_cpp_function_declarator(declarator)?;
    let name_declarator = function.child_by_field_name("declarator")?;
    if c_cpp_declarator_contains_kind(name_declarator, "pointer_declarator") {
        return None;
    }
    let name = c_cpp_declarator_identifier(name_declarator, source)?.to_string();
    let name_surface = node_text(name_declarator, source)?.trim();
    let qualifier = name_surface.rsplit_once("::").and_then(|(qualifier, _)| {
        let components = qualifier
            .split("::")
            .map(str::trim)
            .filter(|component| c_cpp_simple_identifier(component))
            .map(str::to_string)
            .collect::<Vec<_>>();
        (!components.is_empty()).then_some(components)
    });
    let parameters = function.child_by_field_name("parameters")?;
    let suffix = source.get(parameters.end_byte()..function.end_byte())?;
    let signature = format!(
        "{language}:{}:{}",
        c_cpp_normalize_signature_text(node_text(parameters, source)?),
        c_cpp_normalize_signature_text(suffix)
    );
    Some((name, qualifier, signature))
}

fn c_cpp_function_declarator(mut declarator: TsNode<'_>) -> Option<TsNode<'_>> {
    loop {
        if declarator.kind() == "function_declarator" {
            return Some(declarator);
        }
        declarator = declarator.child_by_field_name("declarator").or_else(|| {
            let mut cursor = declarator.walk();
            declarator.named_children(&mut cursor).last()
        })?;
        count_c_cpp_resolution_work(1);
    }
}

fn c_cpp_normalize_signature_text(surface: &str) -> String {
    surface
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn c_cpp_declarator_bound_names(node: TsNode<'_>, source: &str) -> Option<Vec<String>> {
    fn collect(node: TsNode<'_>, source: &str, names: &mut Vec<String>) -> Option<()> {
        count_c_cpp_resolution_work(1);
        match node.kind() {
            "identifier" | "field_identifier" | "type_identifier" => {
                names.push(node_text(node, source)?.to_string());
                Some(())
            }
            "qualified_identifier" => collect(node.child_by_field_name("name")?, source, names),
            "structured_binding_declarator" => {
                let mut cursor = node.walk();
                let bindings = node.named_children(&mut cursor).collect::<Vec<_>>();
                if bindings.is_empty()
                    || bindings
                        .iter()
                        .any(|binding| binding.kind() != "identifier")
                {
                    return None;
                }
                for binding in bindings {
                    collect(binding, source, names)?;
                }
                Some(())
            }
            "init_declarator"
            | "pointer_declarator"
            | "reference_declarator"
            | "array_declarator"
            | "function_declarator"
            | "parenthesized_declarator"
            | "attributed_declarator"
            | "variadic_declarator" => {
                let declarator = node.child_by_field_name("declarator").or_else(|| {
                    let mut cursor = node.walk();
                    let children = node.named_children(&mut cursor).collect::<Vec<_>>();
                    let [declarator] = children.as_slice() else {
                        return None;
                    };
                    Some(*declarator)
                })?;
                collect(declarator, source, names)
            }
            _ => None,
        }
    }

    let mut names = Vec::new();
    collect(node, source, &mut names)?;
    names.sort();
    names.dedup();
    (!names.is_empty()).then_some(names)
}

fn c_cpp_untrusted_declaration_expression(root: TsNode<'_>) -> bool {
    let mut stack = vec![root];
    let mut has_type_callee = false;
    let mut has_pointer = false;
    while let Some(node) = stack.pop() {
        count_c_cpp_resolution_work(1);
        if node.kind() == "pointer_expression" {
            has_pointer = true;
        }
        if node.kind() == "call_expression"
            && node
                .child_by_field_name("function")
                .is_some_and(|function| {
                    matches!(function.kind(), "primitive_type" | "type_identifier")
                })
        {
            has_type_callee = true;
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    has_type_callee && has_pointer
}

fn c_cpp_declarator_identifier<'a>(mut node: TsNode<'_>, source: &'a str) -> Option<&'a str> {
    loop {
        if matches!(
            node.kind(),
            "identifier" | "field_identifier" | "type_identifier"
        ) {
            return node_text(node, source);
        }
        if matches!(node.kind(), "operator_name" | "operator_cast") {
            return None;
        }
        let next = node
            .child_by_field_name("declarator")
            .or_else(|| node.child_by_field_name("name"))
            .or_else(|| {
                let mut cursor = node.walk();
                node.named_children(&mut cursor).last()
            })?;
        count_c_cpp_resolution_work(1);
        node = next;
    }
}

fn c_cpp_typed_binding(node: TsNode<'_>, source: &str) -> Option<(String, CCppLexicalBinding)> {
    count_c_cpp_resolution_work(2);
    let ty = node.child_by_field_name("type")?;
    let declarator = node.child_by_field_name("declarator")?;
    let names = c_cpp_declarator_bound_names(declarator, source)?;
    let [name] = names.as_slice() else {
        return None;
    };
    Some((
        name.clone(),
        c_cpp_binding_for_declarator(ty, declarator, source),
    ))
}

fn c_cpp_binding_for_declarator(
    ty: TsNode<'_>,
    declarator: TsNode<'_>,
    source: &str,
) -> CCppLexicalBinding {
    let type_name = c_cpp_simple_type_name(ty, source);
    let pointer = c_cpp_declarator_contains_kind(declarator, "pointer_declarator");
    type_name
        .filter(|_| !pointer)
        .map_or(CCppLexicalBinding::Other, |owner_name| {
            CCppLexicalBinding::Receiver { owner_name }
        })
}

fn c_cpp_simple_type_name(node: TsNode<'_>, source: &str) -> Option<String> {
    let surface = node_text(node, source)?.trim();
    c_cpp_simple_identifier(surface).then(|| surface.to_string())
}

fn c_cpp_simple_identifier(surface: &str) -> bool {
    let mut chars = surface.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn c_cpp_declarator_contains_kind(root: TsNode<'_>, kind: &str) -> bool {
    let mut node = Some(root);
    while let Some(current) = node {
        count_c_cpp_resolution_work(1);
        if current.kind() == kind {
            return true;
        }
        node = current.child_by_field_name("declarator").or_else(|| {
            let mut cursor = current.walk();
            current.named_children(&mut cursor).last()
        });
    }
    false
}

fn c_cpp_direct_constructor_type(node: TsNode<'_>, source: &str) -> Option<String> {
    (node.kind() == "call_expression").then_some(())?;
    let function = node.child_by_field_name("function")?;
    matches!(function.kind(), "identifier" | "type_identifier")
        .then(|| node_text(function, source))
        .flatten()
        .filter(|name| c_cpp_simple_identifier(name))
        .map(str::to_string)
}

fn c_cpp_function_is_virtual(node: TsNode<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| {
        child.kind() == "virtual" || node_text(child, source).is_some_and(|text| text == "virtual")
    })
}

fn c_cpp_function_is_static(node: TsNode<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| {
        child.kind() == "static" || node_text(child, source).is_some_and(|text| text == "static")
    })
}

fn c_cpp_has_direct_kind(node: TsNode<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind() == kind)
}

fn c_cpp_generated_source(source: &str) -> bool {
    source
        .lines()
        .take(8)
        .any(|line| line.to_ascii_lowercase().contains("generated"))
}

fn c_cpp_source_role(path: &Path) -> CachedCCppSourceRole {
    match crate::normalized_path_extension(path).as_deref() {
        Some("h" | "hpp" | "hh" | "hxx") => CachedCCppSourceRole::Header,
        _ => CachedCCppSourceRole::Source,
    }
}

fn c_cpp_sort_calls_by_span(calls: &mut Vec<IndexedCCppCall<'_>>) {
    for shift in (0..usize::BITS).step_by(8) {
        let mut buckets = (0..=u8::MAX)
            .map(|_| Vec::new())
            .collect::<Vec<Vec<IndexedCCppCall<'_>>>>();
        for call in std::mem::take(calls) {
            count_c_cpp_resolution_work(1);
            let bucket = (call.callee.start_byte() >> shift) & usize::from(u8::MAX);
            buckets[bucket].push(call);
        }
        for bucket in buckets {
            count_c_cpp_resolution_work(bucket.len());
            calls.extend(bucket);
        }
    }
}

fn parser_config_for_indexed_language(
    path: &Path,
    selected_language: &str,
) -> Option<crate::LanguageConfig> {
    let extension = crate::normalized_path_extension(path)?;
    let extension_config = crate::get_language_for_ext(&extension)?;
    if extension_config.language_name == selected_language {
        return Some(extension_config);
    }

    (extension == "h" && selected_language == "cpp").then(crate::cpp_language_config)
}

fn expected_parser_fingerprint(path: &Path, language: &str) -> Option<String> {
    parser_config_for_indexed_language(path, language)
        .map(|config| crate::resolution_parser_fingerprint(&config))
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
                    && file.adapter_version == adapter_version(language)
                    && file.parser_fingerprint == expected_parser_fingerprint
                    && artifact.call_resolution_inputs.iter().all(|call| {
                        call.language == language
                            && call.adapter_version == adapter_version(language)
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
struct IndexedJavaKotlinCall<'tree> {
    callee: TsNode<'tree>,
    form: CalleeForm,
    raw_target: String,
    caller: Option<NodeId>,
    callable_id: Option<usize>,
    owner_name: Option<String>,
    unsupported: bool,
    identifier_shadowed: bool,
    receiver: JavaKotlinCallReceiver,
}

#[derive(Debug, Clone)]
struct JavaKotlinImportBinding {
    package_name: String,
    owner_name: Option<String>,
    imported_name: String,
    local_name: String,
    node: NodeId,
    visible_names: Option<HashSet<String>>,
    hidden_names: HashSet<String>,
}

impl JavaKotlinImportBinding {
    fn allows(&self, name: &str) -> bool {
        self.visible_names
            .as_ref()
            .is_none_or(|visible| visible.contains(name))
            && !self.hidden_names.contains(name)
    }
}

#[derive(Debug, Clone)]
enum JavaKotlinCallReceiver {
    None,
    Implicit,
    ExactType {
        owner_name: String,
        constructor: bool,
        receiver_name: Option<String>,
        import_prefix: Option<String>,
    },
    Named {
        receiver_name: String,
    },
    Blocked,
}

#[derive(Debug, Clone)]
enum JavaKotlinLexicalBinding {
    Other,
    Receiver {
        owner_name: String,
        constructor: bool,
    },
}

#[derive(Clone, Copy)]
struct JavaKotlinWalkContext {
    callable_id: Option<usize>,
    caller: Option<NodeId>,
    owner_index: Option<usize>,
    owner_virtual: bool,
    unsupported: bool,
}

struct JavaKotlinResolutionIndex<'tree> {
    language: &'tree str,
    calls: Vec<IndexedJavaKotlinCall<'tree>>,
    call_indices_by_span: HashMap<(usize, usize), usize>,
    declarations: Vec<CachedTopLevelDeclaration>,
    declaration_indices_by_name: HashMap<String, Vec<usize>>,
    classes: Vec<CachedClassDeclaration>,
    class_indices_by_name: HashMap<String, Vec<usize>>,
    class_method_indices_by_name: HashMap<(usize, String), Vec<usize>>,
    class_names: HashSet<String>,
    callable_nodes: HashMap<(u32, String), Vec<NodeId>>,
    class_nodes: HashMap<(u32, String), Vec<NodeId>>,
    import_nodes: HashMap<u32, Vec<NodeId>>,
    import_names: HashMap<NodeId, String>,
    imports_by_name: HashMap<String, Vec<JavaKotlinImportBinding>>,
    type_imports_by_name: HashMap<String, Vec<JavaKotlinImportBinding>>,
    whole_module_imports: Vec<JavaKotlinImportBinding>,
    prefixed_imports: HashMap<String, Vec<JavaKotlinImportBinding>>,
    owner_bindings: HashMap<(String, String), Vec<JavaKotlinLexicalBinding>>,
    rebound_receivers: HashSet<(usize, String)>,
    unsupported_type_names: HashSet<String>,
    package_name: Option<String>,
    wildcard_import: bool,
    overloads: HashSet<String>,
    virtual_methods: HashSet<String>,
    extension_methods: HashSet<String>,
    has_annotated_declaration: bool,
    has_delegation: bool,
    domain_poisoned: bool,
}

impl<'tree> JavaKotlinResolutionIndex<'tree> {
    fn build(
        tree: &'tree Tree,
        source: &str,
        source_path: &Path,
        language: &'tree str,
        file_id: NodeId,
        nodes: &[Node],
    ) -> Self {
        let mut callable_nodes = HashMap::<(u32, String), Vec<NodeId>>::new();
        let mut class_nodes = HashMap::<(u32, String), Vec<NodeId>>::new();
        let mut import_nodes = HashMap::<u32, Vec<NodeId>>::new();
        let mut import_names = HashMap::<NodeId, String>::new();
        for node in nodes
            .iter()
            .filter(|node| node.file_node_id == Some(file_id))
        {
            count_java_kotlin_resolution_work(1);
            let Some(line) = node.start_line else {
                continue;
            };
            let name = graph_leaf_name(&node.serialized_name).to_string();
            match node.kind {
                NodeKind::FUNCTION | NodeKind::METHOD => {
                    count_java_kotlin_resolution_work(1);
                    callable_nodes
                        .entry((line, name))
                        .or_default()
                        .push(node.id);
                }
                NodeKind::CLASS | NodeKind::STRUCT => {
                    count_java_kotlin_resolution_work(1);
                    class_nodes.entry((line, name)).or_default().push(node.id);
                }
                NodeKind::MODULE | NodeKind::UNKNOWN => {
                    count_java_kotlin_resolution_work(1);
                    import_nodes.entry(line).or_default().push(node.id);
                    import_names.insert(node.id, node.serialized_name.clone());
                }
                _ => {}
            }
        }

        let nominal_source_poison = is_csharp_swift_dart_language(language)
            && (generated_source_marker(source)
                || language == "swift"
                    && source
                        .lines()
                        .any(|line| line.trim_start().starts_with("extension ")));
        let mut result = Self {
            language,
            calls: Vec::new(),
            call_indices_by_span: HashMap::new(),
            declarations: Vec::new(),
            declaration_indices_by_name: HashMap::new(),
            classes: Vec::new(),
            class_indices_by_name: HashMap::new(),
            class_method_indices_by_name: HashMap::new(),
            class_names: HashSet::new(),
            callable_nodes,
            class_nodes,
            import_nodes,
            import_names,
            imports_by_name: HashMap::new(),
            type_imports_by_name: HashMap::new(),
            whole_module_imports: Vec::new(),
            prefixed_imports: HashMap::new(),
            owner_bindings: HashMap::new(),
            rebound_receivers: HashSet::new(),
            unsupported_type_names: HashSet::new(),
            package_name: csd_source_domain(language, source_path),
            wildcard_import: false,
            overloads: HashSet::new(),
            virtual_methods: HashSet::new(),
            extension_methods: HashSet::new(),
            has_annotated_declaration: nominal_source_poison,
            has_delegation: false,
            domain_poisoned: nominal_source_poison,
        };
        let root = tree.root_node();
        JavaKotlinProducer::new(&mut result, source, root.id()).visit(
            root,
            JavaKotlinWalkContext {
                callable_id: None,
                caller: None,
                owner_index: None,
                owner_virtual: false,
                unsupported: false,
            },
        );

        if is_java_kotlin_language(language) {
            result
                .calls
                .sort_by_key(|call| (call.callee.start_byte(), call.callee.end_byte()));
        }
        for (index, call) in result.calls.iter().enumerate() {
            count_java_kotlin_resolution_work(1);
            result
                .call_indices_by_span
                .insert((call.callee.start_byte(), call.callee.end_byte()), index);
        }
        if is_java_kotlin_language(language) {
            result.declarations.sort_by(|left, right| {
                left.name
                    .cmp(&right.name)
                    .then(left.declaration.cmp(&right.declaration))
            });
            result.classes.sort_by_key(|class| class.declaration);
        }
        for (index, declaration) in result.declarations.iter().enumerate() {
            count_java_kotlin_resolution_work(1);
            result
                .declaration_indices_by_name
                .entry(declaration.name.clone())
                .or_default()
                .push(index);
        }
        for (index, class) in result.classes.iter_mut().enumerate() {
            if is_java_kotlin_language(language) {
                class.methods.sort_by(|left, right| {
                    left.name
                        .cmp(&right.name)
                        .then(left.declaration.cmp(&right.declaration))
                });
            }
            count_java_kotlin_resolution_work(2);
            result
                .class_indices_by_name
                .entry(class.name.clone())
                .or_default()
                .push(index);
            result.class_names.insert(class.name.clone());
            for (method_index, method) in class.methods.iter().enumerate() {
                count_java_kotlin_resolution_work(1);
                result
                    .class_method_indices_by_name
                    .entry((index, method.name.clone()))
                    .or_default()
                    .push(method_index);
            }
        }
        result
    }

    fn map_callable_declaration(&self, node: TsNode<'_>, source: &str) -> Option<NodeId> {
        let name = declaration_name(node, source)?;
        count_java_kotlin_resolution_work(1);
        let matches = self
            .callable_nodes
            .get(&(node.start_position().row as u32 + 1, name.to_string()))?;
        matches.first().copied().filter(|_| matches.len() == 1)
    }

    fn map_class_declaration(&self, node: TsNode<'_>, source: &str) -> Option<NodeId> {
        let name = declaration_name(node, source)?;
        count_java_kotlin_resolution_work(1);
        let matches = self
            .class_nodes
            .get(&(node.start_position().row as u32 + 1, name.to_string()))?;
        matches.first().copied().filter(|_| matches.len() == 1)
    }

    fn resolve_syntax_claim(
        &self,
        _source: &str,
        callee: TsNode<'tree>,
        form: CalleeForm,
        raw_target: &str,
    ) -> (Option<NodeId>, CachedResolutionBinding) {
        count_java_kotlin_resolution_work(1);
        let Some(call_index) = self
            .call_indices_by_span
            .get(&(callee.start_byte(), callee.end_byte()))
        else {
            return (None, CachedResolutionBinding::Unsupported);
        };
        let call = &self.calls[*call_index];
        let Some(caller) = call.caller else {
            return (None, CachedResolutionBinding::Unsupported);
        };
        if call.form != form || call.raw_target != raw_target {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        count_java_kotlin_resolution_work(4);
        if call.unsupported
            || self.wildcard_import
            || self.virtual_methods.contains(raw_target)
            || self.extension_methods.contains(raw_target)
            || self.has_delegation
            || self.has_annotated_declaration
            || self.domain_poisoned
        {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }

        if form == CalleeForm::Identifier {
            if call.identifier_shadowed {
                return (
                    Some(caller),
                    if is_csharp_swift_dart_language(self.language) {
                        CachedResolutionBinding::Unsupported
                    } else {
                        CachedResolutionBinding::MissingBinding
                    },
                );
            }
            count_java_kotlin_resolution_work(1);
            if self.overloads.contains(&format!("overload:{raw_target}")) {
                return (Some(caller), CachedResolutionBinding::Ambiguous);
            }
            count_java_kotlin_resolution_work(1);
            if !is_csharp_swift_dart_language(self.language)
                && let Some(imports) = self.imports_by_name.get(raw_target)
            {
                return match imports.as_slice() {
                    [import] => (
                        Some(caller),
                        CachedResolutionBinding::JavaKotlinImportedFunction {
                            package_name: import.package_name.clone(),
                            owner_name: import.owner_name.clone(),
                            name: import.imported_name.clone(),
                            import: import.node,
                        },
                    ),
                    _ => (Some(caller), CachedResolutionBinding::Ambiguous),
                };
            }
            count_java_kotlin_resolution_work(1);
            let candidates = self
                .declaration_indices_by_name
                .get(raw_target)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if is_csharp_swift_dart_language(self.language) {
                return match candidates {
                    [_declaration] => (
                        Some(caller),
                        CachedResolutionBinding::JavaKotlinPackageFunction {
                            package_name: self.package_name.clone().unwrap_or_default(),
                            name: raw_target.to_string(),
                        },
                    ),
                    [] if self.imports_by_name.contains_key(raw_target) => {
                        match self.imports_by_name[raw_target].as_slice() {
                            [import] => (
                                Some(caller),
                                CachedResolutionBinding::JavaKotlinImportedFunction {
                                    package_name: import.package_name.clone(),
                                    owner_name: None,
                                    name: import.imported_name.clone(),
                                    import: import.node,
                                },
                            ),
                            _ => (Some(caller), CachedResolutionBinding::Ambiguous),
                        }
                    }
                    [] if matches!(self.whole_module_imports.as_slice(), [import] if import.allows(raw_target)) =>
                    {
                        let import = &self.whole_module_imports[0];
                        (
                            Some(caller),
                            CachedResolutionBinding::JavaKotlinImportedFunction {
                                package_name: import.package_name.clone(),
                                owner_name: None,
                                name: raw_target.to_string(),
                                import: import.node,
                            },
                        )
                    }
                    [] if !self.whole_module_imports.is_empty() => {
                        (Some(caller), CachedResolutionBinding::Ambiguous)
                    }
                    [] if matches!(self.language, "csharp" | "swift")
                        && self.package_name.is_some() =>
                    {
                        (
                            Some(caller),
                            CachedResolutionBinding::JavaKotlinPackageFunction {
                                package_name: self.package_name.clone().unwrap_or_default(),
                                name: raw_target.to_string(),
                            },
                        )
                    }
                    [] => (Some(caller), CachedResolutionBinding::MissingBinding),
                    _ => (Some(caller), CachedResolutionBinding::Ambiguous),
                };
            }
            return match candidates {
                [declaration] => (
                    Some(caller),
                    CachedResolutionBinding::SameFile {
                        declaration: self.declarations[*declaration].declaration,
                        rust_glob_local_module: None,
                    },
                ),
                [] if matches!(self.language, "kotlin" | "csharp" | "swift" | "dart")
                    && self.package_name.is_some() =>
                {
                    (
                        Some(caller),
                        CachedResolutionBinding::JavaKotlinPackageFunction {
                            package_name: self.package_name.clone().unwrap_or_default(),
                            name: raw_target.to_string(),
                        },
                    )
                }
                [] => (Some(caller), CachedResolutionBinding::MissingBinding),
                _ => (Some(caller), CachedResolutionBinding::Ambiguous),
            };
        }

        let receiver_name = match &call.receiver {
            JavaKotlinCallReceiver::ExactType { receiver_name, .. } => receiver_name.as_ref(),
            JavaKotlinCallReceiver::Named { receiver_name } => Some(receiver_name),
            JavaKotlinCallReceiver::None
            | JavaKotlinCallReceiver::Implicit
            | JavaKotlinCallReceiver::Blocked => None,
        };
        if let (Some(callable_id), Some(receiver_name)) = (call.callable_id, receiver_name) {
            count_java_kotlin_resolution_work(1);
            if self
                .rebound_receivers
                .contains(&(callable_id, receiver_name.clone()))
            {
                return (Some(caller), CachedResolutionBinding::Ambiguous);
            }
        }

        if self.language == "dart"
            && let JavaKotlinCallReceiver::Named { receiver_name } = &call.receiver
            && let Some(imports) = self.prefixed_imports.get(receiver_name)
        {
            count_java_kotlin_resolution_work(1);
            return match imports.as_slice() {
                [import] if import.allows(raw_target) => (
                    Some(caller),
                    CachedResolutionBinding::JavaKotlinImportedFunction {
                        package_name: import.package_name.clone(),
                        owner_name: None,
                        name: raw_target.to_string(),
                        import: import.node,
                    },
                ),
                _ => (Some(caller), CachedResolutionBinding::Ambiguous),
            };
        }

        let Some((owner_name, constructor)) = self.call_receiver_owner(call) else {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        };
        count_java_kotlin_resolution_work(1);
        if self.unsupported_type_names.contains(&owner_name) {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        if let JavaKotlinCallReceiver::ExactType {
            import_prefix: Some(prefix),
            ..
        } = &call.receiver
        {
            count_java_kotlin_resolution_work(1);
            return match self.prefixed_imports.get(prefix).map(Vec::as_slice) {
                Some([import]) if import.allows(&owner_name) => (
                    Some(caller),
                    CachedResolutionBinding::JavaKotlinImportedReceiver {
                        package_name: import.package_name.clone(),
                        owner_name,
                        method_name: raw_target.to_string(),
                        import: import.node,
                        constructor,
                    },
                ),
                _ => (Some(caller), CachedResolutionBinding::Ambiguous),
            };
        }
        count_java_kotlin_resolution_work(1);
        if let Some(imports) = self.type_imports_by_name.get(&owner_name) {
            return match imports.as_slice() {
                [import] => (
                    Some(caller),
                    CachedResolutionBinding::JavaKotlinImportedReceiver {
                        package_name: import.package_name.clone(),
                        owner_name: import.imported_name.clone(),
                        method_name: raw_target.to_string(),
                        import: import.node,
                        constructor,
                    },
                ),
                _ => (Some(caller), CachedResolutionBinding::Ambiguous),
            };
        }
        if matches!(self.language, "swift" | "dart") && !self.class_names.contains(&owner_name) {
            count_java_kotlin_resolution_work(1);
            return match self.whole_module_imports.as_slice() {
                [import] if import.allows(&owner_name) => (
                    Some(caller),
                    CachedResolutionBinding::JavaKotlinImportedReceiver {
                        package_name: import.package_name.clone(),
                        owner_name,
                        method_name: raw_target.to_string(),
                        import: import.node,
                        constructor,
                    },
                ),
                [] => (Some(caller), CachedResolutionBinding::MissingBinding),
                _ => (Some(caller), CachedResolutionBinding::Ambiguous),
            };
        }
        count_java_kotlin_resolution_work(1);
        if matches!(self.language, "java" | "csharp" | "swift" | "dart")
            && !self.class_names.contains(&owner_name)
            && let Some(package_name) = &self.package_name
        {
            return (
                Some(caller),
                CachedResolutionBinding::JavaKotlinPackageReceiver {
                    package_name: package_name.clone(),
                    owner_name,
                    method_name: raw_target.to_string(),
                    constructor,
                },
            );
        }
        count_java_kotlin_resolution_work(1);
        if self.language == "kotlin" && !constructor && !self.class_names.contains(&owner_name) {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        count_java_kotlin_resolution_work(1);
        let classes = self
            .class_indices_by_name
            .get(&owner_name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let [class_index] = classes else {
            return (
                Some(caller),
                if classes.is_empty() {
                    CachedResolutionBinding::MissingBinding
                } else {
                    CachedResolutionBinding::Ambiguous
                },
            );
        };
        let class = &self.classes[*class_index];
        count_java_kotlin_resolution_work(1);
        let matching_methods = self
            .class_method_indices_by_name
            .get(&(*class_index, raw_target.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let [method_index] = matching_methods else {
            return (
                Some(caller),
                if matching_methods.is_empty() {
                    CachedResolutionBinding::MissingBinding
                } else {
                    CachedResolutionBinding::Ambiguous
                },
            );
        };
        if is_csharp_swift_dart_language(self.language) {
            return (
                Some(caller),
                CachedResolutionBinding::JavaKotlinPackageReceiver {
                    package_name: self.package_name.clone().unwrap_or_default(),
                    owner_name,
                    method_name: raw_target.to_string(),
                    constructor,
                },
            );
        }
        if form == CalleeForm::ImplicitReceiver {
            return (
                Some(caller),
                CachedResolutionBinding::ImplicitReceiver {
                    owner: class.declaration,
                    declaration: class.methods[*method_index].declaration,
                    owner_name,
                },
            );
        }
        let class_binding = CachedClassBinding::SameFile {
            owner: class.declaration,
            owner_name,
        };
        (
            Some(caller),
            if constructor {
                CachedResolutionBinding::ConstructorBinding {
                    class_binding,
                    method_name: raw_target.to_string(),
                }
            } else {
                CachedResolutionBinding::ExplicitReceiverType {
                    class_binding,
                    method_name: raw_target.to_string(),
                }
            },
        )
    }

    fn call_receiver_owner(&self, call: &IndexedJavaKotlinCall<'_>) -> Option<(String, bool)> {
        match &call.receiver {
            JavaKotlinCallReceiver::Implicit => call.owner_name.clone().map(|owner| (owner, false)),
            JavaKotlinCallReceiver::ExactType {
                owner_name,
                constructor,
                ..
            } => Some((owner_name.clone(), *constructor)),
            JavaKotlinCallReceiver::Named { receiver_name } => {
                let owner_name = call.owner_name.as_ref()?;
                count_java_kotlin_resolution_work(1);
                let bindings = self
                    .owner_bindings
                    .get(&(owner_name.clone(), receiver_name.clone()))?;
                match bindings.as_slice() {
                    [
                        JavaKotlinLexicalBinding::Receiver {
                            owner_name,
                            constructor,
                        },
                    ] => Some((owner_name.clone(), *constructor)),
                    _ => None,
                }
            }
            JavaKotlinCallReceiver::None | JavaKotlinCallReceiver::Blocked => None,
        }
    }
}

struct JavaKotlinProducer<'index, 'tree> {
    index: &'index mut JavaKotlinResolutionIndex<'tree>,
    source: &'index str,
    root_id: usize,
    active_bindings: HashMap<String, Vec<JavaKotlinLexicalBinding>>,
    scope_insertions: Vec<Vec<String>>,
}

impl<'index, 'tree> JavaKotlinProducer<'index, 'tree> {
    fn new(
        index: &'index mut JavaKotlinResolutionIndex<'tree>,
        source: &'index str,
        root_id: usize,
    ) -> Self {
        Self {
            index,
            source,
            root_id,
            active_bindings: HashMap::new(),
            scope_insertions: vec![Vec::new()],
        }
    }

    fn visit(&mut self, node: TsNode<'tree>, context: JavaKotlinWalkContext) {
        count_java_kotlin_resolution_work(1);
        let is_callable = java_kotlin_callable_kind(self.index.language, node.kind());
        let is_scope = is_callable || node.kind() == "block";
        if is_scope {
            self.scope_insertions.push(Vec::new());
        }

        let mut context = context;
        if matches!(
            node.kind(),
            "interface_declaration"
                | "protocol_declaration"
                | "mixin_declaration"
                | "extension_declaration"
                | "when_expression"
        ) || self.index.language == "java" && node.kind() == "cast_expression"
            || self.index.language == "swift"
                && matches!(node.kind(), "statements" | "directive")
                && node_text(node, self.source).is_some_and(|surface| surface.contains("#if"))
        {
            context.unsupported = true;
            if is_csharp_swift_dart_language(self.index.language) {
                self.index.domain_poisoned = true;
            }
        }
        if matches!(
            node.kind(),
            "interface_declaration" | "protocol_declaration" | "mixin_declaration"
        ) && let Some(name) = declaration_name(node, self.source)
        {
            count_java_kotlin_resolution_work(1);
            self.index.unsupported_type_names.insert(name.to_string());
        }

        if java_kotlin_class_kind(self.index.language, node.kind()) {
            context = self.enter_class(node, context);
        }
        if is_callable {
            context = self.enter_callable(node, context);
        }

        self.collect_package(node);
        self.collect_import(node);
        self.collect_binding(node, context);
        self.collect_write(node, context);
        self.collect_call(node, context);

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.visit(child, context);
        }

        if is_scope {
            self.leave_scope();
        }
    }

    fn enter_class(
        &mut self,
        node: TsNode<'tree>,
        mut context: JavaKotlinWalkContext,
    ) -> JavaKotlinWalkContext {
        let annotated = java_kotlin_declaration_has_annotation(node, self.source);
        let delegated = self.index.language == "kotlin"
            && java_kotlin_has_direct_child_kind(node, "delegation_specifiers");
        let nominal_poison = csd_owner_is_unsupported(node, self.source, self.index.language);
        let virtual_owner = self.index.language == "java"
            && node.child_by_field_name("superclass").is_some()
            || matches!(self.index.language, "swift" | "dart") && nominal_poison;
        self.index.has_annotated_declaration |= annotated;
        self.index.has_delegation |= delegated;
        self.index.domain_poisoned |= nominal_poison;
        context.unsupported |= annotated
            || delegated
            || nominal_poison
            || virtual_owner
            || node.child_by_field_name("type_parameters").is_some()
            || java_kotlin_has_direct_child_kind(node, "type_parameters");
        context.owner_virtual = virtual_owner;
        context.owner_index = None;

        let Some(name) = declaration_name(node, self.source).map(str::to_string) else {
            return context;
        };
        let Some(declaration) = self.index.map_class_declaration(node, self.source) else {
            return context;
        };
        count_java_kotlin_resolution_work(2);
        self.index.class_names.insert(name.clone());
        let cross_module_visible = csd_cross_module_visible(node, self.source, self.index.language);
        let runtime_closed = csd_runtime_closed(node, self.source, self.index.language);
        let super_name = csd_super_name(node, self.source, self.index.language);
        self.index.classes.push(CachedClassDeclaration {
            name,
            declaration,
            methods: Vec::new(),
            cross_module_visible,
            runtime_closed,
            super_name,
        });
        context.owner_index = Some(self.index.classes.len() - 1);
        context
    }

    fn enter_callable(
        &mut self,
        node: TsNode<'tree>,
        mut context: JavaKotlinWalkContext,
    ) -> JavaKotlinWalkContext {
        let declaration_node = nominal_callable_declaration(node, self.index.language);
        let annotated = java_kotlin_declaration_has_annotation(declaration_node, self.source);
        let unsupported_callable =
            csd_callable_is_unsupported(declaration_node, self.source, self.index.language);
        self.index.has_annotated_declaration |= annotated;
        self.index.domain_poisoned |= unsupported_callable;
        context.unsupported |= annotated
            || unsupported_callable
            || declaration_node
                .child_by_field_name("type_parameters")
                .is_some()
            || java_kotlin_has_direct_child_kind(declaration_node, "type_parameters");

        let Some(name) = declaration_name(declaration_node, self.source).map(str::to_string) else {
            context.caller = None;
            context.callable_id = Some(node.id());
            return context;
        };
        count_java_kotlin_resolution_work(1);
        if !self.index.overloads.insert(name.clone()) {
            count_java_kotlin_resolution_work(1);
            self.index.overloads.insert(format!("overload:{name}"));
        }
        if context.owner_virtual {
            count_java_kotlin_resolution_work(1);
            self.index.virtual_methods.insert(name.clone());
        }
        if self.index.language == "kotlin"
            && java_kotlin_kotlin_extension_declaration(node, self.source)
        {
            count_java_kotlin_resolution_work(1);
            self.index.extension_methods.insert(name.clone());
        }

        let caller = self
            .index
            .map_callable_declaration(declaration_node, self.source);
        if let (Some(owner_index), Some(declaration)) = (context.owner_index, caller) {
            count_java_kotlin_resolution_work(1);
            self.index.classes[owner_index]
                .methods
                .push(CachedClassMethod {
                    name: name.clone(),
                    declaration,
                    cross_module_visible: csd_cross_module_visible(
                        declaration_node,
                        self.source,
                        self.index.language,
                    ),
                });
        }
        if let Some(declaration) = caller {
            let same_file = if matches!(self.index.language, "kotlin" | "swift") {
                declaration_node
                    .parent()
                    .is_some_and(|parent| parent.id() == self.root_id)
            } else if self.index.language == "dart" {
                declaration_node
                    .parent()
                    .is_some_and(|parent| parent.kind() == "program")
            } else {
                java_kotlin_java_method_is_static(declaration_node, self.source)
            };
            if same_file {
                count_java_kotlin_resolution_work(1);
                self.index.declarations.push(CachedTopLevelDeclaration {
                    name,
                    declaration,
                    module_path: Vec::new(),
                    cross_module_visible: csd_cross_module_visible(
                        declaration_node,
                        self.source,
                        self.index.language,
                    ),
                });
            }
        }
        context.callable_id = Some(node.id());
        context.caller = caller;
        if self.index.language == "dart" {
            for (binding_name, binding) in dart_callable_bindings(declaration_node, self.source) {
                self.insert_active(binding_name, binding);
            }
        }
        context
    }

    fn collect_package(&mut self, node: TsNode<'tree>) {
        let expected = match self.index.language {
            "java" => "package_declaration",
            "kotlin" => "package_header",
            "csharp" if node.kind() == "namespace_declaration" => "namespace_declaration",
            "csharp" => "file_scoped_namespace_declaration",
            _ => return,
        };
        if node.kind() != expected {
            return;
        }
        let prefix = if self.index.language == "csharp" {
            "namespace"
        } else {
            "package"
        };
        let parsed = if self.index.language == "csharp" {
            node.child_by_field_name("name")
                .and_then(|name| node_text(name, self.source))
                .map(str::trim)
                .map(str::to_string)
                .filter(|name| !name.is_empty())
        } else {
            node_text(node, self.source)
                .and_then(|surface| surface.trim().strip_prefix(prefix))
                .map(str::trim)
                .map(|name| name.trim_end_matches(';').trim().to_string())
                .filter(|name| !name.is_empty())
        };
        count_java_kotlin_resolution_work(1);
        if self.index.language == "csharp" {
            match (&self.index.package_name, parsed) {
                (Some(existing), Some(parsed)) if existing == &parsed => {}
                (Some(existing), Some(parsed))
                    if existing.starts_with("csharp:path:") || existing == "csharp:global" =>
                {
                    self.index.package_name = Some(format!("csharp:{parsed}"));
                }
                (_, Some(_)) => self.index.domain_poisoned = true,
                _ => self.index.domain_poisoned = true,
            }
        } else if self.index.package_name.is_some() {
            self.index.domain_poisoned = true;
        } else {
            self.index.package_name = parsed;
        }
    }

    fn collect_import(&mut self, node: TsNode<'tree>) {
        if is_csharp_swift_dart_language(self.index.language) {
            let expected = match self.index.language {
                "csharp" => "using_directive",
                "swift" => "import_declaration",
                "dart" => "import_or_export",
                _ => unreachable!("nominal import collector language"),
            };
            if node.kind() == expected {
                self.collect_csd_import(node);
            }
            return;
        }
        if !node.kind().contains("import") {
            return;
        }
        let Some(surface) = node_text(node, self.source)
            .map(str::trim)
            .map(|surface| surface.trim_end_matches(';').trim())
        else {
            return;
        };
        if surface.ends_with(".*") {
            self.index.wildcard_import = true;
            return;
        }
        let Some(import) = self.parse_import(node, surface) else {
            return;
        };
        count_java_kotlin_resolution_work(1);
        self.index
            .imports_by_name
            .entry(import.local_name.clone())
            .or_default()
            .push(import.clone());
        if import.owner_name.is_none() {
            count_java_kotlin_resolution_work(1);
            self.index
                .type_imports_by_name
                .entry(import.local_name.clone())
                .or_default()
                .push(import);
        }
    }

    fn collect_csd_import(&mut self, node: TsNode<'tree>) {
        let Some(surface) = node_text(node, self.source).map(str::trim) else {
            self.index.domain_poisoned = true;
            return;
        };
        let line = node.start_position().row as u32 + 1;
        count_java_kotlin_resolution_work(1);
        let Some(import_nodes) = self.index.import_nodes.get(&line).map(Vec::as_slice) else {
            self.index.domain_poisoned = true;
            return;
        };
        match self.index.language {
            "csharp" => {
                let Some(rest) = surface.trim_end_matches(';').trim().strip_prefix("using ") else {
                    self.index.domain_poisoned = true;
                    return;
                };
                let Some((alias, qualified)) = rest.split_once('=') else {
                    self.index.domain_poisoned = true;
                    return;
                };
                let alias = alias.trim();
                let qualified = qualified.trim();
                let Some((package_name, imported_name)) = qualified.rsplit_once('.') else {
                    self.index.domain_poisoned = true;
                    return;
                };
                if !java_kotlin_simple_identifier(alias)
                    || !java_kotlin_simple_identifier(imported_name)
                    || package_name.is_empty()
                {
                    self.index.domain_poisoned = true;
                    return;
                }
                let matching_imports = import_nodes
                    .iter()
                    .filter(|candidate| {
                        self.index
                            .import_names
                            .get(candidate)
                            .is_some_and(|name| name == qualified)
                    })
                    .copied()
                    .collect::<Vec<_>>();
                let [import_node] = matching_imports.as_slice() else {
                    self.index.domain_poisoned = true;
                    return;
                };
                let binding = JavaKotlinImportBinding {
                    package_name: format!("csharp:{package_name}"),
                    owner_name: None,
                    imported_name: imported_name.to_string(),
                    local_name: alias.to_string(),
                    node: *import_node,
                    visible_names: None,
                    hidden_names: HashSet::new(),
                };
                self.index
                    .type_imports_by_name
                    .entry(alias.to_string())
                    .or_default()
                    .push(binding);
            }
            "swift" => {
                let [import_node] = import_nodes else {
                    self.index.domain_poisoned = true;
                    return;
                };
                if surface.contains(['.', ':']) {
                    self.index.domain_poisoned = true;
                    return;
                }
                let Some(module) = surface.strip_prefix("import ").map(str::trim) else {
                    self.index.domain_poisoned = true;
                    return;
                };
                if !java_kotlin_simple_identifier(module) {
                    self.index.domain_poisoned = true;
                    return;
                }
                self.index
                    .whole_module_imports
                    .push(JavaKotlinImportBinding {
                        package_name: format!("swift:Sources/{module}"),
                        owner_name: None,
                        imported_name: "*".to_string(),
                        local_name: "*".to_string(),
                        node: *import_node,
                        visible_names: None,
                        hidden_names: HashSet::new(),
                    });
            }
            "dart" => {
                let [import_node] = import_nodes else {
                    self.index.domain_poisoned = true;
                    return;
                };
                if surface.contains(" if ")
                    || surface.contains(" deferred ")
                    || surface.contains("dart:mirrors")
                {
                    self.index.wildcard_import = true;
                    self.index.domain_poisoned = true;
                    return;
                }
                let Some(uri) = quoted_literal(surface) else {
                    self.index.domain_poisoned = true;
                    return;
                };
                if uri.starts_with('/')
                    || uri.contains(':')
                    || uri.split('/').any(|component| component == "..")
                {
                    self.index.domain_poisoned = true;
                    return;
                }
                let binding = JavaKotlinImportBinding {
                    package_name: format!("dart:uri:{uri}"),
                    owner_name: None,
                    imported_name: "*".to_string(),
                    local_name: "*".to_string(),
                    node: *import_node,
                    visible_names: import_show_names(surface)
                        .map(|names| names.into_iter().collect()),
                    hidden_names: import_hide_names(surface)
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                };
                if let Some(prefix) = import_alias(surface) {
                    self.index
                        .prefixed_imports
                        .entry(prefix)
                        .or_default()
                        .push(binding);
                } else if let Some(shown) = import_show_names(surface) {
                    for name in shown {
                        let mut named = binding.clone();
                        named.imported_name = name.clone();
                        named.local_name = name.clone();
                        self.index
                            .imports_by_name
                            .entry(name.clone())
                            .or_default()
                            .push(named.clone());
                        self.index
                            .type_imports_by_name
                            .entry(name)
                            .or_default()
                            .push(named);
                    }
                } else {
                    self.index.whole_module_imports.push(binding);
                }
            }
            _ => unreachable!("C#/Swift/Dart import collector language"),
        }
        count_java_kotlin_resolution_work(1);
    }

    fn parse_import(&self, node: TsNode<'tree>, surface: &str) -> Option<JavaKotlinImportBinding> {
        let line = node.start_position().row as u32 + 1;
        count_java_kotlin_resolution_work(1);
        let nodes = self.index.import_nodes.get(&line)?;
        let [node_id] = nodes.as_slice() else {
            return None;
        };
        let path = surface.strip_prefix("import ")?.trim();
        let (path, static_import) = if self.index.language == "java" {
            match path.strip_prefix("static ") {
                Some(path) => (path.trim(), true),
                None => (path, false),
            }
        } else {
            (path, false)
        };
        let (path, alias) = if self.index.language == "kotlin" {
            path.rsplit_once(" as ")
                .map_or((path, None), |(path, alias)| {
                    (path.trim(), Some(alias.trim()))
                })
        } else {
            (path, None)
        };
        let parts = path.split('.').collect::<Vec<_>>();
        let imported_name = parts.last()?.to_string();
        let local_name = alias.unwrap_or(&imported_name).to_string();
        let (package_name, owner_name) = if static_import {
            (
                parts[..parts.len().saturating_sub(2)].join("."),
                parts
                    .get(parts.len().saturating_sub(2))
                    .map(|part| (*part).to_string()),
            )
        } else {
            (parts[..parts.len().saturating_sub(1)].join("."), None)
        };
        (!package_name.is_empty()).then_some(JavaKotlinImportBinding {
            package_name,
            owner_name,
            imported_name,
            local_name,
            node: *node_id,
            visible_names: None,
            hidden_names: HashSet::new(),
        })
    }

    fn collect_binding(&mut self, node: TsNode<'tree>, context: JavaKotlinWalkContext) {
        let binding = if is_csharp_swift_dart_language(self.index.language) {
            csd_lexical_binding(node, self.source, self.index.language)
        } else {
            java_kotlin_lexical_binding(node, self.source, self.index.language)
        };
        let Some((name, binding)) = binding else {
            return;
        };
        if context.callable_id.is_some() {
            self.insert_active(name, binding);
        } else if let Some(owner_index) = context.owner_index {
            let owner_name = self.index.classes[owner_index].name.clone();
            count_java_kotlin_resolution_work(1);
            self.index
                .owner_bindings
                .entry((owner_name, name))
                .or_default()
                .push(binding);
        }
    }

    fn collect_write(&mut self, node: TsNode<'tree>, context: JavaKotlinWalkContext) {
        if !matches!(node.kind(), "assignment_expression" | "assignment") {
            return;
        }
        let Some(callable_id) = context.callable_id else {
            return;
        };
        let Some(left) = node
            .child_by_field_name("left")
            .or_else(|| node.named_child(0))
        else {
            return;
        };
        let Some(name) = node_text(left, self.source)
            .map(str::trim)
            .filter(|name| java_kotlin_simple_identifier(name))
        else {
            return;
        };
        count_java_kotlin_resolution_work(1);
        self.index
            .rebound_receivers
            .insert((callable_id, name.to_string()));
    }

    fn collect_call(&mut self, node: TsNode<'tree>, context: JavaKotlinWalkContext) {
        let call = match self.index.language {
            "java" => java_call(node, self.source),
            "kotlin" => kotlin_call(node, self.source),
            "csharp" => csharp_call(node, self.source),
            "swift" => swift_call(node, self.source),
            "dart" => dart_call(node, self.source),
            _ => None,
        };
        let Some((callee, form, raw_target)) = call else {
            return;
        };
        if matches!(self.index.language, "swift" | "dart")
            && form == CalleeForm::Identifier
            && raw_target.chars().next().is_some_and(char::is_uppercase)
        {
            return;
        }
        let owner_name = context
            .owner_index
            .map(|owner_index| self.index.classes[owner_index].name.clone());
        let identifier_shadowed = if form == CalleeForm::Identifier {
            count_java_kotlin_resolution_work(1);
            self.active_bindings.contains_key(&raw_target)
        } else {
            false
        };
        let receiver = self.call_receiver(callee, form);
        let reflection = raw_target == "forName"
            && java_kotlin_member_receiver(callee, self.index.language)
                .and_then(|receiver| node_text(receiver, self.source))
                .is_some_and(|receiver| receiver.trim() == "Class");
        count_java_kotlin_resolution_work(1);
        self.index.calls.push(IndexedJavaKotlinCall {
            callee,
            form,
            raw_target,
            caller: context.caller,
            callable_id: context.callable_id,
            owner_name,
            unsupported: context.unsupported || reflection,
            identifier_shadowed,
            receiver,
        });
    }

    fn call_receiver(&self, callee: TsNode<'tree>, form: CalleeForm) -> JavaKotlinCallReceiver {
        if form == CalleeForm::Identifier {
            return JavaKotlinCallReceiver::None;
        }
        if form == CalleeForm::ImplicitReceiver {
            return JavaKotlinCallReceiver::Implicit;
        }
        if self.index.language == "dart" {
            let Some(receiver_surface) = dart_member_receiver_surface(callee, self.source) else {
                return JavaKotlinCallReceiver::Blocked;
            };
            if let Some((owner_name, import_prefix)) = constructor_surface_owner(&receiver_surface)
            {
                return JavaKotlinCallReceiver::ExactType {
                    owner_name,
                    constructor: true,
                    receiver_name: None,
                    import_prefix,
                };
            }
            let receiver_name = receiver_surface.trim_end_matches('?').trim();
            if !java_kotlin_simple_identifier(receiver_name) {
                return JavaKotlinCallReceiver::Blocked;
            }
            count_java_kotlin_resolution_work(1);
            return match self
                .active_bindings
                .get(receiver_name)
                .and_then(|bindings| bindings.last())
            {
                Some(JavaKotlinLexicalBinding::Receiver {
                    owner_name,
                    constructor,
                }) => JavaKotlinCallReceiver::ExactType {
                    owner_name: owner_name.clone(),
                    constructor: *constructor,
                    receiver_name: Some(receiver_name.to_string()),
                    import_prefix: None,
                },
                Some(JavaKotlinLexicalBinding::Other) => JavaKotlinCallReceiver::Blocked,
                None => JavaKotlinCallReceiver::Named {
                    receiver_name: receiver_name.to_string(),
                },
            };
        }
        let Some(receiver) = java_kotlin_member_receiver(callee, self.index.language) else {
            return JavaKotlinCallReceiver::Blocked;
        };
        let direct_constructor = if is_csharp_swift_dart_language(self.index.language) {
            csd_direct_constructor_type(receiver, self.source, self.index.language)
        } else {
            java_kotlin_direct_constructor_type(receiver, self.source, self.index.language)
                .map(|owner| (owner, None))
        };
        if let Some((owner_name, import_prefix)) = direct_constructor {
            return JavaKotlinCallReceiver::ExactType {
                owner_name,
                constructor: true,
                receiver_name: None,
                import_prefix,
            };
        }
        let Some(receiver_name) = node_text(receiver, self.source)
            .map(str::trim)
            .map(|name| name.trim_end_matches('?'))
            .filter(|name| java_kotlin_simple_identifier(name))
        else {
            return JavaKotlinCallReceiver::Blocked;
        };
        count_java_kotlin_resolution_work(1);
        match self
            .active_bindings
            .get(receiver_name)
            .and_then(|bindings| bindings.last())
        {
            Some(JavaKotlinLexicalBinding::Receiver {
                owner_name,
                constructor,
            }) => JavaKotlinCallReceiver::ExactType {
                owner_name: owner_name.clone(),
                constructor: *constructor,
                receiver_name: Some(receiver_name.to_string()),
                import_prefix: None,
            },
            Some(JavaKotlinLexicalBinding::Other) => JavaKotlinCallReceiver::Blocked,
            None if self.index.language == "java"
                && receiver_name.chars().next().is_some_and(char::is_uppercase) =>
            {
                JavaKotlinCallReceiver::ExactType {
                    owner_name: receiver_name.to_string(),
                    constructor: false,
                    receiver_name: None,
                    import_prefix: None,
                }
            }
            None => JavaKotlinCallReceiver::Named {
                receiver_name: receiver_name.to_string(),
            },
        }
    }

    fn insert_active(&mut self, name: String, binding: JavaKotlinLexicalBinding) {
        count_java_kotlin_resolution_work(1);
        self.active_bindings
            .entry(name.clone())
            .or_default()
            .push(binding);
        self.scope_insertions
            .last_mut()
            .expect("root lexical scope exists")
            .push(name);
    }

    fn leave_scope(&mut self) {
        let names = self
            .scope_insertions
            .pop()
            .expect("entered Java/Kotlin lexical scope must exist");
        for name in names.into_iter().rev() {
            count_java_kotlin_resolution_work(1);
            let remove = self.active_bindings.get_mut(&name).is_some_and(|bindings| {
                bindings.pop();
                bindings.is_empty()
            });
            if remove {
                self.active_bindings.remove(&name);
            }
        }
    }
}

fn java_kotlin_callable_kind(language: &str, kind: &str) -> bool {
    matches!(
        (language, kind),
        ("java", "method_declaration")
            | ("kotlin", "function_declaration")
            | ("csharp", "method_declaration")
            | ("swift", "function_declaration")
            | ("dart", "function_body")
    )
}

fn java_kotlin_class_kind(language: &str, kind: &str) -> bool {
    matches!(
        (language, kind),
        ("java", "class_declaration")
            | ("kotlin", "class_declaration")
            | ("csharp", "class_declaration" | "struct_declaration")
            | ("swift", "class_declaration")
            | ("dart", "class_definition")
    )
}

fn nominal_callable_declaration<'tree>(node: TsNode<'tree>, language: &str) -> TsNode<'tree> {
    if language != "dart" || node.kind() != "function_body" {
        return node;
    }
    let Some(signature) = node.prev_named_sibling() else {
        return node;
    };
    if signature.kind() == "function_signature" {
        return signature;
    }
    if signature.kind() == "method_signature" {
        let mut cursor = signature.walk();
        if let Some(function) = signature
            .named_children(&mut cursor)
            .find(|child| child.kind() == "function_signature")
        {
            return function;
        }
    }
    node
}

fn csd_owner_is_unsupported(node: TsNode<'_>, source: &str, language: &str) -> bool {
    let header = node
        .child_by_field_name("body")
        .and_then(|body| source.get(node.start_byte()..body.start_byte()))
        .or_else(|| node_text(node, source))
        .unwrap_or_default();
    match language {
        "csharp" => header.split_whitespace().any(|word| word == "partial"),
        "swift" => {
            let header = header.trim_start();
            header.starts_with("class ") || header.contains("@objc") || header.contains(" dynamic ")
        }
        "dart" => {
            let header = header.trim_start();
            header.starts_with("abstract ")
                || header.starts_with("interface ")
                || header.starts_with("abstract interface ")
                || header.starts_with("mixin ")
                || header.contains(" with ")
                || header.contains(" implements ")
        }
        _ => false,
    }
}

fn csd_declaration_header<'a>(node: TsNode<'_>, source: &'a str) -> &'a str {
    node.child_by_field_name("body")
        .and_then(|body| source.get(node.start_byte()..body.start_byte()))
        .or_else(|| node_text(node, source))
        .unwrap_or_default()
}

fn csd_cross_module_visible(node: TsNode<'_>, source: &str, language: &str) -> bool {
    if language != "swift" {
        return true;
    }
    swift_declaration_cross_module_visible(node, source)
}

fn swift_declaration_cross_module_visible(node: TsNode<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    let Some(modifiers) = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == "modifiers")
    else {
        return false;
    };
    let mut cursor = modifiers.walk();
    modifiers.named_children(&mut cursor).any(|modifier| {
        modifier.kind() == "visibility_modifier"
            && node_text(modifier, source)
                .is_some_and(|text| matches!(text.trim(), "public" | "open"))
    })
}

pub(crate) fn swift_declaration_cross_module_visible_at(
    tree: &Tree,
    source: &str,
    line: u32,
    column: u32,
) -> bool {
    let point = tree_sitter::Point {
        row: line.saturating_sub(1) as usize,
        column: column.saturating_sub(1) as usize,
    };
    let Some(mut node) = tree
        .root_node()
        .named_descendant_for_point_range(point, point)
    else {
        return false;
    };
    loop {
        if matches!(
            node.kind(),
            "class_declaration"
                | "protocol_declaration"
                | "function_declaration"
                | "property_declaration"
        ) {
            return swift_declaration_cross_module_visible(node, source);
        }
        let Some(parent) = node.parent() else {
            return false;
        };
        node = parent;
    }
}

fn csd_runtime_closed(node: TsNode<'_>, source: &str, language: &str) -> bool {
    let header = csd_declaration_header(node, source).trim_start();
    match language {
        "csharp" => header.split_whitespace().any(|token| token == "sealed"),
        "swift" => header.starts_with("struct ") || header.starts_with("enum "),
        "dart" => header.starts_with("final class ") || header.starts_with("sealed class "),
        _ => false,
    }
}

fn csd_super_name(node: TsNode<'_>, source: &str, language: &str) -> Option<String> {
    if language != "dart" {
        return None;
    }
    let header = csd_declaration_header(node, source);
    let mut tokens = header.split(|character: char| {
        character.is_whitespace() || matches!(character, '{' | '(' | ')' | '<' | '>' | ',')
    });
    while let Some(token) = tokens.next() {
        if token == "extends" {
            return tokens
                .find(|candidate| !candidate.is_empty())
                .filter(|candidate| java_kotlin_simple_identifier(candidate))
                .map(str::to_string);
        }
    }
    None
}

fn csd_callable_is_unsupported(node: TsNode<'_>, source: &str, language: &str) -> bool {
    let Some(name) = node.child_by_field_name("name") else {
        return is_csharp_swift_dart_language(language);
    };
    let header = source
        .get(node.start_byte()..name.start_byte())
        .unwrap_or_default();
    match language {
        "csharp" => {
            header.split_whitespace().any(|word| {
                matches!(
                    word,
                    "virtual" | "abstract" | "override" | "extern" | "partial"
                )
            }) || node_text(node, source).is_some_and(|surface| surface.contains("(this "))
        }
        "swift" => {
            header.contains("@objc")
                || header
                    .split_whitespace()
                    .any(|word| matches!(word, "dynamic" | "override" | "class" | "@objc"))
        }
        "dart" => header.contains('<') || header.contains("external"),
        _ => false,
    }
}

fn java_kotlin_simple_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character == '_' || character.is_alphanumeric())
}

fn java_kotlin_simple_type_name(node: TsNode<'_>, source: &str) -> Option<String> {
    let surface = node_text(node, source)?.trim().trim_end_matches('?').trim();
    java_kotlin_simple_identifier(surface).then(|| surface.to_string())
}

fn java_kotlin_declaration_has_annotation(node: TsNode<'_>, source: &str) -> bool {
    let Some(name) = node.child_by_field_name("name") else {
        return false;
    };
    source
        .get(node.start_byte()..name.start_byte())
        .is_some_and(|header| header.contains('@'))
}

fn java_kotlin_java_method_is_static(node: TsNode<'_>, source: &str) -> bool {
    let Some(name) = node.child_by_field_name("name") else {
        return false;
    };
    source
        .get(node.start_byte()..name.start_byte())
        .is_some_and(|header| header.split_whitespace().any(|word| word == "static"))
}

fn java_kotlin_kotlin_extension_declaration(node: TsNode<'_>, source: &str) -> bool {
    let Some(name) = node.child_by_field_name("name") else {
        return false;
    };
    source
        .get(node.start_byte()..name.start_byte())
        .is_some_and(|header| header.contains('.'))
}

fn java_kotlin_has_direct_child_kind(node: TsNode<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).any(|child| {
        count_java_kotlin_resolution_work(1);
        child.kind() == kind
    })
}

fn java_kotlin_direct_constructor_type(
    node: TsNode<'_>,
    source: &str,
    language: &str,
) -> Option<String> {
    if language == "java" {
        (node.kind() == "object_creation_expression").then_some(())?;
        return node
            .child_by_field_name("type")
            .and_then(|ty| java_kotlin_simple_type_name(ty, source));
    }
    (node.kind() == "call_expression").then_some(())?;
    count_java_kotlin_resolution_work(1);
    let callee = node.named_child(0)?;
    let owner = java_kotlin_simple_type_name(callee, source)?;
    owner
        .chars()
        .next()
        .is_some_and(char::is_uppercase)
        .then_some(owner)
}

fn java_kotlin_lexical_binding(
    node: TsNode<'_>,
    source: &str,
    language: &str,
) -> Option<(String, JavaKotlinLexicalBinding)> {
    if language == "java" && node.kind() == "formal_parameter" {
        count_java_kotlin_resolution_work(2);
        let name = node
            .child_by_field_name("name")
            .and_then(|name| node_text(name, source))?
            .to_string();
        let binding = node
            .child_by_field_name("type")
            .and_then(|ty| java_kotlin_simple_type_name(ty, source))
            .map_or(JavaKotlinLexicalBinding::Other, |owner_name| {
                JavaKotlinLexicalBinding::Receiver {
                    owner_name,
                    constructor: false,
                }
            });
        return Some((name, binding));
    }
    if language == "java" && node.kind() == "variable_declarator" {
        let parent = node.parent()?;
        if !matches!(
            parent.kind(),
            "local_variable_declaration" | "field_declaration"
        ) {
            return None;
        }
        count_java_kotlin_resolution_work(2);
        let name = node
            .child_by_field_name("name")
            .and_then(|name| node_text(name, source))?
            .to_string();
        let binding = parent
            .child_by_field_name("type")
            .and_then(|ty| java_kotlin_simple_type_name(ty, source))
            .map_or(JavaKotlinLexicalBinding::Other, |owner_name| {
                JavaKotlinLexicalBinding::Receiver {
                    owner_name,
                    constructor: false,
                }
            });
        return Some((name, binding));
    }
    if language == "kotlin" && node.kind() == "parameter" {
        count_java_kotlin_resolution_work(2);
        let name = node_text(node.named_child(0)?, source)?.to_string();
        let binding = node
            .named_child(1)
            .and_then(|ty| java_kotlin_simple_type_name(ty, source))
            .map_or(JavaKotlinLexicalBinding::Other, |owner_name| {
                JavaKotlinLexicalBinding::Receiver {
                    owner_name,
                    constructor: false,
                }
            });
        return Some((name, binding));
    }
    if language != "kotlin" || node.kind() != "variable_declaration" {
        return None;
    }
    count_java_kotlin_resolution_work(2);
    let name = node_text(node.named_child(0)?, source)?.to_string();
    let explicit_type = node
        .named_child(1)
        .and_then(|ty| java_kotlin_simple_type_name(ty, source));
    let constructor_type = node
        .next_named_sibling()
        .and_then(|initializer| java_kotlin_direct_constructor_type(initializer, source, language));
    let binding = explicit_type
        .map(|owner_name| JavaKotlinLexicalBinding::Receiver {
            owner_name,
            constructor: false,
        })
        .or_else(|| {
            constructor_type.map(|owner_name| JavaKotlinLexicalBinding::Receiver {
                owner_name,
                constructor: true,
            })
        })
        .unwrap_or(JavaKotlinLexicalBinding::Other);
    Some((name, binding))
}

fn typed_binding_from_surface(surface: &str, language: &str) -> Option<(String, String, bool)> {
    let before_initializer = surface.split('=').next()?.trim();
    let constructor_type = surface.split_once('=').and_then(|(_, initializer)| {
        let initializer = initializer.trim().trim_start_matches("new ").trim();
        let (name, arguments) = initializer.split_once('(')?;
        (arguments.contains(')')
            && java_kotlin_simple_identifier(name)
            && name.chars().next().is_some_and(char::is_uppercase))
        .then(|| name.to_string())
    });
    let constructor = constructor_type.is_some();
    if language == "swift" {
        let (name, owner) = if let Some((left, right)) = before_initializer.split_once(':') {
            (
                left.split_whitespace().last()?.trim(),
                right.trim().trim_end_matches('?').to_string(),
            )
        } else {
            (
                before_initializer.split_whitespace().last()?.trim(),
                constructor_type?,
            )
        };
        return (java_kotlin_simple_identifier(name) && java_kotlin_simple_identifier(&owner))
            .then(|| (name.to_string(), owner, constructor));
    }
    let tokens = before_initializer
        .split_whitespace()
        .filter(|token| {
            !matches!(
                *token,
                "final"
                    | "const"
                    | "var"
                    | "required"
                    | "readonly"
                    | "private"
                    | "public"
                    | "protected"
                    | "internal"
                    | "static"
            )
        })
        .collect::<Vec<_>>();
    let name = tokens
        .last()?
        .trim_matches(|character: char| !character.is_alphanumeric() && character != '_');
    let owner = tokens
        .get(tokens.len().saturating_sub(2))
        .filter(|_| tokens.len() >= 2)
        .map(|owner| owner.trim_end_matches('?').to_string())
        .or(constructor_type)?;
    (java_kotlin_simple_identifier(name) && java_kotlin_simple_identifier(&owner))
        .then(|| (name.to_string(), owner, constructor))
}

fn csd_lexical_binding(
    node: TsNode<'_>,
    source: &str,
    language: &str,
) -> Option<(String, JavaKotlinLexicalBinding)> {
    let surface = match (language, node.kind()) {
        ("csharp", "parameter") => node_text(node, source)?,
        ("csharp", "variable_declarator") => {
            node.parent().and_then(|parent| node_text(parent, source))?
        }
        ("swift", "parameter" | "property_declaration") => node_text(node, source)?,
        ("dart", "declaration" | "local_variable_declaration") => node_text(node, source)?,
        _ => return None,
    };
    let declared_name = match language {
        "swift" => surface
            .split([':', '='])
            .next()
            .and_then(|left| left.split_whitespace().last()),
        "csharp" | "dart" => surface
            .split('=')
            .next()
            .and_then(|left| left.split_whitespace().last())
            .map(|name| {
                name.trim_matches(|character: char| {
                    !character.is_alphanumeric() && character != '_'
                })
            }),
        _ => None,
    }
    .filter(|name| java_kotlin_simple_identifier(name))?
    .to_string();
    let Some((name, owner_name, constructor)) = typed_binding_from_surface(surface, language)
    else {
        count_java_kotlin_resolution_work(1);
        return Some((declared_name, JavaKotlinLexicalBinding::Other));
    };
    if name != declared_name {
        return Some((declared_name, JavaKotlinLexicalBinding::Other));
    }
    count_java_kotlin_resolution_work(2);
    Some((
        name,
        if owner_name == "dynamic"
            || owner_name == "var"
            || owner_name.contains(['<', '>', '(', ')'])
        {
            JavaKotlinLexicalBinding::Other
        } else {
            JavaKotlinLexicalBinding::Receiver {
                owner_name,
                constructor,
            }
        },
    ))
}

fn dart_callable_bindings(
    signature: TsNode<'_>,
    source: &str,
) -> Vec<(String, JavaKotlinLexicalBinding)> {
    let mut bindings = Vec::new();
    let mut stack = vec![signature];
    while let Some(node) = stack.pop() {
        count_java_kotlin_resolution_work(1);
        if node.kind() == "formal_parameter"
            && let Some(surface) = node_text(node, source)
        {
            if let Some((name, owner_name, constructor)) =
                typed_binding_from_surface(surface, "dart")
            {
                bindings.push((
                    name,
                    if owner_name == "dynamic" || owner_name == "Function" {
                        JavaKotlinLexicalBinding::Other
                    } else {
                        JavaKotlinLexicalBinding::Receiver {
                            owner_name,
                            constructor,
                        }
                    },
                ));
            } else if let Some(name) = surface
                .split('=')
                .next()
                .and_then(|left| left.split_whitespace().last())
                .map(|name| {
                    name.trim_matches(|character: char| {
                        !character.is_alphanumeric() && character != '_'
                    })
                })
                .filter(|name| java_kotlin_simple_identifier(name))
            {
                bindings.push((name.to_string(), JavaKotlinLexicalBinding::Other));
            }
        }
        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    bindings
}

fn java_call<'tree>(
    node: TsNode<'tree>,
    source: &str,
) -> Option<(TsNode<'tree>, CalleeForm, String)> {
    (node.kind() == "method_invocation").then_some(())?;
    let name = node.child_by_field_name("name")?;
    let object = node.child_by_field_name("object");
    let form = match object {
        None => CalleeForm::Identifier,
        Some(object) if object.kind() == "this" => CalleeForm::ImplicitReceiver,
        Some(_) => CalleeForm::ExplicitReceiver,
    };
    Some((name, form, node_text(name, source)?.to_string()))
}

fn csharp_call<'tree>(
    node: TsNode<'tree>,
    source: &str,
) -> Option<(TsNode<'tree>, CalleeForm, String)> {
    (node.kind() == "invocation_expression").then_some(())?;
    let function = node.child_by_field_name("function")?;
    if function.kind() == "identifier" {
        return Some((
            function,
            CalleeForm::Identifier,
            node_text(function, source)?.to_string(),
        ));
    }
    (function.kind() == "member_access_expression").then_some(())?;
    let name = function.child_by_field_name("name")?;
    let receiver = function.child_by_field_name("expression")?;
    let form = if receiver.kind() == "this_expression" || node_text(receiver, source)? == "this" {
        CalleeForm::ImplicitReceiver
    } else {
        CalleeForm::ExplicitReceiver
    };
    Some((name, form, node_text(name, source)?.to_string()))
}

fn swift_call<'tree>(
    node: TsNode<'tree>,
    source: &str,
) -> Option<(TsNode<'tree>, CalleeForm, String)> {
    (node.kind() == "call_expression").then_some(())?;
    let function = node.named_child(0)?;
    if function.kind() == "simple_identifier" {
        return Some((
            function,
            CalleeForm::Identifier,
            node_text(function, source)?.to_string(),
        ));
    }
    (function.kind() == "navigation_expression").then_some(())?;
    let receiver = function.named_child(0)?;
    let suffix_index = u32::try_from(function.named_child_count().checked_sub(1)?).ok()?;
    let suffix = function.named_child(suffix_index)?;
    let member = first_descendant_named_kind(suffix, "simple_identifier")?;
    let form = if receiver.kind() == "self_expression" {
        CalleeForm::ImplicitReceiver
    } else {
        CalleeForm::ExplicitReceiver
    };
    Some((member, form, node_text(member, source)?.to_string()))
}

fn dart_call<'tree>(
    node: TsNode<'tree>,
    source: &str,
) -> Option<(TsNode<'tree>, CalleeForm, String)> {
    if node.kind() == "method_invocation" {
        let function = node.child_by_field_name("function")?;
        return (function.kind() == "identifier").then(|| {
            (
                function,
                CalleeForm::Identifier,
                node_text(function, source).unwrap_or_default().to_string(),
            )
        });
    }
    let mut cursor = node.walk();
    let children = node.named_children(&mut cursor).collect::<Vec<_>>();
    let mut found = None;
    for pair in children.windows(2) {
        if pair[0].kind() != "selector"
            || pair[1].kind() != "selector"
            || !node_contains_kind(pair[1], "argument_part")
        {
            continue;
        }
        let callee = first_descendant_named_kind(pair[0], "identifier")?;
        let receiver = source.get(node.start_byte()..pair[0].start_byte())?.trim();
        let form = if receiver == "this" {
            CalleeForm::ImplicitReceiver
        } else {
            CalleeForm::ExplicitReceiver
        };
        found = Some((callee, form, node_text(callee, source)?.to_string()));
    }
    found
}

fn node_contains_kind(node: TsNode<'_>, kind: &str) -> bool {
    if node.kind() == kind {
        return true;
    }
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        count_java_kotlin_resolution_work(1);
        let mut cursor = current.walk();
        for child in current.named_children(&mut cursor) {
            if child.kind() == kind {
                return true;
            }
            stack.push(child);
        }
    }
    false
}

fn first_descendant_named_kind<'tree>(node: TsNode<'tree>, kind: &str) -> Option<TsNode<'tree>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        count_java_kotlin_resolution_work(1);
        let mut cursor = current.walk();
        for child in current.named_children(&mut cursor) {
            if child.kind() == kind {
                return Some(child);
            }
            stack.push(child);
        }
    }
    None
}

fn dart_member_receiver_surface(callee: TsNode<'_>, source: &str) -> Option<String> {
    let selector = callee.parent()?.parent()?;
    let expression = selector.parent()?;
    source
        .get(expression.start_byte()..selector.start_byte())
        .map(str::trim)
        .filter(|surface| !surface.is_empty())
        .map(str::to_string)
}

fn constructor_surface_owner(surface: &str) -> Option<(String, Option<String>)> {
    let surface = surface.trim().trim_end_matches('?').trim();
    let callee = surface.strip_suffix("()")?.trim();
    let (prefix, owner) = callee
        .rsplit_once('.')
        .map_or((None, callee), |(prefix, owner)| (Some(prefix), owner));
    (java_kotlin_simple_identifier(owner)
        && prefix.is_none_or(java_kotlin_simple_identifier)
        && owner.chars().next().is_some_and(char::is_uppercase))
    .then(|| (owner.to_string(), prefix.map(str::to_string)))
}

fn csd_direct_constructor_type(
    node: TsNode<'_>,
    source: &str,
    language: &str,
) -> Option<(String, Option<String>)> {
    match language {
        "csharp" => {
            (node.kind() == "object_creation_expression").then_some(())?;
            let owner = node
                .child_by_field_name("type")
                .and_then(|ty| java_kotlin_simple_type_name(ty, source))?;
            Some((owner, None))
        }
        "swift" => {
            (node.kind() == "call_expression").then_some(())?;
            let callee = node.named_child(0)?;
            let surface = node_text(callee, source)?;
            let (prefix, owner) = surface
                .rsplit_once('.')
                .map_or((None, surface), |(prefix, owner)| (Some(prefix), owner));
            (java_kotlin_simple_identifier(owner)
                && prefix.is_none_or(java_kotlin_simple_identifier)
                && owner.chars().next().is_some_and(char::is_uppercase))
            .then(|| (owner.to_string(), prefix.map(str::to_string)))
        }
        _ => None,
    }
}

fn kotlin_call<'tree>(
    node: TsNode<'tree>,
    source: &str,
) -> Option<(TsNode<'tree>, CalleeForm, String)> {
    (node.kind() == "call_expression").then_some(())?;
    count_java_kotlin_resolution_work(1);
    let callee = node.named_child(0)?;
    if callee.kind() == "identifier" {
        return Some((
            callee,
            CalleeForm::Identifier,
            node_text(callee, source)?.to_string(),
        ));
    }
    if callee.kind() != "navigation_expression" {
        return None;
    }
    count_java_kotlin_resolution_work(2);
    let receiver = callee.named_child(0)?;
    let last = u32::try_from(callee.named_child_count().checked_sub(1)?).ok()?;
    let member = callee.named_child(last)?;
    let form = if receiver.kind() == "this_expression" {
        CalleeForm::ImplicitReceiver
    } else {
        CalleeForm::ExplicitReceiver
    };
    Some((
        member,
        form,
        node_text(member, source)?
            .trim_start_matches('?')
            .to_string(),
    ))
}

fn java_kotlin_member_receiver<'tree>(
    callee: TsNode<'tree>,
    language: &str,
) -> Option<TsNode<'tree>> {
    let parent = callee.parent()?;
    match language {
        "java" => return parent.child_by_field_name("object"),
        "csharp" => return parent.child_by_field_name("expression"),
        "swift" => return parent.parent()?.named_child(0),
        "dart" => return None,
        _ => {}
    }
    count_java_kotlin_resolution_work(1);
    parent.named_child(0)
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
    returned_closure_calls: HashSet<usize>,
    returned_closure_outer_blockers: HashSet<(usize, String)>,
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
        let returned_closures = GoReturnedClosureIndex::prepare(root);
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
            returned_closure_calls: HashSet::new(),
            returned_closure_outer_blockers: HashSet::new(),
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
                    let returned_closure = returned_closures.contains(node);
                    let callable_id = go_enclosing_callable(node, &returned_closures)
                        .map(|callable| callable.id());
                    match function.kind() {
                        "identifier" => {
                            if let Some(raw_target) = node_text(function, source) {
                                if returned_closure {
                                    result.returned_closure_calls.insert(function.start_byte());
                                }
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
                            if returned_closure {
                                result.returned_closure_calls.insert(field.start_byte());
                            }
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
                            if returned_closure {
                                result.returned_closure_calls.insert(leaf.start_byte());
                            }
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
        result.binding_decisions = go_prepare_binding_decisions(
            root,
            source,
            &result.calls,
            &result.import_names,
            &returned_closures,
            &mut result.returned_closure_outer_blockers,
        );
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
        if form != CalleeForm::Identifier
            && self.returned_closure_calls.contains(&callee.start_byte())
        {
            return (Some(caller), CachedResolutionBinding::Unsupported);
        }
        match form {
            CalleeForm::Identifier => {
                if (self.returned_closure_calls.contains(&callee.start_byte())
                    && self
                        .returned_closure_outer_blockers
                        .contains(&(callable_id, raw_target.to_string())))
                    || self.import_names.contains(raw_target)
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

struct GoReturnedClosureIndex<'tree> {
    member_owners: HashMap<usize, TsNode<'tree>>,
    outer_body_owners: HashMap<usize, TsNode<'tree>>,
}

impl<'tree> GoReturnedClosureIndex<'tree> {
    fn prepare(root: TsNode<'tree>) -> Self {
        let mut literals = Vec::new();
        let mut outer_body_owners = HashMap::new();
        walk_nodes(root, &mut |node| {
            count_go_resolution_work(1);
            if node.kind() == "func_literal"
                && let Some(owner) = go_direct_returned_closure_owner(node)
            {
                let body = owner
                    .child_by_field_name("body")
                    .expect("direct returned closure owner has a body");
                outer_body_owners.insert(body.id(), owner);
                literals.push((node, owner));
            }
        });
        let mut member_owners = HashMap::new();
        for (literal, owner) in literals {
            go_record_returned_closure_members(
                literal,
                literal.id(),
                owner,
                true,
                &mut member_owners,
            );
        }
        Self {
            member_owners,
            outer_body_owners,
        }
    }

    fn contains(&self, node: TsNode<'_>) -> bool {
        count_go_resolution_work(1);
        self.member_owners.contains_key(&node.id())
    }

    fn member_owner(&self, node: TsNode<'tree>) -> Option<TsNode<'tree>> {
        count_go_resolution_work(1);
        self.member_owners.get(&node.id()).copied()
    }

    fn outer_body_owner(&self, scope_id: usize) -> Option<TsNode<'tree>> {
        count_go_resolution_work(1);
        self.outer_body_owners.get(&scope_id).copied()
    }
}

fn go_record_returned_closure_members<'tree>(
    node: TsNode<'tree>,
    literal_id: usize,
    owner: TsNode<'tree>,
    allow_deferred_child: bool,
    members: &mut HashMap<usize, TsNode<'tree>>,
) {
    count_go_resolution_work(1);
    if node.id() != literal_id && node.kind() == "func_literal" {
        if allow_deferred_child && go_is_immediate_deferred_literal(node) {
            go_record_returned_closure_members(node, node.id(), owner, false, members);
        }
        return;
    }
    members.insert(node.id(), owner);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        go_record_returned_closure_members(child, literal_id, owner, allow_deferred_child, members);
    }
}

fn go_is_immediate_deferred_literal(literal: TsNode<'_>) -> bool {
    let Some(call) = literal
        .parent()
        .filter(|parent| parent.kind() == "call_expression")
    else {
        return false;
    };
    call.child_by_field_name("function")
        .is_some_and(|function| function.id() == literal.id())
        && call
            .parent()
            .is_some_and(|parent| parent.kind() == "defer_statement")
}

fn go_enclosing_callable<'tree>(
    mut node: TsNode<'tree>,
    returned_closures: &GoReturnedClosureIndex<'tree>,
) -> Option<TsNode<'tree>> {
    if let Some(owner) = returned_closures.member_owner(node) {
        return Some(owner);
    }
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

fn go_direct_returned_closure_owner<'tree>(literal: TsNode<'tree>) -> Option<TsNode<'tree>> {
    if literal.kind() != "func_literal" {
        return None;
    }
    let returned = literal.parent()?;
    if returned.kind() != "expression_list" {
        return None;
    }
    let mut cursor = returned.walk();
    let expressions = returned.named_children(&mut cursor).collect::<Vec<_>>();
    let [expression] = expressions.as_slice() else {
        return None;
    };
    if expression.id() != literal.id() {
        return None;
    }
    let returned = returned.parent()?;
    if returned.kind() != "return_statement" {
        return None;
    }
    let mut cursor = returned.walk();
    let return_values = returned.named_children(&mut cursor).collect::<Vec<_>>();
    let [return_value] = return_values.as_slice() else {
        return None;
    };
    if return_value.kind() != "expression_list" || return_value.id() != literal.parent()?.id() {
        return None;
    }
    let statements = returned.parent()?;
    if statements.kind() != "statement_list" {
        return None;
    }
    let body = statements.parent()?;
    if body.kind() != "block" {
        return None;
    }
    let owner = body.parent()?;
    if !matches!(owner.kind(), "function_declaration" | "method_declaration")
        || owner
            .child_by_field_name("body")
            .is_none_or(|owner_body| owner_body.id() != body.id())
    {
        return None;
    }
    Some(owner)
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
    returned_closures: &GoReturnedClosureIndex<'_>,
    outer_blockers: &mut HashSet<(usize, String)>,
) -> HashMap<(usize, usize, String), GoBindingDecision> {
    let shadowed_new = go_callables_declaring_name(root, source, "new", returned_closures);
    let mut intervals = Vec::<GoBindingInterval>::new();
    walk_nodes(root, &mut |node| {
        count_go_resolution_work(1);
        let Some(callable) = go_enclosing_callable(node, returned_closures) else {
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
        let Some((scope_id, scope_start, scope_end, depth)) = go_binding_scope(node, callable)
        else {
            return;
        };
        let returned_closure_binding = returned_closures.contains(node);
        let complete_outer_scope = returned_closures
            .outer_body_owner(scope_id)
            .is_some_and(|owner| owner.id() == callable.id());
        if complete_outer_scope {
            let names = match node.kind() {
                "short_var_declaration" | "assignment_statement" => node
                    .child_by_field_name("left")
                    .map(|left| go_expression_names(left, source))
                    .unwrap_or_default(),
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
                    names
                }
                _ => Vec::new(),
            };
            outer_blockers.extend(names.into_iter().map(|name| (callable.id(), name)));
        }
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
                        start_byte: if returned_closure_binding {
                            scope_start
                        } else {
                            node.end_byte().max(scope_start)
                        },
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
                let receiver = node.child_by_field_name("type").and_then(|type_node| {
                    (!go_binding_uses_control_flow(node, callable))
                        .then(|| go_typed_receiver_binding(type_node, source, import_names))
                        .flatten()
                });
                let mut names = Vec::new();
                go_declared_names(node, source, &mut names);
                for name in names {
                    intervals.push(GoBindingInterval {
                        name,
                        callable_id: callable.id(),
                        start_byte: if returned_closure_binding {
                            scope_start
                        } else {
                            node.end_byte().max(scope_start)
                        },
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
                        start_byte: if returned_closure_binding {
                            scope_start
                        } else {
                            node.end_byte().max(scope_start)
                        },
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
        let Some(callable) = go_outer_callable_for_captured_node(node, returned_closures) else {
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

fn go_callables_declaring_name(
    root: TsNode<'_>,
    source: &str,
    wanted: &str,
    returned_closures: &GoReturnedClosureIndex<'_>,
) -> HashSet<usize> {
    let mut result = HashSet::new();
    walk_nodes(root, &mut |node| {
        let Some(callable) = go_enclosing_callable(node, returned_closures) else {
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

fn go_binding_scope(
    node: TsNode<'_>,
    callable: TsNode<'_>,
) -> Option<(usize, usize, usize, usize)> {
    let mut current = node;
    let mut depth = 0;
    loop {
        if current.kind() == "block" {
            depth += 1;
            return Some((
                current.id(),
                current.start_byte(),
                current.end_byte(),
                depth,
            ));
        }
        if current.id() == callable.id() {
            return Some((callable.id(), callable.start_byte(), callable.end_byte(), 0));
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

fn go_outer_callable_for_captured_node<'tree>(
    mut node: TsNode<'tree>,
    returned_closures: &GoReturnedClosureIndex<'tree>,
) -> Option<TsNode<'tree>> {
    if returned_closures.contains(node) {
        return None;
    }
    let mut crossed_closure = false;
    loop {
        if node.kind() == "func_literal" {
            crossed_closure = true;
        }
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
                | "list_comprehension"
                | "set_comprehension"
                | "dictionary_comprehension"
                | "generator_expression"
                | "import_statement" => result.collect_local_binding_node(node, source),
                "case_pattern" if python_outermost_case_pattern(node) => {
                    result.collect_local_binding_node(node, source);
                }
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
                            cross_module_visible: false,
                        });
                    }
                }
            }
            let class = CachedClassDeclaration {
                name: name.clone(),
                declaration: owner,
                methods,
                cross_module_visible: false,
                runtime_closed: false,
                super_name: None,
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
        if !python_exact_single_name_import(node, module, imported_name, local_name, source) {
            *self
                .module_blockers
                .entry(local_name.to_string())
                .or_default() += 1;
            return;
        }
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
        if (python_direct_call_name(node, source)
            .is_some_and(|name| matches!(name, "setattr" | "delattr"))
            || python_getattr_exposes_namespace(node, source))
            && let Some(function) = python_enclosing_function(node)
        {
            self.dynamic_functions.insert(function.id());
        }
        if python_call_on_self_dict(node, source)
            && let Some(owner) = python_enclosing_function(node)
                .and_then(|function| self.functions.get(&function.id()))
                .and_then(|info| info.owner.as_ref())
                .map(|(owner, _)| *owner)
        {
            self.dynamic_class_owners.insert(owner);
        }
    }

    fn collect_assignment(&mut self, node: TsNode<'tree>, source: &str) {
        let Some(function) = python_enclosing_function(node) else {
            if node.child_by_field_name("left").is_some() {
                let names = python_binding_names(node, source);
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
        let names = python_binding_names(node, source);
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
            self.poison_class_members(owner, python_self_member_binding_names(node, source));
        }
        let direct_block = python_direct_statement_in_function(node, function);
        let receiver = if names.len() == 1 && left.kind() == "identifier" && direct_block {
            let class_name = match node.child_by_field_name("right") {
                Some(right) => python_direct_constructor_name(right, source)
                    .filter(|name| name != "getattr")
                    .map(|name| (name, true)),
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
        for name in names {
            if name == "__dict__" {
                self.dynamic_class_owners.insert(owner);
            }
            self.method_blockers_by_owner_and_name.insert((owner, name));
        }
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
        if python_direct_call_name(call, source).is_some_and(|name| name == "getattr") {
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
                            rust_glob_local_module: None,
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

fn python_getattr_exposes_namespace(node: TsNode<'_>, source: &str) -> bool {
    if python_direct_call_name(node, source) != Some("getattr") {
        return false;
    }
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return false;
    };
    let mut cursor = arguments.walk();
    let mut values = arguments.named_children(&mut cursor);
    let _receiver = values.next();
    matches!(
        values.next().and_then(|value| node_text(value, source)),
        Some("\"__dict__\"" | "'__dict__'")
    )
}

fn python_unwrap_parenthesized(mut node: TsNode<'_>) -> TsNode<'_> {
    while node.kind() == "parenthesized_expression" && node.named_child_count() == 1 {
        let Some(inner) = node.named_child(0) else {
            break;
        };
        node = inner;
    }
    node
}

fn python_is_plain_self(node: TsNode<'_>, source: &str) -> bool {
    let node = python_unwrap_parenthesized(node);
    node.kind() == "identifier" && node_text(node, source) == Some("self")
}

fn python_is_self_dict(node: TsNode<'_>, source: &str) -> bool {
    let node = python_unwrap_parenthesized(node);
    node.kind() == "attribute"
        && node
            .child_by_field_name("object")
            .is_some_and(|object| python_is_plain_self(object, source))
        && node
            .child_by_field_name("attribute")
            .and_then(|attribute| node_text(attribute, source))
            == Some("__dict__")
}

fn python_call_on_self_dict(node: TsNode<'_>, source: &str) -> bool {
    let Some(function) = node.child_by_field_name("function") else {
        return false;
    };
    let function = python_unwrap_parenthesized(function);
    function.kind() == "attribute"
        && function
            .child_by_field_name("object")
            .is_some_and(|object| python_is_self_dict(object, source))
}

fn python_exact_relative_module(module: &str) -> bool {
    python_relative_module_components(module).is_some()
}

fn python_exact_single_name_import(
    node: TsNode<'_>,
    module: &str,
    imported_name: &str,
    local_name: &str,
    source: &str,
) -> bool {
    let Some(surface) = node_text(node, source).map(str::trim) else {
        return false;
    };
    let expected = if imported_name == local_name {
        format!("from {module} import {imported_name}")
    } else {
        format!("from {module} import {imported_name} as {local_name}")
    };
    surface == expected
}

fn python_relative_module_components(module: &str) -> Option<(usize, Vec<&str>)> {
    let depth = module.bytes().take_while(|byte| *byte == b'.').count();
    let components = module.get(depth..)?.split('.').collect::<Vec<_>>();
    (depth > 0
        && !components.is_empty()
        && components
            .iter()
            .all(|component| python_identifier(component)))
    .then_some((depth, components))
}

fn python_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn python_outermost_case_pattern(node: TsNode<'_>) -> bool {
    !node.parent().is_some_and(|parent| {
        matches!(
            parent.kind(),
            "case_pattern"
                | "as_pattern"
                | "class_pattern"
                | "dict_pattern"
                | "keyword_pattern"
                | "list_pattern"
                | "tuple_pattern"
                | "union_pattern"
        )
    })
}

fn python_for_each_binding_target<'tree>(
    node: TsNode<'tree>,
    mut visit: impl FnMut(TsNode<'tree>),
) {
    count_python_resolution_work(1);
    match node.kind() {
        "assignment" | "augmented_assignment" | "for_statement" => {
            if let Some(left) = node.child_by_field_name("left") {
                visit(left);
            }
        }
        "list_comprehension"
        | "set_comprehension"
        | "dictionary_comprehension"
        | "generator_expression" => {
            let mut cursor = node.walk();
            for clause in node.named_children(&mut cursor) {
                count_python_resolution_work(1);
                if clause.kind() == "for_in_clause"
                    && let Some(left) = clause.child_by_field_name("left")
                {
                    visit(left);
                }
            }
        }
        "with_item" => {
            if let Some(alias) = node
                .child_by_field_name("value")
                .filter(|value| value.kind() == "as_pattern")
                .and_then(|value| value.child_by_field_name("alias"))
            {
                visit(alias);
            }
        }
        "except_clause" => {
            if let Some(alias) = node.child_by_field_name("alias").or_else(|| {
                node.child_by_field_name("value")
                    .filter(|value| value.kind() == "as_pattern")
                    .and_then(|value| value.child_by_field_name("alias"))
            }) {
                visit(alias);
            }
        }
        "named_expression" => {
            if let Some(name) = node.child_by_field_name("name") {
                visit(name);
            }
        }
        "delete_statement" => {
            let mut cursor = node.walk();
            for target in node.named_children(&mut cursor) {
                count_python_resolution_work(1);
                visit(target);
            }
        }
        _ => {}
    }
}

fn python_binding_names(node: TsNode<'_>, source: &str) -> Vec<String> {
    let mut names = Vec::new();
    match node.kind() {
        "assignment"
        | "augmented_assignment"
        | "for_statement"
        | "list_comprehension"
        | "set_comprehension"
        | "dictionary_comprehension"
        | "generator_expression"
        | "with_item"
        | "except_clause"
        | "named_expression"
        | "delete_statement" => {
            python_for_each_binding_target(node, |target| {
                python_binding_target_names(target, source, &mut names);
            });
        }
        "case_pattern" => {
            python_collect_case_capture_names(node, source, &mut names);
        }
        "parameters" => {
            python_collect_parameter_binding_names(node, source, &mut names);
        }
        "import_statement" | "import_from_statement" => {
            python_collect_import_binding_names(node, source, &mut names);
        }
        "global_statement" | "nonlocal_statement" => {
            count_python_resolution_work(1);
            let mut cursor = node.walk();
            for name in node
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "identifier")
            {
                python_binding_target_names(name, source, &mut names);
            }
        }
        _ => python_binding_target_names(node, source, &mut names),
    }
    names.sort();
    names.dedup();
    names
}

fn python_collect_parameter_binding_names(node: TsNode<'_>, source: &str, names: &mut Vec<String>) {
    count_python_resolution_work(1);
    let mut cursor = node.walk();
    for parameter in node.named_children(&mut cursor) {
        count_python_resolution_work(1);
        match parameter.kind() {
            "default_parameter" | "typed_default_parameter" => {
                if let Some(name) = parameter.child_by_field_name("name") {
                    python_binding_target_names(name, source, names);
                }
            }
            "typed_parameter" => {
                for index in 0..parameter.named_child_count() {
                    let Ok(index) = u32::try_from(index) else {
                        break;
                    };
                    if parameter.field_name_for_named_child(index) == Some("type") {
                        continue;
                    }
                    if let Some(target) = parameter.named_child(index) {
                        python_binding_target_names(target, source, names);
                    }
                }
            }
            "identifier"
            | "keyword_identifier"
            | "list_splat_pattern"
            | "dictionary_splat_pattern"
            | "tuple_pattern" => {
                python_binding_target_names(parameter, source, names);
            }
            _ => {}
        }
    }
}

fn python_collect_import_binding_names(node: TsNode<'_>, source: &str, names: &mut Vec<String>) {
    count_python_resolution_work(1);
    for index in 0..node.named_child_count() {
        let Ok(index) = u32::try_from(index) else {
            break;
        };
        count_python_resolution_work(1);
        if node.field_name_for_named_child(index) != Some("name") {
            continue;
        }
        let Some(imported) = node.named_child(index) else {
            continue;
        };
        if imported.kind() == "aliased_import" {
            if let Some(alias) = imported.child_by_field_name("alias") {
                python_binding_target_names(alias, source, names);
            }
            continue;
        }
        if imported.kind() != "dotted_name" {
            continue;
        }
        let mut cursor = imported.walk();
        if let Some(binding) = imported
            .named_children(&mut cursor)
            .find(|child| child.kind() == "identifier")
        {
            python_binding_target_names(binding, source, names);
        }
    }
}

fn python_collect_case_capture_names(node: TsNode<'_>, source: &str, names: &mut Vec<String>) {
    count_python_resolution_work(1);
    match node.kind() {
        "dotted_name" => {
            let mut cursor = node.walk();
            let identifiers = node
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "identifier")
                .collect::<Vec<_>>();
            if let [identifier] = identifiers.as_slice()
                && let Some(name) =
                    node_text(*identifier, source).filter(|name| python_identifier(name))
            {
                names.push(name.to_string());
            }
        }
        "class_pattern" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() != "dotted_name" {
                    python_collect_case_capture_names(child, source, names);
                }
            }
        }
        "keyword_pattern" => {
            let mut cursor = node.walk();
            let mut skipped_keyword = false;
            for child in node.named_children(&mut cursor) {
                if child.kind() == "identifier" && !skipped_keyword {
                    skipped_keyword = true;
                } else {
                    python_collect_case_capture_names(child, source, names);
                }
            }
        }
        "dict_pattern" => {
            for index in 0..node.named_child_count() {
                let Ok(index) = u32::try_from(index) else {
                    break;
                };
                let Some(child) = node.named_child(index) else {
                    continue;
                };
                if node.field_name_for_named_child(index) == Some("value")
                    || child.kind() == "splat_pattern"
                {
                    python_collect_case_capture_names(child, source, names);
                }
            }
        }
        "as_pattern" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "case_pattern" {
                    python_collect_case_capture_names(child, source, names);
                } else if child.kind() == "identifier"
                    && let Some(name) =
                        node_text(child, source).filter(|name| python_identifier(name))
                {
                    names.push(name.to_string());
                }
            }
        }
        "splat_pattern" => {
            let mut cursor = node.walk();
            for child in node
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "identifier")
            {
                if let Some(name) = node_text(child, source).filter(|name| python_identifier(name))
                {
                    names.push(name.to_string());
                }
            }
        }
        "case_pattern" | "list_pattern" | "tuple_pattern" | "union_pattern" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                python_collect_case_capture_names(child, source, names);
            }
        }
        _ => {}
    }
}

fn python_binding_target_names(node: TsNode<'_>, source: &str, names: &mut Vec<String>) {
    count_python_resolution_work(1);
    match node.kind() {
        "identifier" | "keyword_identifier" => {
            if let Some(name) = node_text(node, source).filter(|name| python_identifier(name)) {
                names.push(name.to_string());
            }
        }
        "as_pattern_target"
        | "dictionary_splat"
        | "dictionary_splat_pattern"
        | "expression_list"
        | "list"
        | "list_pattern"
        | "list_splat"
        | "list_splat_pattern"
        | "parenthesized_expression"
        | "pattern_list"
        | "tuple"
        | "tuple_pattern" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                python_binding_target_names(child, source, names);
            }
        }
        _ => {}
    }
}

fn python_self_member_binding_names(node: TsNode<'_>, source: &str) -> Vec<String> {
    let mut names = Vec::new();
    python_for_each_binding_target(node, |target| {
        python_collect_self_member_binding_names(target, source, &mut names);
    });
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
    match node.kind() {
        "attribute" => {
            if node
                .child_by_field_name("object")
                .is_some_and(|object| python_is_plain_self(object, source))
                && let Some(name) = node
                    .child_by_field_name("attribute")
                    .and_then(|attribute| node_text(attribute, source))
                    .filter(|name| python_identifier(name))
            {
                names.push(name.to_string());
            }
        }
        "subscript"
            if node
                .child_by_field_name("value")
                .is_some_and(|value| python_is_self_dict(value, source)) =>
        {
            names.push("__dict__".to_owned());
        }
        "as_pattern_target"
        | "dictionary_splat"
        | "dictionary_splat_pattern"
        | "expression_list"
        | "list"
        | "list_pattern"
        | "list_splat"
        | "list_splat_pattern"
        | "parenthesized_expression"
        | "pattern_list"
        | "tuple"
        | "tuple_pattern" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                python_collect_self_member_binding_names(child, source, names);
            }
        }
        _ => {}
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
    typescript_directory_imports_enabled: bool,
}

impl<'tree> JavascriptResolutionIndex<'tree> {
    fn build(
        tree: &'tree Tree,
        source: &str,
        source_path: &Path,
        language: &str,
        file_id: NodeId,
        nodes: &[Node],
    ) -> Self {
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
            typescript_directory_imports_enabled: typescript_directory_imports_enabled(
                language,
                source_path,
            ),
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
            let supported = typescript_import_bindings_for_statement(
                node,
                source,
                self.typescript_directory_imports_enabled,
            );
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
                        cross_module_visible: false,
                        runtime_closed: false,
                        super_name: None,
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
                rust_glob_local_module: None,
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
            cross_module_visible: false,
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
    module_has_glob_import: HashSet<Vec<String>>,
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
    documented_free_function_targets: HashSet<usize>,
    bounded_outer_attribute_ids: HashSet<usize>,
    bounded_attributed_callers: HashSet<usize>,
    bounded_attributed_callsites: HashSet<usize>,
}

impl<'tree> RustResolutionIndex<'tree> {
    fn build(tree: &'tree Tree, source: &str, file_id: NodeId, nodes: &[Node]) -> Self {
        let graph_nodes = RustGraphNodeIndex::prepare(file_id, nodes);
        let (
            attributed_items,
            documented_free_function_targets,
            bounded_outer_attribute_ids,
            bounded_attributed_callers,
            bounded_attributed_callsites,
        ) = rust_prepare_attribute_index(tree.root_node(), source);
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
            module_has_glob_import: HashSet::new(),
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
            attributed_items,
            documented_free_function_targets,
            bounded_outer_attribute_ids,
            bounded_attributed_callers,
            bounded_attributed_callsites,
        };
        result.collect_module(
            tree.root_node(),
            Vec::new(),
            None,
            false,
            source,
            &graph_nodes,
        );
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
        inherited_attributed: bool,
        source: &str,
        graph_nodes: &RustGraphNodeIndex,
    ) {
        let mut domain_complete = !inherited_attributed;
        let mut identifier_module_complete = !inherited_attributed;
        let mut file_children = Vec::new();
        let mut value_blockers = HashSet::new();
        let mut incomplete_value_names = HashSet::new();
        let mut unsupported_value_names = HashSet::new();
        let mut cursor = body.walk();
        let items = body.named_children(&mut cursor).collect::<Vec<_>>();
        for item in &items {
            let direct_domain_poison = item.kind() == "macro_invocation"
                || (item.kind() == "inner_attribute_item"
                    && !rust_inner_allow_preserves_module_bindings(*item, source))
                || (item.kind() == "attribute_item"
                    && !self.bounded_outer_attribute_ids.contains(&item.id()))
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
                    if self.attributed_items.contains(&item.id())
                        && !self.documented_free_function_targets.contains(&item.id())
                    {
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
                    if self.attributed_items.contains(&item.id()) {
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
                        self.attributed_items.contains(&item.id()),
                        source,
                        graph_nodes,
                    );
                }
                "use_declaration" => {
                    if self.attributed_items.contains(&item.id()) {
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
                            self.module_has_glob_import.insert(module_path.clone());
                        }
                        value_blockers.extend(rust_use_bound_names(*item, source));
                    }
                }
                "const_item" | "static_item" => {
                    if let Some(name) = declaration_name(*item, source) {
                        value_blockers.insert(name.to_string());
                    }
                }
                "type_item" => {
                    if let Some(name) = declaration_name(*item, source) {
                        value_blockers.insert(name.to_string());
                    }
                }
                "foreign_mod_item" => {
                    value_blockers.extend(rust_foreign_function_names(*item, source));
                }
                "mod_item" => {
                    let Some(name) = declaration_name(*item, source).map(str::to_string) else {
                        domain_complete = false;
                        identifier_module_complete = false;
                        continue;
                    };
                    if self.attributed_items.contains(&item.id()) {
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
                            inherited_attributed || self.attributed_items.contains(&item.id()),
                            source,
                            graph_nodes,
                        );
                    } else {
                        if inherited_attributed || self.attributed_items.contains(&item.id()) {
                            continue;
                        }
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
        let has_attributed_impl_item = impl_item.child_by_field_name("body").is_none_or(|body| {
            let mut cursor = body.walk();
            body.named_children(&mut cursor)
                .any(|item| self.attributed_items.contains(&item.id()))
        });
        let methods = direct_impl_functions(impl_item);
        let impl_complete = !impl_attributed
            && !has_attributed_impl_item
            && !rust_impl_has_direct_item_macro(impl_item);
        if !impl_complete {
            self.incomplete_inherent_owners
                .insert((module_path.to_vec(), owner_name.clone()));
        }
        for method in methods {
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
            let inherited_domain_complete = current_callable
                .is_none_or(|parent| self.callsite_domain_complete(parent, node.start_byte()));
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
            let attributed = self.attributed_items.contains(&node.id());
            let bounded_free_function = self.bounded_attributed_callers.contains(&node.id())
                && !self.inherent_callable_contexts.contains_key(&node.id());
            self.callable_complete.insert(
                node.id(),
                inherited_domain_complete && (!attributed || bounded_free_function),
            );
            self.callable_poison_ranges.insert(
                node.id(),
                rust_callable_poison_ranges(
                    node,
                    &self.bounded_outer_attribute_ids,
                    &self.bounded_attributed_callers,
                    &self.bounded_attributed_callsites,
                ),
            );
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
                    .callable_type_blockers
                    .get(&callable_id)
                    .is_some_and(|blockers| blockers.contains(raw_target))
                {
                    return (Some(caller), CachedResolutionBinding::Unsupported);
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
                            rust_glob_local_module: self
                                .module_has_glob_import
                                .contains(&module_path)
                                .then_some(module_path),
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

#[allow(clippy::type_complexity)]
fn rust_prepare_attribute_index(
    root: TsNode<'_>,
    source: &str,
) -> (
    HashSet<usize>,
    HashSet<usize>,
    HashSet<usize>,
    HashSet<usize>,
    HashSet<usize>,
) {
    #[allow(clippy::too_many_arguments)]
    fn collect(
        node: TsNode<'_>,
        source: &str,
        attributed_items: &mut HashSet<usize>,
        documented_free_function_targets: &mut HashSet<usize>,
        bounded_outer_attribute_ids: &mut HashSet<usize>,
        bounded_attributed_callers: &mut HashSet<usize>,
        bounded_attributed_callsites: &mut HashSet<usize>,
        direct_method_ids: &mut HashSet<usize>,
    ) {
        count_rust_resolution_work(1);
        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        let direct_method_owner = node
            .parent()
            .filter(|parent| matches!(parent.kind(), "impl_item" | "trait_item"));
        let free_function_container = node.kind() == "source_file"
            || node
                .parent()
                .is_some_and(|parent| parent.kind() == "mod_item");
        let mut attributes = Vec::new();
        let mut attribute_group_tainted = false;
        for child in children {
            count_rust_resolution_work(1);
            if child.kind() == "attribute_item" || rust_is_outer_doc_comment(child, source) {
                attributes.push(child);
                continue;
            }
            if rust_is_ordinary_non_doc_comment(child, source) {
                continue;
            }
            if !attributes.is_empty()
                && (matches!(
                    child.kind(),
                    "line_comment" | "block_comment" | "inner_attribute_item"
                ) || child.is_error())
            {
                attribute_group_tainted = true;
                continue;
            }
            let direct_method = child.kind() == "function_item" && direct_method_owner.is_some();
            if direct_method {
                count_rust_resolution_work(1);
                direct_method_ids.insert(child.id());
                if direct_method_owner.is_some_and(|owner| attributed_items.contains(&owner.id())) {
                    attributed_items.insert(child.id());
                }
            }
            if !attributes.is_empty() {
                attributed_items.insert(child.id());
                if free_function_container
                    && child.kind() == "function_item"
                    && !attribute_group_tainted
                    && !child.has_error()
                    && attributes
                        .iter()
                        .all(|attribute| rust_is_outer_doc_comment(*attribute, source))
                {
                    count_rust_resolution_work(attributes.len());
                    documented_free_function_targets.insert(child.id());
                }
                if !attribute_group_tainted
                    && rust_attribute_group_is_bounded(&attributes, child, source)
                {
                    bounded_outer_attribute_ids
                        .extend(attributes.iter().map(|attribute| attribute.id()));
                }
                if !attribute_group_tainted
                    && child.kind() == "function_item"
                    && !direct_method_ids.contains(&child.id())
                    && rust_attribute_group_preserves_caller(&attributes, source)
                {
                    bounded_attributed_callers.insert(child.id());
                }
                if !attribute_group_tainted
                    && rust_bounded_callsite_category(child)
                    && rust_attribute_group_preserves_callsite(&attributes, source)
                {
                    bounded_attributed_callsites.insert(child.id());
                }
                attributes.clear();
                attribute_group_tainted = false;
            }
            collect(
                child,
                source,
                attributed_items,
                documented_free_function_targets,
                bounded_outer_attribute_ids,
                bounded_attributed_callers,
                bounded_attributed_callsites,
                direct_method_ids,
            );
        }
    }

    let mut attributed_items = HashSet::new();
    let mut documented_free_function_targets = HashSet::new();
    let mut bounded_outer_attribute_ids = HashSet::new();
    let mut bounded_attributed_callers = HashSet::new();
    let mut bounded_attributed_callsites = HashSet::new();
    let mut direct_method_ids = HashSet::new();
    collect(
        root,
        source,
        &mut attributed_items,
        &mut documented_free_function_targets,
        &mut bounded_outer_attribute_ids,
        &mut bounded_attributed_callers,
        &mut bounded_attributed_callsites,
        &mut direct_method_ids,
    );
    (
        attributed_items,
        documented_free_function_targets,
        bounded_outer_attribute_ids,
        bounded_attributed_callers,
        bounded_attributed_callsites,
    )
}

fn rust_bounded_callsite_category(node: TsNode<'_>) -> bool {
    node.kind() == "block"
        || node.kind() == "expression_statement"
        || node.kind().ends_with("_expression")
}

fn rust_attribute_body<'source>(
    attribute: TsNode<'_>,
    source: &'source str,
) -> Option<&'source str> {
    node_text(attribute, source)?
        .trim()
        .strip_prefix("#[")?
        .strip_suffix(']')
        .map(str::trim)
}

fn rust_is_outer_doc_comment(node: TsNode<'_>, source: &str) -> bool {
    let Some(surface) = node_text(node, source).map(str::trim_start) else {
        return false;
    };
    match node.kind() {
        "line_comment" => surface.starts_with("///") && !surface.starts_with("////"),
        "block_comment" => surface.starts_with("/**") && !surface.starts_with("/***"),
        _ => false,
    }
}

fn rust_is_ordinary_non_doc_comment(node: TsNode<'_>, source: &str) -> bool {
    let Some(surface) = node_text(node, source).map(str::trim_start) else {
        return false;
    };
    match node.kind() {
        "line_comment" => !surface.starts_with("//!"),
        "block_comment" => surface.ends_with("*/") && !surface.starts_with("/*!"),
        _ => false,
    }
}

fn rust_attribute_has_bounded_shape(attribute: TsNode<'_>, source: &str, name: &str) -> bool {
    let Some(body) = rust_attribute_body(attribute, source) else {
        return false;
    };
    let Some(remainder) = body.strip_prefix(name).map(str::trim) else {
        return false;
    };
    match name {
        "cfg" | "allow" => remainder
            .strip_prefix('(')
            .and_then(|arguments| arguments.strip_suffix(')'))
            .is_some_and(|arguments| !arguments.trim().is_empty()),
        "doc" => {
            remainder
                .strip_prefix('=')
                .is_some_and(|value| !value.trim().is_empty())
                || remainder
                    .strip_prefix('(')
                    .and_then(|arguments| arguments.strip_suffix(')'))
                    .is_some_and(|arguments| !arguments.trim().is_empty())
        }
        "inline" => remainder.is_empty() || matches!(remainder, "(always)" | "(never)"),
        _ => false,
    }
}

fn rust_attribute_group_preserves_caller(attributes: &[TsNode<'_>], source: &str) -> bool {
    attributes.iter().all(|attribute| {
        rust_is_outer_doc_comment(*attribute, source)
            || ["cfg", "allow", "doc", "inline"]
                .into_iter()
                .any(|name| rust_attribute_has_bounded_shape(*attribute, source, name))
    })
}

fn rust_attribute_group_preserves_callsite(attributes: &[TsNode<'_>], source: &str) -> bool {
    attributes.iter().all(|attribute| {
        rust_is_outer_doc_comment(*attribute, source)
            || ["cfg", "allow", "doc"]
                .into_iter()
                .any(|name| rust_attribute_has_bounded_shape(*attribute, source, name))
    })
}

fn rust_attribute_group_is_bounded(
    attributes: &[TsNode<'_>],
    attributed_item: TsNode<'_>,
    source: &str,
) -> bool {
    let is_helper_type = matches!(attributed_item.kind(), "struct_item" | "enum_item");
    let has_derive = attributes.iter().any(|attribute| {
        node_text(*attribute, source).is_some_and(|surface| {
            surface
                .trim()
                .strip_prefix("#[")
                .is_some_and(|body| body.starts_with("derive("))
        })
    });
    attributes.iter().all(|attribute| {
        if rust_is_outer_doc_comment(*attribute, source) {
            return true;
        }
        let Some(surface) = node_text(*attribute, source) else {
            return false;
        };
        let Some(body) = surface.trim().strip_prefix("#[") else {
            return false;
        };
        let name = body
            .split(|character: char| {
                character == '('
                    || character == '='
                    || character == ']'
                    || character.is_whitespace()
            })
            .next()
            .unwrap_or_default();
        match name {
            "allow" | "cfg" | "derive" | "doc" => true,
            "inline" => attributed_item.kind() == "function_item",
            "serde" | "error" => is_helper_type && has_derive,
            "cfg_attr" if is_helper_type => body
                .strip_prefix("cfg_attr(")
                .and_then(|arguments| arguments.strip_suffix(")]"))
                .and_then(|arguments| arguments.split_once(','))
                .is_some_and(|(_, injected)| {
                    let injected = injected.trim();
                    injected.starts_with("derive(") && injected.ends_with(')')
                }),
            _ => false,
        }
    })
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

fn rust_callable_poison_ranges(
    function: TsNode<'_>,
    bounded_outer_attribute_ids: &HashSet<usize>,
    bounded_attributed_callers: &HashSet<usize>,
    bounded_attributed_callsites: &HashSet<usize>,
) -> Vec<(usize, usize)> {
    fn collect(
        node: TsNode<'_>,
        root_id: usize,
        scope: (usize, usize),
        bounded_outer_attribute_ids: &HashSet<usize>,
        bounded_attributed_callers: &HashSet<usize>,
        bounded_attributed_callsites: &HashSet<usize>,
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
            || (node.kind() == "use_declaration" && contains_node_kind(node, "use_wildcard"))
        {
            output.push(scope);
        }

        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        let mut pending_attributes = Vec::new();
        for child in children {
            if child.kind() == "attribute_item" || bounded_outer_attribute_ids.contains(&child.id())
            {
                pending_attributes.push(child);
                continue;
            }
            if !pending_attributes.is_empty() {
                if bounded_attributed_callers.contains(&child.id()) {
                    output.push((scope.0, child.start_byte()));
                    output.push((child.end_byte(), scope.1));
                } else if !bounded_attributed_callsites.contains(&child.id()) {
                    output.push(scope);
                }
                pending_attributes.clear();
            }
            collect(
                child,
                root_id,
                scope,
                bounded_outer_attribute_ids,
                bounded_attributed_callers,
                bounded_attributed_callsites,
                output,
            );
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
        bounded_outer_attribute_ids,
        bounded_attributed_callers,
        bounded_attributed_callsites,
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
    let Some(argument) = declaration.child_by_field_name("argument") else {
        return Vec::new();
    };
    let mut names = Vec::new();
    rust_collect_use_bound_names(argument, None, source, &mut names);
    names.sort();
    names.dedup();
    names
}

fn rust_collect_use_bound_names(
    clause: TsNode<'_>,
    scoped_name: Option<&str>,
    source: &str,
    names: &mut Vec<String>,
) {
    match clause.kind() {
        "use_as_clause" => {
            if let Some(alias) = clause
                .child_by_field_name("alias")
                .and_then(|alias| node_text(alias, source))
                .filter(|alias| rust_simple_identifier(alias))
            {
                names.push(alias.to_string());
            }
        }
        "scoped_use_list" => {
            let path_name = clause
                .child_by_field_name("path")
                .and_then(|path| rust_use_terminal_name(path, source));
            if let Some(list) = clause.child_by_field_name("list") {
                rust_collect_use_bound_names(list, path_name.as_deref(), source, names);
            }
        }
        "use_list" => {
            let mut cursor = clause.walk();
            for child in clause.named_children(&mut cursor) {
                rust_collect_use_bound_names(child, scoped_name, source, names);
            }
        }
        "scoped_identifier" => {
            if let Some(name) = rust_use_terminal_name(clause, source) {
                names.push(name);
            }
        }
        "identifier" => {
            if let Some(name) =
                node_text(clause, source).filter(|name| rust_simple_identifier(name))
            {
                names.push(name.to_string());
            }
        }
        "self" => {
            if let Some(name) = scoped_name {
                names.push(name.to_string());
            }
        }
        "use_wildcard" | "crate" | "super" | "metavariable" => {}
        _ => {}
    }
}

fn rust_use_terminal_name(path: TsNode<'_>, source: &str) -> Option<String> {
    let name = if path.kind() == "scoped_identifier" {
        path.child_by_field_name("name")?
    } else {
        path
    };
    node_text(name, source)
        .filter(|name| rust_simple_identifier(name))
        .map(str::to_string)
}

fn rust_foreign_function_names(declaration: TsNode<'_>, source: &str) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(body) = declaration.child_by_field_name("body") {
        let mut cursor = body.walk();
        for item in body.named_children(&mut cursor) {
            if item.kind() == "function_signature_item"
                && let Some(name) = declaration_name(item, source)
            {
                names.push(name.to_string());
            }
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
    directory_imports_enabled: bool,
) -> Option<Vec<TypescriptImportBinding>> {
    let source_node = statement.child_by_field_name("source")?;
    let module_specifier = simple_typescript_string(source_node, source)?;
    let directory_specifier = matches!(module_specifier, "." | "..");
    if (directory_specifier && !directory_imports_enabled)
        || (!directory_specifier
            && !module_specifier.starts_with("./")
            && !module_specifier.starts_with("../"))
    {
        return None;
    }
    if has_direct_unnamed_token(statement, "type") {
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
            let mut parsed = Vec::with_capacity(specifiers.len());
            for specifier in specifiers {
                let type_only = has_direct_unnamed_token(specifier, "type");
                let imported = specifier.child_by_field_name("name")?;
                if imported.kind() != "identifier" {
                    return None;
                }
                let local = specifier.child_by_field_name("alias").unwrap_or(imported);
                if local.kind() != "identifier" {
                    return None;
                }
                let imported_name = node_text(imported, source)?;
                parsed.push((
                    typescript_import_binding(
                        local,
                        imported_name,
                        module_specifier,
                        false,
                        source,
                    )?,
                    type_only,
                ));
            }
            let mut local_counts = HashMap::<String, usize>::new();
            for (binding, _) in &parsed {
                *local_counts.entry(binding.local_name.clone()).or_default() += 1;
            }
            if local_counts.values().any(|count| *count != 1) {
                return None;
            }
            bindings.extend(
                parsed
                    .into_iter()
                    .filter_map(|(binding, type_only)| (!type_only).then_some(binding)),
            );
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

fn typescript_directory_imports_enabled(language: &str, source_path: &Path) -> bool {
    matches!(language, "typescript" | "tsx")
        && source_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "ts" | "tsx"))
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

fn exact_call_edge_projection_updates(
    syntax: &[ExactSyntaxCallsiteCorrelationInput<'_>],
    targets: &[NodeId],
    edge_inputs: &[OrdinaryCallEdgeCorrelationInput<'_>],
    edges: &[&Edge],
    raw_targets: &[&Node],
) -> Result<Vec<ExactCallEdgeProjection>> {
    if syntax.len() != targets.len()
        || edge_inputs.len() != edges.len()
        || edges.len() != raw_targets.len()
    {
        return Err(anyhow!(
            "exact CALL edge projection correlation inputs are misaligned"
        ));
    }
    let correlations = correlate_exact_syntax_callsites(syntax, edge_inputs);
    let mut candidates = Vec::with_capacity(syntax.len());
    let mut invalid_groups = HashSet::new();
    let mut edge_owners = HashMap::<EdgeId, (FileId, u32, NodeId, String)>::new();
    for (syntax_index, correlation) in correlations.into_iter().enumerate() {
        let Ok(edge_index) = correlation else {
            continue;
        };
        let input = syntax[syntax_index];
        let group = (
            input.file_id,
            input.line,
            input.caller,
            input.raw_target.to_owned(),
        );
        let edge = edges[edge_index];
        let raw_target = raw_targets[edge_index];
        let target = targets[syntax_index];
        let raw_target_is_eligible = match raw_target.kind {
            NodeKind::FUNCTION | NodeKind::METHOD => edge.target == target,
            NodeKind::UNKNOWN => {
                raw_target.file_node_id == edge.file_node_id && raw_target.start_line == edge.line
            }
            _ => false,
        };
        if edge.kind != EdgeKind::CALL || !raw_target_is_eligible {
            invalid_groups.insert(group.clone());
        }
        if let Some(previous_group) = edge_owners.insert(edge.id, group.clone()) {
            invalid_groups.insert(previous_group);
            invalid_groups.insert(group.clone());
        }
        candidates.push((group, edge_index, input.caller, target));
    }
    let mut projections = Vec::with_capacity(candidates.len());
    for (group, edge_index, caller, target) in candidates {
        if !invalid_groups.contains(&group) {
            let edge = edges[edge_index];
            let raw_target = raw_targets[edge_index];
            projections.push(ExactCallEdgeProjection {
                edge_id: edge.id,
                raw_source: edge.source,
                raw_target: edge.target,
                raw_kind: edge.kind,
                file_node_id: edge.file_node_id,
                line: edge.line,
                callsite_identity: edge.callsite_identity.clone(),
                raw_target_kind: raw_target.kind,
                raw_target_file_node_id: raw_target.file_node_id,
                raw_target_start_line: raw_target.start_line,
                raw_target_name: raw_target.serialized_name.clone(),
                caller,
                target,
            });
        }
    }
    Ok(projections)
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
    let mut csd_nodes_by_file = HashMap::<i64, Vec<Node>>::new();
    for node in nodes.iter().filter(|node| {
        node.file_node_id.is_some_and(|file_id| {
            file_by_id
                .get(&file_id.0)
                .is_some_and(|file| semantic_cache_requires_source_reauthentication(&file.language))
        })
    }) {
        let file_id = node
            .file_node_id
            .expect("filtered semantic node has file")
            .0;
        if file_by_id[&file_id].language == "bash" {
            count_bash_resolution_work(BashResolutionPhase::CacheReauthentication, 1);
        } else {
            count_java_kotlin_resolution_work(1);
        }
        csd_nodes_by_file
            .entry(file_id)
            .or_default()
            .push(node.clone());
    }
    let mut php_graph_names_by_file = HashMap::<i64, Vec<String>>::new();
    for node in nodes.iter().filter(|node| node.kind == NodeKind::NAMESPACE) {
        count_ruby_php_resolution_work(1);
        if let (Some(file_id), Some(name)) = (
            node.file_node_id,
            canonical_php_namespace_name(&node.serialized_name),
        ) {
            php_graph_names_by_file
                .entry(file_id.0)
                .or_default()
                .push(name);
        }
    }
    let mut edges = store.get_edges()?;
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
        let c_cpp_source_identity_matches = if is_c_cpp_language(&indexed_file.language) {
            record.file.c_cpp_file.as_ref().is_some_and(|file| {
                file.source_path == indexed_file.path
                    && file.source_role == c_cpp_source_role(&indexed_file.path)
            })
        } else {
            record.file.c_cpp_file.is_none()
        };
        let php_namespace_identity_matches = if indexed_file.language == "php" {
            count_ruby_php_resolution_work(1);
            let graph_names = php_graph_names_by_file
                .get(&indexed_file.id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            match &record.file.php_namespace {
                CachedPhpNamespace::Global => graph_names.is_empty(),
                CachedPhpNamespace::Named(name) => {
                    matches!(graph_names, [graph_name] if graph_name == name)
                }
                CachedPhpNamespace::Invalid => record.file.export_poison_all,
            }
        } else {
            record.file.php_namespace == CachedPhpNamespace::Invalid
        };
        if record.file.file_id != NodeId(indexed_file.id)
            || record.file.source_sha256 != *stored_hash
            || record.file.language != indexed_file.language
            || record.file.complete != indexed_file.complete
            || record.file.adapter_version != adapter_version(&indexed_file.language)
            || !c_cpp_source_identity_matches
            || !php_namespace_identity_matches
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
        if semantic_cache_requires_source_reauthentication(&indexed_file.language) {
            let source_bytes = std::fs::read(&indexed_file.path).with_context(|| {
                format!(
                    "proof resolution cannot authenticate semantic cache source {}",
                    indexed_file.path.display()
                )
            })?;
            if indexed_file.language == "bash" {
                count_bash_resolution_work(
                    BashResolutionPhase::CacheReauthentication,
                    source_bytes.len().saturating_add(1),
                );
            }
            if source_content_hash(&source_bytes) != *stored_hash {
                return Err(anyhow!(
                    "proof resolution semantic cache source drifted for {}",
                    indexed_file.path.display()
                ));
            }
            let source = std::str::from_utf8(&source_bytes).with_context(|| {
                format!(
                    "proof resolution semantic cache source is not UTF-8 for {}",
                    indexed_file.path.display()
                )
            })?;
            let config =
                parser_config_for_indexed_language(&indexed_file.path, &indexed_file.language)
                    .ok_or_else(|| {
                        anyhow!(
                            "proof resolution semantic cache has no selected parser for {} ({})",
                            indexed_file.language,
                            indexed_file.path.display()
                        )
                    })?;
            let mut parser = Parser::new();
            parser.set_language(&config.language).map_err(|error| {
                anyhow!("proof resolution semantic cache parser failed: {error:?}")
            })?;
            let tree = parser
                .parse(source, None)
                .ok_or_else(|| anyhow!("proof resolution semantic cache source did not parse"))?;
            let regenerated = collect_call_resolution_inputs(
                &tree,
                source,
                &indexed_file.path,
                &indexed_file.language,
                &expected_parser_fingerprint,
                NodeId(indexed_file.id),
                csd_nodes_by_file
                    .get(&indexed_file.id)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
            if regenerated.file.as_ref() != Some(&record.file) || regenerated.calls != record.calls
            {
                return Err(anyhow!(
                    "proof resolution semantic cache does not match authenticated source for {}",
                    indexed_file.path.display()
                ));
            }
        }
        records.push(record);
    }
    let mut linear_records = Vec::new();
    let mut records = records
        .into_iter()
        .filter_map(|record| {
            if matches!(
                record.file.language.as_str(),
                "bash" | "ruby" | "php" | "csharp" | "swift" | "dart"
            ) {
                if record.file.language == "bash" {
                    count_bash_resolution_work(BashResolutionPhase::Projection, 1);
                } else {
                    count_ruby_php_resolution_work(1);
                }
                linear_records.push(record);
                None
            } else {
                Some(record)
            }
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.path.cmp(&right.path));
    records.extend(linear_records);
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
    let mut linear_inputs = Vec::new();
    let mut inputs = records
        .iter()
        .flat_map(|record| record.calls.iter().cloned().map(move |call| (record, call)))
        .filter_map(|(record, call)| {
            if matches!(
                record.file.language.as_str(),
                "bash" | "ruby" | "php" | "csharp" | "swift" | "dart"
            ) {
                if record.file.language == "bash" {
                    count_bash_resolution_work(BashResolutionPhase::Projection, 1);
                } else {
                    count_ruby_php_resolution_work(1);
                }
                linear_inputs.push((record, call));
                None
            } else {
                Some((record, call))
            }
        })
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
    inputs.extend(linear_inputs);
    let mut callsite_members = HashSet::new();
    if inputs.iter().any(|(_, input)| {
        if matches!(
            input.language.as_str(),
            "bash" | "ruby" | "php" | "csharp" | "swift" | "dart"
        ) {
            if input.language == "bash" {
                count_bash_resolution_work(BashResolutionPhase::Projection, 1);
            } else {
                count_ruby_php_resolution_work(1);
            }
        }
        !callsite_members.insert((
            input.callsite.file_id,
            input.callsite.start_byte,
            input.callsite.end_byte_exclusive,
        ))
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
    let java_kotlin_projection_index = JavaKotlinProjectionIndex::prepare(&records);
    let python_projection_index = PythonProjectionIndex::prepare(&records, &record_by_path)?;
    let claim_indexes = SyntaxClaimIndexes {
        files: &file_by_id,
        records: &record_by_path,
        rust: &rust_projection_index,
        go: &go_projection_index,
        java_kotlin: &java_kotlin_projection_index,
        python: &python_projection_index,
    };
    let mut claims = inputs
        .into_iter()
        .map(|(source_record, input)| resolve_syntax_claim(&claim_indexes, source_record, input))
        .collect::<Result<Vec<_>>>()?;
    let bash_claim_count = claims
        .iter()
        .filter(|claim| claim.input.language == "bash")
        .count();
    if bash_claim_count > 0 {
        count_bash_resolution_work(
            BashResolutionPhase::GraphCorrelation,
            bash_claim_count
                .saturating_add(edges.len())
                .saturating_add(1),
        );
    }
    enforce_go_exact_callable_ownership(&mut claims, &nodes);
    enforce_exact_dependency_eligibility(
        &mut claims,
        &file_by_id,
        &node_by_id,
        &file_content_hash_by_id,
        &governed_by_id,
        &record_by_file_id,
    )?;
    enforce_exact_evidence_corroboration(
        &mut claims,
        &node_by_id,
        &file_by_id,
        &exact_evidence_validation,
    );
    let exact_claim_indices = claims
        .iter()
        .enumerate()
        .filter_map(|(index, claim)| {
            (claim.status == ProofResolutionStatus::Exact).then_some(index)
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

    let projection_syntax_inputs = exact_claim_indices
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
                target: NodeId(0),
                raw_target: &claim.input.callsite.raw_target,
            }
        })
        .collect::<Vec<_>>();
    let projection_targets = exact_claim_indices
        .iter()
        .map(|index| {
            claims[*index]
                .target
                .expect("Exact syntax claim has a target")
        })
        .collect::<Vec<_>>();
    let projection_edge_indices = edges
        .iter()
        .enumerate()
        .filter_map(|(index, edge)| {
            (edge.kind == EdgeKind::CALL
                && node_by_id.contains_key(&edge.target)
                && !constructor_evidence_nodes.contains(&edge.target))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let projection_edges = projection_edge_indices
        .iter()
        .map(|index| &edges[*index])
        .collect::<Vec<_>>();
    let projection_raw_targets = projection_edges
        .iter()
        .map(|edge| node_by_id[&edge.target])
        .collect::<Vec<_>>();
    let projection_edge_inputs = projection_edges
        .iter()
        .map(|edge| {
            let raw = node_by_id[&edge.target];
            let direct_member_edge = matches!(raw.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                || raw.file_node_id != edge.file_node_id;
            OrdinaryCallEdgeCorrelationInput {
                file_id: edge.file_node_id.map(|file| FileId(file.0)),
                line: edge.line,
                caller: edge.source,
                target: NodeId(0),
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
                semantic_exact: true,
            }
        })
        .collect::<Vec<_>>();
    let edge_projections = exact_call_edge_projection_updates(
        &projection_syntax_inputs,
        &projection_targets,
        &projection_edge_inputs,
        &projection_edges,
        &projection_raw_targets,
    )?;
    store.project_exact_call_edge_resolutions(&edge_projections)?;
    edges = store.get_edges()?;

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
        if claims[claim_index].input.language == "bash" {
            count_bash_resolution_work(BashResolutionPhase::Projection, 1);
            count_bash_resolution_work(BashResolutionPhase::GraphCorrelation, 1);
        }
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

#[derive(Clone)]
struct PythonPackageMarker {
    file_id: FileId,
}

#[derive(Default)]
struct PythonModuleCandidates {
    indexed: Vec<FileId>,
    uncovered: bool,
}

#[derive(Default)]
struct PythonProjectionIndex {
    complete_directories: HashSet<WorkspacePathIdentity>,
    aliased_directories: HashSet<WorkspacePathIdentity>,
    package_markers: HashMap<WorkspacePathIdentity, PythonPackageMarker>,
    package_ancestry_by_file: HashMap<i64, Vec<WorkspacePathIdentity>>,
    module_candidates: HashMap<(WorkspacePathIdentity, String), PythonModuleCandidates>,
    package_directories_by_marker: HashMap<i64, WorkspacePathIdentity>,
    file_directories: HashMap<i64, WorkspacePathIdentity>,
    record_identities_by_file: HashMap<i64, WorkspacePathIdentity>,
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
            if let Some(existing) = directories.insert(identity.clone(), directory.to_path_buf())
                && existing != directory
            {
                index.aliased_directories.insert(identity.clone());
            }
            index
                .file_directories
                .insert(record.file.file_id.0, identity);
            index.record_identities_by_file.insert(
                record.file.file_id.0,
                workspace_path_identity(&record.path).map_err(|error| {
                    anyhow!(
                        "Python proof resolution source has no native identity ({}): {error}",
                        record.path.display()
                    )
                })?,
            );
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
        for record in records
            .iter()
            .filter(|record| record.file.language == "python")
        {
            count_python_resolution_work(1);
            if record
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("py")
            {
                continue;
            }
            let Some(directory) = record.path.parent() else {
                continue;
            };
            let Ok(directory_identity) = workspace_path_identity(directory) else {
                continue;
            };
            let Some(file_name) = record.path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if file_name == "__init__.py" {
                if std::fs::symlink_metadata(&record.path)
                    .map_or(true, |metadata| metadata.file_type().is_symlink())
                {
                    continue;
                }
                let Some(parent) = directory.parent() else {
                    continue;
                };
                let Ok(parent_identity) = workspace_path_identity(parent) else {
                    continue;
                };
                if index.aliased_directories.contains(&directory_identity)
                    || std::fs::symlink_metadata(directory)
                        .map_or(true, |metadata| metadata.file_type().is_symlink())
                {
                    continue;
                }
                index.package_markers.insert(
                    directory_identity.clone(),
                    PythonPackageMarker {
                        file_id: FileId(record.file.file_id.0),
                    },
                );
                index
                    .package_directories_by_marker
                    .insert(record.file.file_id.0, directory_identity);
                let Some(package_name) = directory.file_name().and_then(|name| name.to_str())
                else {
                    continue;
                };
                if python_identifier(package_name) {
                    index
                        .module_candidates
                        .entry((parent_identity, package_name.to_owned()))
                        .or_default()
                        .indexed
                        .push(FileId(record.file.file_id.0));
                }
            } else if let Some(stem) = record.path.file_stem().and_then(|stem| stem.to_str())
                && python_identifier(stem)
            {
                index
                    .module_candidates
                    .entry((directory_identity, stem.to_owned()))
                    .or_default()
                    .indexed
                    .push(FileId(record.file.file_id.0));
            }
        }
        for candidates in index.module_candidates.values_mut() {
            candidates.indexed.sort();
            candidates.indexed.dedup();
        }
        for record in records
            .iter()
            .filter(|record| record.file.language == "python")
        {
            if let Some(ancestry) = python_prepared_package_ancestry(
                &record.path,
                &index.package_markers,
                &index.aliased_directories,
            ) {
                index
                    .package_ancestry_by_file
                    .insert(record.file.file_id.0, ancestry);
            }
        }
        for (identity, directory) in directories {
            let mut complete = true;
            if index.aliased_directories.contains(&identity)
                || std::fs::symlink_metadata(&directory)
                    .map_or(true, |metadata| metadata.file_type().is_symlink())
            {
                continue;
            }
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
                let entries = match std::fs::read_dir(&directory) {
                    Ok(entries) => entries,
                    Err(_) => continue,
                };
                for entry in entries {
                    count_python_resolution_work(1);
                    let Ok(entry) = entry else {
                        complete = false;
                        break;
                    };
                    let path = entry.path();
                    let Ok(kind) = entry.file_type() else {
                        complete = false;
                        break;
                    };
                    if !kind.is_dir() && !kind.is_symlink() {
                        continue;
                    }
                    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    if !python_identifier(name) {
                        continue;
                    }
                    let domain = index
                        .module_candidates
                        .entry((identity.clone(), name.to_owned()))
                        .or_default();
                    if kind.is_symlink() {
                        domain.uncovered = true;
                        continue;
                    }
                    let marker = path.join("__init__.py");
                    match std::fs::symlink_metadata(&marker) {
                        Ok(metadata) if metadata.file_type().is_symlink() || kind.is_symlink() => {
                            domain.uncovered = true;
                        }
                        Ok(_) => match workspace_path_identity(&marker) {
                            Ok(marker_identity)
                                if records_by_path.contains_key(&marker_identity) => {}
                            _ => domain.uncovered = true,
                        },
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => domain.uncovered = true,
                    }
                }
            }
            if complete {
                index.complete_directories.insert(identity);
            }
        }
        Ok(index)
    }

    fn directory_identity_is_complete(&self, directory: &WorkspacePathIdentity) -> bool {
        count_python_resolution_work(1);
        self.complete_directories.contains(directory)
    }

    fn module_candidates(
        &self,
        directory: &WorkspacePathIdentity,
        name: &str,
    ) -> (&[FileId], bool) {
        count_python_resolution_work(1);
        self.module_candidates
            .get(&(directory.clone(), name.to_owned()))
            .map(|domain| (domain.indexed.as_slice(), domain.uncovered))
            .unwrap_or_default()
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

fn python_prepared_package_ancestry(
    source: &Path,
    package_markers: &HashMap<WorkspacePathIdentity, PythonPackageMarker>,
    aliased_directories: &HashSet<WorkspacePathIdentity>,
) -> Option<Vec<WorkspacePathIdentity>> {
    let mut directory = source.parent()?.to_path_buf();
    let mut ancestry = Vec::new();
    let mut visited = HashSet::new();
    loop {
        count_python_resolution_work(1);
        if !visited.insert(directory.clone()) {
            return None;
        }
        let marker = directory.join("__init__.py");
        match std::fs::symlink_metadata(&marker) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(ancestry),
            Err(_) => return None,
            Ok(marker_metadata)
                if marker_metadata.file_type().is_symlink() || !marker_metadata.is_file() =>
            {
                return None;
            }
            Ok(_) => {}
        }
        let directory_metadata = std::fs::symlink_metadata(&directory).ok()?;
        if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
            return None;
        }
        let identity = workspace_path_identity(&directory).ok()?;
        if aliased_directories.contains(&identity) || !package_markers.contains_key(&identity) {
            return None;
        }
        ancestry.push(identity);
        directory = directory.parent()?.to_path_buf();
    }
}

fn resolve_python_relative_import<'a>(
    source_record: &ResolutionCacheRecord,
    module_specifier: &str,
    records: &'a HashMap<WorkspacePathIdentity, &ResolutionCacheRecord>,
    python_index: &PythonProjectionIndex,
) -> Result<PythonRelativeImportResolution<'a>> {
    let Some((depth, components)) = python_relative_module_components(module_specifier) else {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Missing,
            dependencies: Vec::new(),
        });
    };
    let Some(ancestry) = python_index
        .package_ancestry_by_file
        .get(&source_record.file.file_id.0)
    else {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Incomplete,
            dependencies: Vec::new(),
        });
    };
    let Some(base) = ancestry.get(depth - 1).cloned() else {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Missing,
            dependencies: Vec::new(),
        });
    };
    let mut package_directories = ancestry[..depth].to_vec();
    let mut dependency_ids = package_directories
        .iter()
        .filter_map(|directory| python_index.package_markers.get(directory))
        .map(|marker| marker.file_id)
        .collect::<Vec<_>>();
    let mut current = base;
    for component in &components[..components.len() - 1] {
        let (candidates, uncovered) = python_index.module_candidates(&current, component);
        if uncovered {
            return Ok(PythonRelativeImportResolution {
                target: RelativeImportResolution::Incomplete,
                dependencies: Vec::new(),
            });
        }
        let Some(marker_file) = (match candidates {
            [file_id] => python_index.package_directories_by_marker.get(&file_id.0),
            _ => None,
        })
        .cloned() else {
            return Ok(PythonRelativeImportResolution {
                target: if candidates.is_empty() && !uncovered {
                    RelativeImportResolution::Missing
                } else {
                    RelativeImportResolution::Incomplete
                },
                dependencies: Vec::new(),
            });
        };
        let marker = python_index
            .package_markers
            .get(&marker_file)
            .expect("package marker directory maps to a marker");
        dependency_ids.push(marker.file_id);
        package_directories.push(marker_file.clone());
        current = marker_file;
    }
    let (candidates, uncovered) =
        python_index.module_candidates(&current, components.last().expect("relative module leaf"));
    if uncovered {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Incomplete,
            dependencies: Vec::new(),
        });
    }
    let [target_file_id] = candidates else {
        return Ok(PythonRelativeImportResolution {
            target: if candidates.is_empty() && !uncovered {
                RelativeImportResolution::Missing
            } else {
                RelativeImportResolution::Incomplete
            },
            dependencies: Vec::new(),
        });
    };
    let Some(target_identity) = python_index
        .record_identities_by_file
        .get(&target_file_id.0)
    else {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Incomplete,
            dependencies: Vec::new(),
        });
    };
    let Some(target) = records.get(target_identity).copied() else {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Incomplete,
            dependencies: Vec::new(),
        });
    };
    let Some(target_directory) = python_index.file_directories.get(&target_file_id.0) else {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Incomplete,
            dependencies: Vec::new(),
        });
    };
    package_directories.push(target_directory.clone());
    if package_directories
        .iter()
        .any(|directory| !python_index.directory_identity_is_complete(directory))
    {
        return Ok(PythonRelativeImportResolution {
            target: RelativeImportResolution::Incomplete,
            dependencies: Vec::new(),
        });
    }
    dependency_ids.push(*target_file_id);
    let mut dependencies = dependency_ids;
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
    let candidates = if matches!(source_record.file.language.as_str(), "typescript" | "tsx")
        && matches!(module_specifier, "." | "..")
    {
        vec![base.join("index.ts"), base.join("index.tsx")]
    } else if base.extension().is_some() {
        let supported = match source_record.file.language.as_str() {
            "typescript" | "tsx" => ["ts", "tsx", "mts", "cts"].as_slice(),
            "javascript" => ["js", "jsx", "mjs", "cjs"].as_slice(),
            "ruby" => ["rb"].as_slice(),
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
            "ruby" => vec![base.with_extension("rb")],
            _ => return Ok(RelativeImportResolution::Missing),
        }
    };
    let mut matches = Vec::new();
    let mut uncovered = false;
    for candidate in candidates {
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata)
                if source_record.file.language == "ruby"
                    && (metadata.file_type().is_symlink() || !metadata.is_file()) =>
            {
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
    if source_record.file.language == "ruby" {
        let mut members = HashSet::new();
        matches.retain(|record| {
            count_ruby_php_resolution_work(1);
            members.insert(record.file.file_id)
        });
    } else {
        matches.sort_by_key(|record| record.file.file_id);
        matches.dedup_by_key(|record| record.file.file_id);
    }
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

enum GoCallableContainmentSubject {
    Callable(NodeId),
    Claim(usize),
}

struct GoCallableContainmentEvent {
    file_id: NodeId,
    line: u32,
    column: u32,
    order: u8,
    subject: GoCallableContainmentSubject,
}

fn enforce_go_exact_callable_ownership(claims: &mut [ResolvedSyntaxClaim], nodes: &[Node]) {
    let mut events = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD))
    {
        let (Some(file_id), Some(start_line), Some(end_line)) =
            (node.file_node_id, node.start_line, node.end_line)
        else {
            continue;
        };
        let start_column = node.start_col.unwrap_or(0);
        let end_column = node.end_col.unwrap_or(u32::MAX);
        if (end_line, end_column) < (start_line, start_column) {
            continue;
        }
        events.push(GoCallableContainmentEvent {
            file_id,
            line: start_line,
            column: start_column,
            order: 0,
            subject: GoCallableContainmentSubject::Callable(node.id),
        });
        events.push(GoCallableContainmentEvent {
            file_id,
            line: end_line,
            column: end_column,
            order: 2,
            subject: GoCallableContainmentSubject::Callable(node.id),
        });
    }
    for (index, claim) in claims.iter().enumerate().filter(|(_, claim)| {
        claim.status == ProofResolutionStatus::Exact && claim.input.language == "go"
    }) {
        events.push(GoCallableContainmentEvent {
            file_id: NodeId(claim.input.callsite.file_id.0),
            line: claim.input.callsite.line,
            column: claim.input.callsite.column,
            order: 1,
            subject: GoCallableContainmentSubject::Claim(index),
        });
    }
    events.sort_by(|left, right| {
        (left.file_id, left.line, left.column, left.order).cmp(&(
            right.file_id,
            right.line,
            right.column,
            right.order,
        ))
    });
    let mut active_file = None;
    let mut active = HashSet::new();
    for event in events {
        if active_file != Some(event.file_id) {
            active.clear();
            active_file = Some(event.file_id);
        }
        match event.subject {
            GoCallableContainmentSubject::Callable(callable) if event.order == 0 => {
                active.insert(callable);
            }
            GoCallableContainmentSubject::Callable(callable) => {
                active.remove(&callable);
            }
            GoCallableContainmentSubject::Claim(index) => {
                let claim = &mut claims[index];
                if active.len() != 1 || !active.contains(&claim.caller) {
                    claim.status = ProofResolutionStatus::Ambiguous;
                    claim.reason = ProofResolutionReason::MultipleBindings;
                    claim.target = None;
                    claim.evidence_chain.clear();
                }
            }
        }
    }
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

#[derive(Default)]
struct TypescriptDirectoryImportState {
    marker: Option<&'static str>,
    marker_seen: bool,
    admissible: usize,
    conflicting: usize,
}

impl TypescriptDirectoryImportState {
    fn record(&mut self, marker: Option<&'static str>, admissible: bool) {
        self.marker_seen |= marker.is_some();
        if admissible {
            let marker = marker.expect("directory marker admission requires a marker");
            self.marker.get_or_insert(marker);
            self.admissible += 1;
        } else {
            self.conflicting += 1;
        }
    }

    fn unique_marker(&self) -> Option<&'static str> {
        (self.admissible == 1 && self.conflicting == 0)
            .then_some(self.marker)
            .flatten()
    }
}

fn typescript_directory_specifier(literal: &str) -> Option<&'static str> {
    match literal {
        "'.'" | "\".\"" => Some("."),
        "'..'" | "\"..\"" => Some(".."),
        _ => None,
    }
}

struct ExactEvidenceValidationIndex {
    import_relations: HashMap<(NodeId, NodeId, NodeId), ProofRelationState>,
    swift_module_import_relations: HashMap<(NodeId, NodeId), ProofRelationState>,
    typescript_directory_import_relations:
        HashMap<(NodeId, NodeId), TypescriptDirectoryImportState>,
    member_relations: HashMap<(NodeId, NodeId), ProofRelationState>,
    member_by_owner_and_name: HashMap<(NodeId, String), Option<NodeId>>,
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
        let mut swift_module_import_relations = HashMap::<_, ProofRelationState>::new();
        let mut typescript_directory_import_relations =
            HashMap::<_, TypescriptDirectoryImportState>::new();
        let mut member_relations = HashMap::<_, ProofRelationState>::new();
        let mut python_import_edges = HashMap::<NodeId, Vec<NodeId>>::new();
        for edge in edges {
            if edge.kind == EdgeKind::IMPORT
                && edge.source == edge.target
                && let Some(file_id) = edge.file_node_id
                && let Some(import) = nodes.get(&edge.source)
                && import.kind == NodeKind::MODULE
                && import.file_node_id == Some(file_id)
            {
                let state = swift_module_import_relations
                    .entry((file_id, edge.source))
                    .or_default();
                if edge.effective_source() == edge.source
                    && edge.effective_target() == edge.target
                    && edge.resolved_target.is_none()
                    && edge.candidate_targets.is_empty()
                {
                    state.admissible += 1;
                } else {
                    state.conflicting += 1;
                }
            }
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
            if edge.kind == EdgeKind::IMPORT
                && let Some(import) = nodes.get(&edge.source)
                && import.kind == NodeKind::UNKNOWN
                && let Some(source_file) = import.file_node_id
            {
                let target = nodes.get(&edge.target);
                let marker = target
                    .filter(|target| target.kind == NodeKind::MODULE)
                    .and_then(|target| typescript_directory_specifier(&target.serialized_name));
                let admissible = marker.is_some()
                    && edge.file_node_id == Some(source_file)
                    && target.is_some_and(|target| target.file_node_id == Some(source_file))
                    && edge.effective_source() == edge.source
                    && edge.effective_target() == edge.target
                    && edge.resolved_target.is_none()
                    && edge.candidate_targets.is_empty();
                typescript_directory_import_relations
                    .entry((source_file, edge.source))
                    .or_default()
                    .record(marker, admissible);
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
        let mut member_by_owner_and_name = HashMap::new();
        for (&(owner, member), state) in &member_relations {
            if !state.is_unique() {
                continue;
            }
            let Some(member_node) = nodes.get(&member) else {
                continue;
            };
            let key = (
                owner,
                graph_leaf_name(&member_node.serialized_name).to_string(),
            );
            member_by_owner_and_name
                .entry(key)
                .and_modify(|candidate| *candidate = None)
                .or_insert(Some(member));
        }
        Self {
            import_relations,
            swift_module_import_relations,
            typescript_directory_import_relations,
            member_relations,
            member_by_owner_and_name,
            python_import_paths,
            python_import_path_counts,
        }
    }

    fn has_import(&self, file: NodeId, import: NodeId, target: NodeId) -> bool {
        self.import_relations
            .get(&(file, import, target))
            .is_some_and(ProofRelationState::is_unique)
    }

    fn has_swift_module_import(&self, file: NodeId, import: NodeId) -> bool {
        self.swift_module_import_relations
            .get(&(file, import))
            .is_some_and(ProofRelationState::is_unique)
    }

    fn typescript_directory_import(&self, file: NodeId, import: NodeId) -> Option<&'static str> {
        self.typescript_directory_import_relations
            .get(&(file, import))
            .and_then(TypescriptDirectoryImportState::unique_marker)
    }

    fn typescript_directory_marker_seen(&self, file: NodeId, import: NodeId) -> bool {
        self.typescript_directory_import_relations
            .get(&(file, import))
            .is_some_and(|state| state.marker_seen)
    }

    fn has_member(&self, owner: NodeId, member: NodeId) -> bool {
        self.member_relations
            .get(&(owner, member))
            .is_some_and(ProofRelationState::is_unique)
    }

    fn has_cpp_member_definition(
        &self,
        owner: NodeId,
        member: NodeId,
        nodes: &HashMap<NodeId, &Node>,
    ) -> bool {
        if self.has_member(owner, member) {
            return true;
        }
        let (Some(owner_node), Some(member_node)) = (nodes.get(&owner), nodes.get(&member)) else {
            return false;
        };
        if !matches!(owner_node.kind, NodeKind::CLASS | NodeKind::STRUCT)
            || member_node.kind != NodeKind::FUNCTION
            || owner_node.file_node_id.is_none()
            || owner_node.file_node_id != member_node.file_node_id
        {
            return false;
        }
        let member_name = graph_leaf_name(&member_node.serialized_name);
        let owner_name = owner_node
            .qualified_name
            .as_deref()
            .unwrap_or(&owner_node.serialized_name);
        let member_identity = member_node
            .qualified_name
            .as_deref()
            .unwrap_or(&member_node.serialized_name);
        if member_identity != format!("{owner_name}::{member_name}") {
            return false;
        }
        self.member_by_owner_and_name
            .get(&(owner, member_name.to_string()))
            .is_some_and(Option::is_some)
    }

    fn has_member_for_language(
        &self,
        language: &str,
        owner: NodeId,
        member: NodeId,
        nodes: &HashMap<NodeId, &Node>,
    ) -> bool {
        self.has_member(owner, member)
            || (language == "cpp" && self.has_cpp_member_definition(owner, member, nodes))
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
        files: &HashMap<i64, &codestory_store::FileInfo>,
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
        let typescript_directory_import =
            matches!(claim.input.language.as_str(), "typescript" | "tsx")
                && matches!(
                    &claim.input.binding,
                    CachedResolutionBinding::StaticImport { module_specifier, .. }
                        if matches!(module_specifier.as_str(), "." | "..")
                );
        let typescript_directory_import_is_correlated = typescript_directory_import
            && matches!(
                &claim.input.binding,
                CachedResolutionBinding::StaticImport {
                    module_specifier,
                    ..
                } if matches!(module_specifier.as_str(), "." | "..")
            )
            && matches!(
                &claim.input.binding,
                CachedResolutionBinding::StaticImport {
                    module_specifier,
                    import,
                    ..
                } if self.typescript_directory_import(source_file, *import) == Some(module_specifier)
            );
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
                matches!(
                    claim.input.language.as_str(),
                    "go" | "kotlin" | "csharp" | "swift" | "dart"
                ) && *declaration == target
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
                *declaration == target && nodes.get(import).is_some_and(|import_node| {
                    import_node.file_node_id == Some(source_file)
                        && (graph_leaf_name(&import_node.serialized_name)
                            == claim.input.callsite.raw_target
                            || matches!(
                                &claim.input.binding,
                                CachedResolutionBinding::JavaKotlinImportedFunction {
                                    import: binding_import,
                                    ..
                                } if is_csharp_swift_dart_language(&claim.input.language)
                                    && binding_import == import
                            )
                            || matches!(
                                &claim.input.binding,
                                CachedResolutionBinding::JavaKotlinImportedFunction {
                                    package_name,
                                    name,
                                    import: binding_import,
                                    ..
                                } if claim.input.language == "php"
                                    && binding_import == import
                                    && import_node.serialized_name
                                        == format!("{}\\{}", package_name.replace('.', "\\"), name)
                            ))
                })
                    && if matches!(claim.input.language.as_str(), "java" | "kotlin") {
                        target_node.file_node_id.is_some()
                            && target_node.qualified_name.as_deref().is_some_and(|name| {
                                name.ends_with(&claim.input.callsite.raw_target)
                            })
                    } else if claim.input.language == "dart" {
                        target_node.file_node_id.is_some_and(|target_file| {
                            dart_literal_import_target_is_authenticated(
                                source_file,
                                *import,
                                target_file,
                                &claim.exact_dependency_files,
                                nodes,
                                files,
                            )
                        })
                    } else if typescript_directory_import {
                        typescript_directory_import_is_correlated
                    } else if self.typescript_directory_marker_seen(source_file, *import) {
                        false
                    } else {
                        self.has_import(source_file, *import, target)
                    }
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
                    } else if claim.input.language == "cpp" {
                        matches!(target_node.kind, NodeKind::METHOD | NodeKind::FUNCTION)
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
                    && self.has_member_for_language(
                        &claim.input.language,
                        *owner,
                        claim.caller,
                        nodes,
                    )
                    && self.has_member_for_language(&claim.input.language, *owner, target, nodes)
            }
            (
                CalleeForm::ImplicitReceiver,
                [
                    ResolutionEvidence::ImplicitReceiver { owner },
                    ResolutionEvidence::SamePackageDeclaration { declaration },
                ],
            ) => {
                matches!(
                    claim.input.language.as_str(),
                    "go" | "csharp" | "swift" | "dart"
                ) && *declaration == target
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
                claim.caller,
                *constructor,
                *declaration,
                target,
                target_node,
                false,
                &claim.exact_dependency_files,
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
                claim.caller,
                *receiver_type,
                *declaration,
                target,
                target_node,
                false,
                &claim.exact_dependency_files,
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
                    &claim.exact_dependency_files,
                    nodes,
                    files,
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
                &claim.exact_dependency_files,
                nodes,
                files,
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
                    &claim.exact_dependency_files,
                    nodes,
                    files,
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
                    &claim.exact_dependency_files,
                    nodes,
                    files,
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
            ) if matches!(claim.input.language.as_str(), "java" | "kotlin")
                && *owner == *constructor
                && *owner == *receiver_type =>
            {
                self.imported_receiver_is_correlated(
                    &claim.input.language,
                    source_file,
                    *import,
                    *owner,
                    target,
                    target_node,
                    components,
                    &claim.exact_dependency_files,
                    nodes,
                    files,
                )
            }
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
                claim.caller,
                *constructor,
                *declaration,
                target,
                target_node,
                true,
                &claim.exact_dependency_files,
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
                claim.caller,
                *receiver_type,
                *declaration,
                target,
                target_node,
                true,
                &claim.exact_dependency_files,
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
        caller: NodeId,
        owner: NodeId,
        declaration: NodeId,
        target: NodeId,
        target_node: &Node,
        same_package: bool,
        domain_dependencies: &[FileId],
        nodes: &HashMap<NodeId, &Node>,
    ) -> bool {
        let Some(owner_node) = nodes.get(&owner) else {
            return false;
        };
        let receiver_domain_is_correlated = if same_package && language == "java" {
            let Some(caller_node) = nodes.get(&caller) else {
                return false;
            };
            let Some(owner_file) = owner_node.file_node_id else {
                return false;
            };
            let owner_qualified = owner_node.qualified_name.as_deref();
            let target_owner_qualified = target_node
                .qualified_name
                .as_deref()
                .and_then(|qualified| qualified.rsplit_once('.'))
                .map(|(owner, _)| owner);
            let caller_owner_qualified = caller_node
                .qualified_name
                .as_deref()
                .and_then(|qualified| qualified.rsplit_once('.'))
                .map(|(owner, _)| owner);
            let owner_package = owner_qualified
                .and_then(|qualified| qualified.rsplit_once('.'))
                .map(|(package, _)| package);
            let caller_package = caller_owner_qualified
                .and_then(|qualified| qualified.rsplit_once('.'))
                .map(|(package, _)| package);
            owner_node.kind == NodeKind::CLASS
                && caller_node.kind == NodeKind::METHOD
                && caller_node.file_node_id == Some(source_file)
                && owner_file != source_file
                && target_node.file_node_id == Some(owner_file)
                && owner_package.is_some()
                && owner_package == caller_package
                && owner_qualified == target_owner_qualified
                && domain_dependencies
                    .iter()
                    .any(|dependency| dependency.0 == source_file.0)
                && domain_dependencies
                    .iter()
                    .any(|dependency| dependency.0 == owner_file.0)
        } else if same_package {
            if is_csharp_swift_dart_language(language) {
                nodes.get(&caller).is_some_and(|caller_node| {
                    caller_node.file_node_id == Some(source_file)
                        && matches!(caller_node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                }) && owner_node.file_node_id.is_some()
                    && owner_node.file_node_id == target_node.file_node_id
                    && domain_dependencies
                        .iter()
                        .any(|dependency| dependency.0 == source_file.0)
                    && owner_node.file_node_id.is_some_and(|owner_file| {
                        domain_dependencies
                            .iter()
                            .any(|dependency| dependency.0 == owner_file.0)
                    })
            } else {
                language == "go"
                    && owner_node.file_node_id.is_some()
                    && target_node.file_node_id.is_some()
            }
        } else if language == "go" {
            owner_node.file_node_id.is_some() && target_node.file_node_id.is_some()
        } else {
            owner_node.file_node_id == Some(source_file)
                && owner_node.file_node_id == target_node.file_node_id
        };
        declaration == target
            && if matches!(language, "python" | "cpp") {
                target_node.kind == NodeKind::FUNCTION
            } else {
                target_node.kind == NodeKind::METHOD
            }
            && matches!(
                owner_node.kind,
                NodeKind::CLASS | NodeKind::STRUCT | NodeKind::ENUM
            )
            && receiver_domain_is_correlated
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
        domain_dependencies: &[FileId],
        nodes: &HashMap<NodeId, &Node>,
        files: &HashMap<i64, &codestory_store::FileInfo>,
    ) -> bool {
        let java_kotlin_literal_import = matches!(language, "java" | "kotlin")
            && nodes.get(&import).is_some_and(|import_node| {
                import_node.file_node_id == Some(source_file)
                    && matches!(import_node.kind, NodeKind::MODULE | NodeKind::UNKNOWN)
                    && nodes.get(&owner).is_some_and(|owner_node| {
                        owner_node.qualified_name.as_deref()
                            == Some(import_node.serialized_name.as_str())
                    })
            });
        let python_path = language == "python"
            && components.len() >= 4
            && components[components.len() - 2] == owner
            && components.last() == Some(&target)
            && self.has_python_import_path(
                source_file,
                import,
                &components[..components.len() - 2],
            );
        let swift_module_import = language == "swift"
            && components == [owner, target]
            && nodes.get(&import).is_some_and(|import_node| {
                import_node.kind == NodeKind::MODULE
                    && import_node.file_node_id == Some(source_file)
                    && self.has_swift_module_import(source_file, import)
                    && nodes
                        .get(&owner)
                        .and_then(|owner_node| owner_node.file_node_id)
                        .and_then(|file_id| files.get(&file_id.0))
                        .and_then(|file| swift_project_module(&file.path))
                        == Some(import_node.serialized_name.as_str())
            })
            && {
                let dependency_set = domain_dependencies.iter().copied().collect::<HashSet<_>>();
                let module = nodes
                    .get(&import)
                    .map(|import_node| import_node.serialized_name.as_str());
                module.is_some_and(|module| {
                    let expected = files
                        .values()
                        .filter(|file| {
                            file.indexed
                                && file.language == "swift"
                                && swift_project_module(&file.path) == Some(module)
                        })
                        .map(|file| FileId(file.id))
                        .collect::<HashSet<_>>();
                    !expected.is_empty() && dependency_set == expected
                })
            };
        let dart_literal_import = language == "dart"
            && components == [owner, target]
            && target_node.file_node_id.is_some_and(|target_file| {
                dart_literal_import_target_is_authenticated(
                    source_file,
                    import,
                    target_file,
                    domain_dependencies,
                    nodes,
                    files,
                )
            });
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
            && (python_path
                || java_kotlin_literal_import
                || swift_module_import
                || dart_literal_import
                || self.has_import(source_file, import, owner))
            && self.has_member(owner, target)
    }
}

fn proof_import_node_kind_is_literal(language: &str, kind: NodeKind) -> bool {
    match language {
        "go" | "rust" => kind == NodeKind::MODULE,
        "java" | "kotlin" | "csharp" | "swift" | "dart" | "javascript" | "typescript" | "tsx"
        | "python" | "php" | "ruby" => {
            matches!(kind, NodeKind::MODULE | NodeKind::UNKNOWN)
        }
        _ => false,
    }
}

fn enforce_exact_evidence_corroboration(
    claims: &mut [ResolvedSyntaxClaim],
    nodes: &HashMap<NodeId, &Node>,
    files: &HashMap<i64, &codestory_store::FileInfo>,
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
        if !validation.claim_has_literal_corroboration(claim, nodes, files) {
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
    dependency_files: Vec<FileId>,
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

    fn glob_local_module_dependencies(
        &self,
        record: &ResolutionCacheRecord,
        relative_module: &[String],
    ) -> Option<Vec<FileId>> {
        count_rust_resolution_work(1);
        let origin = self.origins.get(&record.file.file_id.0)?;
        let mut absolute_module = origin.base_module.clone();
        absolute_module.extend_from_slice(relative_module);
        let modules = self.modules.get(&(origin.root.clone(), absolute_module))?;
        let [module_match] = modules.as_slice() else {
            return None;
        };
        (module_match.record.file.file_id == record.file.file_id
            && module_match.relative_module == relative_module)
            .then(|| origin.dependency_files.clone())
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
            dependency_files: vec![FileId(file_id)],
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
                let mut dependency_files = parent.dependency_files;
                dependency_files.push(FileId(file_id));
                RustRecordOrigin {
                    root: parent.root,
                    base_module,
                    dependency_files,
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

#[derive(Debug, Clone)]
struct JavaKotlinImportCandidate {
    owner: NodeId,
    declaration: NodeId,
    file_id: FileId,
}

#[derive(Default)]
struct JavaKotlinImportDomain {
    complete: bool,
    poisoned: bool,
    dependencies: Vec<FileId>,
    dependency_members: HashSet<FileId>,
    declarations: HashMap<(Option<String>, String), Vec<JavaKotlinImportCandidate>>,
    classes: HashMap<String, Vec<JavaKotlinImportCandidate>>,
    cross_module_visible_nodes: HashSet<NodeId>,
    dart_runtime_closed_types: HashSet<String>,
    dart_overridden_methods: HashSet<(String, String)>,
    dart_parent_candidates: HashMap<String, Vec<Option<String>>>,
    dart_declared_methods: HashMap<String, Vec<String>>,
    dart_ancestry_complete: bool,
}

struct JavaKotlinProjectionIndex {
    domains: HashMap<(String, String), JavaKotlinImportDomain>,
    php_domains: HashMap<CachedPhpNamespace, JavaKotlinImportDomain>,
    php_identity_by_file: HashMap<i64, CachedPhpNamespace>,
    ruby_complete: bool,
    ruby_dependencies: Vec<FileId>,
    ruby_functions: HashMap<String, Vec<JavaKotlinImportCandidate>>,
    ruby_classes: HashMap<String, Vec<JavaKotlinImportCandidate>>,
    ruby_methods: HashMap<(String, String), Vec<JavaKotlinImportCandidate>>,
}

enum JavaKotlinImportResolution {
    Exact {
        owner: NodeId,
        declaration: NodeId,
        file_id: FileId,
        dependencies: Vec<FileId>,
    },
    Missing,
    Ambiguous,
    Incomplete,
}

fn dart_library_root(path: &Path) -> Option<&Path> {
    let mut current = path.parent();
    while let Some(directory) = current {
        if directory.file_name().and_then(|name| name.to_str()) == Some("lib") {
            return Some(directory);
        }
        current = directory.parent();
    }
    None
}

fn resolve_dart_literal_import(
    projection: &JavaKotlinProjectionIndex,
    records: &HashMap<WorkspacePathIdentity, &ResolutionCacheRecord>,
    source_record: &ResolutionCacheRecord,
    encoded_uri: &str,
    owner_name: Option<&str>,
    imported_name: &str,
    direct_construction: bool,
) -> JavaKotlinImportResolution {
    let Some(uri) = encoded_uri.strip_prefix("dart:uri:") else {
        return JavaKotlinImportResolution::Incomplete;
    };
    let relative = Path::new(uri);
    if relative
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("dart")
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return JavaKotlinImportResolution::Incomplete;
    }
    let Some(source_directory) = source_record.path.parent() else {
        return JavaKotlinImportResolution::Incomplete;
    };
    let target_path = source_directory.join(relative);
    if !matches!(
        std::fs::symlink_metadata(&target_path),
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file()
    ) {
        return JavaKotlinImportResolution::Incomplete;
    }
    let (Some(source_library), Some(target_library)) = (
        dart_library_root(&source_record.path),
        dart_library_root(&target_path),
    ) else {
        return JavaKotlinImportResolution::Incomplete;
    };
    if workspace_path_identity(source_library).ok() != workspace_path_identity(target_library).ok()
    {
        return JavaKotlinImportResolution::Incomplete;
    }
    let Ok(target_identity) = workspace_path_identity(&target_path) else {
        return JavaKotlinImportResolution::Incomplete;
    };
    let Some(target_record) = records.get(&target_identity).copied() else {
        return JavaKotlinImportResolution::Incomplete;
    };
    if target_record.file.language != "dart"
        || !target_record.file.complete
        || !target_record.file.lookup_input_complete
        || target_record.file.export_poison_all
        || target_record.file.file_id == source_record.file.file_id
    {
        return JavaKotlinImportResolution::Incomplete;
    }
    let Some(domain) = projection
        .domains
        .get(&(
            "dart".to_string(),
            target_record
                .file
                .java_kotlin_package
                .clone()
                .unwrap_or_default(),
        ))
        .filter(|domain| domain.complete && !domain.poisoned)
    else {
        return JavaKotlinImportResolution::Incomplete;
    };
    let file_id = FileId(target_record.file.file_id.0);
    let candidates = if let Some(owner_name) = owner_name {
        if !projection.dart_dispatch_is_closed(
            target_record
                .file
                .java_kotlin_package
                .as_deref()
                .unwrap_or_default(),
            owner_name,
            imported_name,
            direct_construction,
        ) {
            return JavaKotlinImportResolution::Incomplete;
        }
        let owners = target_record
            .file
            .classes
            .iter()
            .filter(|class| class.name == owner_name)
            .collect::<Vec<_>>();
        let [owner] = owners.as_slice() else {
            return if owners.is_empty() {
                JavaKotlinImportResolution::Missing
            } else {
                JavaKotlinImportResolution::Ambiguous
            };
        };
        owner
            .methods
            .iter()
            .filter(|method| method.name == imported_name)
            .map(|method| (owner.declaration, method.declaration))
            .collect::<Vec<_>>()
    } else {
        target_record
            .file
            .top_level_declarations
            .iter()
            .filter(|declaration| declaration.name == imported_name)
            .map(|declaration| (declaration.declaration, declaration.declaration))
            .collect::<Vec<_>>()
    };
    match candidates.as_slice() {
        [(owner, declaration)] => JavaKotlinImportResolution::Exact {
            owner: *owner,
            declaration: *declaration,
            file_id,
            dependencies: domain.dependencies.clone(),
        },
        [] => JavaKotlinImportResolution::Missing,
        _ => JavaKotlinImportResolution::Ambiguous,
    }
}

impl JavaKotlinProjectionIndex {
    fn prepare(records: &[ResolutionCacheRecord]) -> Self {
        let mut domains = HashMap::<(String, String), JavaKotlinImportDomain>::new();
        let mut php_domains = HashMap::<CachedPhpNamespace, JavaKotlinImportDomain>::new();
        let mut php_identity_by_file = HashMap::new();
        let mut php_invalid_namespace = false;
        let mut ruby_complete = true;
        let mut ruby_dependencies = Vec::new();
        let mut ruby_dependency_members = HashSet::new();
        let mut ruby_functions = HashMap::<String, Vec<JavaKotlinImportCandidate>>::new();
        let mut ruby_classes = HashMap::<String, Vec<JavaKotlinImportCandidate>>::new();
        let mut ruby_methods = HashMap::<(String, String), Vec<JavaKotlinImportCandidate>>::new();
        for record in records
            .iter()
            .filter(|record| record.file.language == "ruby")
        {
            ruby_complete &= record.file.complete
                && record.file.lookup_input_complete
                && !record.file.export_poison_all;
            let file_id = FileId(record.file.file_id.0);
            count_ruby_php_resolution_work(1);
            if ruby_dependency_members.insert(file_id) {
                ruby_dependencies.push(file_id);
            }
            for declaration in &record.file.top_level_declarations {
                count_ruby_php_resolution_work(1);
                ruby_functions
                    .entry(declaration.name.clone())
                    .or_default()
                    .push(JavaKotlinImportCandidate {
                        owner: declaration.declaration,
                        declaration: declaration.declaration,
                        file_id: FileId(record.file.file_id.0),
                    });
            }
            for class in &record.file.classes {
                count_ruby_php_resolution_work(1);
                ruby_classes.entry(class.name.clone()).or_default().push(
                    JavaKotlinImportCandidate {
                        owner: class.declaration,
                        declaration: class.declaration,
                        file_id,
                    },
                );
                for method in &class.methods {
                    count_ruby_php_resolution_work(1);
                    ruby_methods
                        .entry((class.name.clone(), method.name.clone()))
                        .or_default()
                        .push(JavaKotlinImportCandidate {
                            owner: class.declaration,
                            declaration: method.declaration,
                            file_id: FileId(record.file.file_id.0),
                        });
                }
            }
        }
        for record in records
            .iter()
            .filter(|record| record.file.language == "php")
        {
            count_ruby_php_resolution_work(1);
            let namespace = record.file.php_namespace.clone();
            if namespace == CachedPhpNamespace::Invalid {
                php_invalid_namespace = true;
                continue;
            }
            php_identity_by_file.insert(record.file.file_id.0, namespace.clone());
            count_ruby_php_resolution_work(1);
            let domain = php_domains.entry(namespace).or_default();
            let file_complete = record.file.complete && record.file.lookup_input_complete;
            if domain.dependencies.is_empty() {
                domain.complete = file_complete;
            } else {
                domain.complete &= file_complete;
            }
            domain.poisoned |= record.file.export_poison_all;
            let file_id = FileId(record.file.file_id.0);
            count_ruby_php_resolution_work(1);
            if domain.dependency_members.insert(file_id) {
                domain.dependencies.push(file_id);
            }
            for declaration in &record.file.top_level_declarations {
                count_ruby_php_resolution_work(1);
                domain
                    .declarations
                    .entry((None, declaration.name.clone()))
                    .or_default()
                    .push(JavaKotlinImportCandidate {
                        owner: declaration.declaration,
                        declaration: declaration.declaration,
                        file_id: FileId(record.file.file_id.0),
                    });
            }
            for class in &record.file.classes {
                count_ruby_php_resolution_work(1);
                domain.classes.entry(class.name.clone()).or_default().push(
                    JavaKotlinImportCandidate {
                        owner: class.declaration,
                        declaration: class.declaration,
                        file_id,
                    },
                );
                for method in &class.methods {
                    count_ruby_php_resolution_work(1);
                    domain
                        .declarations
                        .entry((Some(class.name.clone()), method.name.clone()))
                        .or_default()
                        .push(JavaKotlinImportCandidate {
                            owner: class.declaration,
                            declaration: method.declaration,
                            file_id: FileId(record.file.file_id.0),
                        });
                }
            }
        }
        if php_invalid_namespace {
            for domain in php_domains.values_mut() {
                count_ruby_php_resolution_work(1);
                domain.poisoned = true;
            }
        }
        for record in records
            .iter()
            .filter(|record| is_nominal_language(&record.file.language))
        {
            let Some(package_name) = record.file.java_kotlin_package.as_ref() else {
                continue;
            };
            let domain = domains
                .entry((record.file.language.clone(), package_name.clone()))
                .or_default();
            let file_complete = record.file.complete && record.file.lookup_input_complete;
            if domain.dependencies.is_empty() {
                domain.complete = file_complete;
            } else {
                domain.complete &= file_complete;
            }
            domain.poisoned |= record.file.export_poison_all;
            let file_id = FileId(record.file.file_id.0);
            if domain.dependency_members.insert(file_id) {
                domain.dependencies.push(file_id);
            }
            for declaration in &record.file.top_level_declarations {
                domain
                    .declarations
                    .entry((None, declaration.name.clone()))
                    .or_default()
                    .push(JavaKotlinImportCandidate {
                        owner: declaration.declaration,
                        declaration: declaration.declaration,
                        file_id,
                    });
                if declaration.cross_module_visible {
                    domain
                        .cross_module_visible_nodes
                        .insert(declaration.declaration);
                }
            }
            for class in &record.file.classes {
                domain.classes.entry(class.name.clone()).or_default().push(
                    JavaKotlinImportCandidate {
                        owner: class.declaration,
                        declaration: class.declaration,
                        file_id,
                    },
                );
                if class.cross_module_visible {
                    domain.cross_module_visible_nodes.insert(class.declaration);
                }
                if record.file.language == "dart" && class.runtime_closed {
                    domain.dart_runtime_closed_types.insert(class.name.clone());
                }
                if record.file.language == "dart" {
                    domain
                        .dart_parent_candidates
                        .entry(class.name.clone())
                        .or_default()
                        .push(class.super_name.clone());
                }
                for method in &class.methods {
                    domain
                        .declarations
                        .entry((Some(class.name.clone()), method.name.clone()))
                        .or_default()
                        .push(JavaKotlinImportCandidate {
                            owner: class.declaration,
                            declaration: method.declaration,
                            file_id,
                        });
                    if class.cross_module_visible && method.cross_module_visible {
                        domain.cross_module_visible_nodes.insert(method.declaration);
                    }
                    if record.file.language == "dart" {
                        domain
                            .dart_declared_methods
                            .entry(class.name.clone())
                            .or_default()
                            .push(method.name.clone());
                    }
                }
            }
        }
        for ((language, _), domain) in &mut domains {
            if is_java_kotlin_language(language) {
                domain.dependencies.sort();
                domain.dependencies.dedup();
                for candidates in domain.declarations.values_mut() {
                    candidates.sort_by_key(|candidate| candidate.declaration);
                    candidates.dedup_by(|left, right| left.declaration == right.declaration);
                }
            } else if language == "dart" {
                domain.dart_ancestry_complete = true;
                domain.dart_runtime_closed_types.retain(|name| {
                    matches!(
                        domain.dart_parent_candidates.get(name).map(Vec::as_slice),
                        Some([_])
                    )
                });
                for (subclass, methods) in &domain.dart_declared_methods {
                    let mut current = subclass.as_str();
                    let mut visited = HashSet::new();
                    loop {
                        if !visited.insert(current.to_string()) {
                            domain.dart_ancestry_complete = false;
                            break;
                        }
                        let Some([parent]) = domain
                            .dart_parent_candidates
                            .get(current)
                            .map(Vec::as_slice)
                        else {
                            domain.dart_ancestry_complete = false;
                            break;
                        };
                        let Some(parent) = parent.as_deref() else {
                            break;
                        };
                        let Some([_]) =
                            domain.dart_parent_candidates.get(parent).map(Vec::as_slice)
                        else {
                            domain.dart_ancestry_complete = false;
                            break;
                        };
                        for method in methods {
                            domain
                                .dart_overridden_methods
                                .insert((parent.to_string(), method.clone()));
                        }
                        current = parent;
                    }
                }
            }
        }
        Self {
            domains,
            php_domains,
            php_identity_by_file,
            ruby_complete,
            ruby_dependencies,
            ruby_functions,
            ruby_classes,
            ruby_methods,
        }
    }

    fn resolve(
        &self,
        language: &str,
        package_name: &str,
        owner_name: Option<&str>,
        imported_name: &str,
    ) -> JavaKotlinImportResolution {
        let Some(domain) = self
            .domains
            .get(&(language.to_string(), package_name.to_string()))
        else {
            return JavaKotlinImportResolution::Missing;
        };
        let resolution = Self::resolve_domain(domain, owner_name, imported_name);
        let (Some(owner_name), JavaKotlinImportResolution::Exact { owner, .. }) =
            (owner_name, &resolution)
        else {
            return resolution;
        };
        if matches!(
            domain.classes.get(owner_name).map(Vec::as_slice),
            Some([candidate]) if candidate.owner == *owner
        ) {
            resolution
        } else {
            JavaKotlinImportResolution::Ambiguous
        }
    }

    fn resolve_imported(
        &self,
        language: &str,
        package_name: &str,
        owner_name: Option<&str>,
        imported_name: &str,
    ) -> JavaKotlinImportResolution {
        let resolution = self.resolve(language, package_name, owner_name, imported_name);
        if language != "swift" {
            return resolution;
        }
        let Some(domain) = self
            .domains
            .get(&(language.to_string(), package_name.to_string()))
        else {
            return JavaKotlinImportResolution::Missing;
        };
        match &resolution {
            JavaKotlinImportResolution::Exact {
                owner, declaration, ..
            } if domain.cross_module_visible_nodes.contains(owner)
                && domain.cross_module_visible_nodes.contains(declaration) =>
            {
                resolution
            }
            JavaKotlinImportResolution::Exact { .. } => JavaKotlinImportResolution::Missing,
            _ => resolution,
        }
    }

    fn dart_dispatch_is_closed(
        &self,
        package_name: &str,
        owner_name: &str,
        method_name: &str,
        direct_construction: bool,
    ) -> bool {
        self.domains
            .get(&("dart".to_string(), package_name.to_string()))
            .is_some_and(|domain| {
                domain.dart_ancestry_complete
                    && (direct_construction
                        || (domain.dart_runtime_closed_types.contains(owner_name)
                            && !domain
                                .dart_overridden_methods
                                .contains(&(owner_name.to_string(), method_name.to_string()))))
            })
    }

    fn resolve_php(
        &self,
        namespace: &CachedPhpNamespace,
        owner_name: Option<&str>,
        imported_name: &str,
    ) -> JavaKotlinImportResolution {
        count_ruby_php_resolution_work(1);
        let Some(domain) = self.php_domains.get(namespace) else {
            return JavaKotlinImportResolution::Missing;
        };
        let resolution = Self::resolve_domain(domain, owner_name, imported_name);
        let (Some(owner_name), JavaKotlinImportResolution::Exact { owner, .. }) =
            (owner_name, &resolution)
        else {
            return resolution;
        };
        count_ruby_php_resolution_work(1);
        if matches!(
            domain.classes.get(owner_name).map(Vec::as_slice),
            Some([candidate]) if candidate.owner == *owner
        ) {
            resolution
        } else {
            JavaKotlinImportResolution::Ambiguous
        }
    }

    fn resolve_php_named(
        &self,
        namespace: &str,
        owner_name: Option<&str>,
        imported_name: &str,
    ) -> JavaKotlinImportResolution {
        self.resolve_php(
            &CachedPhpNamespace::Named(namespace.to_owned()),
            owner_name,
            imported_name,
        )
    }

    fn resolve_domain(
        domain: &JavaKotlinImportDomain,
        owner_name: Option<&str>,
        imported_name: &str,
    ) -> JavaKotlinImportResolution {
        if !domain.complete || domain.poisoned {
            return JavaKotlinImportResolution::Incomplete;
        }
        count_ruby_php_resolution_work(1);
        let candidates = domain
            .declarations
            .get(&(owner_name.map(str::to_string), imported_name.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        match candidates {
            [candidate] => JavaKotlinImportResolution::Exact {
                owner: candidate.owner,
                declaration: candidate.declaration,
                file_id: candidate.file_id,
                dependencies: domain.dependencies.clone(),
            },
            [] => JavaKotlinImportResolution::Missing,
            _ => JavaKotlinImportResolution::Ambiguous,
        }
    }

    fn php_dependencies(
        &self,
        source_file: FileId,
        evidence_files: impl IntoIterator<Item = FileId>,
    ) -> Option<Vec<FileId>> {
        let mut dependencies = Vec::new();
        let mut members = HashSet::new();
        for file_id in std::iter::once(source_file).chain(evidence_files) {
            count_ruby_php_resolution_work(1);
            let namespace = self.php_identity_by_file.get(&file_id.0)?;
            let domain = self.php_domains.get(namespace)?;
            if !domain.complete || domain.poisoned {
                return None;
            }
            for dependency in &domain.dependencies {
                count_ruby_php_resolution_work(1);
                if members.insert(*dependency) {
                    dependencies.push(*dependency);
                }
            }
        }
        Some(dependencies)
    }

    fn ruby_function(&self, name: &str, declaration: NodeId) -> Option<Vec<FileId>> {
        count_ruby_php_resolution_work(1);
        self.ruby_complete
            .then_some(())
            .filter(|_| {
                matches!(
                    self.ruby_functions.get(name).map(Vec::as_slice),
                    Some([candidate]) if candidate.declaration == declaration
                )
            })
            .map(|_| self.ruby_dependencies.clone())
    }

    fn ruby_method(
        &self,
        owner_name: &str,
        method_name: &str,
        owner: NodeId,
        declaration: NodeId,
    ) -> Option<Vec<FileId>> {
        count_ruby_php_resolution_work(2);
        self.ruby_complete
            .then_some(())
            .filter(|_| {
                matches!(
                    self.ruby_classes.get(owner_name).map(Vec::as_slice),
                    Some([candidate]) if candidate.owner == owner
                ) && matches!(
                    self.ruby_methods
                        .get(&(owner_name.to_string(), method_name.to_string()))
                        .map(Vec::as_slice),
                    Some([candidate])
                        if candidate.owner == owner && candidate.declaration == declaration
                )
            })
            .map(|_| self.ruby_dependencies.clone())
    }
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

struct SyntaxClaimIndexes<'a, 'records> {
    files: &'a HashMap<i64, &'records codestory_store::FileInfo>,
    records: &'a HashMap<WorkspacePathIdentity, &'records ResolutionCacheRecord>,
    rust: &'a RustProjectionIndex<'records>,
    go: &'a GoProjectionIndex<'records>,
    java_kotlin: &'a JavaKotlinProjectionIndex,
    python: &'a PythonProjectionIndex,
}
fn resolve_syntax_claim(
    indexes: &SyntaxClaimIndexes<'_, '_>,
    source_record: &ResolutionCacheRecord,
    input: CachedCallResolutionInput,
) -> Result<ResolvedSyntaxClaim> {
    if input.language == "bash" {
        count_bash_resolution_work(BashResolutionPhase::Projection, 1);
    }
    let files = indexes.files;
    let records = indexes.records;
    let rust_index = indexes.rust;
    let go_index = indexes.go;
    let java_kotlin_index = indexes.java_kotlin;
    let python_index = indexes.python;
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
        CachedResolutionBinding::SameFile {
            declaration,
            rust_glob_local_module,
        } => {
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
            } else if let Some(module_path) = rust_glob_local_module {
                if let Some(dependencies) =
                    rust_index.glob_local_module_dependencies(source_record, module_path)
                {
                    status = ProofResolutionStatus::Exact;
                    reason = ProofResolutionReason::ExactResolution;
                    target = Some(*declaration);
                    evidence_chain.push(ResolutionEvidence::SameFileDeclaration {
                        declaration: *declaration,
                    });
                    exact_node_file_expectations.push((*declaration, input.callsite.file_id));
                    exact_dependency_files = dependencies;
                } else {
                    status = ProofResolutionStatus::IncompleteDomain;
                    reason = ProofResolutionReason::LookupDomainIncomplete;
                }
            } else {
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(*declaration);
                evidence_chain.push(ResolutionEvidence::SameFileDeclaration {
                    declaration: *declaration,
                });
                exact_node_file_expectations.push((*declaration, input.callsite.file_id));
                if source_record.file.language == "ruby" {
                    if let Some(dependencies) =
                        java_kotlin_index.ruby_function(&input.callsite.raw_target, *declaration)
                    {
                        exact_dependency_files = dependencies;
                    } else {
                        status = ProofResolutionStatus::Unsupported;
                        reason = ProofResolutionReason::UnsupportedConstruct;
                        target = None;
                        evidence_chain.clear();
                    }
                } else if source_record.file.language == "php" {
                    let resolution = java_kotlin_index.resolve_php(
                        &source_record.file.php_namespace,
                        None,
                        &input.callsite.raw_target,
                    );
                    if let JavaKotlinImportResolution::Exact {
                        declaration: resolved,
                        dependencies,
                        ..
                    } = resolution
                        && resolved == *declaration
                    {
                        exact_dependency_files = dependencies;
                    } else {
                        status = ProofResolutionStatus::Unsupported;
                        reason = ProofResolutionReason::UnsupportedConstruct;
                        target = None;
                        evidence_chain.clear();
                    }
                }
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
                if source_record.file.language == "ruby" {
                    if let Some(dependencies) = java_kotlin_index.ruby_method(
                        owner_name,
                        &input.callsite.raw_target,
                        *owner,
                        *declaration,
                    ) {
                        exact_dependency_files = dependencies;
                    } else {
                        status = ProofResolutionStatus::Unsupported;
                        reason = ProofResolutionReason::UnsupportedConstruct;
                        target = None;
                        evidence_chain.clear();
                    }
                } else if source_record.file.language == "php" {
                    let resolution = java_kotlin_index.resolve_php(
                        &source_record.file.php_namespace,
                        Some(owner_name),
                        &input.callsite.raw_target,
                    );
                    if let JavaKotlinImportResolution::Exact {
                        owner: resolved_owner,
                        declaration: resolved,
                        dependencies,
                        ..
                    } = resolution
                        && resolved_owner == *owner
                        && resolved == *declaration
                    {
                        exact_dependency_files = dependencies;
                    } else {
                        status = ProofResolutionStatus::Unsupported;
                        reason = ProofResolutionReason::UnsupportedConstruct;
                        target = None;
                        evidence_chain.clear();
                    }
                }
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
        CachedResolutionBinding::JavaKotlinPackageFunction { package_name, name } => {
            match java_kotlin_index.resolve(&input.language, package_name, None, name) {
                JavaKotlinImportResolution::Exact {
                    declaration,
                    file_id,
                    mut dependencies,
                    ..
                } => {
                    status = ProofResolutionStatus::Exact;
                    reason = ProofResolutionReason::ExactResolution;
                    target = Some(declaration);
                    evidence_chain.push(if file_id == input.callsite.file_id {
                        ResolutionEvidence::SameFileDeclaration { declaration }
                    } else {
                        ResolutionEvidence::SamePackageDeclaration { declaration }
                    });
                    exact_node_file_expectations.push((declaration, file_id));
                    dependencies.push(input.callsite.file_id);
                    exact_dependency_files = dependencies;
                }
                JavaKotlinImportResolution::Missing => {
                    status = ProofResolutionStatus::MissingBinding;
                    reason = ProofResolutionReason::MissingBinding;
                }
                JavaKotlinImportResolution::Ambiguous => {
                    status = ProofResolutionStatus::Ambiguous;
                    reason = ProofResolutionReason::MultipleBindings;
                }
                JavaKotlinImportResolution::Incomplete => {
                    status = ProofResolutionStatus::IncompleteDomain;
                    reason = ProofResolutionReason::LookupDomainIncomplete;
                }
            }
        }
        CachedResolutionBinding::JavaKotlinImportedFunction {
            package_name,
            owner_name,
            name,
            import,
        } => match if input.language == "php" {
            java_kotlin_index.resolve_php_named(package_name, owner_name.as_deref(), name)
        } else if input.language == "dart" {
            resolve_dart_literal_import(
                java_kotlin_index,
                records,
                source_record,
                package_name,
                owner_name.as_deref(),
                name,
                true,
            )
        } else {
            java_kotlin_index.resolve_imported(
                &input.language,
                package_name,
                owner_name.as_deref(),
                name,
            )
        } {
            JavaKotlinImportResolution::Exact {
                owner,
                declaration,
                file_id,
                mut dependencies,
            } => {
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(declaration);
                evidence_chain.push(ResolutionEvidence::StaticImportBinding {
                    import: *import,
                    declaration,
                });
                exact_node_file_expectations.push((*import, input.callsite.file_id));
                exact_node_file_expectations.push((declaration, file_id));
                dependencies.push(input.callsite.file_id);
                exact_dependency_files = dependencies;
                let _ = owner;
            }
            JavaKotlinImportResolution::Missing => {
                status = ProofResolutionStatus::MissingBinding;
                reason = ProofResolutionReason::MissingBinding;
            }
            JavaKotlinImportResolution::Ambiguous => {
                status = ProofResolutionStatus::Ambiguous;
                reason = ProofResolutionReason::MultipleBindings;
            }
            JavaKotlinImportResolution::Incomplete => {
                status = ProofResolutionStatus::IncompleteDomain;
                reason = ProofResolutionReason::LookupDomainIncomplete;
            }
        },
        CachedResolutionBinding::JavaKotlinPackageReceiver {
            package_name,
            owner_name,
            method_name,
            constructor,
        } => match java_kotlin_index.resolve(
            &input.language,
            package_name,
            Some(owner_name),
            method_name,
        ) {
            JavaKotlinImportResolution::Exact {
                owner,
                declaration,
                file_id,
                mut dependencies,
            } => {
                if input.language == "dart"
                    && !java_kotlin_index.dart_dispatch_is_closed(
                        package_name,
                        owner_name,
                        method_name,
                        *constructor,
                    )
                {
                    status = ProofResolutionStatus::Unsupported;
                    reason = ProofResolutionReason::UnsupportedConstruct;
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
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(declaration);
                if input.callsite.callee_form == CalleeForm::ImplicitReceiver {
                    evidence_chain.push(ResolutionEvidence::ImplicitReceiver { owner });
                } else {
                    if *constructor {
                        evidence_chain
                            .push(ResolutionEvidence::ConstructorBinding { constructor: owner });
                    }
                    evidence_chain.push(ResolutionEvidence::ExplicitReceiverType {
                        receiver_type: owner,
                    });
                }
                evidence_chain.push(if file_id == input.callsite.file_id {
                    ResolutionEvidence::SameFileDeclaration { declaration }
                } else {
                    ResolutionEvidence::SamePackageDeclaration { declaration }
                });
                exact_node_file_expectations.push((owner, file_id));
                exact_node_file_expectations.push((declaration, file_id));
                dependencies.push(input.callsite.file_id);
                exact_dependency_files = dependencies;
            }
            JavaKotlinImportResolution::Missing => {
                status = ProofResolutionStatus::MissingBinding;
                reason = ProofResolutionReason::MissingBinding;
            }
            JavaKotlinImportResolution::Ambiguous => {
                status = ProofResolutionStatus::Ambiguous;
                reason = ProofResolutionReason::MultipleBindings;
            }
            JavaKotlinImportResolution::Incomplete => {
                status = ProofResolutionStatus::IncompleteDomain;
                reason = ProofResolutionReason::LookupDomainIncomplete;
            }
        },
        CachedResolutionBinding::JavaKotlinImportedReceiver {
            package_name,
            owner_name,
            method_name,
            import,
            constructor,
        } => match if input.language == "php" {
            java_kotlin_index.resolve_php_named(package_name, Some(owner_name), method_name)
        } else if input.language == "dart" {
            resolve_dart_literal_import(
                java_kotlin_index,
                records,
                source_record,
                package_name,
                Some(owner_name),
                method_name,
                *constructor,
            )
        } else {
            java_kotlin_index.resolve_imported(
                &input.language,
                package_name,
                Some(owner_name),
                method_name,
            )
        } {
            JavaKotlinImportResolution::Exact {
                owner,
                declaration,
                file_id,
                dependencies,
            } => {
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = Some(declaration);
                evidence_chain.push(ResolutionEvidence::StaticImportBinding {
                    import: *import,
                    declaration: owner,
                });
                if *constructor {
                    evidence_chain
                        .push(ResolutionEvidence::ConstructorBinding { constructor: owner });
                }
                evidence_chain.push(ResolutionEvidence::ExplicitReceiverType {
                    receiver_type: owner,
                });
                evidence_chain.push(ResolutionEvidence::QualifiedPath {
                    components: vec![owner, declaration],
                });
                exact_node_file_expectations.push((*import, input.callsite.file_id));
                exact_node_file_expectations.push((owner, file_id));
                exact_node_file_expectations.push((declaration, file_id));
                exact_dependency_files = dependencies;
            }
            JavaKotlinImportResolution::Missing => {
                status = ProofResolutionStatus::MissingBinding;
                reason = ProofResolutionReason::MissingBinding;
            }
            JavaKotlinImportResolution::Ambiguous => {
                status = ProofResolutionStatus::Ambiguous;
                reason = ProofResolutionReason::MultipleBindings;
            }
            JavaKotlinImportResolution::Incomplete => {
                status = ProofResolutionStatus::IncompleteDomain;
                reason = ProofResolutionReason::LookupDomainIncomplete;
            }
        },
        CachedResolutionBinding::CCppQualified { components } => {
            let recorded = input.language == "cpp"
                && components.len() == 2
                && source_record.file.c_cpp_file.as_ref().is_some_and(|file| {
                    file.namespaces
                        .iter()
                        .any(|namespace| namespace.declaration == components[0])
                        || source_record.file.classes.iter().any(|class| {
                            class.declaration == components[0]
                                && class
                                    .methods
                                    .iter()
                                    .any(|method| method.declaration == components[1])
                        })
                })
                && (source_record
                    .file
                    .top_level_declarations
                    .iter()
                    .any(|declaration| declaration.declaration == components[1])
                    || source_record.file.classes.iter().any(|class| {
                        class
                            .methods
                            .iter()
                            .any(|method| method.declaration == components[1])
                    }));
            if recorded {
                status = ProofResolutionStatus::Exact;
                reason = ProofResolutionReason::ExactResolution;
                target = components.last().copied();
                evidence_chain.push(ResolutionEvidence::QualifiedPath {
                    components: components.clone(),
                });
                exact_node_file_expectations.extend(
                    components
                        .iter()
                        .copied()
                        .map(|component| (component, input.callsite.file_id)),
                );
            } else {
                status = ProofResolutionStatus::Ambiguous;
                reason = ProofResolutionReason::MultipleBindings;
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
                let owner_name = target_record
                    .file
                    .classes
                    .iter()
                    .find(|class| class.declaration == owner)
                    .map(|class| class.name.as_str());
                if source_record.file.language == "ruby" {
                    if let Some(dependencies) = owner_name.and_then(|owner_name| {
                        java_kotlin_index.ruby_method(owner_name, method_name, owner, *method)
                    }) {
                        exact_dependency_files = dependencies;
                    } else {
                        status = ProofResolutionStatus::Unsupported;
                        reason = ProofResolutionReason::UnsupportedConstruct;
                        target = None;
                        evidence_chain.clear();
                    }
                } else if source_record.file.language == "php" {
                    let resolution = owner_name.map(|owner_name| {
                        java_kotlin_index.resolve_php(
                            &target_record.file.php_namespace,
                            Some(owner_name),
                            method_name,
                        )
                    });
                    if let Some(JavaKotlinImportResolution::Exact {
                        owner: resolved_owner,
                        declaration: resolved,
                        dependencies,
                        ..
                    }) = resolution
                        && resolved_owner == owner
                        && resolved == *method
                    {
                        exact_dependency_files = dependencies;
                    } else {
                        status = ProofResolutionStatus::Unsupported;
                        reason = ProofResolutionReason::UnsupportedConstruct;
                        target = None;
                        evidence_chain.clear();
                    }
                }
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
    if status == ProofResolutionStatus::Exact && source_record.file.language == "php" {
        let evidence_files = exact_node_file_expectations
            .iter()
            .map(|(_, file_id)| *file_id);
        if let Some(dependencies) =
            java_kotlin_index.php_dependencies(input.callsite.file_id, evidence_files)
        {
            exact_dependency_files = dependencies;
        } else {
            status = ProofResolutionStatus::IncompleteDomain;
            reason = ProofResolutionReason::LookupDomainIncomplete;
            target = None;
            evidence_chain.clear();
            exact_dependency_files.clear();
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
    let linear_dependency_order = matches!(
        input.language.as_str(),
        "bash" | "ruby" | "php" | "csharp" | "swift" | "dart"
    );
    let mut dependency_ids = Vec::new();
    let mut dependency_members = HashSet::new();
    let mut push_dependency = |file_id: NodeId| {
        if dependency_members.insert(file_id) {
            dependency_ids.push(file_id);
        }
        if input.language == "bash" {
            count_bash_resolution_work(BashResolutionPhase::Projection, 1);
        } else if linear_dependency_order {
            count_ruby_php_resolution_work(1);
        }
    };
    push_dependency(NodeId(input.callsite.file_id.0));
    if status == ProofResolutionStatus::Exact {
        for file in &claim.exact_dependency_files {
            push_dependency(NodeId(file.0));
        }
    }
    for node_id in evidence_chain
        .iter()
        .flat_map(ResolutionEvidence::node_ids)
        .chain(target)
    {
        if let Some(file_id) = nodes.get(&node_id).and_then(|node| node.file_node_id) {
            push_dependency(file_id);
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
    if !linear_dependency_order {
        dependency_file_hashes.sort();
    }
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
    let mut linear_rows = HashMap::<
        (String, Option<CalleeForm>, Option<ResolutionEvidenceKind>),
        ProofResolutionFunnelCounts,
    >::new();
    for fact in facts {
        let evidence_kind = fact.evidence_chain.first().map(ResolutionEvidence::kind);
        let key = (
            fact.provenance.language_adapter.clone(),
            Some(fact.callsite.callee_form),
            evidence_kind,
        );
        let counts = if matches!(
            fact.provenance.language_adapter.as_str(),
            "bash" | "ruby" | "php" | "csharp" | "swift" | "dart"
        ) {
            if fact.provenance.language_adapter == "bash" {
                count_bash_resolution_work(BashResolutionPhase::Projection, 1);
            } else if is_csharp_swift_dart_language(&fact.provenance.language_adapter) {
                count_java_kotlin_resolution_work(1);
            } else {
                count_ruby_php_resolution_work(1);
            }
            linear_rows.entry(key).or_default()
        } else {
            rows.entry(key).or_default()
        };
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
    for language in ["bash", "csharp", "dart", "php", "ruby", "swift"] {
        for callee_form in [
            CalleeForm::Constructor,
            CalleeForm::DynamicAccess,
            CalleeForm::ExplicitReceiver,
            CalleeForm::Identifier,
            CalleeForm::ImplicitReceiver,
            CalleeForm::NamedImport,
            CalleeForm::QualifiedPath,
        ] {
            for evidence_kind in [
                None,
                Some(ResolutionEvidenceKind::ConstructorBinding),
                Some(ResolutionEvidenceKind::ExplicitReceiverType),
                Some(ResolutionEvidenceKind::ImplicitReceiver),
                Some(ResolutionEvidenceKind::QualifiedPath),
                Some(ResolutionEvidenceKind::SameFileDeclaration),
                Some(ResolutionEvidenceKind::SamePackageDeclaration),
                Some(ResolutionEvidenceKind::StaticImportBinding),
            ] {
                let key = (language.to_owned(), Some(callee_form), evidence_kind);
                if language == "bash" {
                    count_bash_resolution_work(BashResolutionPhase::Projection, 1);
                } else if is_csharp_swift_dart_language(language) {
                    count_java_kotlin_resolution_work(1);
                } else {
                    count_ruby_php_resolution_work(1);
                }
                if let Some(counts) = linear_rows.remove(&key) {
                    result.push(ProofResolutionFunnelRow {
                        language: language.to_owned(),
                        callee_form: Some(callee_form),
                        evidence_kind,
                        counts,
                    });
                }
            }
        }
    }
    debug_assert!(linear_rows.is_empty());
    result
}

#[cfg(test)]
mod exact_edge_projection_tests {
    use super::*;

    #[test]
    fn exact_edge_projection_accepts_direct_and_placeholder_raw_targets() {
        let identities = ["1:2:1:3".to_owned(), "1:3:1:9".to_owned()];
        let syntax = [
            ExactSyntaxCallsiteCorrelationInput {
                file_id: FileId(1),
                line: 2,
                start_byte: 20,
                end_byte_exclusive: 26,
                column: 1,
                caller: NodeId(2),
                target: NodeId(0),
                raw_target: "target",
            },
            ExactSyntaxCallsiteCorrelationInput {
                file_id: FileId(1),
                line: 3,
                start_byte: 30,
                end_byte_exclusive: 36,
                column: 1,
                caller: NodeId(2),
                target: NodeId(0),
                raw_target: "target",
            },
        ];
        let targets = [NodeId(3), NodeId(4)];
        let raw_edges = [
            Edge {
                id: EdgeId(7),
                source: NodeId(2),
                target: NodeId(3),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(2),
                callsite_identity: Some(identities[0].clone()),
                ..Default::default()
            },
            Edge {
                id: EdgeId(8),
                source: NodeId(2),
                target: NodeId(9),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(3),
                callsite_identity: Some(identities[1].clone()),
                ..Default::default()
            },
        ];
        let edge_inputs = raw_edges
            .iter()
            .map(|edge| OrdinaryCallEdgeCorrelationInput {
                file_id: Some(FileId(1)),
                line: edge.line,
                caller: NodeId(2),
                target: NodeId(0),
                raw_edge_target: edge.target,
                raw_file_id: Some(FileId(1)),
                raw_line: edge.line,
                raw_target: "target",
                callsite_identity: edge.callsite_identity.as_deref(),
                semantic_exact: true,
            })
            .collect::<Vec<_>>();
        let raw_nodes = [
            Node {
                id: NodeId(3),
                kind: NodeKind::FUNCTION,
                serialized_name: "target".to_owned(),
                file_node_id: Some(NodeId(1)),
                start_line: Some(1),
                ..Default::default()
            },
            Node {
                id: NodeId(9),
                kind: NodeKind::UNKNOWN,
                serialized_name: "target".to_owned(),
                file_node_id: Some(NodeId(1)),
                start_line: Some(3),
                ..Default::default()
            },
        ];
        let projections = exact_call_edge_projection_updates(
            &syntax,
            &targets,
            &edge_inputs,
            &raw_edges.iter().collect::<Vec<_>>(),
            &raw_nodes.iter().collect::<Vec<_>>(),
        )
        .expect("direct and placeholder projections");
        assert_eq!(projections.len(), 2);
        assert_eq!(
            projections
                .iter()
                .map(|projection| (projection.edge_id, projection.caller, projection.target))
                .collect::<Vec<_>>(),
            [
                (EdgeId(7), NodeId(2), NodeId(3)),
                (EdgeId(8), NodeId(2), NodeId(4))
            ]
        );
    }
}

#[cfg(test)]
mod typescript_import_binding_tests {
    use super::*;
    use tree_sitter::Parser;

    fn bindings(source: &str) -> Option<Vec<TypescriptImportBinding>> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .expect("TypeScript grammar must load");
        let tree = parser.parse(source, None).expect("source must parse");
        let mut cursor = tree.root_node().walk();
        let statement = tree
            .root_node()
            .named_children(&mut cursor)
            .find(|node| node.kind() == "import_statement")?;
        typescript_import_bindings_for_statement(statement, source, true)
    }

    #[test]
    fn type_and_value_specifiers_share_one_local_binding_domain() {
        assert!(bindings("import { type target, target } from './target';").is_none());
        assert!(
            bindings(
                "import { target as local, /* duplicate */ type target as local, } from './target';"
            )
            .is_none()
        );
        assert!(bindings("import { target as local, other as local } from './target';").is_none());
        let parsed = bindings("import { type target as TargetType, target } from './target';")
            .expect("different locals sharing one imported spelling are supported");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].local_name, "target");
        assert_eq!(parsed[0].imported_name, "target");
        assert!(bindings("import { target } from '.';").is_some());
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .expect("TypeScript grammar must load");
        let source = "import { target } from '.';";
        let tree = parser.parse(source, None).expect("source must parse");
        let mut cursor = tree.root_node().walk();
        let statement = tree
            .root_node()
            .named_children(&mut cursor)
            .find(|node| node.kind() == "import_statement")
            .expect("import statement");
        assert!(typescript_import_bindings_for_statement(statement, source, false).is_none());
        assert!(typescript_directory_imports_enabled(
            "typescript",
            Path::new("source.tsx")
        ));
        assert!(!typescript_directory_imports_enabled(
            "javascript",
            Path::new("source.ts")
        ));
        assert!(!typescript_directory_imports_enabled(
            "javascript",
            Path::new("source.tsx")
        ));
        assert!(!typescript_directory_imports_enabled(
            "typescript",
            Path::new("source.mts")
        ));
    }
}

#[cfg(test)]
mod bash_complexity_tests {
    use super::*;
    use tree_sitter::Parser;

    fn source_work(functions: usize) -> usize {
        let mut source = String::new();
        let mut nodes = Vec::new();
        for index in 0..functions {
            let line = u32::try_from(index + 1).expect("Bash fixture line");
            let name = format!("target_{index}");
            source.push_str(&format!("{name}() {{ :; }}\n"));
            nodes.push(Node {
                id: NodeId(i64::try_from(index + 2).expect("Bash target id")),
                kind: NodeKind::FUNCTION,
                serialized_name: name,
                file_node_id: Some(NodeId(1)),
                start_line: Some(line),
                ..Default::default()
            });
        }
        let caller_line = u32::try_from(functions + 1).expect("Bash caller line");
        source.push_str("caller() {");
        for index in 0..functions {
            source.push_str(&format!(" target_{index};"));
        }
        source.push_str(" }\n");
        nodes.push(Node {
            id: NodeId(i64::try_from(functions + 2).expect("Bash caller id")),
            kind: NodeKind::FUNCTION,
            serialized_name: "caller".to_string(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(caller_line),
            ..Default::default()
        });
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .expect("Bash grammar");
        let tree = parser.parse(&source, None).expect("Bash source");
        reset_bash_resolution_work();
        reset_ruby_php_resolution_work();
        let index = BashResolutionIndex::build(&tree, &source, NodeId(1), &nodes);
        assert_eq!(
            index
                .calls
                .iter()
                .filter(|call| call.raw_target.starts_with("target_"))
                .count(),
            functions
        );
        assert_eq!(
            ruby_php_resolution_work(),
            0,
            "Bash work leaked into the Ruby/PHP counter"
        );
        bash_resolution_work().preparation
    }

    #[test]
    fn bash_parser_index_work_is_linear_for_doubled_declarations_and_calls() {
        let small = source_work(128);
        let large = source_work(256);
        assert!(small > 0, "Bash work was not counted");
        assert!(
            large <= small.saturating_mul(2).saturating_add(32),
            "Bash parser/index work grew superlinearly: {small} -> {large}"
        );
    }
}

#[cfg(test)]
mod ruby_php_complexity_tests {
    use super::*;
    use tree_sitter::Parser;

    fn graph_node(id: i64, kind: NodeKind, name: &str, line: u32) -> Node {
        Node {
            id: NodeId(id),
            kind,
            serialized_name: name.to_string(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(line),
            ..Default::default()
        }
    }

    fn ruby_receiver_work(calls: usize) -> usize {
        let mut source = String::from(
            "class Worker\n  def target\n  end\nend\ndef caller\n  worker = Worker.new\n",
        );
        for _ in 0..calls {
            source.push_str("  worker.target\n");
        }
        source.push_str("end\n");
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_ruby::LANGUAGE.into())
            .expect("Ruby grammar");
        let tree = parser.parse(&source, None).expect("Ruby source");
        let nodes = [
            graph_node(2, NodeKind::CLASS, "Worker", 1),
            graph_node(3, NodeKind::METHOD, "Worker.target", 2),
            graph_node(4, NodeKind::FUNCTION, "caller", 5),
        ];
        reset_ruby_php_resolution_work();
        let index = RubyResolutionIndex::build(&tree, &source, NodeId(1), &nodes);
        assert_eq!(index.calls.len(), calls);
        ruby_php_resolution_work()
    }

    fn php_receiver_work(calls: usize) -> usize {
        let mut source = String::from(
            "<?php\nclass Worker {\n  public function target() {}\n}\nfunction caller(Worker $worker) {\n",
        );
        for _ in 0..calls {
            source.push_str("  $worker->target();\n");
        }
        source.push_str("}\n");
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_php::LANGUAGE_PHP.into())
            .expect("PHP grammar");
        let tree = parser.parse(&source, None).expect("PHP source");
        let nodes = [
            graph_node(2, NodeKind::CLASS, "Worker", 2),
            graph_node(3, NodeKind::METHOD, "Worker.target", 3),
            graph_node(4, NodeKind::FUNCTION, "caller", 5),
        ];
        reset_ruby_php_resolution_work();
        let index = PhpResolutionIndex::build(&tree, &source, NodeId(1), &nodes);
        assert_eq!(index.calls.len(), calls);
        ruby_php_resolution_work()
    }

    fn ruby_declaration_work(declarations: usize, duplicate: bool) -> usize {
        let mut source = String::new();
        let mut nodes = Vec::new();
        for index in 0..declarations {
            let name = if duplicate {
                "target".to_owned()
            } else {
                format!("target_{index}")
            };
            source.push_str(&format!("def {name}\nend\n"));
            nodes.push(graph_node(
                i64::try_from(index + 2).expect("node id"),
                NodeKind::FUNCTION,
                &name,
                u32::try_from(index * 2 + 1).expect("line"),
            ));
        }
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_ruby::LANGUAGE.into())
            .expect("Ruby grammar");
        let tree = parser.parse(&source, None).expect("Ruby declarations");
        reset_ruby_php_resolution_work();
        let _ = RubyResolutionIndex::build(&tree, &source, NodeId(1), &nodes);
        ruby_php_resolution_work()
    }

    fn projection_record(
        language: &str,
        file_id: i64,
        declarations: usize,
        duplicate: bool,
    ) -> ResolutionCacheRecord {
        let declarations = (0..declarations)
            .map(|index| CachedTopLevelDeclaration {
                name: if duplicate {
                    "target".to_owned()
                } else {
                    format!("target_{file_id}_{index}")
                },
                declaration: NodeId(file_id * 10_000 + index as i64 + 1),
                module_path: Vec::new(),
                cross_module_visible: true,
            })
            .collect();
        ResolutionCacheRecord {
            path: PathBuf::from(format!(
                "file_{file_id}.{}",
                if language == "ruby" { "rb" } else { "php" }
            )),
            file: CachedResolutionFile {
                file_id: NodeId(file_id),
                source_sha256: "0".repeat(64),
                language: language.to_owned(),
                adapter_version: adapter_version(language).to_owned(),
                parser_fingerprint: "0".repeat(64),
                complete: true,
                lookup_input_complete: true,
                typescript_module: false,
                top_level_declarations: declarations,
                inherent_methods: Vec::new(),
                classes: Vec::new(),
                direct_exports: Vec::new(),
                export_poison_all: false,
                poisoned_export_names: Vec::new(),
                rust_modules: Vec::new(),
                rust_types: Vec::new(),
                rust_uses: Vec::new(),
                go_package: None,
                java_kotlin_package: None,
                php_namespace: if language == "php" {
                    CachedPhpNamespace::Named("App".to_owned())
                } else {
                    CachedPhpNamespace::Invalid
                },
                c_cpp_file: None,
            },
            calls: Vec::new(),
        }
    }

    fn projection_domain_work(files: usize, declarations: usize, duplicate: bool) -> usize {
        let records = (0..files)
            .flat_map(|index| {
                [
                    projection_record("ruby", index as i64 + 1, declarations, duplicate),
                    projection_record(
                        "php",
                        index as i64 + files as i64 + 1,
                        declarations,
                        duplicate,
                    ),
                ]
            })
            .collect::<Vec<_>>();
        reset_ruby_php_resolution_work();
        let index = JavaKotlinProjectionIndex::prepare(&records);
        for record in &records {
            for declaration in &record.file.top_level_declarations {
                if record.file.language == "ruby" {
                    let _ = index.ruby_function(&declaration.name, declaration.declaration);
                } else {
                    let _ = index.resolve_php(&record.file.php_namespace, None, &declaration.name);
                }
            }
        }
        ruby_php_resolution_work()
    }

    #[test]
    fn receiver_heavy_ruby_and_php_resolution_work_is_linear() {
        for (language, small, large) in [
            ("ruby", ruby_receiver_work(128), ruby_receiver_work(256)),
            ("php", php_receiver_work(128), php_receiver_work(256)),
        ] {
            assert!(
                large <= small.saturating_mul(2).saturating_add(32),
                "{language} work grew superlinearly: 1x={small}, 2x={large}"
            );
        }
    }

    #[test]
    fn ruby_php_preparation_domain_and_projection_work_is_independently_linear() {
        let calls_1x = ruby_receiver_work(64) + php_receiver_work(64);
        let calls_2x = ruby_receiver_work(128) + php_receiver_work(128);
        let declarations_1x = ruby_declaration_work(64, false);
        let declarations_2x = ruby_declaration_work(128, false);
        let hostile_1x = ruby_declaration_work(64, true);
        let hostile_2x = ruby_declaration_work(128, true);
        let files_1x = projection_domain_work(32, 1, false);
        let files_2x = projection_domain_work(64, 1, false);
        let domains_1x = projection_domain_work(8, 8, true);
        let domains_2x = projection_domain_work(16, 8, true);
        let combined_1x = projection_domain_work(16, 4, false);
        let combined_2x = projection_domain_work(32, 4, false);
        for (class, small, large, allowance) in [
            ("calls", calls_1x, calls_2x, 64),
            ("declarations", declarations_1x, declarations_2x, 64),
            ("duplicate hostile declarations", hostile_1x, hostile_2x, 64),
            ("files", files_1x, files_2x, 64),
            ("hostile domains", domains_1x, domains_2x, 64),
            ("combined", combined_1x, combined_2x, 128),
        ] {
            assert!(small > 0, "{class} work was not counted");
            assert!(
                large <= small.saturating_mul(2).saturating_add(allowance),
                "{class} work grew superlinearly: 1x={small}, 2x={large}"
            );
        }
    }
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

    fn glob_local_source(count: usize) -> String {
        let mut source = String::from("use crate::*;\nuse std::prelude::rust_2024::*;\n");
        for index in 0..count {
            source.push_str(&format!("fn target_{index}() {{}}\n"));
        }
        for index in 0..count {
            source.push_str(&format!("fn caller_{index}() {{ target_{index}(); }}\n"));
        }
        source
    }

    fn bounded_attribute_source(count: usize) -> String {
        let mut source =
            String::from("fn target() {}\n#[cfg(any())]\n#[allow(dead_code)]\nfn caller() {\n");
        for _ in 0..count {
            source.push_str(
                "#[cfg(any())]\n#[allow(dead_code)]\n#[doc = \"bounded\"]\n{ target(); }\n",
            );
        }
        source.push_str("}\n");
        source
    }

    fn nested_attributed_module_source(depth: usize) -> String {
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!(
                "fn target_{index}() {{}}\n#[allow(dead_code)]\nfn caller_{index}() {{ target_{index}(); }}\nmod nested_{index} {{\n"
            ));
        }
        for _ in 0..depth {
            source.push_str("}\n");
        }
        source
    }

    fn documented_declaration_source(count: usize) -> String {
        let mut source = String::new();
        for index in 0..count {
            source.push_str(&format!(
                "/// documented target {index}\n/** bounded documentation {index} */\nfn target_{index}() {{}}\nfn caller_{index}() {{ target_{index}(); }}\n"
            ));
        }
        source
    }

    fn interleaved_documented_group_source(count: usize) -> String {
        let mut source = String::new();
        for index in 0..count {
            source.push_str(&format!(
                "/// documented target {index}\n// ordinary {index}\n/** bounded documentation {index} */\n/* ordinary block {index} */\nfn target_{index}() {{}}\n#[cfg(any())]\n//// ordinary {index}\n/// documented but attributed {index}\nfn attributed_{index}() {{}}\nfn caller_{index}() {{ target_{index}(); attributed_{index}(); }}\n"
            ));
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
                adapter_version: RUST_ADAPTER_VERSION.to_string(),
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
                java_kotlin_package: None,
                php_namespace: CachedPhpNamespace::Invalid,
                c_cpp_file: None,
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

        let small_glob = measured_source_work(&glob_local_source(64));
        let large_glob = measured_source_work(&glob_local_source(128));
        assert!(
            small_glob >= 64 * 8,
            "glob-local declaration and call work was not fully counted: {small_glob}"
        );
        assert!(
            large_glob <= small_glob * 2 + 128,
            "glob-local declaration and call work grew superlinearly: {small_glob} -> {large_glob}"
        );

        let small_attributes = measured_source_work(&bounded_attribute_source(64));
        let large_attributes = measured_source_work(&bounded_attribute_source(128));
        assert!(
            small_attributes >= 64 * 8,
            "bounded attribute preparation and lookup work was not fully counted: {small_attributes}"
        );
        assert!(
            large_attributes <= small_attributes * 2 + 128,
            "bounded attribute preparation and lookup work grew superlinearly: {small_attributes} -> {large_attributes}"
        );

        let small_modules = measured_source_work(&nested_attributed_module_source(64));
        let large_modules = measured_source_work(&nested_attributed_module_source(128));
        assert!(
            small_modules >= 64 * 8,
            "nested-module attribute preparation was not fully counted: {small_modules}"
        );
        assert!(
            large_modules <= small_modules * 2 + 128,
            "nested-module attribute preparation grew superlinearly: {small_modules} -> {large_modules}"
        );

        let small_documented = measured_source_work(&documented_declaration_source(64));
        let large_documented = measured_source_work(&documented_declaration_source(128));
        assert!(
            small_documented >= 64 * 8,
            "documented-declaration preparation and lookup work was not fully counted: {small_documented}"
        );
        assert!(
            large_documented <= small_documented * 2 + 128,
            "documented-declaration preparation and lookup work grew superlinearly: {small_documented} -> {large_documented}"
        );

        let small_interleaved = measured_source_work(&interleaved_documented_group_source(64));
        let large_interleaved = measured_source_work(&interleaved_documented_group_source(128));
        assert!(
            small_interleaved >= 64 * 12,
            "interleaved documented-group work was not fully counted: {small_interleaved}"
        );
        assert!(
            large_interleaved <= small_interleaved * 2 + 128,
            "interleaved documented-group work grew superlinearly: {small_interleaved} -> {large_interleaved}"
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
mod c_cpp_complexity_tests {
    use super::*;
    use tree_sitter::Parser;

    fn measured_work(source: &str) -> usize {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .expect("C++ grammar must load");
        let tree = parser.parse(source, None).expect("C++ source must parse");
        let file_id = NodeId(1);
        let mut nodes = Vec::new();
        let mut next_id = 2_i64;
        walk_nodes(tree.root_node(), &mut |node| {
            let (kind, name) = match node.kind() {
                "class_specifier" | "struct_specifier" => (
                    NodeKind::CLASS,
                    node.child_by_field_name("name")
                        .and_then(|name| node_text(name, source)),
                ),
                "function_definition" => (NodeKind::FUNCTION, c_cpp_function_name(node, source)),
                _ => return,
            };
            let Some(name) = name else {
                return;
            };
            nodes.push(Node {
                id: NodeId(next_id),
                kind,
                serialized_name: name.to_string(),
                file_node_id: Some(file_id),
                start_line: Some(node.start_position().row as u32 + 1),
                ..Node::default()
            });
            next_id += 1;
        });
        reset_c_cpp_resolution_work();
        let index = CCppResolutionIndex::build(
            &tree,
            source,
            Path::new("fixture.cpp"),
            "cpp",
            file_id,
            &nodes,
        );
        for call in &index.calls {
            let _ = index.resolve_syntax_claim(call.callee, call.form, &call.raw_target);
        }
        c_cpp_resolution_work()
    }

    fn source(count: usize) -> String {
        let mut source = String::from("class Worker { public: void run() {} };\n");
        for index in 0..count {
            source.push_str(&format!("class Owner{index} {{ public:\n"));
        }
        source.push_str("void caller() {\n");
        for index in 0..count {
            source.push_str(&format!("Worker receiver{index}; receiver{index}.run();\n"));
        }
        source.push_str("}\n");
        for _ in 0..count {
            source.push_str("};\n");
        }
        source
    }

    #[test]
    fn c_cpp_parser_index_work_is_linear_for_doubled_receivers_and_nested_owners() {
        let small = measured_work(&source(64));
        let large = measured_work(&source(128));
        assert!(
            small >= 64 * 8,
            "C++ parser work was not fully counted: {small}"
        );
        assert!(
            large <= small * 2 + 256,
            "C++ parser/index work grew superlinearly: {small} -> {large}"
        );
    }
}

#[cfg(test)]
mod java_kotlin_complexity_tests {
    use super::*;
    use codestory_contracts::events::EventBus;
    use codestory_store::{IndexPublicationMode, IndexPublicationRecord, Store};
    use codestory_workspace::{BuildMode, RefreshInfo};
    use std::fs;
    use tree_sitter::Parser;

    fn measured_work(language: &str, source: &str) -> usize {
        let mut parser = Parser::new();
        let (grammar, path) = match language {
            "java" => (tree_sitter_java::LANGUAGE.into(), Path::new("Exact.java")),
            "kotlin" => (
                tree_sitter_kotlin_ng::LANGUAGE.into(),
                Path::new("Exact.kt"),
            ),
            "csharp" => (tree_sitter_c_sharp::LANGUAGE.into(), Path::new("Exact.cs")),
            "swift" => (
                tree_sitter_swift::LANGUAGE.into(),
                Path::new("Sources/App/Exact.swift"),
            ),
            "dart" => (
                tree_sitter_dart_orchard::LANGUAGE.into(),
                Path::new("lib/exact.dart"),
            ),
            _ => panic!("unsupported nominal test language {language}"),
        };
        parser.set_language(&grammar).expect("grammar must load");
        let tree = parser.parse(source, None).expect("source must parse");
        let file_id = NodeId(1);
        let mut nodes = Vec::new();
        let mut next_id = 2_i64;
        walk_nodes(tree.root_node(), &mut |node| {
            let kind = if java_kotlin_callable_kind(language, node.kind()) {
                NodeKind::METHOD
            } else if java_kotlin_class_kind(language, node.kind()) {
                NodeKind::CLASS
            } else {
                return;
            };
            let Some(name) = declaration_name(node, source) else {
                return;
            };
            nodes.push(Node {
                id: NodeId(next_id),
                kind,
                serialized_name: name.to_string(),
                file_node_id: Some(file_id),
                start_line: Some(node.start_position().row as u32 + 1),
                ..Node::default()
            });
            next_id += 1;
        });
        reset_java_kotlin_resolution_work();
        let index =
            JavaKotlinResolutionIndex::build(&tree, source, path, language, file_id, &nodes);
        for call in &index.calls {
            let _ = index.resolve_syntax_claim(source, call.callee, call.form, &call.raw_target);
        }
        java_kotlin_resolution_work()
    }

    fn csd_source(language: &str, count: usize) -> String {
        let mut source = match language {
            "csharp" => String::from(
                "public sealed class Worker { public void Run() {} }\npublic static class Calls {\n",
            ),
            "swift" => String::from("struct Worker { func run() {} }\n"),
            "dart" => String::from("final class Worker { void run() {} }\n"),
            _ => panic!("unsupported nominal test language {language}"),
        };
        for index in 0..count {
            match language {
                "csharp" => source.push_str(&format!(
                    "public static void Caller{index}() {{ Worker receiver{index} = new Worker(); receiver{index}.Run(); }}\n"
                )),
                "swift" => source.push_str(&format!(
                    "func caller{index}() {{ let receiver{index}: Worker = Worker(); receiver{index}.run() }}\n"
                )),
                "dart" => source.push_str(&format!(
                    "void caller{index}() {{ Worker receiver{index} = Worker(); receiver{index}.run(); }}\n"
                )),
                _ => unreachable!(),
            }
        }
        if language == "csharp" {
            source.push_str("}\n");
        }
        source
    }

    fn csd_pipeline_sources(language: &str, axis: &str, count: usize) -> Vec<(PathBuf, String)> {
        let extension = match language {
            "csharp" => "cs",
            "swift" => "swift",
            "dart" => "dart",
            _ => unreachable!(),
        };
        if matches!(axis, "files" | "domains") {
            return (0..count)
                .map(|index| {
                    let path = match (language, axis) {
                        ("csharp", "domains") => {
                            PathBuf::from(format!("src/Domain{index}/Exact.{extension}"))
                        }
                        ("swift", "domains") => {
                            PathBuf::from(format!("Sources/Module{index}/Exact.{extension}"))
                        }
                        ("dart", "domains") => {
                            PathBuf::from(format!("packages/package{index}/lib/exact.{extension}"))
                        }
                        ("csharp", _) => PathBuf::from(format!("src/App/Exact{index}.{extension}")),
                        ("swift", _) => {
                            PathBuf::from(format!("Sources/App/Exact{index}.{extension}"))
                        }
                        ("dart", _) => PathBuf::from(format!("lib/exact{index}.{extension}")),
                        _ => unreachable!(),
                    };
                    let source = match language {
                        "csharp" => format!(
                            "namespace Domain{index}; public static class Calls{index} {{ public static void Target{index}() {{}} public static void Caller{index}() {{ Target{index}(); }} }}\n"
                        ),
                        "swift" => format!(
                            "public func target{index}() {{}}\npublic func caller{index}() {{ target{index}() }}\n"
                        ),
                        "dart" => format!(
                            "void target{index}() {{}}\nvoid caller{index}() {{ target{index}(); }}\n"
                        ),
                        _ => unreachable!(),
                    };
                    (path, source)
                })
                .collect();
        }
        let source = match axis {
            "calls" | "repeated" => csd_source(language, count),
            "declarations" => {
                let mut source = csd_source(language, 1);
                for index in 0..count {
                    let declaration = match language {
                        "csharp" => format!(
                            "public sealed class Extra{index} {{ public void Run{index}() {{}} }}\n"
                        ),
                        "swift" => format!("struct Extra{index} {{ func run{index}() {{}} }}\n"),
                        "dart" => {
                            format!("final class Extra{index} {{ void run{index}() {{}} }}\n")
                        }
                        _ => unreachable!(),
                    };
                    source.push_str(&declaration);
                }
                source
            }
            "nested" => {
                let mut source = csd_source(language, 1);
                for index in 0..count {
                    let owner = match language {
                        "csharp" => format!(
                            "public sealed class Owner{index} {{ public void Caller() {{ {{ Worker value = new Worker(); value.Run(); }} }} }}\n"
                        ),
                        "swift" => format!(
                            "func owner{index}() {{ do {{ let value: Worker = Worker(); value.run() }} }}\n"
                        ),
                        "dart" => format!(
                            "void owner{index}() {{ {{ final value = Worker(); value.run(); }} }}\n"
                        ),
                        _ => unreachable!(),
                    };
                    source.push_str(&owner);
                }
                source
            }
            "hostile" => {
                let mut source = csd_source(language, 1);
                for index in 0..count {
                    let hostile = match language {
                        "csharp" => format!("interface Hostile{index} {{ void Run(); }}\n"),
                        "swift" => format!("protocol Hostile{index} {{ func run() }}\n"),
                        "dart" => {
                            format!("abstract interface class Hostile{index} {{ void run(); }}\n")
                        }
                        _ => unreachable!(),
                    };
                    source.push_str(&hostile);
                }
                source
            }
            _ => unreachable!(),
        };
        let path = match language {
            "csharp" => PathBuf::from("src/App/Exact.cs"),
            "swift" => PathBuf::from("Sources/App/Exact.swift"),
            "dart" => PathBuf::from("lib/exact.dart"),
            _ => unreachable!(),
        };
        vec![(path, source)]
    }

    fn measured_csd_pipeline_work(language: &str, axis: &str, count: usize) -> (usize, usize) {
        let project = tempfile::tempdir().expect("temp project");
        let sources = csd_pipeline_sources(language, axis, count);
        let mut paths = Vec::new();
        for (relative, source) in sources {
            let path = project.path().join(relative);
            fs::create_dir_all(path.parent().expect("source parent")).expect("create source dir");
            fs::write(&path, source).expect("write source");
            paths.push(path);
        }
        reset_java_kotlin_resolution_work();
        codestory_store::reset_store_replay_work();
        let mut store = Store::new_in_memory().expect("store");
        crate::WorkspaceIndexer::new(project.path().to_path_buf())
            .run_incremental(
                &mut store,
                &RefreshInfo {
                    mode: BuildMode::Incremental,
                    files_to_index: paths,
                    files_to_remove: Vec::new(),
                    existing_file_ids: HashMap::new(),
                },
                &EventBus::new(),
                None,
            )
            .expect("index sources");
        let publication = IndexPublicationRecord {
            generation: 1,
            generation_id: "linear-generation".to_string(),
            run_id: "linear-run".to_string(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        rematerialize_proof_resolution_projection(&mut store, &publication)
            .expect("rematerialize proof resolution");
        store
            .validate_proof_resolution_publication(&publication)
            .expect("replay proof resolution");
        (
            java_kotlin_resolution_work(),
            codestory_store::store_replay_work(),
        )
    }

    fn source(language: &str, count: usize) -> String {
        let mut source = if language == "java" {
            String::from("class Worker { void run() {} }\n")
        } else {
            String::from("class Worker { fun run() {} }\n")
        };
        for index in 0..count {
            source.push_str(&format!("class Owner{index} {{\n"));
        }
        source.push_str(if language == "java" {
            "void caller() {\n"
        } else {
            "fun caller() {\n"
        });
        for index in 0..count {
            if language == "java" {
                source.push_str(&format!(
                    "Worker receiver{index} = new Worker(); receiver{index}.run();\n"
                ));
            } else {
                source.push_str(&format!(
                    "val receiver{index}: Worker = Worker(); receiver{index}.run()\n"
                ));
            }
        }
        source.push_str("}\n");
        for _ in 0..count {
            source.push_str("}\n");
        }
        source
    }

    #[test]
    fn java_kotlin_parser_index_work_is_linear_for_doubled_calls_and_nested_owners() {
        for language in ["java", "kotlin"] {
            let small = measured_work(language, &source(language, 64));
            let large = measured_work(language, &source(language, 128));
            assert!(
                small >= 64 * 8,
                "{language} parser work was not fully counted: {small}"
            );
            assert!(
                large <= small * 2 + 256,
                "{language} parser/index work grew superlinearly: {small} -> {large}"
            );
        }
    }

    #[test]
    fn csharp_swift_dart_parser_index_work_is_linear_for_doubled_receivers_and_callers() {
        for language in ["csharp", "swift", "dart"] {
            let small = measured_work(language, &csd_source(language, 64));
            let large = measured_work(language, &csd_source(language, 128));
            assert!(
                small >= 64 * 8,
                "{language} parser work was not fully counted: {small}"
            );
            assert!(
                large <= small * 2 + 256,
                "{language} parser/index work grew superlinearly: {small} -> {large}"
            );
        }
    }

    #[test]
    fn csharp_swift_dart_complete_pipeline_work_is_linear_for_each_growth_axis() {
        for language in ["csharp", "swift", "dart"] {
            for axis in [
                "calls",
                "files",
                "declarations",
                "domains",
                "nested",
                "repeated",
                "hostile",
            ] {
                let small = measured_csd_pipeline_work(language, axis, 4);
                let large = measured_csd_pipeline_work(language, axis, 8);
                assert!(
                    small.0 > 0 && small.1 > 0,
                    "{language}/{axis} did not count the complete pipeline: {small:?}"
                );
                assert!(
                    large.0 <= small.0 * 2 + 512,
                    "{language}/{axis} index/projection work grew superlinearly: {small:?} -> {large:?}"
                );
                assert!(
                    large.1 <= small.1 * 2 + 512,
                    "{language}/{axis} store/replay work grew superlinearly: {small:?} -> {large:?}"
                );
                assert!(
                    large.0 + large.1 <= (small.0 + small.1) * 2 + 1024,
                    "{language}/{axis} total work grew superlinearly: {small:?} -> {large:?}"
                );
            }
        }
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
        measured_source(&source)
    }

    fn returned_closure_source(count: usize) -> String {
        let mut source = String::from(
            "package proof\ntype Handler func()\nfunc target() {}\nfunc caller() Handler { return func() {\n",
        );
        for _ in 0..count {
            source.push_str("  target()\n");
        }
        source.push_str("} }\n");
        source
    }

    fn measured_returned_closure_work(count: usize) -> usize {
        measured_source(&returned_closure_source(count))
    }

    fn nested_returned_closure_source(depth: usize) -> String {
        let mut source = String::from(
            "package proof\ntype Handler func()\nfunc target() {}\nfunc caller() Handler { return func() {\n",
        );
        for _ in 0..depth {
            source.push_str("{\n");
        }
        source.push_str("target()\n");
        for _ in 0..depth {
            source.push_str("}\n");
        }
        source.push_str("} }\n");
        source
    }

    fn measured_nested_returned_closure_work(depth: usize) -> usize {
        measured_source(&nested_returned_closure_source(depth))
    }

    fn many_deferred_children_source(count: usize) -> String {
        let mut source = String::from(
            "package proof\ntype Handler func()\nfunc target() {}\nfunc caller() Handler { return func() {\n",
        );
        for _ in 0..count {
            source.push_str("defer func() { target() }()\n");
        }
        source.push_str("} }\n");
        source
    }

    fn measured_many_deferred_children_work(count: usize) -> usize {
        measured_source(&many_deferred_children_source(count))
    }

    fn measured_source(source: &str) -> usize {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .expect("Go grammar must load");
        let tree = parser.parse(source, None).expect("source must parse");
        reset_go_resolution_work();
        let _ = GoResolutionIndex::build(&tree, source, NodeId(1), &[]);
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
                    adapter_version: GO_ADAPTER_VERSION.to_string(),
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
                    java_kotlin_package: None,
                    php_namespace: CachedPhpNamespace::Invalid,
                    c_cpp_file: None,
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

        let small_closure = measured_returned_closure_work(64);
        let large_closure = measured_returned_closure_work(128);
        assert!(
            small_closure > 0,
            "Go returned-closure work was not instrumented"
        );
        assert!(
            large_closure <= small_closure * 2 + 128,
            "Go returned-closure work grew superlinearly: {small_closure} -> {large_closure}"
        );

        let shallow_nested = measured_nested_returned_closure_work(32);
        let deep_nested = measured_nested_returned_closure_work(64);
        assert!(
            shallow_nested > 32,
            "Go returned-closure membership work was not counted: {shallow_nested}"
        );
        assert!(
            deep_nested <= shallow_nested * 2 + 128,
            "Go nested returned-closure membership work grew superlinearly: {shallow_nested} -> {deep_nested}"
        );

        let small_deferred = measured_many_deferred_children_work(64);
        let large_deferred = measured_many_deferred_children_work(128);
        assert!(
            small_deferred >= 64 * 8,
            "Go deferred-child membership work was not fully counted: {small_deferred}"
        );
        assert!(
            large_deferred <= small_deferred * 2 + 128,
            "Go deferred-child membership work grew superlinearly: {small_deferred} -> {large_deferred}"
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
                        cross_module_visible: false,
                    }],
                    cross_module_visible: false,
                    runtime_closed: false,
                    super_name: None,
                }],
                direct_exports: Vec::new(),
                export_poison_all: false,
                poisoned_export_names: Vec::new(),
                rust_modules: Vec::new(),
                rust_types: Vec::new(),
                rust_uses: Vec::new(),
                go_package: None,
                java_kotlin_package: None,
                php_namespace: CachedPhpNamespace::Invalid,
                c_cpp_file: None,
            },
            calls: Vec::new(),
        }
    }

    fn measured_source_work(call_count: usize) -> usize {
        let mut source = String::from(
            "class Worker:\n    def run(self):\n        pass\n\ndef target():\n    pass\n\ndef caller(obj):\n",
        );
        for index in 0..call_count {
            source.push_str(&format!(
                "    getattr(obj, 'marker')\n    worker_{index} = Worker()\n    worker_{index}.run()\n    target()\n"
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

    fn measured_nested_match_work(depth: usize) -> usize {
        let mut pattern = "capture".to_owned();
        for level in 0..depth {
            pattern = match level % 5 {
                0 => format!("[{pattern}]"),
                1 => format!("({pattern},)"),
                2 => format!("Box({pattern})"),
                3 => format!("Box(value={pattern})"),
                _ => format!("{{'key': {pattern}}}"),
            };
        }
        let source = format!(
            "class Box:\n    pass\ndef target():\n    pass\ndef caller(value):\n    match value:\n        case {pattern}:\n            target()\n"
        );
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("Python grammar must load");
        let tree = parser.parse(&source, None).expect("source must parse");
        assert!(!tree.root_node().has_error(), "nested pattern must parse");
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
    fn python_nested_match_binding_work_is_linear() {
        let small = measured_nested_match_work(64);
        let large = measured_nested_match_work(128);
        assert!(small > 0, "nested match work was not counted");
        assert!(
            large <= small * 2 + 512,
            "nested match work grew superlinearly: {small} -> {large}"
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
