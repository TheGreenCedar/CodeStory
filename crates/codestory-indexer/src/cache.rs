use crate::compilation_database::{CompilationInfo, CxxStandard};
use crate::{IndexResult, LanguageConfig, intermediate_storage::IntermediateStorage};
use codestory_contracts::graph::{
    AccessKind, CallableProjectionState, Edge, Node, NodeId, Occurrence,
};
use codestory_contracts::proof_resolution::ExactCallsite;
use codestory_store::FileInfo;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

// Versioned with proof-input semantics so older parser artifacts fail closed.
const INDEX_ARTIFACT_CACHE_VERSION: u32 = 29;
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x00000100000001B3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedIndexArtifact {
    #[serde(default)]
    pub resolution_input_schema_version: u32,
    pub files: Vec<FileInfo>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub occurrences: Vec<Occurrence>,
    pub component_access: Vec<(NodeId, AccessKind)>,
    pub callable_projection_states: Vec<CallableProjectionState>,
    pub impl_anchor_node_ids: Vec<NodeId>,
    #[serde(default)]
    pub call_resolution_inputs: Vec<CachedCallResolutionInput>,
    #[serde(default)]
    pub resolution_file: Option<CachedResolutionFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedCallResolutionInput {
    pub callsite: ExactCallsite,
    pub caller: Option<NodeId>,
    pub binding: CachedResolutionBinding,
    pub language: String,
    pub adapter_version: String,
    pub parser_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CachedResolutionBinding {
    SameFile {
        declaration: NodeId,
        #[serde(default)]
        rust_glob_local_module: Option<Vec<String>>,
    },
    StaticImport {
        import: NodeId,
        module_specifier: String,
        imported_name: String,
        is_default: bool,
    },
    ImplicitReceiver {
        owner: NodeId,
        declaration: NodeId,
        owner_name: String,
    },
    ConstructorBinding {
        class_binding: CachedClassBinding,
        method_name: String,
    },
    ExplicitReceiverType {
        class_binding: CachedClassBinding,
        method_name: String,
    },
    RustPath {
        module_path: Vec<String>,
        components: Vec<String>,
        import: Option<CachedRustUseBinding>,
        associated_owner: Option<NodeId>,
    },
    RustImplicitReceiver {
        module_path: Vec<String>,
        owner_name: String,
        import: CachedRustUseBinding,
        declaration: NodeId,
    },
    RustExplicitReceiver {
        module_path: Vec<String>,
        owner_name: String,
        import: Option<CachedRustUseBinding>,
        constructor: bool,
        constructor_record: bool,
        constructor_method: Option<String>,
    },
    GoPackageFunction {
        package_name: String,
        name: String,
    },
    GoImplicitReceiver {
        package_name: String,
        owner_name: String,
        receiver_is_pointer: bool,
    },
    GoExplicitReceiver {
        package_name: String,
        owner_name: String,
        receiver_is_pointer: bool,
        constructor: bool,
        constructor_uses_builtin_new: bool,
    },
    JavaKotlinImportedReceiver {
        package_name: String,
        owner_name: String,
        method_name: String,
        import: NodeId,
        constructor: bool,
    },
    JavaKotlinPackageFunction {
        package_name: String,
        name: String,
    },
    JavaKotlinImportedFunction {
        package_name: String,
        owner_name: Option<String>,
        name: String,
        import: NodeId,
    },
    JavaKotlinPackageReceiver {
        package_name: String,
        owner_name: String,
        method_name: String,
        constructor: bool,
    },
    CCppQualified {
        components: Vec<NodeId>,
    },
    Ambiguous,
    MissingBinding,
    Unsupported,
    IncompleteDomain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CachedClassBinding {
    SameFile {
        owner: NodeId,
        owner_name: String,
    },
    StaticImport {
        import: NodeId,
        module_specifier: String,
        imported_name: String,
        is_default: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedResolutionFile {
    pub file_id: NodeId,
    pub source_sha256: String,
    pub language: String,
    pub adapter_version: String,
    pub parser_fingerprint: String,
    pub complete: bool,
    pub lookup_input_complete: bool,
    pub typescript_module: bool,
    pub top_level_declarations: Vec<CachedTopLevelDeclaration>,
    pub inherent_methods: Vec<CachedInherentMethod>,
    #[serde(default)]
    pub classes: Vec<CachedClassDeclaration>,
    pub direct_exports: Vec<CachedDirectExport>,
    #[serde(default)]
    pub export_poison_all: bool,
    #[serde(default)]
    pub poisoned_export_names: Vec<String>,
    #[serde(default)]
    pub rust_modules: Vec<CachedRustModule>,
    #[serde(default)]
    pub rust_types: Vec<CachedRustType>,
    #[serde(default)]
    pub rust_uses: Vec<CachedRustUseBinding>,
    #[serde(default)]
    pub go_package: Option<CachedGoPackage>,
    #[serde(default)]
    pub java_kotlin_package: Option<String>,
    #[serde(default)]
    pub php_namespace: CachedPhpNamespace,
    #[serde(default)]
    pub c_cpp_file: Option<CachedCCppFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedCCppFile {
    pub source_path: PathBuf,
    pub source_role: CachedCCppSourceRole,
    pub namespaces: Vec<CachedCCppNamespace>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub(crate) enum CachedPhpNamespace {
    Global,
    Named(String),
    #[default]
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CachedCCppSourceRole {
    Source,
    Header,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedCCppNamespace {
    pub path: Vec<String>,
    pub declaration: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedGoPackage {
    pub name: String,
    pub build_constrained: bool,
    pub generated: bool,
    pub package_blockers: Vec<String>,
    pub types: Vec<CachedGoType>,
    pub methods: Vec<CachedGoMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedGoType {
    pub name: String,
    pub declaration: NodeId,
    pub interface: bool,
    pub generic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedGoMethod {
    pub owner_name: String,
    pub method_name: String,
    pub declaration: NodeId,
    pub pointer_receiver: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedTopLevelDeclaration {
    pub name: String,
    pub declaration: NodeId,
    #[serde(default)]
    pub module_path: Vec<String>,
    #[serde(default)]
    pub cross_module_visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedInherentMethod {
    pub owner_name: String,
    pub method_name: String,
    pub declaration: NodeId,
    #[serde(default)]
    pub module_path: Vec<String>,
    #[serde(default)]
    pub owner: Option<NodeId>,
    #[serde(default)]
    pub has_self: bool,
    #[serde(default)]
    pub return_owner: Option<String>,
    #[serde(default)]
    pub domain_complete: bool,
    #[serde(default)]
    pub cross_module_visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedRustModule {
    pub module_path: Vec<String>,
    pub declaration: Option<NodeId>,
    pub domain_complete: bool,
    #[serde(default)]
    pub value_blockers: Vec<String>,
    #[serde(default)]
    pub incomplete_value_names: Vec<String>,
    #[serde(default)]
    pub file_children: Vec<CachedRustFileModule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedRustFileModule {
    pub name: String,
    pub declaration: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedRustType {
    pub module_path: Vec<String>,
    pub name: String,
    pub declaration: NodeId,
    pub generic: bool,
    pub cross_module_visible: bool,
    pub unit_constructor: bool,
    pub record_constructor: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedRustUseBinding {
    pub module_path: Vec<String>,
    pub local_name: String,
    pub components: Vec<String>,
    pub import: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedClassDeclaration {
    pub name: String,
    pub declaration: NodeId,
    pub methods: Vec<CachedClassMethod>,
    #[serde(default)]
    pub cross_module_visible: bool,
    #[serde(default)]
    pub runtime_closed: bool,
    #[serde(default)]
    pub super_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedClassMethod {
    pub name: String,
    pub declaration: NodeId,
    #[serde(default)]
    pub cross_module_visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedDirectExport {
    pub exported_name: String,
    pub declaration: NodeId,
    pub is_default: bool,
    #[serde(default)]
    pub declaration_kind: CachedDeclarationKind,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CachedDeclarationKind {
    #[default]
    Callable,
    Class,
}

impl CachedIndexArtifact {
    #[cfg(test)]
    pub(crate) fn from_index_result(index_result: IndexResult) -> Self {
        Self::from_index_result_with_resolution_inputs(index_result, Vec::new(), None)
    }

    pub(crate) fn from_index_result_with_resolution_inputs(
        index_result: IndexResult,
        call_resolution_inputs: Vec<CachedCallResolutionInput>,
        resolution_file: Option<CachedResolutionFile>,
    ) -> Self {
        Self {
            resolution_input_schema_version: 28,
            files: index_result.files,
            nodes: index_result.nodes,
            edges: index_result.edges,
            occurrences: index_result.occurrences,
            component_access: index_result.component_access,
            callable_projection_states: index_result.callable_projection_states,
            impl_anchor_node_ids: index_result.impl_anchor_node_ids,
            call_resolution_inputs,
            resolution_file,
        }
    }

    pub(crate) fn into_intermediate_storage(self) -> IntermediateStorage {
        IntermediateStorage {
            files: self.files,
            file_content_hashes: Vec::new(),
            nodes: self.nodes,
            structural_unit_node_ids: Vec::new(),
            structural_text_units: Vec::new(),
            structural_text_projections: Vec::new(),
            structural_text_cache_writes: Vec::new(),
            edges: self.edges,
            occurrences: self.occurrences,
            component_access: self.component_access,
            callable_projection_states: self.callable_projection_states,
            impl_anchor_node_ids: self.impl_anchor_node_ids,
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedStructuralArtifact {
    pub descriptor_version: u32,
    pub files: Vec<FileInfo>,
    pub file_content_hashes: Vec<codestory_store::FileContentHash>,
    pub nodes: Vec<Node>,
    pub structural_unit_node_ids: Vec<NodeId>,
    pub structural_text_units: Vec<codestory_store::StructuralTextUnit>,
    pub structural_text_projections: Vec<codestory_store::StructuralTextProjection>,
    pub edges: Vec<Edge>,
    pub occurrences: Vec<Occurrence>,
    pub component_access: Vec<(NodeId, AccessKind)>,
    pub callable_projection_states: Vec<CallableProjectionState>,
}

impl CachedStructuralArtifact {
    pub(crate) fn from_storage(storage: IntermediateStorage) -> Self {
        Self {
            descriptor_version: codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
            files: storage.files,
            file_content_hashes: storage.file_content_hashes,
            nodes: storage.nodes,
            structural_unit_node_ids: storage.structural_unit_node_ids,
            structural_text_units: storage.structural_text_units,
            structural_text_projections: storage.structural_text_projections,
            edges: storage.edges,
            occurrences: storage.occurrences,
            component_access: storage.component_access,
            callable_projection_states: storage.callable_projection_states,
        }
    }

    pub(crate) fn into_intermediate_storage(self) -> IntermediateStorage {
        IntermediateStorage {
            files: self.files,
            file_content_hashes: self.file_content_hashes,
            nodes: self.nodes,
            structural_unit_node_ids: self.structural_unit_node_ids,
            structural_text_units: self.structural_text_units,
            structural_text_projections: self.structural_text_projections,
            structural_text_cache_writes: Vec::new(),
            edges: self.edges,
            occurrences: self.occurrences,
            component_access: self.component_access,
            callable_projection_states: self.callable_projection_states,
            impl_anchor_node_ids: Vec::new(),
            errors: Vec::new(),
        }
    }
}

// Bumped to 4 because workspace-relative role classification changes persisted
// structural file metadata and zero-byte JSON admission.
pub(crate) const STRUCTURAL_ARTIFACT_CACHE_VERSION: u32 = 4;

pub(crate) fn build_structural_artifact_cache_key(
    cache_path: &Path,
    source_bytes: &[u8],
    producer: &str,
) -> Option<String> {
    let mut state = FNV_OFFSET_BASIS;
    mix_str(&mut state, "structural-artifact");
    mix_u32(
        &mut state,
        codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
    );
    mix_path(&mut state, cache_path)?;
    mix_bytes(&mut state, source_bytes);
    mix_str(&mut state, producer);
    Some(format!("v{STRUCTURAL_ARTIFACT_CACHE_VERSION}:{state:016x}"))
}

pub(crate) fn build_index_artifact_cache_key(
    root: &Path,
    cache_path: &Path,
    source_bytes: &[u8],
    language_config: &LanguageConfig,
    compilation_info: Option<&CompilationInfo>,
    legacy_edge_identity: bool,
    lazy_graph_execution: bool,
) -> Option<String> {
    let mut state = FNV_OFFSET_BASIS;
    mix_str(&mut state, "index-artifact");
    mix_u32(&mut state, INDEX_ARTIFACT_CACHE_VERSION);
    mix_path(&mut state, cache_path)?;
    mix_bytes(&mut state, source_bytes);
    mix_str(&mut state, language_config.language_name);
    mix_str(&mut state, language_config.graph_query);
    mix_optional_str(&mut state, language_config.tags_query);
    // Rust-side callable identity/scope extraction changed independently of the
    // graph rules. Invalidate affected languages without discarding unrelated
    // parser artifacts or changing the shared cache serialization schema.
    if matches!(
        language_config.language_name,
        "c" | "cpp" | "javascript" | "typescript"
    ) {
        mix_str(&mut state, "callable-identity-and-scope-v2");
    }
    mix_bool(&mut state, legacy_edge_identity);
    mix_bool(&mut state, lazy_graph_execution);
    mix_compilation_info(&mut state, root, compilation_info)?;
    Some(format!("v{INDEX_ARTIFACT_CACHE_VERSION}:{state:016x}"))
}

pub(crate) fn index_artifact_cache_path(root: &Path, path: &Path) -> Option<PathBuf> {
    let relative = if path.is_absolute() {
        path.strip_prefix(root).ok()?
    } else {
        path
    };
    let mut portable = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => portable.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if portable.as_os_str().is_empty() {
        return Some(PathBuf::from("."));
    }
    Some(portable)
}

fn mix_compilation_info(
    state: &mut u64,
    root: &Path,
    compilation_info: Option<&CompilationInfo>,
) -> Option<()> {
    let Some(compilation_info) = compilation_info else {
        mix_bool(state, false);
        return Some(());
    };
    mix_bool(state, true);
    mix_path(state, &portable_compile_path(root, &compilation_info.file)?)?;
    mix_path(
        state,
        &portable_compile_path(root, &compilation_info.working_directory)?,
    )?;
    mix_optional_standard(state, compilation_info.standard);

    let mut include_paths = compilation_info
        .include_paths
        .iter()
        .map(|path| portable_compile_path(root, path))
        .collect::<Option<Vec<_>>>()?;
    include_paths.sort_unstable();
    for include_path in include_paths {
        mix_path(state, &include_path)?;
    }

    let mut system_include_paths = compilation_info
        .system_include_paths
        .iter()
        .map(|path| portable_compile_path(root, path))
        .collect::<Option<Vec<_>>>()?;
    system_include_paths.sort_unstable();
    for system_include_path in system_include_paths {
        mix_path(state, &system_include_path)?;
    }

    let mut defines = compilation_info
        .defines
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<Vec<_>>();
    defines.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, value) in defines {
        mix_str(state, &name);
        mix_optional_string(state, value.as_ref());
    }

    let mut other_flags = compilation_info
        .other_flags
        .iter()
        .map(|flag| portable_compile_flag(root, flag))
        .collect::<Option<Vec<_>>>()?;
    other_flags.sort_unstable();
    for flag in other_flags {
        mix_str(state, &flag);
    }
    Some(())
}

fn portable_compile_path(root: &Path, path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return index_artifact_cache_path(root, path);
    }
    index_artifact_cache_path(root, path)
}

fn portable_compile_flag(root: &Path, flag: &str) -> Option<String> {
    let path = Path::new(flag);
    if path.is_absolute() {
        return index_artifact_cache_path(root, path)
            .map(|path| format!("path:{}", path.to_string_lossy()));
    }
    if is_standalone_slash_root_path_like(flag) {
        return None;
    }
    let root_text = root.to_string_lossy();
    if !root_text.is_empty() && flag.contains(root_text.as_ref()) {
        return None;
    }
    if has_unportable_embedded_absolute_path(flag) {
        return None;
    }
    Some(format!("flag:{flag}"))
}

fn is_standalone_slash_root_path_like(flag: &str) -> bool {
    let Some(rest) = flag.strip_prefix('/').or_else(|| flag.strip_prefix('\\')) else {
        return false;
    };
    rest.contains('/') || rest.contains('\\')
}

fn has_unportable_embedded_absolute_path(flag: &str) -> bool {
    if flag.contains("=/") || flag.contains("=\\") {
        return true;
    }
    if flag.as_bytes().windows(3).any(|bytes| {
        bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && is_path_separator(bytes[2])
    }) {
        return true;
    }
    ["-include", "-imacros", "-include-pch", "-isysroot"]
        .iter()
        .any(|prefix| {
            flag.strip_prefix(prefix)
                .is_some_and(starts_with_absolute_path_like)
        })
}

fn starts_with_absolute_path_like(value: &str) -> bool {
    value.starts_with('/') || value.starts_with('\\') || is_windows_absolute_path_like(value)
}

fn is_windows_absolute_path_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && is_path_separator(bytes[2])
}

fn is_path_separator(byte: u8) -> bool {
    byte == b'/' || byte == b'\\'
}

fn mix_path(state: &mut u64, path: &Path) -> Option<()> {
    let token = index_artifact_cache_path(Path::new(""), path)?;
    mix_str(state, &token.to_string_lossy().replace('\\', "/"));
    Some(())
}

fn mix_optional_standard(state: &mut u64, standard: Option<CxxStandard>) {
    mix_optional_str(
        state,
        standard.map(|standard| match standard {
            CxxStandard::C89 => "c89",
            CxxStandard::C99 => "c99",
            CxxStandard::C11 => "c11",
            CxxStandard::C17 => "c17",
            CxxStandard::C23 => "c23",
            CxxStandard::Cxx98 => "c++98",
            CxxStandard::Cxx03 => "c++03",
            CxxStandard::Cxx11 => "c++11",
            CxxStandard::Cxx14 => "c++14",
            CxxStandard::Cxx17 => "c++17",
            CxxStandard::Cxx20 => "c++20",
            CxxStandard::Cxx23 => "c++23",
        }),
    );
}

fn mix_optional_string(state: &mut u64, value: Option<&String>) {
    mix_optional_str(state, value.map(String::as_str));
}

fn mix_optional_str(state: &mut u64, value: Option<&str>) {
    match value {
        Some(value) => {
            mix_bool(state, true);
            mix_str(state, value);
        }
        None => mix_bool(state, false),
    }
}

fn mix_bool(state: &mut u64, value: bool) {
    mix_bytes(state, &[u8::from(value)]);
}

fn mix_u32(state: &mut u64, value: u32) {
    mix_bytes(state, &value.to_le_bytes());
}

fn mix_str(state: &mut u64, value: &str) {
    mix_bytes(state, value.as_bytes());
}

fn mix_bytes(state: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *state ^= u64::from(*byte);
        *state = state.wrapping_mul(FNV_PRIME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_artifact_cache_key_is_portable_across_roots() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let root_a = temp.path().join("root-a");
        let root_b = temp.path().join("root-b");
        let config = crate::get_language_for_ext("cpp").expect("cpp config");
        let source = b"int main() { return 0; }";
        let cache_path = Path::new("src/main.cpp");

        let key_a = build_index_artifact_cache_key(
            &root_a,
            cache_path,
            source,
            &config,
            Some(&CompilationInfo {
                file: root_a.join("src/main.cpp"),
                working_directory: root_a.clone(),
                include_paths: vec![root_a.join("include")],
                system_include_paths: Vec::new(),
                defines: HashMap::from([("FOO".to_string(), Some("1".to_string()))]),
                standard: Some(CxxStandard::Cxx20),
                other_flags: vec!["src/main.cpp".to_string()],
            }),
            false,
            true,
        )
        .expect("portable source-root compile info");
        let key_b = build_index_artifact_cache_key(
            &root_b,
            cache_path,
            source,
            &config,
            Some(&CompilationInfo {
                file: root_b.join("src/main.cpp"),
                working_directory: root_b.clone(),
                include_paths: vec![root_b.join("include")],
                system_include_paths: Vec::new(),
                defines: HashMap::from([("FOO".to_string(), Some("1".to_string()))]),
                standard: Some(CxxStandard::Cxx20),
                other_flags: vec!["src/main.cpp".to_string()],
            }),
            false,
            true,
        )
        .expect("portable target-root compile info");

        assert_eq!(key_a, key_b);
        Ok(())
    }

    #[test]
    fn parser_cache_key_distinguishes_raw_bytes_with_the_same_lossy_text() {
        let config = crate::get_language_for_ext("c").expect("C config");
        let root = Path::new("project");
        let cache_path = Path::new("src/non-utf8.c");
        let first = build_index_artifact_cache_key(
            root,
            cache_path,
            b"/* \x80 */",
            &config,
            None,
            false,
            true,
        )
        .expect("first cache key");
        let second = build_index_artifact_cache_key(
            root,
            cache_path,
            b"/* \x81 */",
            &config,
            None,
            false,
            true,
        )
        .expect("second cache key");

        assert_eq!(
            String::from_utf8_lossy(b"/* \x80 */"),
            String::from_utf8_lossy(b"/* \x81 */")
        );
        assert_ne!(first, second);
    }

    #[test]
    fn parser_cache_key_invalidates_changed_callable_extraction_rules() {
        for extension in ["c", "cpp", "js", "ts", "tsx"] {
            let config = crate::get_language_for_ext(extension).expect("parser config");
            let key = |config: &crate::LanguageConfig| {
                build_index_artifact_cache_key(
                    Path::new("project"),
                    Path::new("source"),
                    b"unchanged source",
                    config,
                    None,
                    false,
                    true,
                )
                .expect("portable cache key")
            };
            let current = key(&config);
            let old_config = crate::LanguageConfig {
                graph_query: "(older_callable_rule)",
                ..config
            };
            assert_ne!(
                current,
                key(&old_config),
                "{extension} must not reuse the old projection"
            );
        }
    }

    #[test]
    fn callable_scope_cache_revision_is_limited_to_affected_languages() {
        for extension in ["c", "cpp", "js", "ts", "tsx", "rs", "py"] {
            let config = crate::get_language_for_ext(extension).expect("parser config");
            let root = Path::new("project");
            let cache_path = Path::new("source");
            let source = b"unchanged source";
            // Reconstruct the previously shipped key, with identical grammar
            // and source bytes, to test Rust-side extraction invalidation.
            let mut previous = FNV_OFFSET_BASIS;
            mix_str(&mut previous, "index-artifact");
            mix_u32(&mut previous, INDEX_ARTIFACT_CACHE_VERSION);
            mix_path(&mut previous, cache_path).expect("portable path");
            mix_bytes(&mut previous, source);
            mix_str(&mut previous, config.language_name);
            mix_str(&mut previous, config.graph_query);
            mix_optional_str(&mut previous, config.tags_query);
            mix_bool(&mut previous, false);
            mix_bool(&mut previous, true);
            mix_compilation_info(&mut previous, root, None).expect("portable config");
            let previous = format!("v{INDEX_ARTIFACT_CACHE_VERSION}:{previous:016x}");
            let current = build_index_artifact_cache_key(
                root, cache_path, source, &config, None, false, true,
            )
            .expect("cache key");
            assert_eq!(
                current == previous,
                matches!(extension, "rs" | "py"),
                "{extension}"
            );
        }
    }

    #[test]
    fn test_artifact_cache_key_skips_unportable_compile_paths() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        let config = crate::get_language_for_ext("cpp").expect("cpp config");

        let key = build_index_artifact_cache_key(
            &root,
            Path::new("src/main.cpp"),
            b"int main() { return 0; }",
            &config,
            Some(&CompilationInfo {
                file: root.join("src/main.cpp"),
                working_directory: root.clone(),
                include_paths: vec![outside.join("include")],
                system_include_paths: Vec::new(),
                defines: HashMap::new(),
                standard: None,
                other_flags: Vec::new(),
            }),
            false,
            true,
        );

        assert!(key.is_none());
        Ok(())
    }

    #[test]
    fn test_artifact_cache_key_skips_unportable_raw_compile_flags() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("root");
        let config = crate::get_language_for_ext("cpp").expect("cpp config");

        for flag in [
            "--sysroot=/abs/sdk",
            "-include/abs/header.h",
            "/abs/sdk",
            "\\abs\\sdk",
            "/abs/header.h",
        ] {
            let key = build_index_artifact_cache_key(
                &root,
                Path::new("src/main.cpp"),
                b"int main() { return 0; }",
                &config,
                Some(&CompilationInfo {
                    file: root.join("src/main.cpp"),
                    working_directory: root.clone(),
                    include_paths: Vec::new(),
                    system_include_paths: Vec::new(),
                    defines: HashMap::new(),
                    standard: None,
                    other_flags: vec![flag.to_string()],
                }),
                false,
                true,
            );

            assert!(key.is_none(), "{flag} must fail closed");
        }
        Ok(())
    }

    #[test]
    fn parser_cache_without_resolution_inputs_decodes_as_an_empty_legacy_projection() {
        let legacy = serde_json::json!({
            "files": [],
            "nodes": [],
            "edges": [],
            "occurrences": [],
            "component_access": [],
            "callable_projection_states": [],
            "impl_anchor_node_ids": []
        });

        let decoded: CachedIndexArtifact = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.resolution_input_schema_version, 0);
        assert!(decoded.call_resolution_inputs.is_empty());
        assert!(decoded.resolution_file.is_none());
        assert!(
            !crate::proof_resolution::cached_resolution_inputs_are_current(
                &decoded,
                "typescript",
                &"0".repeat(64),
                &"0".repeat(64),
            )
        );
        assert!(
            !crate::proof_resolution::cached_resolution_inputs_are_current(
                &decoded, "go", "unused", "unused",
            ),
            "a legacy cache without proof inputs cannot satisfy the installed Go adapter"
        );
    }
}
