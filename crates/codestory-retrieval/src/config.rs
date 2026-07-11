use anyhow::{Context, Result};
use directories::ProjectDirs;
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Phase 2 lexical shard pin (local index + optional Zoekt webserver).
pub const ZOEKT_REAL_VERSION_PIN: &str = "zoekt-20250506123554";

/// Zoekt webserver image for `COMPOSE_PROFILES=real`.
pub const ZOEKT_WEBSERVER_IMAGE_PIN: &str = "sourcegraph/zoekt-webserver:0.0.0-20250506123554-490422d1adb4@sha256:34c77a62bcafc41ce3ee193e44f42aa84690d9ec51b953e7efae4dfdfae80aff";

/// Qdrant container image pin for local dev and CI smoke.
pub const QDRANT_IMAGE_PIN: &str =
    "qdrant/qdrant:v1.12.5@sha256:05fecce7dce45d1254e0468bc037e8210e187fd56fa847688b012293d5f08aae";

/// llama.cpp server image for `COMPOSE_PROFILES=real` embed service (see `docker/retrieval-compose.yml`).
#[allow(dead_code)]
pub const LLAMACPP_SERVER_IMAGE_PIN: &str = "ghcr.io/ggml-org/llama.cpp:server@sha256:f16ca66f3ba316b7a7a16003ddfa88d29c3404fbe86550da086736864c11574c";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarImagePins {
    pub qdrant: String,
    pub zoekt: String,
    pub embed: String,
}

pub fn default_sidecar_image_pins() -> SidecarImagePins {
    SidecarImagePins {
        qdrant: QDRANT_IMAGE_PIN.into(),
        zoekt: ZOEKT_WEBSERVER_IMAGE_PIN.into(),
        embed: LLAMACPP_SERVER_IMAGE_PIN.into(),
    }
}

pub const DEFAULT_ZOEKT_HTTP_PORT: u16 = 6070;
pub const DEFAULT_QDRANT_HTTP_PORT: u16 = 6333;
pub const DEFAULT_QDRANT_GRPC_PORT: u16 = 6334;
pub const DEFAULT_EMBED_HTTP_PORT: u16 = 8080;
pub const DEFAULT_AGENT_RUN_ID: &str = "shared-agent";
pub const NATIVE_LLAMA_MANAGED_CACHE_REL_PATH: &str =
    "managed-embeddings/llama/b9058/llama-b9058-bin-win-vulkan-x64/llama-server.exe";
pub const NATIVE_LLAMA_SOURCE_CACHE_REL_PATH: &str = "target/llamacpp/b8840/llama-server.exe";
const LLAMA_SIDECAR_BACKENDS_JSON: &str = include_str!("../assets/llama-sidecar-backends.json");
const TEST_HOST_PLATFORM_ENV: &str = "CODESTORY_TEST_HOST_PLATFORM";
const AGENT_PORT_RESERVATION_GRACE: Duration = Duration::from_secs(10 * 60);

