use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
#[cfg(any(test, feature = "test-support"))]
use std::time::SystemTime;

pub const DEFAULT_AGENT_RUN_ID: &str = "shared-agent";

const LOCAL_RETRIEVAL_NAMESPACE: &str = "codestory-v3";
const AGENT_RETRIEVAL_NAMESPACE_PREFIX: &str = "codestory-agent-v3-";
const RETRIEVAL_STATE_FILE: &str = "retrieval-generations-v1.state";
const RETRIEVAL_ARTIFACTS_DIR: &str = "retrieval";
const RUNTIME_ENV_KEYS: &[&str] = &[
    "CODESTORY_RETRIEVAL_PROFILE",
    "CODESTORY_RETRIEVAL_RUN_ID",
    "CI",
    "GITHUB_ACTIONS",
    "CODESTORY_EMBED_ALLOW_CPU",
    "CODESTORY_HYBRID_RETRIEVAL_ENABLED",
    "CODESTORY_SEMANTIC_DOC_SCOPE",
    "CODESTORY_SEMANTIC_DOC_ALIAS_MODE",
    "CODESTORY_SEMANTIC_DOC_MAX_TOKENS",
    "CODESTORY_LLM_DOC_EMBED_BATCH_SIZE",
    "CODESTORY_SEMANTIC_STREAM_PENDING_DOCS",
    "CODESTORY_SEMANTIC_STREAM_SORT_WINDOW_BATCHES",
    "CODESTORY_SUMMARY_ENDPOINT",
    "CODESTORY_SUMMARY_MODEL",
    "CODESTORY_SUMMARY_API_KEY",
    "CODESTORY_SUMMARY_MAX_TOKENS",
    "CODESTORY_SUMMARY_TIMEOUT_SECS",
];

#[derive(Debug, Clone)]
pub struct SidecarLayout {
    pub lexical_data_dir: PathBuf,
    pub semantic_data_dir: PathBuf,
    pub scip_artifacts_root: PathBuf,
    /// Coordination anchor for generation publication and retention.
    ///
    /// No server or process state is written here.
    pub state_file: PathBuf,
}

impl SidecarLayout {
    pub fn from_env() -> Self {
        SidecarRuntimeConfig::local().layout
    }

    pub fn from_env_for_project(project_root: &Path) -> Self {
        SidecarRuntimeConfig::for_project_auto(project_root).layout
    }

    pub fn from_env_agent(project_root: &Path) -> Self {
        SidecarRuntimeConfig::for_project_profile(Some(project_root), SidecarProfile::Agent).layout
    }

    pub fn from_env_local(project_root: Option<&Path>) -> Self {
        SidecarRuntimeConfig::for_project_profile(project_root, SidecarProfile::Local).layout
    }

    pub fn scip_project_dir(&self, project_id: &str) -> PathBuf {
        self.scip_artifacts_root.join(project_id)
    }

