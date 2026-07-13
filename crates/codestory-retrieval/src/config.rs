use anyhow::{Context, Result};
use directories::ProjectDirs;
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Qdrant container image pin for local dev and CI smoke.
pub const QDRANT_IMAGE_PIN: &str =
    "qdrant/qdrant:v1.12.5@sha256:05fecce7dce45d1254e0468bc037e8210e187fd56fa847688b012293d5f08aae";

/// llama.cpp server image for `COMPOSE_PROFILES=real` embed service (see `docker/retrieval-compose.yml`).
#[allow(dead_code)]
pub const LLAMACPP_SERVER_IMAGE_PIN: &str = "ghcr.io/ggml-org/llama.cpp:server@sha256:f16ca66f3ba316b7a7a16003ddfa88d29c3404fbe86550da086736864c11574c";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarImagePins {
    pub qdrant: String,
    pub embed: String,
}

pub fn default_sidecar_image_pins() -> SidecarImagePins {
    SidecarImagePins {
        qdrant: QDRANT_IMAGE_PIN.into(),
        embed: LLAMACPP_SERVER_IMAGE_PIN.into(),
    }
}

pub const DEFAULT_QDRANT_HTTP_PORT: u16 = 6333;
pub const DEFAULT_QDRANT_GRPC_PORT: u16 = 6334;
pub const DEFAULT_EMBED_HTTP_PORT: u16 = 8080;
pub const DEFAULT_AGENT_RUN_ID: &str = "shared-agent";
pub const NATIVE_LLAMA_MANAGED_CACHE_REL_PATH: &str =
    "managed-embeddings/llama/b9058/llama-b9058-bin-win-vulkan-x64/llama-server.exe";
