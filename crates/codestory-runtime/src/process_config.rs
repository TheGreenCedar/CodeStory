use crate::RuntimeRetrievalConfig;
use codestory_contracts::workspace::SourceIndexPolicy;
use std::path::Path;

/// Immutable process-owned defaults injected into one runtime.
///
/// Adapters capture sidecar defaults and source-index policy once, then pass
/// this value through every retained project context. Other feature and
/// evaluation controls remain owned by their respective subsystems.
#[derive(Debug, Clone)]
pub struct RuntimeProcessConfig {
    pub sidecar: RuntimeRetrievalConfig,
    pub source_index_policy: SourceIndexPolicy,
}

impl RuntimeProcessConfig {
    /// Preserve the existing retrieval-config constructor while adapters move
    /// to the runtime-owned wrapper in staged S2 slices.
    pub fn new(
        sidecar: codestory_retrieval::SidecarRuntimeConfig,
        source_index_policy: SourceIndexPolicy,
    ) -> Self {
        Self::new_with_retrieval_config(sidecar.into(), source_index_policy)
    }

    pub fn new_with_retrieval_config(
        sidecar: RuntimeRetrievalConfig,
        source_index_policy: SourceIndexPolicy,
    ) -> Self {
        Self {
            sidecar,
            source_index_policy,
        }
    }

    pub fn local() -> Self {
        Self::new_with_retrieval_config(
            RuntimeRetrievalConfig::local(),
            SourceIndexPolicy::default(),
        )
    }

    /// Return the non-secret identity of the immutable project runtime
    /// configuration captured by the owning adapter.
    ///
    /// The project cache root is supplied separately because the embedding
    /// server cache is process-scoped while core storage remains project-owned.
    /// Keeping this digest below adapters gives activation leases and retained
    /// transport contexts one definition of configuration equality.
    pub fn configuration_id(&self, project_cache_root: &Path) -> String {
        // Credentials never enter the digest. Their presence participates in
        // the immutable boundary, while secret material remains outside logs
        // and diagnostic identifiers.
        let sidecar = self.sidecar.as_inner();
        let source_index_policy = &self.source_index_policy;
        let mut identity = format!(
            "{}\0{:?}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
            configuration_path_identity(project_cache_root),
            sidecar.profile,
            sidecar.namespace,
            sidecar.embedding.allow_cpu,
            sidecar.retrieval.hybrid_enabled,
            sidecar.retrieval.semantic_doc_scope,
            sidecar.retrieval.semantic_doc_alias_mode,
            sidecar.retrieval.semantic_doc_max_tokens,
            sidecar.retrieval.llm_doc_embed_batch_size,
            sidecar.retrieval.stream_pending_docs,
            sidecar.retrieval.stream_sort_window_batches,
            sidecar.summary.endpoint.as_deref().unwrap_or(""),
            sidecar.summary.api_key.is_some(),
        );
        identity.push_str(&format!(
            "\0{}\0{:?}\0{:?}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
            sidecar.summary.model,
            sidecar.summary.max_tokens,
            sidecar.summary.timeout,
            sidecar.run_id.as_deref().unwrap_or(""),
            configuration_path_identity(&sidecar.layout.lexical_data_dir),
            configuration_path_identity(&sidecar.layout.semantic_data_dir),
            configuration_path_identity(&sidecar.layout.scip_artifacts_root),
            configuration_path_identity(&sidecar.layout.state_file),
            source_index_policy.policy_version,
            source_index_policy.byte_cap,
            source_index_policy.structural_byte_cap,
            source_index_policy.structural_unit_cap,
        ));
        fnv1a_hex(identity.as_bytes())
    }
}

fn configuration_path_identity(path: &Path) -> String {
    #[cfg(windows)]
    {
        windows_ordinal_configuration_path_identity(path)
    }
    #[cfg(not(windows))]
    {
        clean_path_string(&path.to_string_lossy())
    }
}

fn clean_path_string(path: &str) -> String {
    let mut stringified = path.replace('\\', "/");
    if let Some(stripped) = stringified.strip_prefix("//?/UNC/") {
        stringified = format!("//{stripped}");
    } else if stringified.starts_with("//?/") {
        stringified = stringified[4..].to_string();
    }
    stringified
}

#[cfg(windows)]
fn windows_ordinal_configuration_path_identity(path: &Path) -> String {
    use std::ptr;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LCMapStringEx(
            locale_name: *const u16,
            map_flags: u32,
            source: *const u16,
            source_len: i32,
            destination: *mut u16,
            destination_len: i32,
            version_information: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
            sort_handle: isize,
        ) -> i32;
    }

    const LCMAP_UPPERCASE: u32 = 0x0000_0200;
    let normalized = clean_path_string(&path.to_string_lossy()).replace('/', "\\");
    let source = normalized.encode_utf16().collect::<Vec<_>>();
    let Ok(source_len) = i32::try_from(source.len()) else {
        return normalized.to_uppercase();
    };
    let invariant_locale = [0_u16];
    // SAFETY: all pointers remain valid for the supplied lengths. The
    // invariant locale uses the same language-independent uppercase table as
    // Windows ordinal ignore-case comparison.
    let required = unsafe {
        LCMapStringEx(
            invariant_locale.as_ptr(),
            LCMAP_UPPERCASE,
            source.as_ptr(),
            source_len,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        )
    };
    if required <= 0 {
        return normalized.to_uppercase();
    }
    let mut mapped = vec![0_u16; required as usize];
    // SAFETY: `mapped` has the size returned by the preceding mapping query.
    let written = unsafe {
        LCMapStringEx(
            invariant_locale.as_ptr(),
            LCMAP_UPPERCASE,
            source.as_ptr(),
            source_len,
            mapped.as_mut_ptr(),
            required,
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        )
    };
    if written <= 0 {
        return normalized.to_uppercase();
    }
    String::from_utf16_lossy(&mapped[..written as usize])
}

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod structural_cap_identity_tests {
    use super::*;

    /// `configuration_id` is a hand-written `\0`-joined format string, not a
    /// derive, so a new `SourceIndexPolicy` field is omitted silently. Two
    /// runtimes that differ only in the structural bound would then share an
    /// identity and reuse each other's ready-leases and retained transport
    /// contexts. Nothing else in the tree can catch that.
    #[test]
    fn configuration_id_separates_policies_that_differ_only_in_the_structural_cap() {
        let sidecar = crate::test_sidecar_runtime_from_env();
        let base = SourceIndexPolicy::default();
        let narrowed = SourceIndexPolicy {
            structural_byte_cap: base.structural_byte_cap / 2,
            ..base.clone()
        };
        let cache_root = Path::new("/tmp/codestory-configuration-identity");
        assert_ne!(
            RuntimeProcessConfig::new(sidecar.clone(), base).configuration_id(cache_root),
            RuntimeProcessConfig::new(sidecar, narrowed).configuration_id(cache_root),
            "the structural bound must take part in the configuration identity"
        );
    }
}