    pub fn ensure_data_dirs(&self) -> Result<()> {
        for dir in [
            &self.lexical_data_dir,
            &self.semantic_data_dir,
            &self.scip_artifacts_root,
        ] {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("create retrieval data dir {}", dir.display()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarProfile {
    Local,
    Agent,
}

impl SidecarProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Agent => "agent",
        }
    }
}

/// Fixed process embedding policy.
///
/// All model, tokenizer, pooling, normalization, and backend selection inputs
/// are compile-time product contracts. The sole runtime policy is whether an
/// explicitly requested CPU engine is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingRuntimeConfig {
    pub allow_cpu: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidecarRuntimeDefaults {
    values: BTreeMap<String, String>,
}

impl SidecarRuntimeDefaults {
    pub fn from_process_env() -> Self {
        Self {
            values: RUNTIME_ENV_KEYS
                .iter()
                .filter_map(|name| {
                    std::env::var(name)
                        .ok()
                        .map(|value| ((*name).to_string(), value))
                })
                .collect(),
        }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
}

/// Immutable process-scoped inputs used by every open repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarProcessDefaults {
    cache_root: PathBuf,
    runtime: SidecarRuntimeDefaults,
}

impl SidecarProcessDefaults {
    pub fn new(cache_root: PathBuf, runtime: SidecarRuntimeDefaults) -> Self {
        Self {
            cache_root,
            runtime,
        }
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    pub fn runtime(&self) -> &SidecarRuntimeDefaults {
        &self.runtime
    }

    pub fn embedding_allow_cpu(&self) -> bool {
        default_flag(&self.runtime, "CODESTORY_EMBED_ALLOW_CPU", false)
    }

    pub fn with_cache_root(&self, cache_root: PathBuf) -> Self {
        Self {
            cache_root,
            runtime: self.runtime.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalRuntimeConfig {
    pub hybrid_enabled: bool,
    pub semantic_doc_scope: String,
    pub semantic_doc_alias_mode: String,
    pub semantic_doc_max_tokens: usize,
    pub llm_doc_embed_batch_size: usize,
    pub stream_pending_docs: bool,
    pub stream_sort_window_batches: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryRuntimeConfig {
    pub endpoint: Option<String>,
    pub model: String,
    pub api_key: Option<String>,
    pub max_tokens: Option<usize>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidecarRuntimeOverrides {
    pub hybrid_retrieval_enabled: Option<bool>,
    pub semantic_doc_scope: Option<String>,
    pub semantic_doc_alias_mode: Option<String>,
    pub summary_endpoint: Option<String>,
    pub summary_model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SidecarRuntimeConfig {
    pub project_identity: Option<codestory_workspace::ProjectIdentityV3>,
    pub cache_root: PathBuf,
    pub layout: SidecarLayout,
    pub profile: SidecarProfile,
    pub run_id: Option<String>,
    pub namespace: String,
    pub embedding: EmbeddingRuntimeConfig,
    pub retrieval: RetrievalRuntimeConfig,
    pub summary: SummaryRuntimeConfig,
}

impl SidecarRuntimeConfig {
    pub fn local() -> Self {
        Self::for_project_profile(None, SidecarProfile::Local)
    }

    pub fn for_project_auto(project_root: &Path) -> Self {
        Self::for_project_auto_with_process_defaults(
            project_root,
            &sidecar_process_defaults(),
            &SidecarRuntimeOverrides::default(),
        )
    }

    #[doc(hidden)]
    pub fn for_project_auto_with_process_defaults(
        project_root: &Path,
        process_defaults: &SidecarProcessDefaults,
        overrides: &SidecarRuntimeOverrides,
    ) -> Self {
        let defaults = process_defaults.runtime();
        let (profile, run_id) = auto_runtime_selection(
            env_profile(defaults),
            env_agent_run_id(defaults),
            running_in_ci_agent(defaults),
        );
        Self::for_project_profile_with_process_defaults(
            Some(project_root),
            profile,
            run_id.as_deref(),
            process_defaults,
            overrides,
        )
    }

    pub fn for_project_profile(project_root: Option<&Path>, profile: SidecarProfile) -> Self {
        Self::for_project_profile_with_run_id(project_root, profile, None)
    }

    pub fn for_project_profile_with_run_id(
        project_root: Option<&Path>,
        profile: SidecarProfile,
        run_id: Option<&str>,
    ) -> Self {
        Self::for_project_profile_with_process_defaults(
            project_root,
            profile,
            run_id,
            &sidecar_process_defaults(),
            &SidecarRuntimeOverrides::default(),
        )
    }

    #[doc(hidden)]
    pub fn for_project_profile_with_process_defaults(
        project_root: Option<&Path>,
        profile: SidecarProfile,
        run_id: Option<&str>,
        process_defaults: &SidecarProcessDefaults,
        overrides: &SidecarRuntimeOverrides,
    ) -> Self {
        let cache_root = process_defaults.cache_root().to_path_buf();
        let defaults = process_defaults.runtime();
        let run_id = (profile == SidecarProfile::Agent).then(|| agent_run_id(run_id, defaults));
        let project_identity = project_root.map(codestory_workspace::project_identity_v3);
        let namespace = namespace_for(project_identity.as_ref(), profile, run_id.as_deref());
        let artifact_root = match profile {
            SidecarProfile::Local => cache_root.clone(),
            SidecarProfile::Agent => cache_root.join(RETRIEVAL_ARTIFACTS_DIR).join(&namespace),
        };
        let layout = SidecarLayout {
            lexical_data_dir: artifact_root.join("lexical"),
            semantic_data_dir: artifact_root.join("semantic"),
            scip_artifacts_root: artifact_root.join("scip"),
            state_file: artifact_root.join(RETRIEVAL_STATE_FILE),
        };
        Self {
            project_identity,
            cache_root,
            layout,
            profile,
            run_id,
            namespace,
            embedding: EmbeddingRuntimeConfig {
                allow_cpu: process_defaults.embedding_allow_cpu(),
            },
            retrieval: retrieval_runtime_config(defaults, overrides),
            summary: summary_runtime_config(defaults, overrides),
        }
    }

    pub fn with_profile_and_run_id(
        &self,
        project_root: Option<&Path>,
        profile: SidecarProfile,
        run_id: Option<&str>,
    ) -> Self {
        let process_defaults =
            SidecarProcessDefaults::new(self.cache_root.clone(), SidecarRuntimeDefaults::default());
        let mut selected = Self::for_project_profile_with_process_defaults(
            project_root,
            profile,
            run_id,
            &process_defaults,
            &SidecarRuntimeOverrides::default(),
        );
        selected.embedding = self.embedding;
        selected.retrieval = self.retrieval.clone();
        selected.summary = self.summary.clone();
        selected
    }

    pub(crate) fn validated_project_identity(
        &self,
        project_root: &Path,
    ) -> Result<codestory_workspace::ProjectIdentityV3> {
        let current = codestory_workspace::project_identity_v3(project_root);
        let Some(retained) = self.project_identity.as_ref() else {
            return Ok(current);
        };
        if retained.project_id != current.project_id
            || retained.workspace_id != current.workspace_id
        {
            anyhow::bail!(
                "stable project identity changed after retrieval runtime selection: retained_project_id={} retained_workspace_id={} current_project_id={} current_workspace_id={}; rebuild the runtime before publishing or querying retrieval artifacts",
                retained.project_id,
                retained.workspace_id,
                current.project_id,
                current.workspace_id,
            );
        }
        // Artifact eligibility follows the current verified source state. It is
        // intentionally not part of the immutable runtime/context identity.
        Ok(current)
    }
}

fn retrieval_runtime_config(
    defaults: &SidecarRuntimeDefaults,
    overrides: &SidecarRuntimeOverrides,
) -> RetrievalRuntimeConfig {
    RetrievalRuntimeConfig {
        hybrid_enabled: default_optional_bool(defaults, "CODESTORY_HYBRID_RETRIEVAL_ENABLED")
            .or(overrides.hybrid_retrieval_enabled)
            .unwrap_or(true),
        semantic_doc_scope: default_nonempty(defaults, "CODESTORY_SEMANTIC_DOC_SCOPE")
            .or_else(|| overrides.semantic_doc_scope.clone())
            .unwrap_or_else(|| "durable".to_string()),
        semantic_doc_alias_mode: default_nonempty(defaults, "CODESTORY_SEMANTIC_DOC_ALIAS_MODE")
            .or_else(|| overrides.semantic_doc_alias_mode.clone())
            .unwrap_or_else(|| "alias_variant".to_string()),
        semantic_doc_max_tokens: default_bounded_usize(
            defaults,
            "CODESTORY_SEMANTIC_DOC_MAX_TOKENS",
            16,
            8192,
        )
        .unwrap_or(128),
        llm_doc_embed_batch_size: default_bounded_usize(
            defaults,
            "CODESTORY_LLM_DOC_EMBED_BATCH_SIZE",
            1,
            2048,
        )
        .unwrap_or(128),
        stream_pending_docs: default_optional_bool(
            defaults,
            "CODESTORY_SEMANTIC_STREAM_PENDING_DOCS",
        )
        .unwrap_or(true),
        stream_sort_window_batches: default_bounded_usize(
            defaults,
            "CODESTORY_SEMANTIC_STREAM_SORT_WINDOW_BATCHES",
            1,
            16,
        )
        .unwrap_or(1),
    }
}

fn summary_runtime_config(
    defaults: &SidecarRuntimeDefaults,
    overrides: &SidecarRuntimeOverrides,
) -> SummaryRuntimeConfig {
    SummaryRuntimeConfig {
        endpoint: default_nonempty(defaults, "CODESTORY_SUMMARY_ENDPOINT")
            .or_else(|| overrides.summary_endpoint.clone()),
        model: default_nonempty(defaults, "CODESTORY_SUMMARY_MODEL")
            .or_else(|| overrides.summary_model.clone())
            .unwrap_or_else(|| "codestory-symbol-summary".to_string()),
        api_key: default_nonempty(defaults, "CODESTORY_SUMMARY_API_KEY"),
        max_tokens: default_bounded_usize(
            defaults,
            "CODESTORY_SUMMARY_MAX_TOKENS",
            1,
            u32::MAX as usize,
        ),
        timeout: Duration::from_secs(
            default_bounded_usize(defaults, "CODESTORY_SUMMARY_TIMEOUT_SECS", 1, 300).unwrap_or(30)
                as u64,
        ),
    }
}

fn default_nonempty(defaults: &SidecarRuntimeDefaults, name: &str) -> Option<String> {
    defaults
        .get(name)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn default_optional_bool(defaults: &SidecarRuntimeDefaults, name: &str) -> Option<bool> {
    defaults
        .get(name)
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
}

fn default_bounded_usize(
    defaults: &SidecarRuntimeDefaults,
    name: &str,
    min: usize,
    max: usize,
) -> Option<usize> {
    defaults
        .get(name)
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(min, max))
}

fn default_flag(defaults: &SidecarRuntimeDefaults, name: &str, default: bool) -> bool {
    default_optional_bool(defaults, name).unwrap_or(default)
}

fn env_profile(defaults: &SidecarRuntimeDefaults) -> Option<SidecarProfile> {
    defaults
        .get("CODESTORY_RETRIEVAL_PROFILE")
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "agent" | "ci" => Some(SidecarProfile::Agent),
            "local" | "dev" => Some(SidecarProfile::Local),
            _ => None,
        })
}

fn env_agent_run_id(defaults: &SidecarRuntimeDefaults) -> Option<String> {
    defaults
        .get("CODESTORY_RETRIEVAL_RUN_ID")
        .and_then(normalized_label_component)
}

fn running_in_ci_agent(defaults: &SidecarRuntimeDefaults) -> bool {
    default_flag(defaults, "CI", false) || default_flag(defaults, "GITHUB_ACTIONS", false)
}

fn auto_runtime_selection(
    explicit_profile: Option<SidecarProfile>,
    env_run_id: Option<String>,
    ci_agent: bool,
) -> (SidecarProfile, Option<String>) {
    match explicit_profile {
        Some(SidecarProfile::Local) => (SidecarProfile::Local, None),
        Some(SidecarProfile::Agent) => (SidecarProfile::Agent, env_run_id),
        None if env_run_id.is_some() || ci_agent => (SidecarProfile::Agent, env_run_id),
        None => (SidecarProfile::Local, None),
    }
}

fn namespace_for(
    project_identity: Option<&codestory_workspace::ProjectIdentityV3>,
    profile: SidecarProfile,
    run_id: Option<&str>,
) -> String {
    match (profile, project_identity) {
        (SidecarProfile::Local, _) => LOCAL_RETRIEVAL_NAMESPACE.into(),
        (SidecarProfile::Agent, Some(identity)) => format!(
            "{AGENT_RETRIEVAL_NAMESPACE_PREFIX}{}-{}",
            identity.workspace_id,
            run_id.unwrap_or("run")
        ),
        (SidecarProfile::Agent, None) => format!(
            "{AGENT_RETRIEVAL_NAMESPACE_PREFIX}{}-{}",
            std::process::id(),
            run_id.unwrap_or("run")
        ),
    }
}

fn agent_run_id(explicit: Option<&str>, defaults: &SidecarRuntimeDefaults) -> String {
    explicit
        .and_then(normalized_label_component)
        .or_else(|| env_agent_run_id(defaults))
        .unwrap_or_else(|| DEFAULT_AGENT_RUN_ID.to_string())
}

fn normalized_label_component(value: &str) -> Option<String> {
    let mut normalized = String::with_capacity(value.len());
    let mut previous_dash = false;
    for ch in value.trim().chars() {
        let next = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if next == '-' {
            if previous_dash {
                continue;
            }
            previous_dash = true;
        } else {
            previous_dash = false;
        }
        normalized.push(next);
    }
    let normalized = normalized.trim_matches('-').to_string();
    (!normalized.is_empty()).then_some(normalized)
}

fn uncached_user_cache_root() -> PathBuf {
    if let Ok(path) = std::env::var("CODESTORY_CACHE_ROOT") {
        let path = path.trim();
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    ProjectDirs::from("dev", "codestory", "codestory")
        .map(|dirs| dirs.cache_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("codestory").join("cache"))
}

#[cfg(not(test))]
fn frozen_process_defaults(cell: &OnceLock<SidecarProcessDefaults>) -> &SidecarProcessDefaults {
    cell.get_or_init(|| {
        SidecarProcessDefaults::new(
            uncached_user_cache_root(),
            SidecarRuntimeDefaults::from_process_env(),
        )
    })
}

pub fn sidecar_process_defaults() -> SidecarProcessDefaults {
    #[cfg(test)]
    let defaults = SidecarProcessDefaults::new(
        uncached_user_cache_root(),
        SidecarRuntimeDefaults::default(),
    );
    #[cfg(not(test))]
    let defaults = {
        static PROCESS_DEFAULTS: OnceLock<SidecarProcessDefaults> = OnceLock::new();
        frozen_process_defaults(&PROCESS_DEFAULTS).clone()
    };

    #[cfg(any(test, feature = "test-support"))]
    if let Some(cache_root) = test_cache_root_override() {
        return defaults.with_cache_root(cache_root);
    }
    defaults
}

pub fn user_cache_root() -> PathBuf {
    sidecar_process_defaults().cache_root
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static TEST_CACHE_ROOT_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn active_test_cache_root() -> Option<PathBuf> {
    test_cache_root_override()
}

#[cfg(any(test, feature = "test-support"))]
fn test_cache_root_override() -> Option<PathBuf> {
    let explicit = TEST_CACHE_ROOT_OVERRIDE.with(|root| root.borrow().clone());
    if explicit.is_some() {
        return explicit;
    }
    #[cfg(feature = "test-support")]
    if !AUTOMATIC_TEST_CACHE_ROOT_ENABLED.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    automatic_unit_test_cache_root()
}

#[cfg(feature = "test-support")]
static AUTOMATIC_TEST_CACHE_ROOT_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn enable_automatic_test_cache_root_for_process() {
    AUTOMATIC_TEST_CACHE_ROOT_ENABLED.store(true, std::sync::atomic::Ordering::Release);
}

#[cfg(any(test, feature = "test-support"))]
fn automatic_unit_test_cache_root() -> Option<PathBuf> {
    let thread = std::thread::current();
    let name = thread.name()?;
    if name == "main" {
        return None;
    }
    static PROCESS_ROOT: OnceLock<PathBuf> = OnceLock::new();
    let process_root = PROCESS_ROOT.get_or_init(|| {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir()
            .join("codestory-unit-tests")
            .join(format!("{}-{nonce}", std::process::id()))
    });
    let label = normalized_label_component(name)?;
    let prefix = &label[..label.len().min(32)];
    let root = process_root.join(format!("{prefix}-{}", &fnv1a_hex(label.as_bytes())[..12]));
    std::fs::create_dir_all(root.join(RETRIEVAL_ARTIFACTS_DIR)).ok()?;
    Some(root)
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
#[allow(dead_code)]
pub fn with_test_cache_root<T>(root: &Path, task: impl FnOnce() -> T) -> T {
    struct Reset(Option<PathBuf>);
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_CACHE_ROOT_OVERRIDE.with(|root| {
                *root.borrow_mut() = self.0.take();
            });
        }
    }
    let previous =
        TEST_CACHE_ROOT_OVERRIDE.with(|current| current.replace(Some(root.to_path_buf())));
    let _reset = Reset(previous);
    task()
}

#[cfg(any(test, feature = "test-support"))]
fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn dir_size_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries.flatten().fold(0_u64, |total, entry| {
        let path = entry.path();
        let bytes = if path.is_file() {
            entry.metadata().map(|metadata| metadata.len()).unwrap_or(0)
        } else if path.is_dir() {
            dir_size_bytes(&path)
        } else {
            0
        };
        total.saturating_add(bytes)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults(cache_root: &Path, values: &[(&str, &str)]) -> SidecarProcessDefaults {
        SidecarProcessDefaults::new(
            cache_root.to_path_buf(),
            SidecarRuntimeDefaults {
                values: values
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            },
        )
    }

    #[test]
    fn process_defaults_capture_the_only_embedding_policy() {
        let root = tempfile::tempdir().expect("cache root");
        let defaults = defaults(root.path(), &[("CODESTORY_EMBED_ALLOW_CPU", "1")]);
        let runtime = SidecarRuntimeConfig::for_project_profile_with_process_defaults(
            None,
            SidecarProfile::Local,
            None,
            &defaults,
            &SidecarRuntimeOverrides::default(),
        );
        assert!(runtime.embedding.allow_cpu);
        assert_eq!(runtime.cache_root, root.path());
    }

    #[test]
    fn profile_selection_changes_only_the_artifact_namespace() {
        let root = tempfile::tempdir().expect("cache root");
        let project = tempfile::tempdir().expect("project");
        let process = defaults(root.path(), &[]);
        let local = SidecarRuntimeConfig::for_project_profile_with_process_defaults(
            Some(project.path()),
            SidecarProfile::Local,
            None,
            &process,
            &SidecarRuntimeOverrides::default(),
        );
        let agent = local.with_profile_and_run_id(
            Some(project.path()),
            SidecarProfile::Agent,
            Some("Run 42"),
        );

        assert_eq!(local.cache_root, agent.cache_root);
        assert_eq!(local.layout.lexical_data_dir, root.path().join("lexical"));
        assert!(agent.namespace.ends_with("-run-42"));
        assert_eq!(
            agent.layout.lexical_data_dir,
            root.path()
                .join(RETRIEVAL_ARTIFACTS_DIR)
                .join(&agent.namespace)
                .join("lexical")
        );
        assert_eq!(local.embedding, agent.embedding);
    }
}