pub const ZOEKT_HEALTH_BUDGET: Duration = Duration::from_millis(100);
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
    pub zoekt_http_port: u16,
    pub qdrant_http_port: u16,
    pub qdrant_grpc_port: u16,
    pub zoekt_data_dir: PathBuf,
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
    pub zoekt_http: u16,
    pub qdrant_http: u16,
    pub qdrant_grpc: u16,
    pub embed_http: u16,
    pub embed_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SidecarOwnership {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_identity: Option<codestory_workspace::ProjectIdentityV2>,
    pub owner: String,
    pub profile: String,
    pub namespace: String,
    pub compose_project: String,
    pub state_file: String,
    pub cleanup_command: String,
    pub ports: SidecarPorts,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct SidecarRuntimeConfig {
    pub project_identity: Option<codestory_workspace::ProjectIdentityV2>,
    pub layout: SidecarLayout,
    pub profile: SidecarProfile,
    pub run_id: Option<String>,
    pub namespace: String,
    pub compose_project: String,
    pub embed_http_port: u16,
    pub cleanup_command: String,
    pub labels: BTreeMap<String, String>,
}

impl SidecarLayout {
    pub fn from_env() -> Self {
        Self::from_env_for_profile(None, SidecarProfile::Local)
    }

    pub fn from_env_for_project(project_root: &Path) -> Self {
        let runtime = SidecarRuntimeConfig::for_project_auto(project_root);
        runtime.activate_embed_url_default();
        runtime.layout
    }

    fn from_env_for_profile(project_root: Option<&Path>, profile: SidecarProfile) -> Self {
        SidecarRuntimeConfig::for_project_profile(project_root, profile).layout
    }

    pub fn from_env_agent(project_root: &Path) -> Self {
        let runtime =
            SidecarRuntimeConfig::for_project_profile(Some(project_root), SidecarProfile::Agent);
        runtime.activate_embed_url_default();
        runtime.layout
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
        let explicit_profile = env_profile();
        let env_run_id = env_agent_run_id();
        let latest_run_id = if explicit_profile.is_none() && env_run_id.is_none() {
            latest_agent_run_id(project_root)
        } else {
            None
        };
        let (profile, run_id) = auto_runtime_selection(
            explicit_profile,
            env_run_id,
            latest_run_id,
            running_in_ci_agent(),
        );
        Self::for_project_profile_with_run_id(Some(project_root), profile, run_id.as_deref())
    }

    pub fn for_project_profile(project_root: Option<&Path>, profile: SidecarProfile) -> Self {
        Self::for_project_profile_with_run_id(project_root, profile, None)
    }

    pub fn for_project_profile_with_run_id(
        project_root: Option<&Path>,
        profile: SidecarProfile,
        run_id: Option<&str>,
    ) -> Self {
        let base = user_cache_root();
        let run_id = (profile == SidecarProfile::Agent).then(|| agent_run_id(run_id));
        let namespace = namespace_for(project_root, profile, run_id.as_deref());
        let state_file = match profile {
            SidecarProfile::Local => base.join("retrieval-sidecars.json"),
            SidecarProfile::Agent => base
                .join("sidecars")
                .join(&namespace)
                .join("retrieval-sidecars.json"),
        };
        let stored = read_ports_from_state(&state_file);
        let dynamic = profile == SidecarProfile::Agent && stored.is_none();
        let configured_ports = [
            env_port("CODESTORY_ZOEKT_PORT", DEFAULT_ZOEKT_HTTP_PORT),
            env_port("CODESTORY_QDRANT_HTTP_PORT", DEFAULT_QDRANT_HTTP_PORT),
            env_port("CODESTORY_QDRANT_GRPC_PORT", DEFAULT_QDRANT_GRPC_PORT),
            env_port("CODESTORY_EMBED_PORT", DEFAULT_EMBED_HTTP_PORT),
        ];
        let dynamic_ports =
            dynamic.then(|| dynamic_agent_ports(&base, &namespace, configured_ports));
        let dynamic_failed = dynamic_ports.as_ref().is_some_and(|ports| {
            [
                ports.zoekt_http,
                ports.qdrant_http,
                ports.qdrant_grpc,
                ports.embed_http,
            ]
            .contains(&0)
        });
        let selected_port =
            |configured: Option<u16>, stored: Option<u16>, dynamic: Option<u16>, default| {
                if dynamic_failed {
                    0
                } else {
                    configured.or(stored).or(dynamic).unwrap_or(default)
                }
            };
        let zoekt_http_port = selected_port(
            configured_ports[0],
            stored.as_ref().map(|ports| ports.zoekt_http),
            dynamic_ports.as_ref().map(|ports| ports.zoekt_http),
            DEFAULT_ZOEKT_HTTP_PORT,
        );
        let qdrant_http_port = selected_port(
            configured_ports[1],
            stored.as_ref().map(|ports| ports.qdrant_http),
            dynamic_ports.as_ref().map(|ports| ports.qdrant_http),
            DEFAULT_QDRANT_HTTP_PORT,
        );
        let qdrant_grpc_port = selected_port(
            configured_ports[2],
            stored.as_ref().map(|ports| ports.qdrant_grpc),
            dynamic_ports.as_ref().map(|ports| ports.qdrant_grpc),
            DEFAULT_QDRANT_GRPC_PORT,
        );
        let embed_http_port = selected_port(
            configured_ports[3],
            stored.as_ref().map(|ports| ports.embed_http),
            dynamic_ports.as_ref().map(|ports| ports.embed_http),
            DEFAULT_EMBED_HTTP_PORT,
        );
        let root = match profile {
            SidecarProfile::Local => base.clone(),
            SidecarProfile::Agent => base.join("sidecars").join(&namespace),
        };
        let layout = SidecarLayout {
            zoekt_http_port,
            qdrant_http_port,
            qdrant_grpc_port,
            zoekt_data_dir: root.join("zoekt"),
            qdrant_data_dir: root.join("qdrant"),
            scip_artifacts_root: root.join("scip"),
            state_file: state_file.clone(),
        };
        let cleanup_command = project_root
            .map(|path| retrieval_command("down", path, profile, run_id.as_deref(), None))
            .unwrap_or_else(|| "codestory-cli retrieval down".to_string());
        let project_identity = project_root.map(codestory_workspace::cached_project_identity_v2);
        let mut labels = BTreeMap::new();
        labels.insert("dev.codestory.owner".into(), "codestory".into());
        labels.insert("dev.codestory.profile".into(), profile.as_str().into());
        labels.insert("dev.codestory.namespace".into(), namespace.clone());
        if let Some(project_root) = project_root {
            let hash = project_hash(project_root);
            labels.insert("dev.codestory.project_hash".into(), hash.clone());
            if let Some(identity) = project_identity.as_ref() {
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
                zoekt_http: self.layout.zoekt_http_port,
                qdrant_http: self.layout.qdrant_http_port,
                qdrant_grpc: self.layout.qdrant_grpc_port,
                embed_http: self.embed_http_port,
                embed_url: SidecarLayout::embed_base_url(self.embed_http_port),
            },
            labels: self.labels.clone(),
        }
    }

    pub(crate) fn ensure_ports_allocated(&self) -> Result<()> {
        if self.profile == SidecarProfile::Agent
            && [
                self.layout.zoekt_http_port,
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
        Ok(())
    }

    pub fn activate_embed_url_default(&self) {
        let url_missing_or_blank = std::env::var("CODESTORY_EMBED_LLAMACPP_URL")
            .ok()
            .is_none_or(|value| value.trim().is_empty());
        let managed_url = std::env::var(MANAGED_LLAMACPP_URL_ENV).is_ok();

        if url_missing_or_blank || managed_url {
            // SAFETY: this is command-local setup before sidecar probes/query embedding calls.
            unsafe {
                std::env::set_var(
                    "CODESTORY_EMBED_LLAMACPP_URL",
                    SidecarLayout::embed_base_url(self.embed_http_port),
                );
                std::env::set_var(MANAGED_LLAMACPP_URL_ENV, "1");
            }
        }
    }

    pub fn activate_embed_url(&self) {
        // SAFETY: this is command-local setup before sidecar probes/query embedding calls.
        unsafe {
            std::env::set_var(
                "CODESTORY_EMBED_LLAMACPP_URL",
                SidecarLayout::embed_base_url(self.embed_http_port),
            );
            std::env::set_var(MANAGED_LLAMACPP_URL_ENV, "1");
        }
    }
}

impl SidecarLayout {
    pub fn zoekt_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.zoekt_http_port)
    }

    pub fn qdrant_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.qdrant_http_port)
    }

    pub fn scip_project_dir(&self, project_id: &str) -> PathBuf {
        self.scip_artifacts_root.join(project_id)
    }

    pub fn ensure_data_dirs(&self) -> Result<()> {
        for dir in [
            &self.zoekt_data_dir,
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

fn env_profile() -> Option<SidecarProfile> {
    std::env::var("CODESTORY_RETRIEVAL_PROFILE")
        .ok()
        .or_else(|| std::env::var("CODESTORY_SIDECAR_PROFILE").ok())
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "agent" | "ci" => Some(SidecarProfile::Agent),
            "local" | "dev" => Some(SidecarProfile::Local),
            _ => None,
        })
}

