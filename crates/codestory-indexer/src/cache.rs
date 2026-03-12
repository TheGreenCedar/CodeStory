use crate::compilation_database::{CompilationInfo, CxxStandard};
use crate::{IndexResult, LanguageConfig, intermediate_storage::IntermediateStorage};
use codestory_contracts::graph::{
    AccessKind, CallableProjectionState, Edge, Node, NodeId, Occurrence,
};
use codestory_store::FileInfo;
use serde::{Deserialize, Serialize};
use std::path::Path;

const INDEX_ARTIFACT_CACHE_VERSION: u32 = 1;
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
    path: &Path,
    source_bytes: &[u8],
    language_config: &LanguageConfig,
    compilation_info: Option<&CompilationInfo>,
    legacy_edge_identity: bool,
    lazy_graph_execution: bool,
) -> String {
    let mut state = FNV_OFFSET_BASIS;
    mix_str(&mut state, "index-artifact");
    mix_u32(&mut state, INDEX_ARTIFACT_CACHE_VERSION);
    mix_str(&mut state, &path.to_string_lossy());
    mix_bytes(&mut state, source_bytes);
    mix_str(&mut state, language_config.language_name);
    mix_str(&mut state, language_config.graph_query);
    mix_optional_str(&mut state, language_config.tags_query);
    mix_bool(&mut state, legacy_edge_identity);
    mix_bool(&mut state, lazy_graph_execution);
    mix_compilation_info(&mut state, compilation_info);
    format!("v{INDEX_ARTIFACT_CACHE_VERSION}:{state:016x}")
}

fn mix_compilation_info(state: &mut u64, compilation_info: Option<&CompilationInfo>) {
    let Some(compilation_info) = compilation_info else {
        mix_bool(state, false);
        return;
    };
    mix_bool(state, true);
    mix_str(state, &compilation_info.file.to_string_lossy());
    mix_str(state, &compilation_info.working_directory.to_string_lossy());
    mix_optional_standard(state, compilation_info.standard);

    let mut include_paths = compilation_info
        .include_paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    include_paths.sort_unstable();
    for include_path in include_paths {
        mix_str(state, &include_path);
    }

    let mut system_include_paths = compilation_info
        .system_include_paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    system_include_paths.sort_unstable();
    for system_include_path in system_include_paths {
        mix_str(state, &system_include_path);
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

    let mut other_flags = compilation_info.other_flags.clone();
    other_flags.sort_unstable();
    for flag in other_flags {
        mix_str(state, &flag);
    }
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
