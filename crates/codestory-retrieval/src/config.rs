use crate::port_registry::{
    AGENT_PORT_LEASE_TTL, AgentPortAllocation, allocate_agent_ports, free_local_port,
    renew_agent_port_lease, revalidate_agent_embedding_port,
};
use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use fs4::fs_std::FileExt;
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

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
pub(crate) const LOCAL_SIDECAR_NAMESPACE_V3: &str = "codestory-v3";
pub(crate) const AGENT_SIDECAR_NAMESPACE_PREFIX_V3: &str = "codestory-agent-v3-";
pub(crate) const SIDECAR_STATE_FILE_V3: &str = "retrieval-sidecars-v3.json";
pub(crate) const LEGACY_SIDECAR_STATE_FILE: &str = "retrieval-sidecars.json";
const EMBEDDING_ENDPOINT_FINGERPRINT_KEY_FILE: &str = "embedding-endpoint-fingerprint-hmac.key";
const EMBEDDING_ENDPOINT_FINGERPRINT_KEY_LOCK_FILE: &str =
    "embedding-endpoint-fingerprint-hmac.lock";
const EMBEDDING_ENDPOINT_FINGERPRINT_KEY_BYTES: usize = 32;
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
    #[serde(default)]
    pub artifact_bytes: u64,
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