fn env_agent_run_id() -> Option<String> {
    std::env::var("CODESTORY_SIDECAR_RUN_ID")
        .ok()
        .or_else(|| std::env::var("CODESTORY_AGENT_RUN_ID").ok())
        .and_then(|value| normalized_label_component(&value))
}

fn running_in_ci_agent() -> bool {
    env_flag("CODESTORY_AGENT", false)
        || env_flag("CODESTORY_AGENT_RUN", false)
        || env_flag("CI", false)
        || env_flag("GITHUB_ACTIONS", false)
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
    project_root: Option<&Path>,
    profile: SidecarProfile,
    run_id: Option<&str>,
) -> String {
    match (profile, project_root) {
        (SidecarProfile::Local, _) => "codestory".into(),
        (SidecarProfile::Agent, Some(path)) => {
            format!(
                "codestory-agent-{}-{}",
                project_hash(path),
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

fn project_hash(project_root: &Path) -> String {
    fnv1a_hex(project_root.to_string_lossy().as_bytes())
}

fn agent_namespace_prefix(project_root: &Path) -> String {
    format!("codestory-agent-{}-", project_hash(project_root))
}

fn latest_agent_run_id(project_root: &Path) -> Option<String> {
    let prefix = agent_namespace_prefix(project_root);
    let sidecars_root = user_cache_root().join("sidecars");
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

fn agent_run_id(explicit: Option<&str>) -> String {
    explicit
        .and_then(normalized_label_component)
        .or_else(env_agent_run_id)
        .unwrap_or_else(default_agent_run_id)
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

fn read_ports_from_state(path: &Path) -> Option<SidecarPorts> {
    let value =
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(path).ok()?).ok()?;
    Some(SidecarPorts {
        zoekt_http: value.get("zoekt_http_port")?.as_u64()?.try_into().ok()?,
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

fn dynamic_agent_ports(base: &Path, namespace: &str, configured: [Option<u16>; 4]) -> SidecarPorts {
    allocate_agent_ports_in_registry(base, namespace, configured).unwrap_or_else(|error| {
        eprintln!(
            "CodeStory sidecar port allocation failed closed: namespace={namespace} error={error:#}"
        );
        SidecarPorts {
            zoekt_http: 0,
            qdrant_http: 0,
            qdrant_grpc: 0,
            embed_http: 0,
            embed_url: SidecarLayout::embed_base_url(0),
        }
    })
}

fn allocate_agent_ports_in_registry(
    base: &Path,
    namespace: &str,
    configured: [Option<u16>; 4],
) -> Result<SidecarPorts> {
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
    let mut registry = read_agent_port_registry(&registry_path)?;
    let existing = registry.get(namespace).cloned();
    let current_reservation_is_recent =
        agent_port_reservation_is_recent(&root, namespace, now_epoch_ms())?;
    let cleanup = prune_agent_port_registry(&root, namespace, &mut registry);
    let mut reserved = reserved_registry_ports_excluding(&registry, namespace);
    let zoekt_http = select_agent_port(
        configured[0],
        existing.as_ref().map(|ports| ports.zoekt_http),
        namespace,
        "zoekt",
        &mut reserved,
    )?;
    let qdrant_http = select_agent_port(
        configured[1],
        existing.as_ref().map(|ports| ports.qdrant_http),
        namespace,
        "qdrant-http",
        &mut reserved,
    )?;
    let qdrant_grpc = select_agent_port(
        configured[2],
        existing.as_ref().map(|ports| ports.qdrant_grpc),
        namespace,
        "qdrant-grpc",
        &mut reserved,
    )?;
    let embed_http = select_agent_port(
        configured[3],
        existing.as_ref().map(|ports| ports.embed_http),
        namespace,
        "embed",
        &mut reserved,
    )?;
    let ports = SidecarPorts {
        zoekt_http,
        qdrant_http,
        qdrant_grpc,
        embed_http,
        embed_url: SidecarLayout::embed_base_url(embed_http),
    };
    let changed = existing.as_ref() != Some(&ports);
    if changed
        && existing
            .as_ref()
            .is_some_and(|ports| current_reservation_is_recent || sidecar_ports_are_bound(ports))
    {
        anyhow::bail!(
            "agent sidecar namespace {namespace} already has an active or recently reserved port allocation"
        );
    }
    write_agent_port_reservation(&root, namespace)?;
    if changed {
        registry.insert(namespace.to_string(), ports.clone());
    }
    if changed || cleanup.pruned > 0 {
        write_agent_port_registry(&registry_path, &registry)?;
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
    Ok(ports)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AgentPortRegistryCleanup {
    pruned: usize,
    retained: usize,
    failures: usize,
    failure_details: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentPortReservation {
    reserved_at_epoch_ms: i64,
}

fn write_agent_port_reservation(root: &Path, namespace: &str) -> Result<()> {
    let reservation = AgentPortReservation {
        reserved_at_epoch_ms: now_epoch_ms(),
    };
    let bytes = serde_json::to_vec_pretty(&reservation)?;
    codestory_workspace::atomic_file::write_bytes_atomic(
        &agent_port_reservation_path(root, namespace),
        "agent-port-reservation",
        &bytes,
    )
    .with_context(|| format!("write agent port reservation for {namespace}"))
}

fn agent_port_reservation_is_recent(
    root: &Path,
    namespace: &str,
    now_epoch_ms: i64,
) -> Result<bool> {
    let path = agent_port_reservation_path(root, namespace);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let reservation: AgentPortReservation =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    Ok(
        now_epoch_ms.saturating_sub(reservation.reserved_at_epoch_ms)
            < AGENT_PORT_RESERVATION_GRACE.as_millis() as i64,
    )
}

fn write_agent_port_registry(path: &Path, registry: &BTreeMap<String, SidecarPorts>) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(registry)?;
    codestory_workspace::atomic_file::write_bytes_atomic(path, "agent-port-registry", &bytes)
        .with_context(|| format!("write sidecar port allocation registry {}", path.display()))
}

fn prune_agent_port_registry(
    root: &Path,
    current_namespace: &str,
    registry: &mut BTreeMap<String, SidecarPorts>,
) -> AgentPortRegistryCleanup {
    let now = now_epoch_ms();
    let mut cleanup = AgentPortRegistryCleanup::default();
    registry.retain(|namespace, ports| {
        if namespace == current_namespace {
            return true;
        }
        match agent_port_allocation_is_retained(root, namespace, ports, now) {
            Ok(true) => {
                cleanup.retained += 1;
                true
            }
            Ok(false) => {
                cleanup.pruned += 1;
                if let Err(error) =
                    std::fs::remove_file(agent_port_reservation_path(root, namespace))
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    cleanup.failures += 1;
                    cleanup.record_failure(format!(
                        "namespace={namespace} failed to remove stale reservation: {error}"
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

fn agent_port_allocation_is_retained(
    root: &Path,
    namespace: &str,
    ports: &SidecarPorts,
    now_epoch_ms: i64,
) -> Result<bool> {
    if !is_agent_namespace_path_component(namespace) {
        anyhow::bail!("registry namespace is not a safe agent path component");
    }
    let namespace_root = root.join(namespace);
    if agent_port_reservation_is_recent(root, namespace, now_epoch_ms)? {
        return Ok(true);
    }

    let state_path = namespace_root.join("retrieval-sidecars.json");
    let state_bytes = match std::fs::read(&state_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(sidecar_ports_are_bound(ports));
        }
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", state_path.display()));
        }
    };
    let value: serde_json::Value = serde_json::from_slice(&state_bytes)
        .with_context(|| format!("parse {}", state_path.display()))?;
    if value.get("owner").and_then(|value| value.as_str()) != Some("codestory")
        || value.get("namespace").and_then(|value| value.as_str()) != Some(namespace)
    {
        anyhow::bail!("state owner or namespace does not match registry allocation");
    }
    let state_ports = sidecar_ports_from_value(&value)
        .context("state does not contain a complete sidecar port allocation")?;
    if &state_ports != ports {
        anyhow::bail!("state ports do not match registry allocation");
    }
    // A valid state file still causes runtime construction to reuse these ports, even if the
    // owner is temporarily down. Reclaim only after normal teardown removes the state contract.
    Ok(true)
}

fn is_agent_namespace_path_component(namespace: &str) -> bool {
    !namespace.is_empty()
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn sidecar_ports_are_bound(ports: &SidecarPorts) -> bool {
    [
        ports.zoekt_http,
        ports.qdrant_http,
        ports.qdrant_grpc,
        ports.embed_http,
    ]
    .into_iter()
    .any(|port| !local_port_available(port))
}

fn agent_port_reservation_path(root: &Path, namespace: &str) -> PathBuf {
    root.join(format!(".port-allocation-{namespace}.json"))
}

fn sidecar_ports_from_value(value: &serde_json::Value) -> Option<SidecarPorts> {
    let embed_http = value.get("embed_http_port")?.as_u64()?.try_into().ok()?;
    Some(SidecarPorts {
        zoekt_http: value.get("zoekt_http_port")?.as_u64()?.try_into().ok()?,
        qdrant_http: value.get("qdrant_http_port")?.as_u64()?.try_into().ok()?,
        qdrant_grpc: value.get("qdrant_grpc_port")?.as_u64()?.try_into().ok()?,
        embed_http,
        embed_url: value.get("embed_url")?.as_str().map(ToOwned::to_owned)?,
    })
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn read_agent_port_registry(path: &Path) -> Result<BTreeMap<String, SidecarPorts>> {
    let body = match std::fs::read_to_string(path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("read sidecar port allocation registry {}", path.display())
            });
        }
    };
    serde_json::from_str(&body)
        .with_context(|| format!("parse sidecar port allocation registry {}", path.display()))
}

fn reserved_registry_ports_excluding(
    registry: &BTreeMap<String, SidecarPorts>,
    namespace: &str,
) -> BTreeSet<u16> {
    registry
        .iter()
        .filter(|(candidate, _)| candidate.as_str() != namespace)
        .flat_map(|(_, ports)| {
            [
                ports.zoekt_http,
                ports.qdrant_http,
                ports.qdrant_grpc,
                ports.embed_http,
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
) -> Result<u16> {
    if let Some(port) = configured.or(existing) {
        if !reserved.insert(port) {
            anyhow::bail!("agent sidecar port {port} is already reserved");
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
    ProjectDirs::from("dev", "codestory", "codestory")
        .map(|dirs| dirs.cache_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("codestory").join("cache"))
}

/// Docker compose profile for mandatory sidecars.
pub fn retrieval_compose_profile() -> String {
    "real".to_string()
}

const MANAGED_LLAMACPP_URL_ENV: &str = "CODESTORY_EMBED_LLAMACPP_URL_MANAGED";

fn operator_explicit_llamacpp_endpoint_configured() -> bool {
    std::env::var("CODESTORY_EMBED_LLAMACPP_URL")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
        && std::env::var(MANAGED_LLAMACPP_URL_ENV).is_err()
}

pub fn embedding_server_launch_mode() -> Result<EmbeddingServerLaunchMode> {
    if let Some(mode) = std::env::var("CODESTORY_EMBED_SERVER_LAUNCH")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .and_then(|value| match value.as_str() {
            "native_spawned" | "native" => Some(EmbeddingServerLaunchMode::NativeSpawned),
            "docker_compose_embed" | "docker" | "compose" => {
                Some(EmbeddingServerLaunchMode::DockerComposeEmbed)
            }
            "external_endpoint" | "external" | "endpoint"
                if !std::env::var("CODESTORY_EMBED_LLAMACPP_URL")
                    .unwrap_or_default()
                    .trim()
                    .is_empty() =>
            {
                Some(EmbeddingServerLaunchMode::ExternalEndpoint)
            }
            _ => None,
        })
    {
        return Ok(mode);
    }
    if std::env::var("CODESTORY_EMBED_SERVER_LAUNCH").is_ok() {
        anyhow::bail!(
            "CODESTORY_EMBED_SERVER_LAUNCH must be docker_compose_embed, native_spawned, or external_endpoint"
        );
    }
    if operator_explicit_llamacpp_endpoint_configured() {
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

fn env_port(name: &str, default: u16) -> Option<u16> {
    std::env::var(name).ok().map(|value| {
        value
            .parse()
            .ok()
            .filter(|port| *port != 0)
            .unwrap_or(default)
    })
}

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim(),
            "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
        ),
        Err(_) => default,
    }
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
        assert_eq!(first.layout.zoekt_http_port, second.layout.zoekt_http_port);
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
    fn project_runtime_exposes_v2_identity_without_changing_namespace_contract() {
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
            runtime.namespace.starts_with(&format!(
                "codestory-agent-{}-",
                project_hash(project.path())
            )),
            "0.14 identity metadata must not rename existing sidecar namespaces"
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
        assert_eq!(first.layout.zoekt_http_port, second.layout.zoekt_http_port);
        assert_eq!(
            first.layout.qdrant_http_port,
            second.layout.qdrant_http_port
        );
        assert!(first.cleanup_command.contains("--profile agent"));
        assert!(first.cleanup_command.contains("--run-id review-fix"));
    }

    #[test]
    fn agent_profile_reuses_persisted_state_ports_before_registry() {
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
            r#"{
  "zoekt_http_port": 31001,
  "qdrant_http_port": 31002,
  "qdrant_grpc_port": 31003,
  "embed_http_port": 31004,
  "embed_url": "http://127.0.0.1:31004/v1/embeddings"
}"#,
        )
        .expect("state file");

        let second = SidecarRuntimeConfig::for_project_profile_with_run_id(
            Some(project.path()),
            SidecarProfile::Agent,
            Some("persisted"),
        );

        assert_eq!(second.layout.zoekt_http_port, 31001);
        assert_eq!(second.layout.qdrant_http_port, 31002);
        assert_eq!(second.layout.qdrant_grpc_port, 31003);
        assert_eq!(second.embed_http_port, 31004);
    }

    #[test]
    fn agent_port_registry_reuses_namespace_and_avoids_registered_ports() {
        let cache = tempdir().expect("cache");

        let first = allocate_agent_ports_in_registry(cache.path(), "codestory-agent-a", [None; 4])
            .expect("first");
        let same = allocate_agent_ports_in_registry(cache.path(), "codestory-agent-a", [None; 4])
            .expect("same");
        let other = allocate_agent_ports_in_registry(cache.path(), "codestory-agent-b", [None; 4])
            .expect("other");

        assert_eq!(first, same);
        let ports = [
            first.zoekt_http,
            first.qdrant_http,
            first.qdrant_grpc,
            first.embed_http,
            other.zoekt_http,
            other.qdrant_http,
            other.qdrant_grpc,
            other.embed_http,
        ];
        let unique: BTreeSet<_> = ports.into_iter().collect();
        assert_eq!(unique.len(), ports.len());
    }

    #[test]
    fn agent_port_registry_refuses_recent_same_namespace_reassignment() {
        let cache = tempdir().expect("cache");
        let first =
            allocate_agent_ports_in_registry(cache.path(), "same", [None; 4]).expect("first");
        let (listeners, configured) = test_sidecar_ports();
        drop(listeners);

        allocate_agent_ports_in_registry(
            cache.path(),
            "same",
            [
                Some(configured.zoekt_http),
                Some(configured.qdrant_http),
                Some(configured.qdrant_grpc),
                Some(configured.embed_http),
            ],
        )
        .expect_err("recent allocation must not be reassigned");

        let registry =
            read_agent_port_registry(&cache.path().join("sidecars").join("port-allocations.json"))
                .expect("registry");
        assert_eq!(registry.get("same"), Some(&first));
    }

    #[test]
    fn agent_port_registry_tracks_explicit_runtime_ports() {
        let _lock = crate::test_support::env_lock();
        let cache = tempdir().expect("cache");
        let (listeners, configured) = test_sidecar_ports();
        drop(listeners);
        let _cache = EnvGuard::set(
            "CODESTORY_CACHE_ROOT",
            cache.path().to_str().expect("utf8 cache"),
        );
        let _zoekt = EnvGuard::set("CODESTORY_ZOEKT_PORT", &configured.zoekt_http.to_string());
        let _qdrant_http = EnvGuard::set(
            "CODESTORY_QDRANT_HTTP_PORT",
            &configured.qdrant_http.to_string(),
        );
        let _qdrant_grpc = EnvGuard::set(
            "CODESTORY_QDRANT_GRPC_PORT",
            &configured.qdrant_grpc.to_string(),
        );
        let _embed = EnvGuard::set("CODESTORY_EMBED_PORT", &configured.embed_http.to_string());

        let runtime = SidecarRuntimeConfig::for_project_profile_with_run_id(
            None,
            SidecarProfile::Agent,
            Some("configured"),
        );
        let root = cache.path().join("sidecars");
        let registry =
            read_agent_port_registry(&root.join("port-allocations.json")).expect("read registry");

        assert_eq!(registry.get(&runtime.namespace), Some(&configured));
        std::fs::remove_file(agent_port_reservation_path(&root, &runtime.namespace))
            .expect("remove startup reservation");
        write_test_sidecar_state(&root, &runtime.namespace, &configured, None);
        let mut registry = registry;
        let cleanup = prune_agent_port_registry(&root, "other", &mut registry);
        assert_eq!(cleanup.retained, 1);
        assert_eq!(cleanup.failures, 0);
    }

    fn test_sidecar_ports() -> (Vec<TcpListener>, SidecarPorts) {
        let listeners: Vec<_> = (0..4)
            .map(|_| TcpListener::bind(("127.0.0.1", 0)).expect("reserve test port"))
            .collect();
        let ports: Vec<_> = listeners
            .iter()
            .map(|listener| listener.local_addr().expect("local address").port())
            .collect();
        (
            listeners,
            SidecarPorts {
                zoekt_http: ports[0],
                qdrant_http: ports[1],
                qdrant_grpc: ports[2],
                embed_http: ports[3],
                embed_url: SidecarLayout::embed_base_url(ports[3]),
            },
        )
    }

    fn write_test_sidecar_state(
        root: &Path,
        namespace: &str,
        ports: &SidecarPorts,
        embedding_launch: Option<serde_json::Value>,
    ) {
        let namespace_root = root.join(namespace);
        std::fs::create_dir_all(&namespace_root).expect("namespace root");
        std::fs::write(
            namespace_root.join("retrieval-sidecars.json"),
            serde_json::to_vec(&serde_json::json!({
                "owner": "codestory",
                "namespace": namespace,
                "zoekt_http_port": ports.zoekt_http,
                "qdrant_http_port": ports.qdrant_http,
                "qdrant_grpc_port": ports.qdrant_grpc,
                "embed_http_port": ports.embed_http,
                "embed_url": ports.embed_url,
                "embedding_launch": embedding_launch,
            }))
            .expect("serialize state"),
        )
        .expect("write state");
    }

    #[test]
    fn agent_port_registry_prunes_missing_state() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let (listeners, ports) = test_sidecar_ports();
        drop(listeners);
        let mut registry = BTreeMap::from([("stale".to_string(), ports)]);

        let cleanup = prune_agent_port_registry(&root, "current", &mut registry);

        assert!(registry.is_empty());
        assert_eq!(cleanup.pruned, 1);
        assert_eq!(cleanup.failures, 0);
    }

    #[test]
    fn agent_port_registry_preserves_live_bound_state() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let (listeners, ports) = test_sidecar_ports();
        write_test_sidecar_state(&root, "live", &ports, None);
        let mut registry = BTreeMap::from([("live".to_string(), ports)]);

        let cleanup = prune_agent_port_registry(&root, "current", &mut registry);

        assert_eq!(listeners.len(), 4);
        assert!(registry.contains_key("live"));
        assert_eq!(cleanup.retained, 1);
        assert_eq!(cleanup.pruned, 0);
    }

    #[test]
    fn agent_port_registry_preserves_bound_port_without_state() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let (listeners, ports) = test_sidecar_ports();
        let mut registry = BTreeMap::from([("ownerless".to_string(), ports)]);

        let cleanup = prune_agent_port_registry(&root, "current", &mut registry);

        assert_eq!(listeners.len(), 4);
        assert!(registry.contains_key("ownerless"));
        assert_eq!(cleanup.retained, 1);
    }

    #[test]
    fn agent_port_registry_preserves_malformed_state_fail_closed() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let namespace_root = root.join("malformed");
        std::fs::create_dir_all(&namespace_root).expect("namespace root");
        std::fs::write(namespace_root.join("retrieval-sidecars.json"), b"{")
            .expect("malformed state");
        let (_listeners, ports) = test_sidecar_ports();
        let mut registry = BTreeMap::from([("malformed".to_string(), ports)]);

        let cleanup = prune_agent_port_registry(&root, "current", &mut registry);

        assert!(registry.contains_key("malformed"));
        assert_eq!(cleanup.failures, 1);
        assert_eq!(cleanup.pruned, 0);
    }

    #[test]
    fn agent_port_registry_preserves_unreadable_state_shape_fail_closed() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        std::fs::create_dir_all(root.join("unreadable").join("retrieval-sidecars.json"))
            .expect("state path directory");
        let (listeners, ports) = test_sidecar_ports();
        drop(listeners);
        let mut registry = BTreeMap::from([("unreadable".to_string(), ports)]);

        let cleanup = prune_agent_port_registry(&root, "current", &mut registry);

        assert!(registry.contains_key("unreadable"));
        assert_eq!(cleanup.failures, 1);
    }

    #[test]
    fn agent_port_registry_rejects_traversal_namespace_without_touching_outside_file() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        std::fs::create_dir_all(&root).expect("sidecars root");
        let sentinel = cache.path().join("outside.json");
        std::fs::write(&sentinel, b"keep").expect("outside sentinel");
        let (listeners, ports) = test_sidecar_ports();
        drop(listeners);
        let mut registry = BTreeMap::from([("../outside".to_string(), ports)]);

        let cleanup = prune_agent_port_registry(&root, "current", &mut registry);

        assert!(registry.contains_key("../outside"));
        assert_eq!(cleanup.failures, 1);
        assert_eq!(std::fs::read(&sentinel).expect("outside sentinel"), b"keep");
    }

    #[test]
    fn agent_port_registry_preserves_state_with_reused_pid_fail_closed() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let (listeners, ports) = test_sidecar_ports();
        drop(listeners);
        write_test_sidecar_state(
            &root,
            "reused",
            &ports,
            Some(serde_json::json!({
                "provider": "llamacpp",
                "launch_mode": "native_spawned",
                "endpoint": ports.embed_url,
                "pid": std::process::id(),
            })),
        );
        let mut registry = BTreeMap::from([("reused".to_string(), ports)]);

        let cleanup = prune_agent_port_registry(&root, "current", &mut registry);

        assert!(registry.contains_key("reused"));
        assert_eq!(cleanup.retained, 1);
    }

    #[test]
    fn agent_port_registry_preserves_recent_startup_reservation() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        write_agent_port_reservation(&root, "starting").expect("reservation");
        let (_listeners, ports) = test_sidecar_ports();
        let mut registry = BTreeMap::from([("starting".to_string(), ports)]);

        let cleanup = prune_agent_port_registry(&root, "current", &mut registry);

        assert!(registry.contains_key("starting"));
        assert_eq!(cleanup.retained, 1);
    }

    #[test]
    fn agent_port_registry_compaction_is_complete_and_parseable() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        std::fs::create_dir_all(&root).expect("sidecars root");
        let (listeners, stale_ports) = test_sidecar_ports();
        drop(listeners);
        let registry_path = root.join("port-allocations.json");
        write_agent_port_registry(
            &registry_path,
            &BTreeMap::from([("stale".to_string(), stale_ports)]),
        )
        .expect("seed registry");

        let current = allocate_agent_ports_in_registry(cache.path(), "current", [None; 4])
            .expect("allocate after compaction");
        let compacted = read_agent_port_registry(&registry_path).expect("parse compacted registry");

        assert_eq!(
            compacted,
            BTreeMap::from([("current".to_string(), current)])
        );
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
        let _zoekt = EnvGuard::set("CODESTORY_ZOEKT_PORT", "invalid");
        let _qdrant_http = EnvGuard::set("CODESTORY_QDRANT_HTTP_PORT", "invalid");
        let _qdrant_grpc = EnvGuard::set("CODESTORY_QDRANT_GRPC_PORT", "invalid");
        let _embed = EnvGuard::set("CODESTORY_EMBED_PORT", "invalid");

        let runtime = SidecarRuntimeConfig::for_project_profile_with_run_id(
            None,
            SidecarProfile::Agent,
            Some("current"),
        );

        assert_eq!(runtime.layout.zoekt_http_port, 0);
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
        let _managed = EnvGuard::remove(MANAGED_LLAMACPP_URL_ENV);
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
        let _managed = EnvGuard::remove(MANAGED_LLAMACPP_URL_ENV);
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
    fn managed_llamacpp_url_after_activation_keeps_native_launch() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::remove("CODESTORY_EMBED_SERVER_LAUNCH");
        let _url = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_URL");
        let _managed = EnvGuard::remove(MANAGED_LLAMACPP_URL_ENV);
        let _host = EnvGuard::set(TEST_HOST_PLATFORM_ENV, "windows/x86_64");
        let _allow_cpu = EnvGuard::remove("CODESTORY_EMBED_ALLOW_CPU");
        let _policy = EnvGuard::remove("CODESTORY_EMBED_DEVICE_POLICY");
        let runtime = SidecarRuntimeConfig::for_project_profile(None, SidecarProfile::Agent);

        runtime.activate_embed_url_default();

        assert_eq!(
            embedding_server_launch_mode().expect("launch mode"),
            EmbeddingServerLaunchMode::NativeSpawned
        );
    }

    #[test]
    fn managed_llamacpp_url_after_activation_retargets_runtime_url() {
        let _lock = crate::test_support::env_lock();
        let _mode = EnvGuard::remove("CODESTORY_EMBED_SERVER_LAUNCH");
        let _url = EnvGuard::set(
            "CODESTORY_EMBED_LLAMACPP_URL",
            "http://127.0.0.1:11111/v1/embeddings",
        );
        let _managed = EnvGuard::set(MANAGED_LLAMACPP_URL_ENV, "1");
        let runtime = SidecarRuntimeConfig::for_project_profile(None, SidecarProfile::Agent);

        runtime.activate_embed_url_default();

        assert_eq!(
            std::env::var("CODESTORY_EMBED_LLAMACPP_URL").expect("managed embed url"),
            SidecarLayout::embed_base_url(runtime.embed_http_port)
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