pub const NATIVE_LLAMA_SOURCE_CACHE_REL_PATH: &str = "target/llamacpp/b8840/llama-server.exe";
const LLAMA_SIDECAR_BACKENDS_JSON: &str = include_str!("../assets/llama-sidecar-backends.json");
const TEST_HOST_PLATFORM_ENV: &str = "CODESTORY_TEST_HOST_PLATFORM";
const AGENT_PORT_LEASE_TTL: Duration = Duration::from_secs(10 * 60);
const AGENT_PORT_REGISTRY_SCHEMA_VERSION: u32 = 2;
#[cfg(test)]
const PORT_LEASE_ABORT_BASE_ENV: &str = "CODESTORY_TEST_PORT_LEASE_ABORT_BASE";
#[cfg(test)]
const PORT_LEASE_ABORT_SENTINEL_ENV: &str = "CODESTORY_TEST_PORT_LEASE_ABORT_SENTINEL";
const RUNTIME_ENV_KEYS: &[&str] = &[
    "CODESTORY_RETRIEVAL_PROFILE",
    "CODESTORY_SIDECAR_PROFILE",
    "CODESTORY_SIDECAR_RUN_ID",
    "CODESTORY_AGENT_RUN_ID",
    "CODESTORY_AGENT",
    "CODESTORY_AGENT_RUN",
    "CI",
    "GITHUB_ACTIONS",
    "CODESTORY_QDRANT_HTTP_PORT",
    "CODESTORY_QDRANT_GRPC_PORT",
    "CODESTORY_EMBED_PORT",
    "CODESTORY_EMBED_BACKEND",
    "CODESTORY_EMBED_RUNTIME_MODE",
    "CODESTORY_EMBED_LLAMACPP_URL",
    "CODESTORY_EMBED_PROFILE",
    "CODESTORY_EMBED_MODEL_ID",
    "CODESTORY_EMBED_POOLING",
    "CODESTORY_EMBED_QUERY_PREFIX",
    "CODESTORY_EMBED_DOCUMENT_PREFIX",
    "CODESTORY_EMBED_LAYER_NORM",
    "CODESTORY_EMBED_TRUNCATE_DIM",
    "CODESTORY_EMBED_EXPECTED_DIM",
    "CODESTORY_EMBED_LLAMACPP_BATCH_SIZE",
    "CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT",
    "CODESTORY_ALLOW_REMOTE_EMBEDDINGS",
    "CODESTORY_EMBED_DEVICE_POLICY",
    "CODESTORY_EMBED_ALLOW_CPU",
    "CODESTORY_EMBED_SERVER_LAUNCH",
    "CODESTORY_EMBED_ONNX_MODEL",
    "CODESTORY_EMBED_ONNX_TOKENIZER",
    "CODESTORY_EMBED_ONNX_PROVIDER",
    "CODESTORY_EMBED_ONNX_THREADS",
    "CODESTORY_EMBED_ONNX_BATCH_TOKENS",
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

pub const QDRANT_HEALTH_BUDGET: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmbeddingHostPlatform {
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LlamaSidecarBackendManifest {
    backends: Vec<LlamaSidecarBackend>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct LlamaSidecarBackend {
    pub id: String,
    pub os: String,
    pub arch: String,
    pub provider: String,
    pub launch_mode: String,
    pub artifact: String,
    pub url: String,
    pub sha256: String,
    pub executable_archive_path: String,
    pub executable_rel_path: String,
    pub executable_sha256: String,
    pub managed_cache_rel_dir: String,
    pub launch_args: Vec<String>,
    pub device_arg_policy: String,
    pub log_markers: Vec<String>,
}

pub(crate) fn embedding_host_platform() -> EmbeddingHostPlatform {
    if let Some((os, arch)) = std::env::var(TEST_HOST_PLATFORM_ENV)
        .ok()
        .and_then(|value| parse_test_host_platform(&value))
    {
        return EmbeddingHostPlatform { os, arch };
    }
    EmbeddingHostPlatform {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

fn parse_test_host_platform(value: &str) -> Option<(String, String)> {
    let mut parts = value.trim().split('/');
    let os = parts.next()?.trim().to_ascii_lowercase();
    let arch = parts.next()?.trim().to_ascii_lowercase();
    (!os.is_empty() && !arch.is_empty() && parts.next().is_none()).then_some((os, arch))
}

pub(crate) fn selected_llama_sidecar_backend(provider: &str) -> Option<LlamaSidecarBackend> {
    llama_sidecar_backends(provider).into_iter().next()
}

pub(crate) fn llama_sidecar_backends(provider: &str) -> Vec<LlamaSidecarBackend> {
    let platform = embedding_host_platform();
    llama_sidecar_backend_manifest()
        .backends
        .into_iter()
        .filter(|backend| {
            backend.os == platform.os
                && backend.arch == platform.arch
                && backend.provider == provider
        })
        .collect()
}

fn llama_sidecar_backend_manifest() -> LlamaSidecarBackendManifest {
    serde_json::from_str(LLAMA_SIDECAR_BACKENDS_JSON)
        .expect("embedded llama sidecar backend manifest must be valid")
}

#[derive(Debug, Clone)]
pub struct SidecarLayout {
    pub qdrant_http_port: u16,
    pub qdrant_grpc_port: u16,
    pub lexical_data_dir: PathBuf,
    pub qdrant_data_dir: PathBuf,
    pub scip_artifacts_root: PathBuf,
    pub state_file: PathBuf,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingServerLaunchMode {
    DockerComposeEmbed,
    NativeSpawned,
    ExternalEndpoint,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingEndpointOrigin {
    #[default]
    ManagedSidecar,
    ProcessEnvironment,
    TrustedUserConfig,
    TrustedProjectConfig,
    BuiltInDefault,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingRuntimeConfig {
    pub configuration_error: Option<String>,
    pub backend: String,
    pub endpoint: String,
    pub endpoint_origin: EmbeddingEndpointOrigin,
    pub profile: String,
    pub model_id: Option<String>,
    pub pooling: Option<String>,
    pub query_prefix: Option<String>,
    pub document_prefix: Option<String>,
    pub layer_norm: Option<bool>,
    pub truncate_dim: Option<usize>,
    pub expected_dim: Option<usize>,
    pub batch_size: usize,
    pub request_count: usize,
    pub allow_remote: bool,
    pub device_policy: String,
    pub server_launch: Option<String>,
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
    pub embedding_profile: Option<String>,
    pub embedding_model_id: Option<String>,
    pub embedding_endpoint: Option<String>,
    pub embedding_endpoint_origin: Option<EmbeddingEndpointOrigin>,
    pub embedding_query_prefix: Option<String>,
    pub embedding_document_prefix: Option<String>,
    pub hybrid_retrieval_enabled: Option<bool>,
    pub semantic_doc_scope: Option<String>,
    pub semantic_doc_alias_mode: Option<String>,
    pub summary_endpoint: Option<String>,
    pub summary_model: Option<String>,
}

impl EmbeddingServerLaunchMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DockerComposeEmbed => "docker_compose_embed",
            Self::NativeSpawned => "native_spawned",
            Self::ExternalEndpoint => "external_endpoint",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SidecarPorts {
    pub qdrant_http: u16,
    pub qdrant_grpc: u16,
    pub embed_http: u16,
    pub embed_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SidecarOwnership {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_identity: Option<codestory_workspace::ProjectIdentityV3>,
    pub owner: String,
    pub profile: String,
    pub namespace: String,
    pub compose_project: String,
    pub state_file: String,
    pub cleanup_command: String,
    pub ports: SidecarPorts,
    #[serde(default)]
    pub embedding_endpoint_origin: EmbeddingEndpointOrigin,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct SidecarRuntimeConfig {
    pub project_identity: Option<codestory_workspace::ProjectIdentityV3>,
    #[doc(hidden)]
    pub accepted_legacy_project_identity: Option<codestory_workspace::ProjectIdentityV3>,
    pub layout: SidecarLayout,
    pub profile: SidecarProfile,
    pub run_id: Option<String>,
    pub namespace: String,
    pub compose_project: String,
    pub embed_http_port: u16,
    pub cleanup_command: String,
    pub labels: BTreeMap<String, String>,
    pub embedding: EmbeddingRuntimeConfig,
    pub retrieval: RetrievalRuntimeConfig,
    pub summary: SummaryRuntimeConfig,
    #[doc(hidden)]
    pub port_lease_owner_id: Option<AgentPortLeaseOwnerToken>,
}

#[doc(hidden)]
#[derive(Clone, PartialEq, Eq)]
pub struct AgentPortLeaseOwnerToken {
    owner_id: String,
    allocated_ports: SidecarPorts,
}

impl std::fmt::Debug for AgentPortLeaseOwnerToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[redacted]")
    }
}

pub(crate) struct AgentPortLeaseHeartbeat {
    stop: Option<std::sync::mpsc::SyncSender<()>>,
    worker: Option<std::thread::JoinHandle<Result<()>>>,
}

impl AgentPortLeaseHeartbeat {
    pub(crate) fn finish(mut self) -> Result<()> {
        self.stop_worker();
        self.join_worker()
    }

    fn stop_worker(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.try_send(());
        }
    }

    fn join_worker(&mut self) -> Result<()> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("agent sidecar port lease heartbeat panicked"))?
    }
}

impl Drop for AgentPortLeaseHeartbeat {
    fn drop(&mut self) {
        self.stop_worker();
        let _ = self.join_worker();
    }
}

impl SidecarLayout {
    pub fn from_env() -> Self {
        Self::from_env_for_profile(None, SidecarProfile::Local)
    }

    pub fn from_env_for_project(project_root: &Path) -> Self {
        SidecarRuntimeConfig::for_project_auto(project_root).layout
    }

    fn from_env_for_profile(project_root: Option<&Path>, profile: SidecarProfile) -> Self {
        SidecarRuntimeConfig::for_project_profile(project_root, profile).layout
    }

    pub fn from_env_agent(project_root: &Path) -> Self {
        SidecarRuntimeConfig::for_project_profile(Some(project_root), SidecarProfile::Agent).layout
    }

    pub fn from_env_local(project_root: Option<&Path>) -> Self {
        Self::from_env_for_profile(project_root, SidecarProfile::Local)
    }

    pub fn embed_base_url(embed_http_port: u16) -> String {
        format!("http://127.0.0.1:{embed_http_port}/v1/embeddings")
    }
}

impl SidecarRuntimeConfig {
    pub fn local() -> Self {
        Self::for_project_profile(None, SidecarProfile::Local)
    }

    pub fn for_project_auto(project_root: &Path) -> Self {
        Self::for_project_auto_with_overrides(project_root, &SidecarRuntimeOverrides::default())
    }

    pub fn for_project_auto_with_overrides(
        project_root: &Path,
        overrides: &SidecarRuntimeOverrides,
    ) -> Self {
        Self::for_project_auto_with_defaults(
            project_root,
            &SidecarRuntimeDefaults::from_process_env(),
            overrides,
        )
    }

    pub fn for_project_auto_with_defaults(
        project_root: &Path,
        defaults: &SidecarRuntimeDefaults,
        overrides: &SidecarRuntimeOverrides,
    ) -> Self {
        Self::for_project_auto_with_defaults_in_cache(
            project_root,
            &user_cache_root(),
            defaults,
            overrides,
        )
    }

    #[doc(hidden)]
    pub fn for_project_auto_with_defaults_in_cache(
        project_root: &Path,
        cache_root: &Path,
        defaults: &SidecarRuntimeDefaults,
        overrides: &SidecarRuntimeOverrides,
    ) -> Self {
        let explicit_profile = env_profile(defaults);
        let env_run_id = env_agent_run_id(defaults);
        let latest_run_id = if explicit_profile.is_none() && env_run_id.is_none() {
            latest_agent_run_id_in_cache(project_root, cache_root)
        } else {
            None
        };
        let (profile, run_id) = auto_runtime_selection(
            explicit_profile,
            env_run_id,
            latest_run_id,
            running_in_ci_agent(defaults),
        );
        Self::for_project_profile_with_run_id_in_cache_defaults_and_overrides(
            Some(project_root),
            profile,
            run_id.as_deref(),
            cache_root,
            defaults,
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
        Self::for_project_profile_with_run_id_and_overrides(
            project_root,
            profile,
            run_id,
            &SidecarRuntimeOverrides::default(),
        )
    }

    pub fn for_project_profile_with_run_id_and_overrides(
        project_root: Option<&Path>,
        profile: SidecarProfile,
        run_id: Option<&str>,
        overrides: &SidecarRuntimeOverrides,
    ) -> Self {
        Self::for_project_profile_with_run_id_defaults_and_overrides(
            project_root,
            profile,
            run_id,
            &SidecarRuntimeDefaults::from_process_env(),
            overrides,
        )
    }

    fn for_project_profile_with_run_id_defaults_and_overrides(
        project_root: Option<&Path>,
        profile: SidecarProfile,
        run_id: Option<&str>,
        defaults: &SidecarRuntimeDefaults,
        overrides: &SidecarRuntimeOverrides,
    ) -> Self {
        Self::for_project_profile_with_run_id_in_cache_defaults_and_overrides(
            project_root,
            profile,
            run_id,
            &user_cache_root(),
            defaults,
            overrides,
        )
    }

    #[doc(hidden)]
    pub fn for_project_profile_with_run_id_in_cache(
        project_root: Option<&Path>,
        profile: SidecarProfile,
        run_id: Option<&str>,
        cache_root: &Path,
    ) -> Self {
        Self::for_project_profile_with_run_id_in_cache_defaults_and_overrides(
            project_root,
            profile,
            run_id,
            cache_root,
            &SidecarRuntimeDefaults::from_process_env(),
            &SidecarRuntimeOverrides::default(),
        )
    }

    #[doc(hidden)]
    pub fn for_project_profile_with_run_id_in_cache_and_overrides(
        project_root: Option<&Path>,
        profile: SidecarProfile,
        run_id: Option<&str>,
        cache_root: &Path,
        overrides: &SidecarRuntimeOverrides,
    ) -> Self {
        Self::for_project_profile_with_run_id_in_cache_defaults_and_overrides(
            project_root,
            profile,
            run_id,
            cache_root,
            &SidecarRuntimeDefaults::from_process_env(),
            overrides,
        )
    }

    fn for_project_profile_with_run_id_in_cache_defaults_and_overrides(
        project_root: Option<&Path>,
        profile: SidecarProfile,
        run_id: Option<&str>,
        cache_root: &Path,
        defaults: &SidecarRuntimeDefaults,
        overrides: &SidecarRuntimeOverrides,
    ) -> Self {
        let base = cache_root.to_path_buf();
        let run_id = (profile == SidecarProfile::Agent).then(|| agent_run_id(run_id, defaults));
        let project_identity = project_root.map(codestory_workspace::project_identity_v3);
        let namespace = namespace_for(project_identity.as_ref(), profile, run_id.as_deref());
        let state_file = match profile {
            SidecarProfile::Local => base.join("retrieval-sidecars.json"),
            SidecarProfile::Agent => base
                .join("sidecars")
                .join(&namespace)
                .join("retrieval-sidecars.json"),
        };
        let stored_value = read_sidecar_state_value(&state_file);
        let stored_value = stored_value.filter(|value| {
            sidecar_state_matches_runtime_selection(
                value,
                project_root,
                project_identity.as_ref(),
                profile,
                &namespace,
                run_id.as_deref(),
            )
        });
        let accepted_legacy_project_identity = stored_value
            .as_ref()
            .and_then(sidecar_state_project_identity)
            .filter(|identity| {
                identity.project_identity_schema_version
                    == codestory_workspace::PROJECT_IDENTITY_SCHEMA_VERSION
            });
        let stored = stored_value.as_ref().and_then(sidecar_ports_from_value);
        let stored_native_embedding = profile == SidecarProfile::Agent
            && stored_value
                .as_ref()
                .is_some_and(sidecar_state_uses_native_embedding);
        let configured_ports = [
            env_port(
                defaults,
                "CODESTORY_QDRANT_HTTP_PORT",
                DEFAULT_QDRANT_HTTP_PORT,
            ),
            env_port(
                defaults,
                "CODESTORY_QDRANT_GRPC_PORT",
                DEFAULT_QDRANT_GRPC_PORT,
            ),
            env_port(defaults, "CODESTORY_EMBED_PORT", DEFAULT_EMBED_HTTP_PORT),
        ];
        let requested_ports = std::array::from_fn(|index| {
            configured_ports[index].or_else(|| {
                if index == 2 && stored_native_embedding {
                    return None;
                }
                stored.as_ref().map(|ports| match index {
                    0 => ports.qdrant_http,
                    1 => ports.qdrant_grpc,
                    _ => ports.embed_http,
                })
            })
        });
        let agent_allocation = (profile == SidecarProfile::Agent)
            .then(|| dynamic_agent_ports(&base, &namespace, requested_ports));
        let dynamic_failed = agent_allocation
            .as_ref()
            .is_some_and(AgentPortAllocation::failed);
        let selected_port =
            |configured: Option<u16>, stored: Option<u16>, dynamic: Option<u16>, default| {
                if dynamic_failed {
                    0
                } else {
                    configured.or(stored).or(dynamic).unwrap_or(default)
                }
            };
        let qdrant_http_port = selected_port(
            configured_ports[0],
            stored.as_ref().map(|ports| ports.qdrant_http),
            agent_allocation
                .as_ref()
                .map(|allocation| allocation.ports.qdrant_http),
            DEFAULT_QDRANT_HTTP_PORT,
        );
        let qdrant_grpc_port = selected_port(
            configured_ports[1],
            stored.as_ref().map(|ports| ports.qdrant_grpc),
            agent_allocation
                .as_ref()
                .map(|allocation| allocation.ports.qdrant_grpc),
            DEFAULT_QDRANT_GRPC_PORT,
        );
        let embed_http_port = selected_port(
            configured_ports[2],
            (!stored_native_embedding)
                .then(|| stored.as_ref().map(|ports| ports.embed_http))
                .flatten(),
            agent_allocation
                .as_ref()
                .map(|allocation| allocation.ports.embed_http),
            DEFAULT_EMBED_HTTP_PORT,
        );
        let root = match profile {
            SidecarProfile::Local => base.clone(),
            SidecarProfile::Agent => base.join("sidecars").join(&namespace),
        };
        let layout = SidecarLayout {
            qdrant_http_port,
            qdrant_grpc_port,
            lexical_data_dir: root.join("lexical"),
            qdrant_data_dir: root.join("qdrant"),
            scip_artifacts_root: root.join("scip"),
            state_file: state_file.clone(),
        };
        let cleanup_command = project_root
            .map(|path| retrieval_command("down", path, profile, run_id.as_deref(), None))
            .unwrap_or_else(|| "codestory-cli retrieval down".to_string());
        let mut labels = BTreeMap::new();
        labels.insert("dev.codestory.owner".into(), "codestory".into());
        labels.insert("dev.codestory.profile".into(), profile.as_str().into());
        labels.insert("dev.codestory.namespace".into(), namespace.clone());
        if let Some(project_root) = project_root {
            if let Some(identity) = project_identity.as_ref() {
                labels.insert(
                    "dev.codestory.project_hash".into(),
                    identity.workspace_id.clone(),
                );
                labels.insert(
                    "dev.codestory.project_id".into(),
                    identity.project_id.clone(),
                );
                labels.insert(
                    "dev.codestory.workspace_id".into(),
                    identity.workspace_id.clone(),
                );
                labels.insert(
                    "dev.codestory.artifact_scope_id".into(),
                    identity.artifact_scope_id.clone(),
                );
                labels.insert(
                    "dev.codestory.project_identity_schema_version".into(),
                    identity.project_identity_schema_version.to_string(),
                );
            }
            labels.insert(
                "dev.codestory.workspace_root".into(),
                project_root.to_string_lossy().to_string(),
            );
        }
        if let Some(run_id) = run_id.as_deref() {
            labels.insert("dev.codestory.run_id".into(), run_id.to_string());
            labels.insert("dev.codestory.agent_id".into(), run_id.to_string());
        }
        let mut embedding = embedding_runtime_config(embed_http_port, defaults, overrides);
        if embedding.endpoint_origin == EmbeddingEndpointOrigin::ManagedSidecar
            && let Some(stored_endpoint) = stored
                .as_ref()
                .map(|ports| ports.embed_url.as_str())
                .filter(|endpoint| local_embedding_endpoint_port(endpoint).is_some())
        {
            embedding.endpoint = stored_endpoint.to_string();
        }
        let retrieval = retrieval_runtime_config(defaults, overrides);
        let summary = summary_runtime_config(defaults, overrides);
        let port_lease_owner_id = agent_allocation
            .as_ref()
            .filter(|allocation| !allocation.failed())
            .map(|allocation| AgentPortLeaseOwnerToken {
                owner_id: allocation.owner_id.clone(),
                allocated_ports: allocation.ports.clone(),
            });
        Self {
            project_identity,
            accepted_legacy_project_identity,
            layout,
            profile,
            run_id,
            namespace: namespace.clone(),
            compose_project: namespace,
            embed_http_port,
            cleanup_command,
            labels,
            embedding,
            retrieval,
            summary,
            port_lease_owner_id,
        }
    }

    pub fn ownership(&self) -> SidecarOwnership {
        SidecarOwnership {
            project_identity: self.project_identity.clone(),
            owner: "codestory".into(),
            profile: self.profile.as_str().into(),
            namespace: self.namespace.clone(),
            compose_project: self.compose_project.clone(),
            state_file: self.layout.state_file.display().to_string(),
            cleanup_command: self.cleanup_command.clone(),
            ports: SidecarPorts {
                qdrant_http: self.layout.qdrant_http_port,
                qdrant_grpc: self.layout.qdrant_grpc_port,
                embed_http: local_embedding_endpoint_port(&self.embedding.endpoint)
                    .unwrap_or(self.embed_http_port),
                embed_url: redacted_embedding_endpoint(&self.embedding.endpoint),
            },
            embedding_endpoint_origin: self.embedding.endpoint_origin,
            labels: self.labels.clone(),
        }
    }

    pub(crate) fn ensure_ports_allocated(&self) -> Result<()> {
        if self.profile == SidecarProfile::Agent
            && [
                self.layout.qdrant_http_port,
                self.layout.qdrant_grpc_port,
                self.embed_http_port,
            ]
            .contains(&0)
        {
            anyhow::bail!(
                "agent sidecar port allocation is unavailable; inspect sidecars/port-allocations.json and retry"
            );
        }
        if self.profile == SidecarProfile::Agent {
            let owner_id = self
                .port_lease_owner_id
                .as_ref()
                .map(|token| token.owner_id.as_str())
                .context("agent sidecar runtime has no port lease owner token")?;
            let cache_root = self
                .layout
                .state_file
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .context("agent sidecar state path has no cache root")?;
            renew_agent_port_lease(
                cache_root,
                &self.namespace,
                owner_id,
                &self
                    .port_lease_owner_id
                    .as_ref()
                    .expect("agent lease token checked above")
                    .allocated_ports,
            )?;
        }
        Ok(())
    }

    pub(crate) fn start_port_lease_heartbeat(&self) -> Result<AgentPortLeaseHeartbeat> {
        self.start_port_lease_heartbeat_with_interval(AGENT_PORT_LEASE_TTL / 3)
    }

    fn start_port_lease_heartbeat_with_interval(
        &self,
        interval: Duration,
    ) -> Result<AgentPortLeaseHeartbeat> {
        self.ensure_ports_allocated()?;
        if self.profile != SidecarProfile::Agent {
            return Ok(AgentPortLeaseHeartbeat {
                stop: None,
                worker: None,
            });
        }
        let base = self
            .layout
            .state_file
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .context("agent sidecar state path has no cache root")?
            .to_path_buf();
        let namespace = self.namespace.clone();
        let owner_id = self
            .port_lease_owner_id
            .as_ref()
            .map(|token| token.owner_id.clone())
            .context("agent sidecar runtime has no port lease owner token")?;
        let ports = self
            .port_lease_owner_id
            .as_ref()
            .expect("agent lease token checked above")
            .allocated_ports
            .clone();
        let (stop, stopped) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("codestory-port-lease-heartbeat".into())
            .spawn(move || {
                loop {
                    match stopped.recv_timeout(interval) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            return Ok(());
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            renew_agent_port_lease(&base, &namespace, &owner_id, &ports)?;
                        }
                    }
                }
            })
            .context("spawn agent sidecar port lease heartbeat")?;
        Ok(AgentPortLeaseHeartbeat {
            stop: Some(stop),
            worker: Some(worker),
        })
    }

    pub fn with_profile_and_run_id(
        &self,
        project_root: Option<&Path>,
        profile: SidecarProfile,
        run_id: Option<&str>,
    ) -> Self {
        let cache_root = match self.profile {
            SidecarProfile::Local => self.layout.state_file.parent(),
            SidecarProfile::Agent => self
                .layout
                .state_file
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent),
        }
        .unwrap_or_else(|| Path::new("."));
        let mut selected = Self::for_project_profile_with_run_id_in_cache_defaults_and_overrides(
            project_root,
            profile,
            run_id,
            cache_root,
            &SidecarRuntimeDefaults::default(),
            &SidecarRuntimeOverrides::default(),
        );
        let selected_managed_endpoint = selected.embedding.endpoint.clone();
        selected.embedding = self.embedding.clone();
        if selected.embedding.endpoint_origin == EmbeddingEndpointOrigin::ManagedSidecar {
            selected.embedding.endpoint = selected_managed_endpoint;
        }
        selected.retrieval = self.retrieval.clone();
        selected.summary = self.summary.clone();
        selected
    }

    /// Retarget the managed embedding endpoint after the broker has established
    /// ownership. A new launch uses the profile's allocated port; a reused
    /// launch uses the already-running process port. The agent's provisional
    /// tuple remains isolated in its registry lease when those ports differ.
    #[doc(hidden)]
    pub fn use_broker_verified_native_embedding_endpoint(&mut self, port: u16) -> Result<()> {
        if port == 0 {
            anyhow::bail!("broker-verified native embedding port cannot be 0");
        }
        if self.profile == SidecarProfile::Agent && self.port_lease_owner_id.is_none() {
            anyhow::bail!("agent sidecar runtime has no port lease owner token");
        }
        self.embedding.endpoint = SidecarLayout::embed_base_url(port);
        Ok(())
    }

    /// Revalidate an agent's provisional native-embedding port immediately before a
    /// broker-owned launch. Allocation and any rotation happen under the registry lock,
    /// closing the gap where another process binds the port after runtime construction.
    #[doc(hidden)]
    pub fn revalidate_broker_native_embedding_port(&mut self) -> Result<()> {
        if self.profile != SidecarProfile::Agent {
            return self.use_broker_verified_native_embedding_endpoint(self.embed_http_port);
        }
        let base = self
            .layout
            .state_file
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .context("agent sidecar state path has no cache root")?
            .to_path_buf();
        let token = self
            .port_lease_owner_id
            .as_ref()
            .context("agent sidecar runtime has no port lease owner token")?;
        let ports = revalidate_agent_embedding_port(
            &base,
            &self.namespace,
            &token.owner_id,
            &token.allocated_ports,
            false,
        )?;
        self.embed_http_port = ports.embed_http;
        self.port_lease_owner_id
            .as_mut()
            .expect("agent port lease token checked above")
            .allocated_ports = ports.clone();
        self.use_broker_verified_native_embedding_endpoint(ports.embed_http)
    }

    /// Select a different managed native-embedding port after the launched server
    /// proves that another process won the bind race. Agent rotations update the
    /// durable lease while holding the registry lock; local rotations remain
    /// process-local because local runtimes have no port registry.
    #[doc(hidden)]
    pub fn rotate_broker_native_embedding_port(&mut self) -> Result<()> {
        if self.profile != SidecarProfile::Agent {
            let previous = self.embed_http_port;
            let mut replacement = free_local_port();
            for _ in 0..10 {
                if replacement != 0 && replacement != previous {
                    break;
                }
                replacement = free_local_port();
            }
            if replacement == 0 || replacement == previous {
                anyhow::bail!("native embedding port rotation is unavailable");
            }
            self.embed_http_port = replacement;
            return self.use_broker_verified_native_embedding_endpoint(replacement);
        }
        let base = self
            .layout
            .state_file
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .context("agent sidecar state path has no cache root")?
            .to_path_buf();
        let token = self
            .port_lease_owner_id
            .as_ref()
            .context("agent sidecar runtime has no port lease owner token")?;
        let ports = revalidate_agent_embedding_port(
            &base,
            &self.namespace,
            &token.owner_id,
            &token.allocated_ports,
            true,
        )?;
        self.embed_http_port = ports.embed_http;
        self.port_lease_owner_id
            .as_mut()
            .expect("agent port lease token checked above")
            .allocated_ports = ports.clone();
        self.use_broker_verified_native_embedding_endpoint(ports.embed_http)
    }
}

impl SidecarLayout {
    pub fn qdrant_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.qdrant_http_port)
    }

    pub fn scip_project_dir(&self, project_id: &str) -> PathBuf {
        self.scip_artifacts_root.join(project_id)
    }

    pub fn ensure_data_dirs(&self) -> Result<()> {
        for dir in [
            &self.lexical_data_dir,
            &self.qdrant_data_dir,
            &self.scip_artifacts_root,
        ] {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("create sidecar data dir {}", dir.display()))?;
        }
        Ok(())
    }
}