/// Immutable process-scoped inputs used to construct sidecar runtimes.
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
    pub embedding_endpoint_fingerprint_sha256: String,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct SidecarRuntimeConfig {
    pub project_identity: Option<codestory_workspace::ProjectIdentityV3>,
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
        let cache_root = process_defaults.cache_root();
        let defaults = process_defaults.runtime();
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
        let cache_root = process_defaults.cache_root();
        let defaults = process_defaults.runtime();
        let base = cache_root.to_path_buf();
        let run_id = (profile == SidecarProfile::Agent).then(|| agent_run_id(run_id, defaults));
        let project_identity = project_root.map(codestory_workspace::project_identity_v3);
        let namespace = namespace_for(project_identity.as_ref(), profile, run_id.as_deref());
        let state_file = match profile {
            SidecarProfile::Local => base.join(SIDECAR_STATE_FILE_V3),
            SidecarProfile::Agent => base
                .join("sidecars")
                .join(&namespace)
                .join(SIDECAR_STATE_FILE_V3),
        };
        let embedding_selection =
            embedding_runtime_config(DEFAULT_EMBED_HTTP_PORT, defaults, overrides);
        let stored_value = read_sidecar_state_value(&state_file);
        let stored_value = stored_value.filter(|value| {
            sidecar_state_matches_runtime_selection(
                value,
                project_identity.as_ref(),
                profile,
                &namespace,
                run_id.as_deref(),
                &embedding_selection,
                cache_root,
            )
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
            embedding_endpoint_fingerprint_sha256: self
                .embedding_endpoint_fingerprint()
                .unwrap_or_default(),
            labels: self.labels.clone(),
        }
    }

    #[doc(hidden)]
    pub fn legacy_state_path_for_compatibility(&self) -> Option<PathBuf> {
        legacy_state_path_for_runtime(self)
    }

    pub(crate) fn embedding_endpoint_fingerprint(&self) -> Result<String> {
        let cache_root = self
            .cache_root()
            .context("derive cache root for the embedding endpoint fingerprint key")?;
        embedding_endpoint_fingerprint_sha256(&self.embedding.endpoint, cache_root)
    }

    fn cache_root(&self) -> Option<&Path> {
        match self.profile {
            SidecarProfile::Local => self.layout.state_file.parent(),
            SidecarProfile::Agent => self.layout.state_file.ancestors().nth(3),
        }
    }

    pub(crate) fn validated_project_identity(
        &self,
        project_root: &Path,
    ) -> Result<codestory_workspace::ProjectIdentityV3> {
        let current = codestory_workspace::project_identity_v3(project_root);
        let Some(retained) = self.project_identity.as_ref() else {
            return Ok(current);
        };
        if retained != &current {
            anyhow::bail!(
                "project identity changed after sidecar runtime selection: retained_workspace_id={} retained_artifact_scope_id={} current_workspace_id={} current_artifact_scope_id={}; rebuild the runtime before publishing or querying retrieval artifacts",
                retained.workspace_id,
                retained.artifact_scope_id,
                current.workspace_id,
                current.artifact_scope_id,
            );
        }
        Ok(retained.clone())
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
                "agent sidecar port allocation is unavailable; inspect sidecars/port-allocations.sqlite3 and retry"
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
        let process_defaults = SidecarProcessDefaults::new(
            cache_root.to_path_buf(),
            SidecarRuntimeDefaults::default(),
        );
        let mut selected = Self::for_project_profile_with_process_defaults(
            project_root,
            profile,
            run_id,
            &process_defaults,
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
        (SidecarProfile::Local, _) => LOCAL_SIDECAR_NAMESPACE_V3.into(),
        (SidecarProfile::Agent, Some(identity)) => {
            format!(
                "{AGENT_SIDECAR_NAMESPACE_PREFIX_V3}{}-{}",
                identity.workspace_id,
                run_id.unwrap_or("run")
            )
        }
        (SidecarProfile::Agent, None) => format!(
            "{AGENT_SIDECAR_NAMESPACE_PREFIX_V3}{}-{}",
            std::process::id(),
            run_id.unwrap_or("run")
        ),
    }
}

fn agent_namespace_prefix(project_root: &Path) -> String {
    format!(
        "{AGENT_SIDECAR_NAMESPACE_PREFIX_V3}{}-",
        codestory_workspace::workspace_id_v3_for_root(project_root)
    )
}

fn legacy_agent_namespace_prefix(project_root: &Path) -> String {
    format!(
        "codestory-agent-{}-",
        fnv1a_hex(project_root.to_string_lossy().as_bytes())
    )
}

pub(crate) fn legacy_state_file_for_runtime(runtime: &SidecarRuntimeConfig) -> Option<PathBuf> {
    let path = legacy_state_path_for_runtime(runtime)?;
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    let legacy_namespace = match runtime.profile {
        SidecarProfile::Local => "codestory",
        SidecarProfile::Agent => path.parent()?.file_name()?.to_str()?,
    };
    let value = read_sidecar_state_value(&path)?;
    let project_identity_matches = match runtime.labels.get("dev.codestory.workspace_root") {
        Some(project_root) => value
            .get("project_identity")
            .cloned()
            .and_then(|identity| serde_json::from_value(identity).ok())
            .is_some_and(|stored: codestory_workspace::ProjectIdentityV2| {
                stored == codestory_workspace::project_identity_v2(Path::new(project_root))
            }),
        None => value
            .get("project_identity")
            .is_none_or(serde_json::Value::is_null),
    };
    let owned_legacy_state = value.get("owner").and_then(serde_json::Value::as_str)
        == Some("codestory")
        && value.get("profile").and_then(serde_json::Value::as_str)
            == Some(runtime.profile.as_str())
        && value.get("namespace").and_then(serde_json::Value::as_str) == Some(legacy_namespace)
        && value
            .get("compose_project")
            .and_then(serde_json::Value::as_str)
            == Some(legacy_namespace)
        && value.get("run_id").and_then(serde_json::Value::as_str) == runtime.run_id.as_deref()
        && project_identity_matches;
    owned_legacy_state.then_some(path)
}

pub(crate) fn legacy_state_path_for_runtime(runtime: &SidecarRuntimeConfig) -> Option<PathBuf> {
    let cache_root = runtime.cache_root()?;
    let path = match runtime.profile {
        SidecarProfile::Local => cache_root.join(LEGACY_SIDECAR_STATE_FILE),
        SidecarProfile::Agent => {
            let project_root = Path::new(runtime.labels.get("dev.codestory.workspace_root")?);
            let legacy_namespace = format!(
                "{}{}",
                legacy_agent_namespace_prefix(project_root),
                runtime.run_id.as_deref().unwrap_or("run")
            );
            cache_root
                .join("sidecars")
                .join(legacy_namespace)
                .join(LEGACY_SIDECAR_STATE_FILE)
        }
    };
    (path != runtime.layout.state_file).then_some(path)
}

fn latest_agent_run_id_in_cache(project_root: &Path, cache_root: &Path) -> Option<String> {
    let prefix = agent_namespace_prefix(project_root);
    let project_identity = codestory_workspace::project_identity_v3(project_root);
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
        let state_file = entry.path().join(SIDECAR_STATE_FILE_V3);
        let Ok(metadata) = std::fs::symlink_metadata(&state_file) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let Some(value) = read_sidecar_state_value(&state_file) else {
            continue;
        };
        if !current_v3_state_ownership_matches(&value, &namespace)
            || value.get("run_id").and_then(serde_json::Value::as_str) != Some(run_id.as_str())
            || !project_identity_matches_runtime(
                sidecar_state_project_identity(&value).as_ref(),
                Some(&project_identity),
            )
        {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
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

fn embedding_endpoint_fingerprint_sha256(endpoint: &str, cache_root: &Path) -> Result<String> {
    let key = embedding_endpoint_fingerprint_key(cache_root)?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, &key);
    let digest = hmac::sign(&key, endpoint.as_bytes());
    let mut fingerprint = String::with_capacity("hmac-sha256:".len() + 64);
    fingerprint.push_str("hmac-sha256:");
    for byte in digest.as_ref() {
        write!(&mut fingerprint, "{byte:02x}").expect("write endpoint fingerprint to string");
    }
    Ok(fingerprint)
}

fn embedding_endpoint_fingerprint_key(cache_root: &Path) -> Result<[u8; 32]> {
    std::fs::create_dir_all(cache_root)
        .with_context(|| format!("create CodeStory cache root {}", cache_root.display()))?;
    let lock_path = cache_root.join(EMBEDDING_ENDPOINT_FINGERPRINT_KEY_LOCK_FILE);
    let lock = open_private_regular_file(&lock_path, true)?;
    FileExt::lock_exclusive(&lock)
        .with_context(|| format!("lock endpoint fingerprint key {}", lock_path.display()))?;
    let path = cache_root.join(EMBEDDING_ENDPOINT_FINGERPRINT_KEY_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => validate_private_regular_metadata(&path, &metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut key = [0_u8; EMBEDDING_ENDPOINT_FINGERPRINT_KEY_BYTES];
            SystemRandom::new()
                .fill(&mut key)
                .map_err(|_| anyhow::anyhow!("generate endpoint fingerprint key"))?;
            codestory_workspace::atomic_file::write_file_atomic(
                &path,
                "embedding-endpoint-fingerprint-key",
                |file| {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        file.set_permissions(std::fs::Permissions::from_mode(0o600))
                            .context("set endpoint fingerprint key mode to 0600")?;
                    }
                    file.write_all(&key)
                        .context("write endpoint fingerprint key temporary file")
                },
                |temp_path| {
                    let metadata = std::fs::symlink_metadata(temp_path).with_context(|| {
                        format!("inspect endpoint fingerprint key {}", temp_path.display())
                    })?;
                    validate_private_regular_metadata(temp_path, &metadata)?;
                    if metadata.len() != EMBEDDING_ENDPOINT_FINGERPRINT_KEY_BYTES as u64 {
                        bail!("endpoint fingerprint key temporary file has an invalid length");
                    }
                    Ok(())
                },
            )
            .with_context(|| format!("publish endpoint fingerprint key {}", path.display()))?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect endpoint fingerprint key {}", path.display()));
        }
    }
    read_private_fingerprint_key(&path)
}

fn open_private_regular_file(path: &Path, initialize: bool) -> Result<File> {
    let file = if initialize {
        let mut options = private_file_open_options();
        match options.create_new(true).open(path) {
            Ok(file) => Ok(file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                private_file_open_options().open(path)
            }
            Err(error) => Err(error),
        }
    } else {
        private_file_open_options().open(path)
    }
    .with_context(|| format!("open private file {}", path.display()))?;
    let path_metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspect private file {}", path.display()))?;
    validate_private_regular_metadata(path, &path_metadata)?;
    let file_metadata = file
        .metadata()
        .with_context(|| format!("inspect opened private file {}", path.display()))?;
    if !file_metadata.file_type().is_file() {
        bail!("private file {} is not a regular file", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if path_metadata.dev() != file_metadata.dev() || path_metadata.ino() != file_metadata.ino()
        {
            bail!(
                "private file {} changed while it was opened",
                path.display()
            );
        }
    }
    Ok(file)
}

fn private_file_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn validate_private_regular_metadata(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    if !metadata.file_type().is_file() {
        bail!("private file {} is not a regular file", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        if mode & 0o077 != 0 {
            bail!(
                "private file {} is accessible outside its owner (mode {:o})",
                path.display(),
                mode & 0o777
            );
        }
    }
    Ok(())
}

fn read_private_fingerprint_key(path: &Path) -> Result<[u8; 32]> {
    let file = open_private_regular_file(path, false)?;
    let mut bytes = Vec::with_capacity(EMBEDDING_ENDPOINT_FINGERPRINT_KEY_BYTES + 1);
    file.take((EMBEDDING_ENDPOINT_FINGERPRINT_KEY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read endpoint fingerprint key {}", path.display()))?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        anyhow::anyhow!(
            "endpoint fingerprint key {} has {} bytes; expected {}",
            path.display(),
            bytes.len(),
            EMBEDDING_ENDPOINT_FINGERPRINT_KEY_BYTES
        )
    })
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

pub(crate) fn current_v3_state_ownership_matches(
    value: &serde_json::Value,
    discovered_namespace: &str,
) -> bool {
    let profile = value.get("profile").and_then(serde_json::Value::as_str);
    let project_identity = sidecar_state_project_identity(value);
    let current_generation = project_identity.as_ref().is_some_and(|identity| {
        identity.project_identity_schema_version
            == codestory_workspace::PROJECT_IDENTITY_V3_SCHEMA_VERSION
    });
    let profile_matches = (discovered_namespace == LOCAL_SIDECAR_NAMESPACE_V3
        && profile == Some("local")
        && (project_identity.is_none() || current_generation))
        || (discovered_namespace.starts_with(AGENT_SIDECAR_NAMESPACE_PREFIX_V3)
            && profile == Some("agent")
            && current_generation);
    profile_matches
        && value.get("owner").and_then(serde_json::Value::as_str) == Some("codestory")
        && value.get("namespace").and_then(serde_json::Value::as_str) == Some(discovered_namespace)
        && value
            .get("compose_project")
            .and_then(serde_json::Value::as_str)
            == Some(discovered_namespace)
}

pub(crate) fn project_identity_matches_runtime(
    stored: Option<&codestory_workspace::ProjectIdentityV3>,
    current: Option<&codestory_workspace::ProjectIdentityV3>,
) -> bool {
    match (stored, current) {
        (None, None) => true,
        (Some(stored), Some(current))
            if stored.project_identity_schema_version
                == codestory_workspace::PROJECT_IDENTITY_V3_SCHEMA_VERSION =>
        {
            stored == current
        }
        _ => false,
    }
}

fn sidecar_state_matches_runtime_selection(
    value: &serde_json::Value,
    project_identity: Option<&codestory_workspace::ProjectIdentityV3>,
    profile: SidecarProfile,
    namespace: &str,
    run_id: Option<&str>,
    embedding: &EmbeddingRuntimeConfig,
    cache_root: &Path,
) -> bool {
    value.get("owner").and_then(serde_json::Value::as_str) == Some("codestory")
        && value.get("profile").and_then(serde_json::Value::as_str) == Some(profile.as_str())
        && value.get("namespace").and_then(serde_json::Value::as_str) == Some(namespace)
        && value
            .get("compose_project")
            .and_then(serde_json::Value::as_str)
            == Some(namespace)
        && value.get("run_id").and_then(serde_json::Value::as_str) == run_id
        && project_identity_matches_runtime(
            sidecar_state_project_identity(value).as_ref(),
            project_identity,
        )
        && sidecar_state_embedding_selection_matches(value, embedding, cache_root)
}

fn sidecar_state_embedding_selection_matches(
    value: &serde_json::Value,
    embedding: &EmbeddingRuntimeConfig,
    cache_root: &Path,
) -> bool {
    let stored_origin = value
        .get("embedding_endpoint_origin")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok());
    let stored_endpoint = value.get("embed_url").and_then(serde_json::Value::as_str);
    let stored_fingerprint = value
        .get("embedding_endpoint_fingerprint_sha256")
        .and_then(serde_json::Value::as_str);
    if stored_origin != Some(embedding.endpoint_origin) {
        return false;
    }
    match embedding.endpoint_origin {
        EmbeddingEndpointOrigin::ManagedSidecar => stored_endpoint.is_some_and(|endpoint| {
            let Ok(endpoint_fingerprint) =
                embedding_endpoint_fingerprint_sha256(endpoint, cache_root)
            else {
                return false;
            };
            local_embedding_endpoint_port(endpoint).is_some()
                && stored_fingerprint == Some(endpoint_fingerprint.as_str())
        }),
        _ => {
            let Ok(expected_fingerprint) =
                embedding_endpoint_fingerprint_sha256(&embedding.endpoint, cache_root)
            else {
                return false;
            };
            let expected_endpoint = redacted_embedding_endpoint(&embedding.endpoint);
            stored_endpoint == Some(expected_endpoint.as_str())
                && stored_fingerprint == Some(expected_fingerprint.as_str())
        }
    }
}

fn sidecar_state_uses_native_embedding(value: &serde_json::Value) -> bool {
    value
        .get("embedding_launch")
        .and_then(|launch| launch.get("launch_mode"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|mode| mode == EmbeddingServerLaunchMode::NativeSpawned.as_str())
}

pub(crate) fn local_embedding_endpoint_port(endpoint: &str) -> Option<u16> {
    endpoint
        .strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.strip_suffix("/v1/embeddings"))
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
}

pub(crate) fn sidecar_ports_from_value(value: &serde_json::Value) -> Option<SidecarPorts> {
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
    allocate_agent_ports(base, namespace, configured).unwrap_or_else(|error| {
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

pub(crate) fn fnv1a_hex(bytes: &[u8]) -> String {
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

#[cfg(test)]
pub(crate) fn test_sidecar_runtime_from_env(
    project_root: Option<&Path>,
    profile: SidecarProfile,
    run_id: Option<&str>,
) -> SidecarRuntimeConfig {
    let defaults = SidecarProcessDefaults::new(
        uncached_user_cache_root(),
        SidecarRuntimeDefaults::from_process_env(),
    );
    SidecarRuntimeConfig::for_project_profile_with_process_defaults(
        project_root,
        profile,
        run_id,
        &defaults,
        &SidecarRuntimeOverrides::default(),
    )
}

#[cfg(test)]
pub(crate) fn test_sidecar_runtime_in_cache(
    project_root: Option<&Path>,
    profile: SidecarProfile,
    run_id: Option<&str>,
    cache_root: &Path,
) -> SidecarRuntimeConfig {
    let defaults = SidecarProcessDefaults::new(
        cache_root.to_path_buf(),
        SidecarRuntimeDefaults::from_process_env(),
    );
    SidecarRuntimeConfig::for_project_profile_with_process_defaults(
        project_root,
        profile,
        run_id,
        &defaults,
        &SidecarRuntimeOverrides::default(),
    )
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
    std::fs::create_dir_all(root.join("sidecars")).ok()?;
    Some(root)
}

#[cfg(any(test, feature = "test-support"))]
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
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    fn embedding_server_launch_mode_from_env() -> Result<EmbeddingServerLaunchMode> {
        let runtime = test_sidecar_runtime_from_env(None, SidecarProfile::Local, None);
        embedding_server_launch_mode_for_runtime(&runtime)
    }

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
    fn process_defaults_are_frozen_after_first_capture() {
        let _lock = crate::test_support::env_lock();
        let cache = tempdir().expect("cache");
        let poison = tempdir().expect("poison cache");
        let _cache = EnvGuard::set(
            "CODESTORY_CACHE_ROOT",
            cache.path().to_str().expect("cache path"),
        );
        let _port = EnvGuard::set("CODESTORY_QDRANT_HTTP_PORT", "32101");
        let cell = OnceLock::new();
        let first = frozen_process_defaults(&cell).clone();

        let _poison_cache = EnvGuard::set(
            "CODESTORY_CACHE_ROOT",
            poison.path().to_str().expect("poison cache path"),
        );
        let _poison_port = EnvGuard::set("CODESTORY_QDRANT_HTTP_PORT", "32102");
        let second = frozen_process_defaults(&cell);

        assert_eq!(first.cache_root(), cache.path());
        assert_eq!(second, &first);
        assert_eq!(
            second.runtime().get("CODESTORY_QDRANT_HTTP_PORT"),
            Some("32101")
        );
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
            test_sidecar_runtime_from_env(Some(project.path()), SidecarProfile::Agent, None);
        let second =
            test_sidecar_runtime_from_env(Some(project.path()), SidecarProfile::Agent, None);

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
            test_sidecar_runtime_from_env(Some(project.path()), SidecarProfile::Agent, None);
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
                .starts_with(&format!("codestory-agent-v3-{}-", identity.workspace_id)),
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

        let first = test_sidecar_runtime_from_env(
            Some(project.path()),
            SidecarProfile::Agent,
            Some("review-fix"),
        );
        let second = test_sidecar_runtime_from_env(
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
        let first = test_sidecar_runtime_from_env(
            Some(project.path()),
            SidecarProfile::Agent,
            Some("persisted"),
        );
        std::fs::create_dir_all(first.layout.state_file.parent().expect("state parent"))
            .expect("state dir");
        let ownership = first.ownership();
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
                "embed_url": ownership.ports.embed_url,
                "embedding_endpoint_origin": ownership.embedding_endpoint_origin,
                "embedding_endpoint_fingerprint_sha256": ownership.embedding_endpoint_fingerprint_sha256,
            }))
            .expect("state json"),
        )
        .expect("state file");

        let second = test_sidecar_runtime_from_env(
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
    fn schema_two_identity_is_never_accepted_as_runtime_identity() {
        let project = tempdir().expect("project");
        let current = codestory_workspace::project_identity_v3(project.path());
        let legacy = codestory_workspace::project_identity_v2(project.path());
        let stored: codestory_workspace::ProjectIdentityV3 = serde_json::from_value(
            serde_json::to_value(&legacy).expect("serialize legacy identity"),
        )
        .expect("read legacy identity through migration shape");

        assert!(!project_identity_matches_runtime(
            Some(&stored),
            Some(&current)
        ));
        assert!(project_identity_matches_runtime(
            Some(&current),
            Some(&current)
        ));
    }

    #[test]
    fn retained_runtime_identity_fails_closed_after_reobservation_drift() {
        let project = tempdir().expect("project");
        let mut runtime = test_sidecar_runtime_from_env(
            Some(project.path()),
            SidecarProfile::Agent,
            Some("identity-drift"),
        );
        let retained = runtime
            .validated_project_identity(project.path())
            .expect("matching retained identity");
        assert_eq!(
            retained,
            codestory_workspace::project_identity_v3(project.path())
        );

        runtime
            .project_identity
            .as_mut()
            .expect("retained identity")
            .artifact_scope_id = "stale-artifact-scope".to_string();
        let error = runtime
            .validated_project_identity(project.path())
            .expect_err("identity drift must fail closed");

        assert!(format!("{error:#}").contains("project identity changed"));
    }

    #[test]
    fn legacy_agent_namespace_is_discovered_but_not_selected() {
        let cache = tempdir().expect("cache");
        let project = tempdir().expect("project");
        let noncanonical_project = project.path().join(".");
        let run_id = "legacy-run";
        let legacy_namespace = format!(
            "{}{}",
            legacy_agent_namespace_prefix(&noncanonical_project),
            run_id
        );
        let legacy_state_file = cache
            .path()
            .join("sidecars")
            .join(&legacy_namespace)
            .join(LEGACY_SIDECAR_STATE_FILE);
        std::fs::create_dir_all(legacy_state_file.parent().expect("legacy state parent"))
            .expect("legacy state directory");
        std::fs::write(&legacy_state_file, b"{}\n").expect("legacy state file");

        let runtime = test_sidecar_runtime_in_cache(
            Some(&noncanonical_project),
            SidecarProfile::Agent,
            Some(run_id),
            cache.path(),
        );

        assert_eq!(legacy_state_file_for_runtime(&runtime), None);
        let legacy_identity = codestory_workspace::project_identity_v2(&noncanonical_project);
        std::fs::write(
            &legacy_state_file,
            serde_json::to_vec_pretty(&serde_json::json!({
                "project_identity": legacy_identity,
                "owner": "codestory",
                "profile": "agent",
                "namespace": legacy_namespace,
                "compose_project": legacy_namespace,
                "run_id": run_id,
            }))
            .expect("owned legacy state json"),
        )
        .expect("owned legacy state");

        assert_eq!(
            legacy_state_file_for_runtime(&runtime).as_ref(),
            Some(&legacy_state_file)
        );
        assert_ne!(runtime.layout.state_file, legacy_state_file);
        assert_eq!(
            latest_agent_run_id_in_cache(&noncanonical_project, cache.path()),
            None
        );

        let v3_run_id = "current-run";
        let v3_runtime = test_sidecar_runtime_in_cache(
            Some(&noncanonical_project),
            SidecarProfile::Agent,
            Some(v3_run_id),
            cache.path(),
        );
        std::fs::create_dir_all(
            v3_runtime
                .layout
                .state_file
                .parent()
                .expect("v3 state parent"),
        )
        .expect("v3 state directory");
        let mut v3_state = serde_json::to_value(v3_runtime.ownership()).expect("v3 ownership");
        v3_state["run_id"] = serde_json::json!(v3_run_id);
        std::fs::write(
            &v3_runtime.layout.state_file,
            serde_json::to_vec_pretty(&v3_state).expect("owned v3 state json"),
        )
        .expect("owned v3 state");

        assert_eq!(
            latest_agent_run_id_in_cache(&noncanonical_project, cache.path()).as_deref(),
            Some(v3_run_id)
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
            embedding_server_launch_mode_from_env().expect("launch mode"),
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
            embedding_server_launch_mode_from_env().expect("launch mode"),
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
            embedding_server_launch_mode_from_env().expect("launch mode"),
            EmbeddingServerLaunchMode::DockerComposeEmbed
        );
    }

    #[test]
    fn explicit_external_launch_requires_non_empty_llamacpp_url() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::set("CODESTORY_EMBED_SERVER_LAUNCH", "external_endpoint");
        let _url = EnvGuard::set("CODESTORY_EMBED_LLAMACPP_URL", " ");

        let error = embedding_server_launch_mode_from_env().expect_err("blank external url");

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
        let runtime = test_sidecar_runtime_from_env(None, SidecarProfile::Agent, None);

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
        let runtime = test_sidecar_runtime_from_env(None, SidecarProfile::Agent, None);

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
        let retained =
            test_sidecar_runtime_in_cache(None, SidecarProfile::Local, None, cache.path());
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
            cache.path().join(SIDECAR_STATE_FILE_V3)
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

        let runtime = test_sidecar_runtime_from_env(None, SidecarProfile::Local, None);

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
        let cache = tempdir().expect("cache");
        let mut runtime =
            test_sidecar_runtime_in_cache(None, SidecarProfile::Local, None, cache.path());
        runtime.embedding.endpoint =
            "http://username-secret:password-secret@127.0.0.1:8080/v1/embeddings?token=query-secret#fragment-secret"
                .into();
        runtime.embedding.endpoint_origin = EmbeddingEndpointOrigin::ProcessEnvironment;

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
        assert_eq!(
            ownership.embedding_endpoint_fingerprint_sha256,
            runtime
                .embedding_endpoint_fingerprint()
                .expect("keyed endpoint fingerprint")
        );
        assert!(
            ownership
                .embedding_endpoint_fingerprint_sha256
                .starts_with("hmac-sha256:")
        );
        assert_ne!(
            ownership.embedding_endpoint_fingerprint_sha256,
            format!(
                "{:x}",
                Sha256::digest(runtime.embedding.endpoint.as_bytes())
            )
        );
        let mut restarted =
            test_sidecar_runtime_in_cache(None, SidecarProfile::Local, None, cache.path());
        restarted.embedding.endpoint = runtime.embedding.endpoint.clone();
        restarted.embedding.endpoint_origin = runtime.embedding.endpoint_origin;
        let retained = restarted.ownership();
        assert_eq!(
            retained.embedding_endpoint_fingerprint_sha256,
            ownership.embedding_endpoint_fingerprint_sha256
        );

        runtime.embedding.endpoint =
            "http://different-user:different-password@127.0.0.1:8080/v1/embeddings?token=different"
                .into();
        let different_secret = runtime.ownership();
        assert_eq!(different_secret.ports.embed_url, ownership.ports.embed_url);
        assert_ne!(
            different_secret.embedding_endpoint_fingerprint_sha256,
            ownership.embedding_endpoint_fingerprint_sha256
        );
    }

    #[test]
    fn endpoint_fingerprint_key_publication_is_atomic_and_rejects_unsafe_files() {
        let cache = tempdir().expect("cache");
        let cache = std::sync::Arc::new(cache.path().to_path_buf());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers = [(), ()].map(|()| {
            let cache = cache.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                embedding_endpoint_fingerprint_key(&cache).expect("fingerprint key")
            })
        });
        let keys = workers.map(|worker| worker.join().expect("fingerprint worker"));
        let key_path = cache.join(EMBEDDING_ENDPOINT_FINGERPRINT_KEY_FILE);

        assert_eq!(keys[0], keys[1]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};

            assert_eq!(
                std::fs::metadata(&key_path)
                    .expect("key metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            let unsafe_cache = tempdir().expect("unsafe cache");
            let unsafe_key = unsafe_cache
                .path()
                .join(EMBEDDING_ENDPOINT_FINGERPRINT_KEY_FILE);
            let external = unsafe_cache.path().join("external-key");
            std::fs::write(&external, [7_u8; EMBEDDING_ENDPOINT_FINGERPRINT_KEY_BYTES])
                .expect("external key");
            symlink(&external, &unsafe_key).expect("key symlink");
            assert!(embedding_endpoint_fingerprint_key(unsafe_cache.path()).is_err());
            assert_eq!(
                std::fs::read(&external).expect("external key remains"),
                [7_u8; EMBEDDING_ENDPOINT_FINGERPRINT_KEY_BYTES]
            );
            std::fs::remove_file(&unsafe_key).expect("remove symlink");
            let lock_path = unsafe_cache
                .path()
                .join(EMBEDDING_ENDPOINT_FINGERPRINT_KEY_LOCK_FILE);
            std::fs::remove_file(&lock_path).expect("remove initialized lock");
            let dangling_target = unsafe_cache.path().join("dangling-lock-target");
            symlink(&dangling_target, &lock_path).expect("dangling lock symlink");
            assert!(embedding_endpoint_fingerprint_key(unsafe_cache.path()).is_err());
            assert!(!dangling_target.exists());
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
        let runtime = test_sidecar_runtime_from_env(None, SidecarProfile::Agent, None);

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
        let runtime = test_sidecar_runtime_from_env(None, SidecarProfile::Agent, None);

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

        let error = embedding_server_launch_mode_from_env().expect_err("invalid mode");

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
            embedding_server_launch_mode_from_env().expect("launch mode"),
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
            embedding_server_launch_mode_from_env().expect("launch mode"),
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
