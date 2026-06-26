use crate::compilation_database::{CompilationInfo, CxxStandard};
use crate::{IndexResult, LanguageConfig, intermediate_storage::IntermediateStorage};
use codestory_contracts::graph::{
    AccessKind, CallableProjectionState, Edge, Node, NodeId, Occurrence,
};
use codestory_store::FileInfo;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

const INDEX_ARTIFACT_CACHE_VERSION: u32 = 2;
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x00000100000001B3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedIndexArtifact {
    pub files: Vec<FileInfo>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub occurrences: Vec<Occurrence>,
    pub component_access: Vec<(NodeId, AccessKind)>,
    pub callable_projection_states: Vec<CallableProjectionState>,
    pub impl_anchor_node_ids: Vec<NodeId>,
}

impl CachedIndexArtifact {
    pub(crate) fn from_index_result(index_result: IndexResult) -> Self {
        Self {
            files: index_result.files,
            nodes: index_result.nodes,
            edges: index_result.edges,
            occurrences: index_result.occurrences,
            component_access: index_result.component_access,
            callable_projection_states: index_result.callable_projection_states,
            impl_anchor_node_ids: index_result.impl_anchor_node_ids,
        }
    }

    pub(crate) fn into_intermediate_storage(self) -> IntermediateStorage {
        IntermediateStorage {
            files: self.files,
            nodes: self.nodes,
            edges: self.edges,
            occurrences: self.occurrences,
            component_access: self.component_access,
            callable_projection_states: self.callable_projection_states,
            impl_anchor_node_ids: self.impl_anchor_node_ids,
            errors: Vec::new(),
        }
    }
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
}