pub fn sidecar_runtime_for_project(
    project_root: &Path,
    profile: SidecarProfile,
) -> SidecarRuntimeConfig {
    SidecarRuntimeConfig::for_project_profile(Some(project_root), profile)
}

pub fn sidecar_runtime_for_project_with_run_id(
    project_root: &Path,
    profile: SidecarProfile,
    run_id: Option<&str>,
) -> SidecarRuntimeConfig {
    SidecarRuntimeConfig::for_project_profile_with_run_id(Some(project_root), profile, run_id)
}

pub fn sidecar_runtime_auto(project_root: &Path) -> SidecarRuntimeConfig {
    SidecarRuntimeConfig::for_project_auto(project_root)
}

fn embedding_runtime_config(
    embed_http_port: u16,
    defaults: &SidecarRuntimeDefaults,
    overrides: &SidecarRuntimeOverrides,
) -> EmbeddingRuntimeConfig {
    const REMOVED_ONNX_ENV_VARS: &[&str] = &[
        "CODESTORY_EMBED_ONNX_MODEL",
        "CODESTORY_EMBED_ONNX_TOKENIZER",
        "CODESTORY_EMBED_ONNX_PROVIDER",
        "CODESTORY_EMBED_ONNX_THREADS",
        "CODESTORY_EMBED_ONNX_BATCH_TOKENS",
    ];
    let configured_backend = default_nonempty(defaults, "CODESTORY_EMBED_BACKEND")
        .or_else(|| default_nonempty(defaults, "CODESTORY_EMBED_RUNTIME_MODE"))
        .unwrap_or_else(|| "llamacpp".to_string());
    let configuration_error = REMOVED_ONNX_ENV_VARS
        .iter()
        .find(|name| defaults.get(name).is_some())
        .map(|name| {
            format!(
                "{name} is no longer supported; CodeStory retrieval requires the llama.cpp sidecar"
            )
        })
        .or_else(|| {
            matches!(
                configured_backend.trim().to_ascii_lowercase().as_str(),
                "onnx" | "ort" | "onnxruntime" | "onnx-runtime"
            )
            .then(|| {
                format!(
                    "embedding backend `{configured_backend}` is no longer supported; CodeStory retrieval requires the llama.cpp sidecar"
                )
            })
        });
    let env_endpoint = default_nonempty(defaults, "CODESTORY_EMBED_LLAMACPP_URL");
    let (mut endpoint, mut endpoint_origin) = if let Some(endpoint) = env_endpoint {
        (endpoint, EmbeddingEndpointOrigin::ProcessEnvironment)
    } else if let Some(endpoint) = overrides.embedding_endpoint.clone() {
        (
            endpoint,
            overrides
                .embedding_endpoint_origin
                .unwrap_or(EmbeddingEndpointOrigin::TrustedUserConfig),
        )
    } else {
        (
            SidecarLayout::embed_base_url(embed_http_port),
            EmbeddingEndpointOrigin::ManagedSidecar,
        )
    };
    let mut server_launch = default_nonempty(defaults, "CODESTORY_EMBED_SERVER_LAUNCH");
    if endpoint_origin != EmbeddingEndpointOrigin::ManagedSidecar {
        let native_selected = server_launch.as_deref().is_some_and(|mode| {
            matches!(
                mode.trim().to_ascii_lowercase().as_str(),
                "native" | "native_spawned"
            )
        });
        if native_selected {
            endpoint = SidecarLayout::embed_base_url(embed_http_port);
            endpoint_origin = EmbeddingEndpointOrigin::ManagedSidecar;
        } else {
            server_launch = Some("external_endpoint".to_string());
        }
    }
    EmbeddingRuntimeConfig {
        configuration_error,
        backend: configured_backend,
        endpoint,
        endpoint_origin,
        profile: default_nonempty(defaults, "CODESTORY_EMBED_PROFILE")
            .or_else(|| overrides.embedding_profile.clone())
            .unwrap_or_else(|| "bge-base-en-v1.5".to_string()),
        model_id: default_nonempty(defaults, "CODESTORY_EMBED_MODEL_ID")
            .or_else(|| overrides.embedding_model_id.clone()),
        pooling: default_nonempty(defaults, "CODESTORY_EMBED_POOLING"),
        query_prefix: defaults
            .get("CODESTORY_EMBED_QUERY_PREFIX")
            .map(str::to_string)
            .or_else(|| overrides.embedding_query_prefix.clone()),
        document_prefix: defaults
            .get("CODESTORY_EMBED_DOCUMENT_PREFIX")
            .map(str::to_string)
            .or_else(|| overrides.embedding_document_prefix.clone()),
        layer_norm: default_optional_bool(defaults, "CODESTORY_EMBED_LAYER_NORM"),
        truncate_dim: default_bounded_usize(defaults, "CODESTORY_EMBED_TRUNCATE_DIM", 1, 8192),
        expected_dim: default_bounded_usize(defaults, "CODESTORY_EMBED_EXPECTED_DIM", 1, 8192),
        batch_size: default_bounded_usize(defaults, "CODESTORY_EMBED_LLAMACPP_BATCH_SIZE", 1, 1024)
            .unwrap_or(128),
        request_count: default_bounded_usize(
            defaults,
            "CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT",
            1,
            16,
        )
        .unwrap_or_else(|| {
            defaults
                .get("CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT")
                .filter(|value| {
                    matches!(
                        value.trim().to_ascii_lowercase().as_str(),
                        "auto" | "available_parallelism"
                    )
                })
                .and_then(|_| std::thread::available_parallelism().ok())
                .map(|value| value.get().clamp(1, 16))
                .unwrap_or(1)
        }),
        allow_remote: default_flag(defaults, "CODESTORY_ALLOW_REMOTE_EMBEDDINGS", false),
        device_policy: if default_flag(defaults, "CODESTORY_EMBED_ALLOW_CPU", false) {
            "allow_cpu".to_string()
        } else {
            default_nonempty(defaults, "CODESTORY_EMBED_DEVICE_POLICY")
                .unwrap_or_else(|| "accelerator_required".to_string())
        },
        server_launch,
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
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
        .or_else(|| defaults.get("CODESTORY_SIDECAR_PROFILE"))
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "agent" | "ci" => Some(SidecarProfile::Agent),
            "local" | "dev" => Some(SidecarProfile::Local),
            _ => None,
        })
}

fn env_agent_run_id(defaults: &SidecarRuntimeDefaults) -> Option<String> {
    defaults
        .get("CODESTORY_SIDECAR_RUN_ID")
        .or_else(|| defaults.get("CODESTORY_AGENT_RUN_ID"))
        .and_then(normalized_label_component)
}

fn running_in_ci_agent(defaults: &SidecarRuntimeDefaults) -> bool {
    default_flag(defaults, "CODESTORY_AGENT", false)
        || default_flag(defaults, "CODESTORY_AGENT_RUN", false)
        || default_flag(defaults, "CI", false)
        || default_flag(defaults, "GITHUB_ACTIONS", false)
}

fn auto_runtime_selection(
    explicit_profile: Option<SidecarProfile>,
    env_run_id: Option<String>,
    latest_run_id: Option<String>,
    ci_agent: bool,
) -> (SidecarProfile, Option<String>) {
    match explicit_profile {
        Some(SidecarProfile::Local) => (SidecarProfile::Local, None),
        Some(SidecarProfile::Agent) => (SidecarProfile::Agent, env_run_id.or(latest_run_id)),
        None if env_run_id.is_some() || latest_run_id.is_some() || ci_agent => {
            (SidecarProfile::Agent, env_run_id.or(latest_run_id))
        }
        None => (SidecarProfile::Local, None),
    }
}

fn namespace_for(
    project_identity: Option<&codestory_workspace::ProjectIdentityV3>,
    profile: SidecarProfile,
    run_id: Option<&str>,
) -> String {
    match (profile, project_identity) {
        (SidecarProfile::Local, _) => "codestory".into(),
        (SidecarProfile::Agent, Some(identity)) => {
            format!(
                "codestory-agent-{}-{}",
                identity.workspace_id,
                run_id.unwrap_or("run")
            )
        }
        (SidecarProfile::Agent, None) => format!(
            "codestory-agent-{}-{}",
            std::process::id(),
            run_id.unwrap_or("run")
        ),
    }
}

fn agent_namespace_prefix(project_root: &Path) -> String {
    format!(
        "codestory-agent-{}-",
        codestory_workspace::workspace_id_v3_for_root(project_root)
    )
}

fn latest_agent_run_id_in_cache(project_root: &Path, cache_root: &Path) -> Option<String> {
    let prefix = agent_namespace_prefix(project_root);
    let sidecars_root = cache_root.join("sidecars");
    let entries = std::fs::read_dir(sidecars_root).ok()?;
    let mut newest: Option<(std::time::SystemTime, String)> = None;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let namespace = entry.file_name().to_string_lossy().to_string();
        let Some(run_id) = namespace
            .strip_prefix(&prefix)
            .and_then(normalized_label_component)
        else {
            continue;
        };
        let state_file = entry.path().join("retrieval-sidecars.json");
        if !state_file.is_file() {
            continue;
        }
        let modified = state_file
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if newest
            .as_ref()
            .is_none_or(|(current, _)| modified > *current)
        {
            newest = Some((modified, run_id));
        }
    }
    newest.map(|(_, run_id)| run_id)
}

fn agent_run_id(explicit: Option<&str>, defaults: &SidecarRuntimeDefaults) -> String {
    explicit
        .and_then(normalized_label_component)
        .or_else(|| env_agent_run_id(defaults))
        .unwrap_or_else(default_agent_run_id)
}

pub(crate) fn redacted_embedding_endpoint(endpoint: &str) -> String {
    let Some((scheme, rest)) = endpoint.split_once("://") else {
        return "[invalid embedding endpoint]".to_string();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let path = &rest[authority_end..];
    let path_end = path.find(['?', '#']).unwrap_or(path.len());
    format!("{scheme}://{host}{}", &path[..path_end])
}

fn default_agent_run_id() -> String {
    DEFAULT_AGENT_RUN_ID.to_string()
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

fn read_sidecar_state_value(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn sidecar_state_project_identity(
    value: &serde_json::Value,
) -> Option<codestory_workspace::ProjectIdentityV3> {
    serde_json::from_value(value.get("project_identity")?.clone()).ok()
}

pub(crate) fn project_identity_matches_runtime(
    stored: Option<&codestory_workspace::ProjectIdentityV3>,
    current: Option<&codestory_workspace::ProjectIdentityV3>,
    accepted_legacy: Option<&codestory_workspace::ProjectIdentityV3>,
) -> bool {
    match (stored, current) {
        (None, None) => true,
        (Some(stored), Some(current))
            if stored.project_identity_schema_version
                == codestory_workspace::PROJECT_IDENTITY_V3_SCHEMA_VERSION =>
        {
            stored == current
        }
        (Some(stored), Some(_))
            if stored.project_identity_schema_version
                == codestory_workspace::PROJECT_IDENTITY_SCHEMA_VERSION =>
        {
            accepted_legacy == Some(stored)
        }
        _ => false,
    }
}

fn sidecar_state_matches_runtime_selection(
    value: &serde_json::Value,
    project_root: Option<&Path>,
    project_identity: Option<&codestory_workspace::ProjectIdentityV3>,
    profile: SidecarProfile,
    namespace: &str,
    run_id: Option<&str>,
) -> bool {
    value.get("owner").and_then(serde_json::Value::as_str) == Some("codestory")
        && value.get("profile").and_then(serde_json::Value::as_str) == Some(profile.as_str())
        && value.get("namespace").and_then(serde_json::Value::as_str) == Some(namespace)
        && value
            .get("compose_project")
            .and_then(serde_json::Value::as_str)
            == Some(namespace)
        && value.get("run_id").and_then(serde_json::Value::as_str) == run_id
        && project_identity_matches_runtime_selection(
            sidecar_state_project_identity(value).as_ref(),
            project_root,
            project_identity,
        )
}

fn project_identity_matches_runtime_selection(
    stored: Option<&codestory_workspace::ProjectIdentityV3>,
    project_root: Option<&Path>,
    current: Option<&codestory_workspace::ProjectIdentityV3>,
) -> bool {
    let Some(stored) = stored else {
        return current.is_none();
    };
    if stored.project_identity_schema_version
        != codestory_workspace::PROJECT_IDENTITY_SCHEMA_VERSION
    {
        return project_identity_matches_runtime(Some(stored), current, None);
    }
    let Some(legacy) = project_root.map(codestory_workspace::project_identity_v2) else {
        return false;
    };
    stored.project_id == legacy.project_id
        && stored.workspace_id == legacy.workspace_id
        && stored.artifact_scope_id == legacy.artifact_scope_id
        && stored.canonical_repository_id == legacy.canonical_repository_id
        && stored.legacy_canonical_repository_id.is_none()
        && stored.legacy_raw_root_project_id == legacy.legacy_raw_root_project_id
        && stored.normalized_root_project_id_alias == legacy.normalized_root_project_id_alias
        && stored.portable_reuse_eligible == legacy.portable_reuse_eligible
}

fn sidecar_state_uses_native_embedding(value: &serde_json::Value) -> bool {
    value
        .get("embedding_launch")
        .and_then(|launch| launch.get("launch_mode"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|mode| mode == EmbeddingServerLaunchMode::NativeSpawned.as_str())
}

fn local_embedding_endpoint_port(endpoint: &str) -> Option<u16> {
    endpoint
        .strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.strip_suffix("/v1/embeddings"))
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
}

fn sidecar_ports_from_value(value: &serde_json::Value) -> Option<SidecarPorts> {
    Some(SidecarPorts {
        qdrant_http: value.get("qdrant_http_port")?.as_u64()?.try_into().ok()?,
        qdrant_grpc: value.get("qdrant_grpc_port")?.as_u64()?.try_into().ok()?,
        embed_http: value
            .get("embed_http_port")
            .and_then(|value| value.as_u64())
            .and_then(|value| value.try_into().ok())
            .unwrap_or(DEFAULT_EMBED_HTTP_PORT),
        embed_url: value
            .get("embed_url")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| SidecarLayout::embed_base_url(DEFAULT_EMBED_HTTP_PORT)),
    })
}

fn dynamic_agent_ports(
    base: &Path,
    namespace: &str,
    configured: [Option<u16>; 3],
) -> AgentPortAllocation {
    allocate_agent_ports_in_registry(base, namespace, configured).unwrap_or_else(|error| {
        eprintln!(
            "CodeStory sidecar port allocation failed closed: namespace={namespace} error={error:#}"
        );
        AgentPortAllocation {
            ports: SidecarPorts {
                qdrant_http: 0,
                qdrant_grpc: 0,
                embed_http: 0,
                embed_url: SidecarLayout::embed_base_url(0),
            },
            owner_id: String::new(),
        }
    })
}

#[derive(Debug, Clone)]
struct AgentPortAllocation {
    ports: SidecarPorts,
    owner_id: String,
}

impl AgentPortAllocation {
    fn failed(&self) -> bool {
        self.owner_id.is_empty()
            || [
                self.ports.qdrant_http,
                self.ports.qdrant_grpc,
                self.ports.embed_http,
            ]
            .contains(&0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AgentPortOwner {
    id: String,
    process_id: u32,
    created_at_epoch_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AgentPortLease {
    namespace: String,
    owner: AgentPortOwner,
    acquired_at_epoch_ms: i64,
    renewed_at_epoch_ms: i64,
    expires_at_epoch_ms: i64,
    ports: SidecarPorts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AgentPortRegistry {
    schema_version: u32,
    leases: BTreeMap<String, AgentPortLease>,
}

impl Default for AgentPortRegistry {
    fn default() -> Self {
        Self {
            schema_version: AGENT_PORT_REGISTRY_SCHEMA_VERSION,
            leases: BTreeMap::new(),
        }
    }
}

fn allocate_agent_ports_in_registry(
    base: &Path,
    namespace: &str,
    configured: [Option<u16>; 3],
) -> Result<AgentPortAllocation> {
    allocate_agent_ports_in_registry_at(base, namespace, configured, now_epoch_ms())
}

fn allocate_agent_ports_in_registry_at(
    base: &Path,
    namespace: &str,
    configured: [Option<u16>; 3],
    now: i64,
) -> Result<AgentPortAllocation> {
    if !is_agent_namespace_path_component(namespace) {
        anyhow::bail!("agent namespace is not a safe path component");
    }
    let root = base.join("sidecars");
    std::fs::create_dir_all(&root)
        .with_context(|| format!("create sidecar port registry dir {}", root.display()))?;
    let lock_path = root.join("port-allocations.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open sidecar port allocation lock {}", lock_path.display()))?;
    FileExt::lock_exclusive(&lock)
        .with_context(|| format!("take sidecar port allocation lock {}", lock_path.display()))?;

    let registry_path = root.join("port-allocations.json");
    let mut registry = read_agent_port_registry(&root, &registry_path)?;
    let cleanup = prune_agent_port_registry(&root, namespace, &mut registry, now);
    let existing = registry.leases.get(namespace).cloned();
    let state_ports = if existing.is_none() {
        owned_sidecar_state_ports(&root, namespace)?
    } else {
        None
    };
    let mut reserved = reserved_registry_ports_excluding(&registry.leases, namespace);
    let qdrant_http = select_agent_port(
        configured[0],
        existing.as_ref().map(|lease| lease.ports.qdrant_http),
        namespace,
        "qdrant-http",
        &mut reserved,
        state_ports
            .as_ref()
            .is_some_and(|ports| Some(ports.qdrant_http) == configured[0]),
    )?;
    let qdrant_grpc = select_agent_port(
        configured[1],
        existing.as_ref().map(|lease| lease.ports.qdrant_grpc),
        namespace,
        "qdrant-grpc",
        &mut reserved,
        state_ports
            .as_ref()
            .is_some_and(|ports| Some(ports.qdrant_grpc) == configured[1]),
    )?;
    let embed_http = select_agent_port(
        configured[2],
        existing.as_ref().map(|lease| lease.ports.embed_http),
        namespace,
        "embed",
        &mut reserved,
        state_ports
            .as_ref()
            .is_some_and(|ports| Some(ports.embed_http) == configured[2]),
    )?;
    let ports = SidecarPorts {
        qdrant_http,
        qdrant_grpc,
        embed_http,
        embed_url: SidecarLayout::embed_base_url(embed_http),
    };
    let owner = match existing.as_ref() {
        Some(lease) if lease.ports != ports => {
            if lease_is_live(&root, lease, now)? {
                anyhow::bail!("agent sidecar namespace {namespace} already has a live port lease");
            }
            new_agent_port_owner(now)
        }
        Some(lease) => match read_agent_port_owner(&root, namespace)? {
            Some(owner) if owner.id == lease.owner.id => owner,
            Some(_) | None if sidecar_state_owns_ports(&root, namespace, &lease.ports)? => {
                new_agent_port_owner(now)
            }
            Some(_) | None if sidecar_ports_are_bound(&lease.ports) => {
                anyhow::bail!(
                    "agent sidecar namespace {namespace} has bound ports without matching lease ownership"
                );
            }
            Some(_) | None => new_agent_port_owner(now),
        },
        None => new_agent_port_owner(now),
    };
    let continued_lease = existing
        .as_ref()
        .filter(|lease| lease.owner.id == owner.id && lease.ports == ports);
    let acquired_at_epoch_ms = continued_lease.map_or(now, |lease| lease.acquired_at_epoch_ms);
    let renewed_at_epoch_ms = match existing.as_ref() {
        Some(lease) => next_lease_renewal_epoch_ms(now, lease.renewed_at_epoch_ms)?,
        None => now,
    };
    let lease = AgentPortLease {
        namespace: namespace.to_string(),
        owner: owner.clone(),
        acquired_at_epoch_ms,
        renewed_at_epoch_ms,
        expires_at_epoch_ms: lease_expiry(renewed_at_epoch_ms),
        ports: ports.clone(),
    };
    write_agent_port_owner(&root, namespace, &owner)?;
    write_agent_port_lease(&root, &lease)?;
    #[cfg(test)]
    if std::env::var_os(PORT_LEASE_ABORT_BASE_ENV).as_deref() == Some(base.as_os_str()) {
        let sentinel = std::env::var_os(PORT_LEASE_ABORT_SENTINEL_ENV)
            .context("port lease abort sentinel is missing")?;
        std::fs::write(sentinel, b"lease-persisted")?;
        std::process::abort();
    }
    registry.leases.insert(namespace.to_string(), lease);
    write_agent_port_registry(&registry_path, &registry)?;
    if let Err(error) =
        remove_file_if_present(&legacy_agent_port_reservation_path(&root, namespace))
    {
        eprintln!(
            "CodeStory sidecar port cleanup warning: namespace={namespace} failed to remove legacy reservation: {error}"
        );
    }
    if cleanup.pruned > 0 || cleanup.failures > 0 {
        eprintln!(
            "CodeStory sidecar port cleanup: pruned={} retained={} failures={}",
            cleanup.pruned, cleanup.retained, cleanup.failures
        );
        for detail in &cleanup.failure_details {
            eprintln!("CodeStory sidecar port cleanup warning: {detail}");
        }
        if cleanup.failures > cleanup.failure_details.len() {
            eprintln!(
                "CodeStory sidecar port cleanup warning: {} additional failures omitted",
                cleanup.failures - cleanup.failure_details.len()
            );
        }
    }
    Ok(AgentPortAllocation {
        ports,
        owner_id: owner.id,
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AgentPortRegistryCleanup {
    pruned: usize,
    retained: usize,
    failures: usize,
    failure_details: Vec<String>,
}

fn new_agent_port_owner(now: i64) -> AgentPortOwner {
    AgentPortOwner {
        id: uuid::Uuid::new_v4().to_string(),
        process_id: std::process::id(),
        created_at_epoch_ms: now,
    }
}

fn lease_expiry(now: i64) -> i64 {
    now.saturating_add(AGENT_PORT_LEASE_TTL.as_millis() as i64)
}

fn next_lease_renewal_epoch_ms(now: i64, previous: i64) -> Result<i64> {
    let after_previous = previous
        .checked_add(1)
        .context("agent sidecar port lease renewal timestamp overflowed")?;
    Ok(now.max(after_previous))
}

fn write_agent_port_owner(root: &Path, namespace: &str, owner: &AgentPortOwner) -> Result<()> {
    std::fs::create_dir_all(root.join(namespace))
        .with_context(|| format!("create agent sidecar namespace {namespace}"))?;
    let bytes = serde_json::to_vec_pretty(owner)?;
    codestory_workspace::atomic_file::write_bytes_atomic(
        &agent_port_owner_path(root, namespace),
        "agent-port-owner",
        &bytes,
    )
    .with_context(|| format!("write agent port owner for {namespace}"))
}

fn read_agent_port_owner(root: &Path, namespace: &str) -> Result<Option<AgentPortOwner>> {
    let path = agent_port_owner_path(root, namespace);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let owner: AgentPortOwner =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(owner))
}

fn write_agent_port_lease(root: &Path, lease: &AgentPortLease) -> Result<()> {
    let path = agent_port_lease_path(root, &lease.namespace);
    std::fs::create_dir_all(path.parent().context("agent port lease has no parent")?)
        .with_context(|| format!("create sidecar port lease dir for {}", lease.namespace))?;
    let bytes = serde_json::to_vec_pretty(lease)?;
    codestory_workspace::atomic_file::write_bytes_atomic(&path, "agent-port-lease", &bytes)
        .with_context(|| format!("write sidecar port lease {}", path.display()))
}

fn write_agent_port_registry(path: &Path, registry: &AgentPortRegistry) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(registry)?;
    codestory_workspace::atomic_file::write_bytes_atomic(path, "agent-port-registry", &bytes)
        .with_context(|| format!("write sidecar port allocation registry {}", path.display()))
}

fn prune_agent_port_registry(
    root: &Path,
    current_namespace: &str,
    registry: &mut AgentPortRegistry,
    now: i64,
) -> AgentPortRegistryCleanup {
    let mut cleanup = AgentPortRegistryCleanup::default();
    registry.leases.retain(|namespace, lease| {
        if namespace == current_namespace {
            return true;
        }
        match lease_is_live(root, lease, now) {
            Ok(true) => {
                cleanup.retained += 1;
                true
            }
            Ok(false) => {
                cleanup.pruned += 1;
                if let Err(error) = std::fs::remove_file(agent_port_lease_path(root, namespace))
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    cleanup.failures += 1;
                    cleanup.record_failure(format!(
                        "namespace={namespace} failed to remove stale reservation: {error}"
                    ));
                }
                if let Err(error) = std::fs::remove_file(agent_port_owner_path(root, namespace))
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    cleanup.failures += 1;
                    cleanup.record_failure(format!(
                        "namespace={namespace} failed to remove stale owner: {error}"
                    ));
                }
                if let Err(error) =
                    std::fs::remove_file(legacy_agent_port_reservation_path(root, namespace))
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    cleanup.failures += 1;
                    cleanup.record_failure(format!(
                        "namespace={namespace} failed to remove legacy reservation: {error}"
                    ));
                }
                if let Err(error) = remove_empty_agent_namespace_dir(root, namespace) {
                    cleanup.failures += 1;
                    cleanup.record_failure(format!(
                        "namespace={namespace} failed to remove empty namespace directory: {error}"
                    ));
                }
                false
            }
            Err(error) => {
                cleanup.failures += 1;
                cleanup.record_failure(format!(
                    "namespace={namespace} preserved unverified allocation: {error:#}"
                ));
                true
            }
        }
    });
    cleanup
}

impl AgentPortRegistryCleanup {
    fn record_failure(&mut self, detail: String) {
        if self.failure_details.len() < 5 {
            self.failure_details.push(detail);
        }
    }
}

fn lease_is_live(root: &Path, lease: &AgentPortLease, now: i64) -> Result<bool> {
    let namespace = &lease.namespace;
    if !is_agent_namespace_path_component(namespace) {
        anyhow::bail!("registry namespace is not a safe agent path component");
    }
    let owner = read_agent_port_owner(root, namespace)?;
    let owner_matches = owner
        .as_ref()
        .is_some_and(|owner| owner.id == lease.owner.id);
    if owner_matches && lease.expires_at_epoch_ms > now {
        return Ok(true);
    }
    Ok(sidecar_ports_are_bound(&lease.ports))
}

fn sidecar_state_owns_ports(root: &Path, namespace: &str, ports: &SidecarPorts) -> Result<bool> {
    Ok(owned_sidecar_state_ports(root, namespace)?
        .as_ref()
        .is_some_and(|state_ports| same_port_numbers(state_ports, ports)))
}

fn owned_sidecar_state_ports(root: &Path, namespace: &str) -> Result<Option<SidecarPorts>> {
    let path = root.join(namespace).join("retrieval-sidecars.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    if value.get("owner").and_then(serde_json::Value::as_str) != Some("codestory")
        || value.get("namespace").and_then(serde_json::Value::as_str) != Some(namespace)
    {
        return Ok(None);
    }
    let state_ports =
        sidecar_ports_from_value(&value).context("sidecar state has incomplete ports")?;
    Ok(Some(state_ports))
}

fn renew_agent_port_lease(
    base: &Path,
    namespace: &str,
    owner_id: &str,
    ports: &SidecarPorts,
) -> Result<()> {
    let root = base.join("sidecars");
    let lock_path = root.join("port-allocations.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open sidecar port allocation lock {}", lock_path.display()))?;
    FileExt::lock_exclusive(&lock)
        .with_context(|| format!("take sidecar port allocation lock {}", lock_path.display()))?;
    let registry_path = root.join("port-allocations.json");
    let mut registry = read_agent_port_registry(&root, &registry_path)?;
    let lease = registry
        .leases
        .get_mut(namespace)
        .with_context(|| format!("agent sidecar namespace {namespace} has no port lease"))?;
    let owner = read_agent_port_owner(&root, namespace)?
        .with_context(|| format!("agent sidecar namespace {namespace} has no port owner"))?;
    if lease.owner.id != owner_id || owner.id != owner_id || !same_port_numbers(&lease.ports, ports)
    {
        anyhow::bail!("agent sidecar namespace {namespace} port lease ownership changed");
    }
    let now = next_lease_renewal_epoch_ms(now_epoch_ms(), lease.renewed_at_epoch_ms)?;
    lease.renewed_at_epoch_ms = now;
    lease.expires_at_epoch_ms = lease_expiry(now);
    write_agent_port_lease(&root, lease)?;
    write_agent_port_registry(&registry_path, &registry)
}

fn revalidate_agent_embedding_port(
    base: &Path,
    namespace: &str,
    owner_id: &str,
    ports: &SidecarPorts,
    force_rotation: bool,
) -> Result<SidecarPorts> {
    let root = base.join("sidecars");
    let lock_path = root.join("port-allocations.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open sidecar port allocation lock {}", lock_path.display()))?;
    FileExt::lock_exclusive(&lock)
        .with_context(|| format!("take sidecar port allocation lock {}", lock_path.display()))?;
    let registry_path = root.join("port-allocations.json");
    let mut registry = read_agent_port_registry(&root, &registry_path)?;
    let current = registry
        .leases
        .get(namespace)
        .cloned()
        .with_context(|| format!("agent sidecar namespace {namespace} has no port lease"))?;
    let owner = read_agent_port_owner(&root, namespace)?
        .with_context(|| format!("agent sidecar namespace {namespace} has no port owner"))?;
    if current.owner.id != owner_id
        || owner.id != owner_id
        || !same_port_numbers(&current.ports, ports)
    {
        anyhow::bail!("agent sidecar namespace {namespace} port lease ownership changed");
    }

    let mut selected = current.ports.clone();
    if force_rotation || !local_port_available(selected.embed_http) {
        let mut reserved = reserved_registry_ports_excluding(&registry.leases, namespace);
        reserved.insert(selected.qdrant_http);
        reserved.insert(selected.qdrant_grpc);
        if force_rotation {
            reserved.insert(selected.embed_http);
        }
        selected.embed_http = reserve_dynamic_agent_port(namespace, "embed", &mut reserved);
        if selected.embed_http == 0 {
            anyhow::bail!("agent sidecar embedding port rotation is unavailable");
        }
        selected.embed_url = SidecarLayout::embed_base_url(selected.embed_http);
    }

    let lease = registry
        .leases
        .get_mut(namespace)
        .expect("agent port lease checked above");
    let now = next_lease_renewal_epoch_ms(now_epoch_ms(), lease.renewed_at_epoch_ms)?;
    lease.ports = selected.clone();
    lease.renewed_at_epoch_ms = now;
    lease.expires_at_epoch_ms = lease_expiry(now);
    write_agent_port_lease(&root, lease)?;
    write_agent_port_registry(&registry_path, &registry)?;
    Ok(selected)
}

fn is_agent_namespace_path_component(namespace: &str) -> bool {
    !namespace.is_empty()
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn sidecar_ports_are_bound(ports: &SidecarPorts) -> bool {
    [ports.qdrant_http, ports.qdrant_grpc, ports.embed_http]
        .into_iter()
        .any(|port| !local_port_available(port))
}

fn same_port_numbers(left: &SidecarPorts, right: &SidecarPorts) -> bool {
    left.qdrant_http == right.qdrant_http
        && left.qdrant_grpc == right.qdrant_grpc
        && left.embed_http == right.embed_http
}

fn agent_port_owner_path(root: &Path, namespace: &str) -> PathBuf {
    root.join(namespace).join("port-owner.json")
}

fn agent_port_lease_path(root: &Path, namespace: &str) -> PathBuf {
    root.join("port-leases").join(format!("{namespace}.json"))
}

fn legacy_agent_port_reservation_path(root: &Path, namespace: &str) -> PathBuf {
    root.join(format!(".port-allocation-{namespace}.json"))
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn remove_empty_agent_namespace_dir(root: &Path, namespace: &str) -> Result<()> {
    let path = root.join(namespace);
    match std::fs::remove_dir(&path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn read_agent_port_registry(root: &Path, path: &Path) -> Result<AgentPortRegistry> {
    let body = match std::fs::read_to_string(path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return recover_agent_port_registry(root, false);
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("read sidecar port allocation registry {}", path.display())
            });
        }
    };
    if let Ok(registry) = serde_json::from_str::<AgentPortRegistry>(&body) {
        if registry.schema_version != AGENT_PORT_REGISTRY_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported sidecar port registry schema {}",
                registry.schema_version
            );
        }
        validate_agent_port_registry(&registry)?;
        let reconciled = reconcile_agent_port_registry(root, registry.clone())?;
        if reconciled != registry {
            write_agent_port_registry(path, &reconciled)?;
        }
        return Ok(reconciled);
    }
    if let Ok(legacy) = serde_json::from_str::<BTreeMap<String, SidecarPorts>>(&body) {
        let now = now_epoch_ms();
        let leases = legacy
            .into_iter()
            .map(|(namespace, ports)| {
                let owner = AgentPortOwner {
                    id: format!("legacy-{namespace}"),
                    process_id: 0,
                    created_at_epoch_ms: now,
                };
                (
                    namespace.clone(),
                    AgentPortLease {
                        namespace,
                        owner,
                        acquired_at_epoch_ms: 0,
                        renewed_at_epoch_ms: 0,
                        expires_at_epoch_ms: 0,
                        ports,
                    },
                )
            })
            .collect();
        let registry = AgentPortRegistry {
            schema_version: AGENT_PORT_REGISTRY_SCHEMA_VERSION,
            leases,
        };
        validate_agent_port_registry(&registry)?;
        return reconcile_agent_port_registry(root, registry);
    }
    let recovered = recover_agent_port_registry(root, true).with_context(|| {
        format!(
            "recover malformed sidecar port allocation registry {}",
            path.display()
        )
    })?;
    write_agent_port_registry(path, &recovered)?;
    Ok(recovered)
}

fn reconcile_agent_port_registry(
    root: &Path,
    mut registry: AgentPortRegistry,
) -> Result<AgentPortRegistry> {
    for (namespace, recovered) in recover_agent_port_registry(root, false)?.leases {
        match registry.leases.get(&namespace) {
            None => {
                registry.leases.insert(namespace, recovered);
            }
            Some(current) if current == &recovered => {}
            Some(current) if recovered.renewed_at_epoch_ms > current.renewed_at_epoch_ms => {
                registry.leases.insert(namespace, recovered);
            }
            Some(_) => anyhow::bail!(
                "compact registry disagrees with atomic lease record for namespace {namespace}"
            ),
        }
    }
    validate_agent_port_registry(&registry)?;
    Ok(registry)
}

fn recover_agent_port_registry(root: &Path, require_evidence: bool) -> Result<AgentPortRegistry> {
    let mut registry = AgentPortRegistry::default();
    let lease_root = root.join("port-leases");
    let entries = match std::fs::read_dir(&lease_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !require_evidence => {
            return Ok(registry);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("malformed registry has no lease records for safe recovery");
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", lease_root.display())),
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("read entry in {}", lease_root.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".json") {
            continue;
        }
        let bytes = std::fs::read(entry.path())
            .with_context(|| format!("read sidecar port lease {}", entry.path().display()))?;
        let lease: AgentPortLease = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse sidecar port lease {}", entry.path().display()))?;
        if agent_port_lease_path(root, &lease.namespace) != entry.path() {
            anyhow::bail!("sidecar port lease filename does not match namespace");
        }
        if registry
            .leases
            .insert(lease.namespace.clone(), lease)
            .is_some()
        {
            anyhow::bail!("duplicate sidecar port lease namespace");
        }
    }
    if require_evidence && registry.leases.is_empty() {
        anyhow::bail!("malformed registry has no valid lease records for safe recovery");
    }
    validate_agent_port_registry(&registry)?;
    Ok(registry)
}

fn validate_agent_port_registry(registry: &AgentPortRegistry) -> Result<()> {
    let mut ports = BTreeSet::new();
    for (namespace, lease) in &registry.leases {
        if namespace != &lease.namespace || !is_agent_namespace_path_component(namespace) {
            anyhow::bail!("sidecar port lease namespace is invalid");
        }
        if lease.owner.id.is_empty() {
            anyhow::bail!("sidecar port lease owner identity is empty");
        }
        let legacy_zero_timestamps = lease.owner.process_id == 0
            && lease.acquired_at_epoch_ms == 0
            && lease.renewed_at_epoch_ms == 0
            && lease.expires_at_epoch_ms == 0;
        if !legacy_zero_timestamps
            && (lease.acquired_at_epoch_ms < 0
                || lease.renewed_at_epoch_ms < lease.acquired_at_epoch_ms
                || lease.expires_at_epoch_ms <= lease.renewed_at_epoch_ms)
        {
            anyhow::bail!("sidecar port lease timestamps are invalid");
        }
        for port in [
            lease.ports.qdrant_http,
            lease.ports.qdrant_grpc,
            lease.ports.embed_http,
        ] {
            if port == 0 || !ports.insert(port) {
                anyhow::bail!("sidecar port registry contains invalid or duplicate ports");
            }
        }
    }
    Ok(())
}

fn reserved_registry_ports_excluding(
    registry: &BTreeMap<String, AgentPortLease>,
    namespace: &str,
) -> BTreeSet<u16> {
    registry
        .iter()
        .filter(|(candidate, _)| candidate.as_str() != namespace)
        .flat_map(|(_, lease)| {
            [
                lease.ports.qdrant_http,
                lease.ports.qdrant_grpc,
                lease.ports.embed_http,
            ]
        })
        .collect()
}

fn select_agent_port(
    configured: Option<u16>,
    existing: Option<u16>,
    namespace: &str,
    salt: &str,
    reserved: &mut BTreeSet<u16>,
    state_owns_port: bool,
) -> Result<u16> {
    if let Some(port) = configured.or(existing) {
        if port == 0 {
            anyhow::bail!("agent sidecar port 0 cannot be leased");
        }
        if !reserved.insert(port) {
            anyhow::bail!("agent sidecar port {port} is already reserved");
        }
        if existing != Some(port) && !state_owns_port && !local_port_available(port) {
            anyhow::bail!("agent sidecar port {port} is already bound without matching ownership");
        }
        return Ok(port);
    }
    Ok(reserve_dynamic_agent_port(namespace, salt, reserved))
}

fn reserve_dynamic_agent_port(namespace: &str, salt: &str, reserved: &mut BTreeSet<u16>) -> u16 {
    let port = dynamic_agent_port_excluding(namespace, salt, reserved);
    reserved.insert(port);
    port
}

fn dynamic_agent_port_excluding(namespace: &str, salt: &str, reserved: &BTreeSet<u16>) -> u16 {
    let seed = fnv1a_hex(format!("{namespace}:{salt}").as_bytes());
    let parsed = u64::from_str_radix(&seed, 16).unwrap_or(0);
    let base = 20_000 + u16::try_from(parsed % 40_000).unwrap_or(0);
    for offset in 0..1000 {
        let port = 20_000 + ((u32::from(base - 20_000) + offset) % 40_000) as u16;
        if !reserved.contains(&port) && local_port_available(port) {
            return port;
        }
    }
    free_local_port_excluding(reserved)
}

fn local_port_available(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn free_local_port_excluding(reserved: &BTreeSet<u16>) -> u16 {
    for _ in 0..100 {
        let port = free_local_port();
        if port == 0 || !reserved.contains(&port) {
            return port;
        }
    }
    0
}

fn free_local_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .unwrap_or(0)
}

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn quote_command_path(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', "\\\""))
}

pub fn retrieval_command(
    action: &str,
    project_root: &Path,
    profile: SidecarProfile,
    run_id: Option<&str>,
    extra_args: Option<&str>,
) -> String {
    let mut command = format!(
        "codestory-cli retrieval {action} --project {}",
        quote_command_path(project_root)
    );
    if profile == SidecarProfile::Agent {
        command.push_str(" --profile agent");
        if let Some(run_id) = run_id.and_then(normalized_label_component) {
            command.push_str(" --run-id ");
            command.push_str(&run_id);
        }
    }
    if let Some(extra_args) = extra_args {
        command.push(' ');
        command.push_str(extra_args);
    }
    command
}

pub fn user_cache_root() -> PathBuf {
    if let Ok(path) = std::env::var("CODESTORY_CACHE_ROOT") {
        let path = path.trim();
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    #[cfg(feature = "test-support")]
    if let Some(path) = active_test_cache_root() {
        return path;
    }
    #[cfg(test)]
    if let Some(path) = automatic_unit_test_cache_root() {
        return path;
    }
    ProjectDirs::from("dev", "codestory", "codestory")
        .map(|dirs| dirs.cache_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("codestory").join("cache"))
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static TEST_CACHE_ROOT_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn active_test_cache_root() -> Option<PathBuf> {
    TEST_CACHE_ROOT_OVERRIDE
        .with(|root| root.borrow().clone())
        .or_else(|| {
            AUTOMATIC_TEST_CACHE_ROOT_ENABLED
                .load(std::sync::atomic::Ordering::Acquire)
                .then(automatic_unit_test_cache_root)
                .flatten()
        })
}

#[cfg(feature = "test-support")]
static AUTOMATIC_TEST_CACHE_ROOT_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Enable named-thread cache isolation for a test binary that explicitly owns
/// every runtime created in the current process.
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
    static PROCESS_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let process_root = PROCESS_ROOT.get_or_init(|| {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir()
            .join("codestory-unit-tests")
            .join(format!("{}-{nonce}", std::process::id()))
    });
    let label = normalized_label_component(name)?;
    let prefix = &label[..label.len().min(32)];
    let root = process_root.join(format!("{prefix}-{}", &fnv1a_hex(label.as_bytes())[..12]));
    std::fs::create_dir_all(root.join("sidecars")).ok()?;
    Some(root)
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
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

/// Docker compose profile for mandatory sidecars.
pub fn retrieval_compose_profile() -> String {
    "real".to_string()
}

pub fn embedding_server_launch_mode() -> Result<EmbeddingServerLaunchMode> {
    embedding_server_launch_mode_for_runtime(&SidecarRuntimeConfig::local())
}

pub fn embedding_server_launch_mode_for_runtime(
    runtime: &SidecarRuntimeConfig,
) -> Result<EmbeddingServerLaunchMode> {
    if let Some(mode) = runtime
        .embedding
        .server_launch
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
        .and_then(|value| match value.as_str() {
            "native_spawned" | "native" => Some(EmbeddingServerLaunchMode::NativeSpawned),
            "docker_compose_embed" | "docker" | "compose" => {
                Some(EmbeddingServerLaunchMode::DockerComposeEmbed)
            }
            "external_endpoint" | "external" | "endpoint"
                if runtime.embedding.endpoint_origin != EmbeddingEndpointOrigin::ManagedSidecar
                    && !runtime.embedding.endpoint.trim().is_empty() =>
            {
                Some(EmbeddingServerLaunchMode::ExternalEndpoint)
            }
            _ => None,
        })
    {
        return Ok(mode);
    }
    if runtime.embedding.server_launch.is_some() {
        anyhow::bail!(
            "CODESTORY_EMBED_SERVER_LAUNCH must be docker_compose_embed, native_spawned, or external_endpoint"
        );
    }
    if runtime.embedding.endpoint_origin != EmbeddingEndpointOrigin::ManagedSidecar {
        return Ok(EmbeddingServerLaunchMode::ExternalEndpoint);
    }
    let request = crate::embeddings::embedding_accelerator_request();
    let host = embedding_host_platform();
    if request.is_some() && ((host.os == "macos" && host.arch == "aarch64") || host.os == "windows")
    {
        Ok(EmbeddingServerLaunchMode::NativeSpawned)
    } else {
        Ok(EmbeddingServerLaunchMode::DockerComposeEmbed)
    }
}

fn env_port(defaults: &SidecarRuntimeDefaults, name: &str, default: u16) -> Option<u16> {
    defaults.get(name).map(|value| {
        value
            .parse()
            .ok()
            .filter(|port| *port != 0)
            .unwrap_or(default)
    })
}

pub fn dir_size_bytes(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            total = total.saturating_add(entry.metadata().map(|m| m.len()).unwrap_or(0));
        } else if path.is_dir() {
            total = total.saturating_add(dir_size_bytes(&path));
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_support_isolates_named_threads_after_explicit_activation() {
        #[cfg(feature = "test-support")]
        enable_automatic_test_cache_root_for_process();
        let root = automatic_unit_test_cache_root().expect("named test thread cache root");
        assert!(root.starts_with(std::env::temp_dir().join("codestory-unit-tests")));
        #[cfg(feature = "test-support")]
        assert_eq!(active_test_cache_root(), Some(root));
    }

    #[test]
    fn agent_profile_default_runtime_reuses_project_shared_run() {
        let _lock = crate::test_support::env_lock();
        let project = tempdir().expect("project");
        let _cache = EnvGuard::set(
            "CODESTORY_CACHE_ROOT",
            project.path().join("cache").to_str().expect("utf8 cache"),
        );

        let first =
            SidecarRuntimeConfig::for_project_profile(Some(project.path()), SidecarProfile::Agent);
        let second =
            SidecarRuntimeConfig::for_project_profile(Some(project.path()), SidecarProfile::Agent);

        assert_eq!(first.run_id.as_deref(), Some(DEFAULT_AGENT_RUN_ID));
        assert_eq!(first.run_id, second.run_id);
        assert_eq!(first.namespace, second.namespace);
        assert_eq!(first.layout.state_file, second.layout.state_file);
        assert_eq!(
            first.layout.qdrant_http_port,
            second.layout.qdrant_http_port
        );
        assert_eq!(
            first.layout.qdrant_grpc_port,
            second.layout.qdrant_grpc_port
        );
        assert_eq!(first.embed_http_port, second.embed_http_port);
        assert!(first.cleanup_command.contains("--run-id shared-agent"));
    }

    #[test]
    fn project_runtime_exposes_v3_identity_and_namespace() {
        let _lock = crate::test_support::env_lock();
        let project = tempdir().expect("project");
        let _cache = EnvGuard::set(
            "CODESTORY_CACHE_ROOT",
            project.path().join("cache").to_str().expect("utf8 cache"),
        );
        let runtime =
            SidecarRuntimeConfig::for_project_profile(Some(project.path()), SidecarProfile::Agent);
        let identity = runtime.project_identity.as_ref().expect("project identity");

        assert_eq!(
            identity.project_identity_schema_version,
            codestory_workspace::PROJECT_IDENTITY_V3_SCHEMA_VERSION
        );

        assert_eq!(
            runtime.labels.get("dev.codestory.project_id"),
            Some(&identity.project_id)
        );
        assert_eq!(
            runtime.labels.get("dev.codestory.workspace_id"),
            Some(&identity.workspace_id)
        );
        assert_eq!(
            runtime.labels.get("dev.codestory.artifact_scope_id"),
            Some(&identity.artifact_scope_id)
        );
        assert!(
            runtime
                .namespace
                .starts_with(&format!("codestory-agent-{}-", identity.workspace_id)),
            "agent namespaces must use the lossless workspace identity"
        );
        assert_eq!(
            runtime.ownership().project_identity.as_ref(),
            Some(identity)
        );
    }

    #[test]
    fn agent_profile_run_id_reuses_namespace_state_and_ports() {
        let _lock = crate::test_support::env_lock();
        let project = tempdir().expect("project");
        let _cache = EnvGuard::set(
            "CODESTORY_CACHE_ROOT",
            project.path().join("cache").to_str().expect("utf8 cache"),
        );

        let first = SidecarRuntimeConfig::for_project_profile_with_run_id(
            Some(project.path()),
            SidecarProfile::Agent,
            Some("review-fix"),
        );
        let second = SidecarRuntimeConfig::for_project_profile_with_run_id(
            Some(project.path()),
            SidecarProfile::Agent,
            Some("review-fix"),
        );

        assert_eq!(first.run_id.as_deref(), Some("review-fix"));
        assert_eq!(first.namespace, second.namespace);
        assert_eq!(first.layout.state_file, second.layout.state_file);
        assert_eq!(
            first.layout.qdrant_http_port,
            second.layout.qdrant_http_port
        );
        assert!(first.cleanup_command.contains("--profile agent"));
        assert!(first.cleanup_command.contains("--run-id review-fix"));
    }

    #[test]
    fn agent_profile_reuses_persisted_state_ports_with_matching_lease() {
        let _lock = crate::test_support::env_lock();
        let project = tempdir().expect("project");
        let _cache = EnvGuard::set(
            "CODESTORY_CACHE_ROOT",
            project.path().join("cache").to_str().expect("utf8 cache"),
        );
        let first = SidecarRuntimeConfig::for_project_profile_with_run_id(
            Some(project.path()),
            SidecarProfile::Agent,
            Some("persisted"),
        );
        std::fs::create_dir_all(first.layout.state_file.parent().expect("state parent"))
            .expect("state dir");
        std::fs::write(
            &first.layout.state_file,
            serde_json::to_vec(&serde_json::json!({
                "project_identity": first.project_identity,
                "owner": "codestory",
                "profile": first.profile.as_str(),
                "namespace": first.namespace,
                "compose_project": first.compose_project,
                "run_id": first.run_id,
                "qdrant_http_port": first.layout.qdrant_http_port,
                "qdrant_grpc_port": first.layout.qdrant_grpc_port,
                "embed_http_port": first.embed_http_port,
                "embed_url": SidecarLayout::embed_base_url(first.embed_http_port),
            }))
            .expect("state json"),
        )
        .expect("state file");

        let second = SidecarRuntimeConfig::for_project_profile_with_run_id(
            Some(project.path()),
            SidecarProfile::Agent,
            Some("persisted"),
        );

        assert_eq!(
            second.layout.qdrant_http_port,
            first.layout.qdrant_http_port
        );
        assert_eq!(
            second.layout.qdrant_grpc_port,
            first.layout.qdrant_grpc_port
        );
        assert_eq!(second.embed_http_port, first.embed_http_port);
    }

    #[test]
    fn legacy_identity_reuse_requires_the_exact_workspace() {
        let project = tempdir().expect("project");
        let current = codestory_workspace::project_identity_v3(project.path());
        let legacy = codestory_workspace::project_identity_v2(project.path());
        let stored: codestory_workspace::ProjectIdentityV3 = serde_json::from_value(
            serde_json::to_value(&legacy).expect("serialize legacy identity"),
        )
        .expect("read legacy identity through migration shape");

        assert!(project_identity_matches_runtime_selection(
            Some(&stored),
            Some(project.path()),
            Some(&current),
        ));
        assert!(project_identity_matches_runtime(
            Some(&stored),
            Some(&current),
            Some(&stored),
        ));
        assert!(!project_identity_matches_runtime(
            Some(&stored),
            Some(&current),
            None,
        ));

        let mut foreign = stored.clone();
        foreign.canonical_repository_id = Some("foreign-repository".to_string());
        foreign.project_id = "foreign-repository".to_string();
        foreign.artifact_scope_id = "foreign-repository".to_string();
        assert!(!project_identity_matches_runtime_selection(
            Some(&foreign),
            Some(project.path()),
            Some(&current),
        ));
        assert!(!project_identity_matches_runtime(
            None,
            Some(&current),
            None
        ));
    }

    #[test]
    fn agent_port_registry_reuses_namespace_and_avoids_registered_ports() {
        let cache = tempdir().expect("cache");

        let first = allocate_agent_ports_in_registry(cache.path(), "codestory-agent-a", [None; 3])
            .expect("first");
        let same = allocate_agent_ports_in_registry(cache.path(), "codestory-agent-a", [None; 3])
            .expect("same");
        let other = allocate_agent_ports_in_registry(cache.path(), "codestory-agent-b", [None; 3])
            .expect("other");

        assert_eq!(first.ports, same.ports);
        assert_eq!(first.owner_id, same.owner_id);
        let ports = [
            first.ports.qdrant_http,
            first.ports.qdrant_grpc,
            first.ports.embed_http,
            other.ports.qdrant_http,
            other.ports.qdrant_grpc,
            other.ports.embed_http,
        ];
        let unique: BTreeSet<_> = ports.into_iter().collect();
        assert_eq!(unique.len(), ports.len());
    }

    #[test]
    fn healthy_owner_renews_lease_without_changing_identity() {
        let cache = tempdir().expect("cache");
        let first = allocate_agent_ports_in_registry_at(cache.path(), "same", [None; 3], 100)
            .expect("first");
        let renewed = allocate_agent_ports_in_registry_at(cache.path(), "same", [None; 3], 200)
            .expect("renewed");
        let root = cache.path().join("sidecars");
        let registry =
            read_agent_port_registry(&root, &root.join("port-allocations.json")).expect("registry");
        let lease = registry.leases.get("same").expect("lease");

        assert_eq!(first.owner_id, renewed.owner_id);
        assert_eq!(lease.acquired_at_epoch_ms, 100);
        assert_eq!(lease.renewed_at_epoch_ms, 200);
        assert_eq!(lease.expires_at_epoch_ms, lease_expiry(200));
    }

    #[test]
    fn same_millisecond_atomic_lease_update_recovers_over_stale_compact_registry() {
        let cache = tempdir().expect("cache");
        let first = allocate_agent_ports_in_registry_at(cache.path(), "same-ms", [None; 3], 100)
            .expect("initial allocation");
        let root = cache.path().join("sidecars");
        let registry_path = root.join("port-allocations.json");
        let compact = read_agent_port_registry(&root, &registry_path).expect("initial registry");
        std::fs::remove_file(agent_port_owner_path(&root, "same-ms"))
            .expect("simulate vanished first owner");
        let replacement =
            allocate_agent_ports_in_registry_at(cache.path(), "same-ms", [None; 3], 100)
                .expect("same-millisecond replacement owner");
        let recovered: AgentPortLease = serde_json::from_slice(
            &std::fs::read(agent_port_lease_path(&root, "same-ms")).expect("newer atomic lease"),
        )
        .expect("parse newer atomic lease");
        assert_ne!(replacement.owner_id, first.owner_id);

        // Simulate a crash after the per-lease atomic write but before the compact
        // registry publication. The newer +1 timestamp must win deterministically.
        write_agent_port_registry(&registry_path, &compact)
            .expect("restore stale compact registry");
        let reconciled = read_agent_port_registry(&root, &registry_path)
            .expect("reconcile same-millisecond crash");
        let selected = reconciled.leases.get("same-ms").expect("recovered lease");

        assert_eq!(selected, &recovered);
        assert!(
            selected.renewed_at_epoch_ms
                > compact
                    .leases
                    .get("same-ms")
                    .expect("compact lease")
                    .renewed_at_epoch_ms
        );
    }

    #[test]
    fn stale_runtime_cannot_renew_successor_owner_lease() {
        let cache = tempdir().expect("cache");
        let first = SidecarRuntimeConfig::for_project_profile_with_run_id_in_cache(
            None,
            SidecarProfile::Agent,
            Some("successor"),
            cache.path(),
        );
        let root = cache.path().join("sidecars");
        std::fs::remove_file(agent_port_owner_path(&root, &first.namespace))
            .expect("remove first owner");
        let successor = SidecarRuntimeConfig::for_project_profile_with_run_id_in_cache(
            None,
            SidecarProfile::Agent,
            Some("successor"),
            cache.path(),
        );

        assert_ne!(first.port_lease_owner_id, successor.port_lease_owner_id);
        first
            .ensure_ports_allocated()
            .expect_err("stale runtime must not renew successor lease");
        successor
            .ensure_ports_allocated()
            .expect("successor renews its lease");
    }

    #[test]
    fn heartbeat_keeps_unbound_ports_past_original_ttl_under_contention() {
        let cache = tempdir().expect("cache");
        let runtime = SidecarRuntimeConfig::for_project_profile_with_run_id_in_cache(
            None,
            SidecarProfile::Agent,
            Some("heartbeat"),
            cache.path(),
        );
        let root = cache.path().join("sidecars");
        let lease_path = agent_port_lease_path(&root, &runtime.namespace);
        let heartbeat = runtime
            .start_port_lease_heartbeat_with_interval(Duration::from_millis(5))
            .expect("heartbeat");
        let original: AgentPortLease =
            serde_json::from_slice(&std::fs::read(&lease_path).expect("original lease"))
                .expect("parse original lease");
        let renewed = (0..100)
            .find_map(|_| {
                std::thread::sleep(Duration::from_millis(5));
                let lease: AgentPortLease =
                    serde_json::from_slice(&std::fs::read(&lease_path).ok()?).ok()?;
                (lease.renewed_at_epoch_ms >= original.renewed_at_epoch_ms + 2).then_some(lease)
            })
            .expect("heartbeat renewed lease");
        let contention_time = original.expires_at_epoch_ms + 1;
        assert!(renewed.expires_at_epoch_ms > contention_time);
        let error = allocate_agent_ports_in_registry_at(
            cache.path(),
            "contender",
            configured_ports(&original.ports),
            contention_time,
        )
        .expect_err("heartbeat must preserve ports beyond original ttl");
        heartbeat.finish().expect("stop heartbeat");

        assert!(error.to_string().contains("already reserved"));
    }

    #[test]
    fn expired_crashed_owner_is_reclaimed_under_lock() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let first = allocate_agent_ports_in_registry_at(cache.path(), "crashed", [None; 3], 100)
            .expect("first");
        let configured = configured_ports(&first.ports);
        let replacement = allocate_agent_ports_in_registry_at(
            cache.path(),
            "replacement",
            configured,
            lease_expiry(100) + 1,
        )
        .expect("replacement");
        let registry =
            read_agent_port_registry(&root, &root.join("port-allocations.json")).expect("registry");

        assert_eq!(replacement.ports, first.ports);
        assert!(!registry.leases.contains_key("crashed"));
        assert_eq!(registry.leases.len(), 1);
    }

    #[test]
    fn reclamation_never_removes_namespace_with_state_or_data() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let first = allocate_agent_ports_in_registry_at(cache.path(), "with-data", [None; 3], 100)
            .expect("first");
        let sentinel = root.join("with-data").join("qdrant").join("sentinel");
        std::fs::create_dir_all(sentinel.parent().expect("sentinel parent")).expect("data dir");
        std::fs::write(&sentinel, b"keep").expect("sentinel");

        allocate_agent_ports_in_registry_at(
            cache.path(),
            "replacement",
            configured_ports(&first.ports),
            lease_expiry(100) + 1,
        )
        .expect("replacement");

        assert_eq!(std::fs::read(&sentinel).expect("sentinel remains"), b"keep");
    }

    #[test]
    fn agent_port_lease_abort_child() {
        let Some(base) = std::env::var_os(PORT_LEASE_ABORT_BASE_ENV).map(PathBuf::from) else {
            return;
        };
        let result = allocate_agent_ports_in_registry(&base, "crash-child", [None; 3]);
        panic!("port lease abort hook returned: {result:?}");
    }

    #[test]
    fn process_abort_after_lease_write_recovers_without_reissue() {
        let cache = tempdir().expect("cache");
        let sentinel = cache.path().join("abort-sentinel");
        let status = std::process::Command::new(
            std::env::current_exe().expect("resolve retrieval test executable"),
        )
        .arg("--exact")
        .arg("config::tests::agent_port_lease_abort_child")
        .arg("--nocapture")
        .env(PORT_LEASE_ABORT_BASE_ENV, cache.path())
        .env(PORT_LEASE_ABORT_SENTINEL_ENV, &sentinel)
        .status()
        .expect("run lease abort child");
        assert!(!status.success(), "abort child exited successfully");
        assert_eq!(
            std::fs::read(&sentinel).expect("abort sentinel"),
            b"lease-persisted"
        );

        let root = cache.path().join("sidecars");
        let registry = read_agent_port_registry(&root, &root.join("port-allocations.json"))
            .expect("recover registry from lease record");
        let ports = registry
            .leases
            .get("crash-child")
            .expect("crashed lease")
            .ports
            .clone();
        let error =
            allocate_agent_ports_in_registry(cache.path(), "replacement", configured_ports(&ports))
                .expect_err("crashed live lease must not be reissued");
        assert!(error.to_string().contains("already reserved"));
    }

    fn test_sidecar_ports() -> (Vec<TcpListener>, SidecarPorts) {
        let listeners: Vec<_> = (0..3)
            .map(|_| TcpListener::bind(("127.0.0.1", 0)).expect("reserve test port"))
            .collect();
        let ports: Vec<_> = listeners
            .iter()
            .map(|listener| listener.local_addr().expect("local address").port())
            .collect();
        (
            listeners,
            SidecarPorts {
                qdrant_http: ports[0],
                qdrant_grpc: ports[1],
                embed_http: ports[2],
                embed_url: SidecarLayout::embed_base_url(ports[2]),
            },
        )
    }

    fn configured_ports(ports: &SidecarPorts) -> [Option<u16>; 3] {
        [
            Some(ports.qdrant_http),
            Some(ports.qdrant_grpc),
            Some(ports.embed_http),
        ]
    }

    fn write_owned_state(root: &Path, namespace: &str, ports: &SidecarPorts) {
        std::fs::create_dir_all(root.join(namespace)).expect("namespace");
        std::fs::write(
            root.join(namespace).join("retrieval-sidecars.json"),
            serde_json::to_vec(&serde_json::json!({
                "owner": "codestory",
                "namespace": namespace,
                "qdrant_http_port": ports.qdrant_http,
                "qdrant_grpc_port": ports.qdrant_grpc,
                "embed_http_port": ports.embed_http,
                "embed_url": ports.embed_url,
            }))
            .expect("state"),
        )
        .expect("state");
    }

    #[test]
    fn missing_owner_is_reclaimed_without_waiting_for_expiry() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let first = allocate_agent_ports_in_registry_at(cache.path(), "missing", [None; 3], 100)
            .expect("first");
        std::fs::remove_file(agent_port_owner_path(&root, "missing")).expect("remove owner");
        let replacement = allocate_agent_ports_in_registry_at(
            cache.path(),
            "replacement",
            configured_ports(&first.ports),
            101,
        )
        .expect("replacement");

        assert_eq!(replacement.ports, first.ports);
    }

    #[test]
    fn bound_ports_are_never_reissued_after_expiry() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let (listeners, ports) = test_sidecar_ports();
        write_owned_state(&root, "live", &ports);
        let first = allocate_agent_ports_in_registry_at(
            cache.path(),
            "live",
            configured_ports(&ports),
            100,
        )
        .expect("first");
        let error = allocate_agent_ports_in_registry_at(
            cache.path(),
            "other",
            configured_ports(&ports),
            lease_expiry(100) + 1,
        )
        .expect_err("bound ports remain reserved");

        assert_eq!(listeners.len(), 3);
        assert_eq!(first.ports, ports);
        assert!(error.to_string().contains("already reserved"));
    }

    #[test]
    fn configured_bound_ports_require_matching_namespace_state() {
        let cache = tempdir().expect("cache");
        let (listeners, ports) = test_sidecar_ports();

        let error = allocate_agent_ports_in_registry_at(
            cache.path(),
            "unowned",
            configured_ports(&ports),
            100,
        )
        .expect_err("bound configured port must not be leased without ownership");

        assert_eq!(listeners.len(), 3);
        assert!(
            error
                .to_string()
                .contains("bound without matching ownership")
        );
    }

    #[test]
    fn pid_reuse_without_owner_token_does_not_preserve_free_ports() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let first = allocate_agent_ports_in_registry_at(cache.path(), "reused", [None; 3], 100)
            .expect("first");
        let impostor = AgentPortOwner {
            id: uuid::Uuid::new_v4().to_string(),
            process_id: std::process::id(),
            created_at_epoch_ms: 101,
        };
        write_agent_port_owner(&root, "reused", &impostor).expect("impostor owner");
        let replacement = allocate_agent_ports_in_registry_at(
            cache.path(),
            "replacement",
            configured_ports(&first.ports),
            102,
        )
        .expect("replacement");

        assert_eq!(impostor.process_id, std::process::id());
        assert_ne!(impostor.id, first.owner_id);
        assert_eq!(replacement.ports, first.ports);
    }

    #[test]
    fn malformed_registry_recovers_from_atomic_lease_records() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let first = allocate_agent_ports_in_registry_at(cache.path(), "first", [None; 3], 100)
            .expect("first");
        std::fs::write(root.join("port-allocations.json"), b"{").expect("corrupt registry");
        let error = allocate_agent_ports_in_registry_at(
            cache.path(),
            "second",
            configured_ports(&first.ports),
            101,
        )
        .expect_err("live lease must survive registry recovery");

        assert!(error.to_string().contains("already reserved"));
        let registry = read_agent_port_registry(&root, &root.join("port-allocations.json"))
            .expect("recovered registry");
        assert_eq!(
            registry.leases.get("first").map(|lease| &lease.ports),
            Some(&first.ports)
        );
    }

    #[test]
    fn partial_registry_reconciles_atomic_lease_records() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let first = allocate_agent_ports_in_registry_at(cache.path(), "first", [None; 3], 100)
            .expect("first");
        write_agent_port_registry(
            &root.join("port-allocations.json"),
            &AgentPortRegistry::default(),
        )
        .expect("partial registry");

        let error = allocate_agent_ports_in_registry_at(
            cache.path(),
            "second",
            configured_ports(&first.ports),
            101,
        )
        .expect_err("recovery lease must fill partial registry");

        assert!(error.to_string().contains("already reserved"));
    }

    #[test]
    fn live_legacy_allocation_migrates_without_pid_trust() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let namespace = "legacy-live";
        let (listeners, ports) = test_sidecar_ports();
        std::fs::create_dir_all(root.join(namespace)).expect("namespace");
        std::fs::write(
            root.join("port-allocations.json"),
            serde_json::to_vec(&BTreeMap::from([(namespace.to_string(), ports.clone())]))
                .expect("legacy registry"),
        )
        .expect("registry");
        std::fs::write(
            root.join(namespace).join("retrieval-sidecars.json"),
            serde_json::to_vec(&serde_json::json!({
                "owner": "codestory",
                "namespace": namespace,
                "qdrant_http_port": ports.qdrant_http,
                "qdrant_grpc_port": ports.qdrant_grpc,
                "embed_http_port": ports.embed_http,
                "embed_url": ports.embed_url,
                "embedding_launch": { "pid": std::process::id() },
            }))
            .expect("state"),
        )
        .expect("state");

        let migrated = allocate_agent_ports_in_registry_at(
            cache.path(),
            namespace,
            configured_ports(&ports),
            100,
        )
        .expect("migrate live allocation");

        assert_eq!(listeners.len(), 3);
        assert_eq!(migrated.ports, ports);
        assert_ne!(migrated.owner_id, format!("legacy-{namespace}"));
    }

    #[test]
    fn malformed_lease_blocks_malformed_registry_recovery() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("port-allocations.json"), b"{").expect("registry");
        std::fs::create_dir_all(root.join("port-leases")).expect("lease dir");
        std::fs::write(root.join("port-leases").join("broken.json"), b"{").expect("lease");
        let error = allocate_agent_ports_in_registry_at(cache.path(), "current", [None; 3], 100)
            .expect_err("malformed lease must fail closed");

        assert!(error.to_string().contains("recover malformed"));
    }

    #[test]
    fn malformed_namespace_state_fails_closed_before_allocation() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars").join("current");
        std::fs::create_dir_all(&root).expect("namespace");
        std::fs::write(root.join("retrieval-sidecars.json"), b"{").expect("state");

        let error = allocate_agent_ports_in_registry_at(cache.path(), "current", [None; 3], 100)
            .expect_err("malformed state must fail closed");

        assert!(error.to_string().contains("parse"));
    }

    #[test]
    fn agent_port_registry_rejects_traversal_namespace_without_touching_outside_file() {
        let cache = tempdir().expect("cache");
        let sentinel = cache.path().join("outside.json");
        std::fs::write(&sentinel, b"keep").expect("outside sentinel");
        allocate_agent_ports_in_registry_at(cache.path(), "../outside", [None; 3], 100)
            .expect_err("traversal namespace");

        assert_eq!(std::fs::read(&sentinel).expect("outside sentinel"), b"keep");
    }

    #[test]
    fn concurrent_allocations_are_unique_and_registry_remains_parseable() {
        let cache = tempdir().expect("cache");
        let base = std::sync::Arc::new(cache.path().to_path_buf());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let base = base.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    allocate_agent_ports_in_registry_at(
                        &base,
                        &format!("worker-{index}"),
                        [None; 3],
                        100,
                    )
                    .expect("allocation")
                    .ports
                })
            })
            .collect();
        let allocations: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("worker"))
            .collect();
        let unique: BTreeSet<_> = allocations
            .iter()
            .flat_map(|ports| [ports.qdrant_http, ports.qdrant_grpc, ports.embed_http])
            .collect();
        let root = cache.path().join("sidecars");
        let registry =
            read_agent_port_registry(&root, &root.join("port-allocations.json")).expect("registry");

        assert_eq!(unique.len(), 24);
        assert_eq!(registry.leases.len(), 8);
    }

    #[test]
    fn long_run_reclamation_keeps_registry_bounded() {
        let cache = tempdir().expect("cache");
        let project = tempdir().expect("project");
        let root = cache.path().join("sidecars");
        let namespace_prefix = agent_namespace_prefix(project.path());
        let mut now = 0;
        for index in 0..256 {
            allocate_agent_ports_in_registry_at(
                cache.path(),
                &format!("{namespace_prefix}run-{index}"),
                [None; 3],
                now,
            )
            .expect("allocation");
            now = lease_expiry(now) + 1;
        }
        let registry =
            read_agent_port_registry(&root, &root.join("port-allocations.json")).expect("registry");

        assert_eq!(registry.leases.len(), 1);
        let latest_namespace = format!("{namespace_prefix}run-255");
        assert!(registry.leases.contains_key(&latest_namespace));
        assert_eq!(
            std::fs::read_dir(root.join("port-leases"))
                .expect("lease dir")
                .count(),
            1
        );
        let latest_ports = &registry
            .leases
            .get(&latest_namespace)
            .expect("latest lease")
            .ports;
        write_owned_state(&root, &latest_namespace, latest_ports);
        let retained_namespace_dirs = std::fs::read_dir(&root)
            .expect("sidecars root")
            .flatten()
            .filter(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(&namespace_prefix)
            })
            .count();
        assert_eq!(retained_namespace_dirs, 1);
        assert_eq!(
            latest_agent_run_id_in_cache(project.path(), cache.path()).as_deref(),
            Some("run-255")
        );
    }

    #[test]
    fn separate_cache_roots_isolate_test_registries() {
        let first_cache = tempdir().expect("first cache");
        let second_cache = tempdir().expect("second cache");
        let first = allocate_agent_ports_in_registry_at(first_cache.path(), "same", [None; 3], 100)
            .expect("first");
        let second = allocate_agent_ports_in_registry_at(
            second_cache.path(),
            "same",
            configured_ports(&first.ports),
            100,
        )
        .expect("second");

        assert_eq!(first.ports, second.ports);
        assert_ne!(first.owner_id, second.owner_id);
    }

    #[test]
    fn malformed_agent_port_registry_fails_closed_in_runtime_path() {
        let _lock = crate::test_support::env_lock();
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        std::fs::create_dir_all(&root).expect("sidecars root");
        let registry_path = root.join("port-allocations.json");
        std::fs::write(&registry_path, b"{").expect("malformed registry");
        let _cache = EnvGuard::set(
            "CODESTORY_CACHE_ROOT",
            cache.path().to_str().expect("utf8 cache"),
        );
        let _qdrant_http = EnvGuard::set("CODESTORY_QDRANT_HTTP_PORT", "invalid");
        let _qdrant_grpc = EnvGuard::set("CODESTORY_QDRANT_GRPC_PORT", "invalid");
        let _embed = EnvGuard::set("CODESTORY_EMBED_PORT", "invalid");

        let runtime = SidecarRuntimeConfig::for_project_profile_with_run_id(
            None,
            SidecarProfile::Agent,
            Some("current"),
        );

        assert_eq!(runtime.layout.qdrant_http_port, 0);
        assert_eq!(runtime.layout.qdrant_grpc_port, 0);
        assert_eq!(runtime.embed_http_port, 0);
        runtime
            .ensure_ports_allocated()
            .expect_err("malformed registry must block sidecar startup");
        assert_eq!(
            std::fs::read(&registry_path).expect("registry remains"),
            b"{"
        );
    }

    #[test]
    fn explicit_local_profile_overrides_latest_agent_run() {
        let (profile, run_id) = auto_runtime_selection(
            Some(SidecarProfile::Local),
            None,
            Some("latest-agent".to_string()),
            false,
        );

        assert_eq!(profile, SidecarProfile::Local);
        assert_eq!(run_id, None);
    }

    #[test]
    fn latest_agent_run_selects_agent_without_explicit_profile() {
        let (profile, run_id) =
            auto_runtime_selection(None, None, Some("latest-agent".to_string()), false);

        assert_eq!(profile, SidecarProfile::Agent);
        assert_eq!(run_id.as_deref(), Some("latest-agent"));
    }

    #[test]
    fn explicit_embedding_launch_modes_parse() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::set("CODESTORY_EMBED_SERVER_LAUNCH", "native_spawned");
        let _url = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_URL");

        assert_eq!(
            embedding_server_launch_mode().expect("launch mode"),
            EmbeddingServerLaunchMode::NativeSpawned
        );
    }

    #[test]
    fn explicit_llamacpp_url_selects_external_endpoint_launch() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::remove("CODESTORY_EMBED_SERVER_LAUNCH");
        let _url = EnvGuard::set(
            "CODESTORY_EMBED_LLAMACPP_URL",
            "http://127.0.0.1:37040/v1/embeddings",
        );

        assert_eq!(
            embedding_server_launch_mode().expect("launch mode"),
            EmbeddingServerLaunchMode::ExternalEndpoint
        );
    }

    #[test]
    fn blank_llamacpp_url_does_not_select_external_endpoint_launch() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::remove("CODESTORY_EMBED_SERVER_LAUNCH");
        let _url = EnvGuard::set("CODESTORY_EMBED_LLAMACPP_URL", " ");
        let _host = EnvGuard::set(TEST_HOST_PLATFORM_ENV, "linux/x86_64");
        let _allow_cpu = EnvGuard::remove("CODESTORY_EMBED_ALLOW_CPU");
        let _policy = EnvGuard::remove("CODESTORY_EMBED_DEVICE_POLICY");

        assert_eq!(
            embedding_server_launch_mode().expect("launch mode"),
            EmbeddingServerLaunchMode::DockerComposeEmbed
        );
    }

    #[test]
    fn explicit_external_launch_requires_non_empty_llamacpp_url() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::set("CODESTORY_EMBED_SERVER_LAUNCH", "external_endpoint");
        let _url = EnvGuard::set("CODESTORY_EMBED_LLAMACPP_URL", " ");

        let error = embedding_server_launch_mode().expect_err("blank external url");

        assert!(
            error
                .to_string()
                .contains("CODESTORY_EMBED_SERVER_LAUNCH must be"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn managed_llamacpp_url_keeps_native_launch_without_environment_activation() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::remove("CODESTORY_EMBED_SERVER_LAUNCH");
        let _url = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_URL");
        let _host = EnvGuard::set(TEST_HOST_PLATFORM_ENV, "windows/x86_64");
        let _allow_cpu = EnvGuard::remove("CODESTORY_EMBED_ALLOW_CPU");
        let _policy = EnvGuard::remove("CODESTORY_EMBED_DEVICE_POLICY");
        let runtime = SidecarRuntimeConfig::for_project_profile(None, SidecarProfile::Agent);

        assert_eq!(
            embedding_server_launch_mode_for_runtime(&runtime).expect("launch mode"),
            EmbeddingServerLaunchMode::NativeSpawned
        );
        assert_eq!(
            runtime.embedding.endpoint,
            SidecarLayout::embed_base_url(runtime.embed_http_port)
        );
    }

    #[test]
    fn managed_llamacpp_url_is_retained_in_runtime_without_environment_mutation() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::remove("CODESTORY_EMBED_SERVER_LAUNCH");
        let _url = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_URL");
        let runtime = SidecarRuntimeConfig::for_project_profile(None, SidecarProfile::Agent);

        assert_eq!(
            runtime.embedding.endpoint,
            SidecarLayout::embed_base_url(runtime.embed_http_port)
        );
        assert!(std::env::var_os("CODESTORY_EMBED_LLAMACPP_URL").is_none());
    }

    #[test]
    fn retained_runtime_profile_selection_does_not_reparse_process_environment() {
        let _lock = crate::test_support::env_lock();
        let cache = tempdir().expect("cache");
        let poison_cache = tempdir().expect("poison cache");
        let _cache = EnvGuard::remove("CODESTORY_CACHE_ROOT");
        let _qdrant = EnvGuard::remove("CODESTORY_QDRANT_HTTP_PORT");
        let _endpoint = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_URL");
        let retained = SidecarRuntimeConfig::for_project_profile_with_run_id_in_cache(
            None,
            SidecarProfile::Local,
            None,
            cache.path(),
        );
        let _cache = EnvGuard::set(
            "CODESTORY_CACHE_ROOT",
            poison_cache.path().to_str().expect("utf8 poison cache"),
        );
        let _qdrant = EnvGuard::set("CODESTORY_QDRANT_HTTP_PORT", "39999");
        let _endpoint = EnvGuard::set(
            "CODESTORY_EMBED_LLAMACPP_URL",
            "http://127.0.0.1:39998/v1/embeddings",
        );
        let _run_id = EnvGuard::set("CODESTORY_AGENT_RUN_ID", "poison-run-id");

        let selected = retained.with_profile_and_run_id(None, SidecarProfile::Local, None);
        let selected_agent = retained.with_profile_and_run_id(None, SidecarProfile::Agent, None);

        assert_eq!(selected.layout.qdrant_http_port, DEFAULT_QDRANT_HTTP_PORT);
        assert_eq!(
            selected.layout.state_file,
            cache.path().join("retrieval-sidecars.json")
        );
        assert_eq!(selected.embedding, retained.embedding);
        assert!(!selected.layout.state_file.starts_with(poison_cache.path()));
        assert_eq!(selected_agent.run_id.as_deref(), Some(DEFAULT_AGENT_RUN_ID));
        assert!(!selected_agent.namespace.contains("poison-run-id"));
    }

    #[test]
    fn retained_runtime_captures_removed_onnx_configuration_error() {
        let _lock = crate::test_support::env_lock();
        let _legacy = EnvGuard::set("CODESTORY_EMBED_ONNX_MODEL", "legacy.onnx");

        let runtime = SidecarRuntimeConfig::local();

        let error = runtime
            .embedding
            .configuration_error
            .as_deref()
            .expect("removed ONNX configuration error");
        assert!(error.contains("CODESTORY_EMBED_ONNX_MODEL"), "{error}");
        assert!(error.contains("no longer supported"), "{error}");
    }

    #[test]
    fn ownership_redacts_external_embedding_endpoint_secrets() {
        let mut runtime = SidecarRuntimeConfig::local();
        runtime.embedding.endpoint =
            "http://username-secret:password-secret@127.0.0.1:8080/v1/embeddings?token=query-secret#fragment-secret"
                .into();

        let ownership = runtime.ownership();

        assert_eq!(
            ownership.ports.embed_url,
            "http://127.0.0.1:8080/v1/embeddings"
        );
        for secret in [
            "username-secret",
            "password-secret",
            "query-secret",
            "fragment-secret",
        ] {
            assert!(!ownership.ports.embed_url.contains(secret));
        }
    }

    #[test]
    fn explicit_native_launch_rewrites_external_endpoint_to_managed_sidecar() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::set("CODESTORY_EMBED_SERVER_LAUNCH", "native_spawned");
        let _url = EnvGuard::set(
            "CODESTORY_EMBED_LLAMACPP_URL",
            "http://127.0.0.1:37040/v1/embeddings",
        );
        let runtime = SidecarRuntimeConfig::for_project_profile(None, SidecarProfile::Agent);

        assert_eq!(
            runtime.embedding.endpoint_origin,
            EmbeddingEndpointOrigin::ManagedSidecar
        );
        assert_eq!(
            runtime.embedding.endpoint,
            SidecarLayout::embed_base_url(runtime.embed_http_port)
        );
        assert_eq!(
            embedding_server_launch_mode_for_runtime(&runtime).expect("native launch"),
            EmbeddingServerLaunchMode::NativeSpawned
        );
    }

    #[test]
    fn external_endpoint_forces_external_launch_instead_of_managed_launch() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::set("CODESTORY_EMBED_SERVER_LAUNCH", "docker_compose_embed");
        let endpoint = "http://127.0.0.1:37040/v1/embeddings";
        let _url = EnvGuard::set("CODESTORY_EMBED_LLAMACPP_URL", endpoint);
        let runtime = SidecarRuntimeConfig::for_project_profile(None, SidecarProfile::Agent);

        assert_eq!(runtime.embedding.endpoint, endpoint);
        assert_eq!(
            runtime.embedding.endpoint_origin,
            EmbeddingEndpointOrigin::ProcessEnvironment
        );
        assert_eq!(
            runtime.embedding.server_launch.as_deref(),
            Some("external_endpoint")
        );
        assert_eq!(
            embedding_server_launch_mode_for_runtime(&runtime).expect("external launch"),
            EmbeddingServerLaunchMode::ExternalEndpoint
        );
    }

    #[test]
    fn invalid_embedding_launch_mode_fails_closed() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::set("CODESTORY_EMBED_SERVER_LAUNCH", "llama-server.exe");
        let _url = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_URL");

        let error = embedding_server_launch_mode().expect_err("invalid mode");

        assert!(
            error
                .to_string()
                .contains("CODESTORY_EMBED_SERVER_LAUNCH must be")
        );
    }

    #[test]
    fn simulated_macos_arm64_accelerator_required_selects_native_launch() {
        let _lock = crate::test_support::env_lock();
        let _host = EnvGuard::set(TEST_HOST_PLATFORM_ENV, "macos/aarch64");
        let _mode = EnvGuard::remove("CODESTORY_EMBED_SERVER_LAUNCH");
        let _url = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_URL");
        let _allow_cpu = EnvGuard::remove("CODESTORY_EMBED_ALLOW_CPU");
        let _policy = EnvGuard::remove("CODESTORY_EMBED_DEVICE_POLICY");

        assert_eq!(
            embedding_server_launch_mode().expect("launch mode"),
            EmbeddingServerLaunchMode::NativeSpawned
        );
    }

    #[test]
    fn simulated_linux_accelerator_required_keeps_docker_launch() {
        let _lock = crate::test_support::env_lock();
        let _host = EnvGuard::set(TEST_HOST_PLATFORM_ENV, "linux/x86_64");
        let _mode = EnvGuard::remove("CODESTORY_EMBED_SERVER_LAUNCH");
        let _url = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_URL");
        let _allow_cpu = EnvGuard::remove("CODESTORY_EMBED_ALLOW_CPU");
        let _policy = EnvGuard::remove("CODESTORY_EMBED_DEVICE_POLICY");

        assert_eq!(
            embedding_server_launch_mode().expect("launch mode"),
            EmbeddingServerLaunchMode::DockerComposeEmbed
        );
    }

    #[test]
    fn simulated_windows_x64_selects_current_vulkan_manifest_backend() {
        let _lock = crate::test_support::env_lock();
        let _host = EnvGuard::set(TEST_HOST_PLATFORM_ENV, "windows/x86_64");

        let selected = selected_llama_sidecar_backend("vulkan").expect("windows vulkan backend");
        let matching = llama_sidecar_backends("vulkan");

        assert_eq!(selected.id, "windows-x86_64-vulkan");
        assert_eq!(selected.artifact, "llama-b9902-bin-win-vulkan-x64.zip");
        assert_eq!(selected.executable_archive_path, "llama-server.exe");
        assert!(selected.managed_cache_rel_dir.contains("/llama/b9902/"));
        assert!(
            matching
                .iter()
                .any(|backend| backend.id == "windows-x86_64-vulkan-b9058-legacy"
                    && backend.artifact == "llama-b9058-bin-win-vulkan-x64.zip"
                    && !backend.sha256.is_empty()
                    && !backend.executable_sha256.is_empty())
        );
    }

    #[test]
    fn linux_backend_matrix_exposes_vulkan_and_contract_only_cells() {
        let _lock = crate::test_support::env_lock();
        let _host = EnvGuard::set(TEST_HOST_PLATFORM_ENV, "linux/x86_64");

        let vulkan = selected_llama_sidecar_backend("vulkan").expect("linux vulkan backend");
        let cuda = selected_llama_sidecar_backend("cuda").expect("linux cuda contract cell");
        let hip = selected_llama_sidecar_backend("hip").expect("linux hip contract cell");
        let sycl = selected_llama_sidecar_backend("sycl").expect("linux sycl contract cell");
        let openvino =
            selected_llama_sidecar_backend("openvino").expect("linux openvino contract cell");

        assert_eq!(vulkan.id, "linux-x86_64-vulkan");
        assert_eq!(vulkan.launch_mode, "docker_compose_embed");
        assert_eq!(vulkan.artifact, "llama-b9902-bin-ubuntu-vulkan-x64.tar.gz");
        assert!(!vulkan.sha256.is_empty());
        for backend in [cuda, hip, sycl, openvino] {
            assert_eq!(backend.launch_mode, "docker_compose_embed");
            assert!(
                backend.artifact.is_empty(),
                "{} should stay contract-only until packaged proof exists",
                backend.id
            );
        }
    }

    #[test]
    fn linux_arm64_vulkan_cell_is_explicit() {
        let _lock = crate::test_support::env_lock();
        let _host = EnvGuard::set(TEST_HOST_PLATFORM_ENV, "linux/aarch64");

        let vulkan = selected_llama_sidecar_backend("vulkan").expect("linux arm64 vulkan backend");

        assert_eq!(vulkan.id, "linux-aarch64-vulkan");
        assert_eq!(vulkan.launch_mode, "docker_compose_embed");
        assert_eq!(
            vulkan.artifact,
            "llama-b9902-bin-ubuntu-vulkan-arm64.tar.gz"
        );
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
