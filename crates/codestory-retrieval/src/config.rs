use anyhow::{Context, Result};
use directories::ProjectDirs;
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tracing::warn;

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
        let dynamic_ports = dynamic.then(|| dynamic_agent_ports(&base, &namespace));
        let zoekt_http_port = env_port("CODESTORY_ZOEKT_PORT", DEFAULT_ZOEKT_HTTP_PORT)
            .or_else(|| stored.as_ref().map(|ports| ports.zoekt_http))
            .or_else(|| dynamic_ports.as_ref().map(|ports| ports.zoekt_http))
            .unwrap_or_else(|| {
                if dynamic {
                    dynamic_agent_port(&namespace, "zoekt")
                } else {
                    DEFAULT_ZOEKT_HTTP_PORT
                }
            });
        let qdrant_http_port = env_port("CODESTORY_QDRANT_HTTP_PORT", DEFAULT_QDRANT_HTTP_PORT)
            .or_else(|| stored.as_ref().map(|ports| ports.qdrant_http))
            .or_else(|| dynamic_ports.as_ref().map(|ports| ports.qdrant_http))
            .unwrap_or_else(|| {
                if dynamic {
                    dynamic_agent_port(&namespace, "qdrant-http")
                } else {
                    DEFAULT_QDRANT_HTTP_PORT
                }
            });
        let qdrant_grpc_port = env_port("CODESTORY_QDRANT_GRPC_PORT", DEFAULT_QDRANT_GRPC_PORT)
            .or_else(|| stored.as_ref().map(|ports| ports.qdrant_grpc))
            .or_else(|| dynamic_ports.as_ref().map(|ports| ports.qdrant_grpc))
            .unwrap_or_else(|| {
                if dynamic {
                    dynamic_agent_port(&namespace, "qdrant-grpc")
                } else {
                    DEFAULT_QDRANT_GRPC_PORT
                }
            });
        let embed_http_port = env_port("CODESTORY_EMBED_PORT", DEFAULT_EMBED_HTTP_PORT)
            .or_else(|| stored.as_ref().map(|ports| ports.embed_http))
            .or_else(|| dynamic_ports.as_ref().map(|ports| ports.embed_http))
            .unwrap_or_else(|| {
                if dynamic {
                    dynamic_agent_port(&namespace, "embed")
                } else {
                    DEFAULT_EMBED_HTTP_PORT
                }
            });
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
        let mut labels = BTreeMap::new();
        labels.insert("dev.codestory.owner".into(), "codestory".into());
        labels.insert("dev.codestory.profile".into(), profile.as_str().into());
        labels.insert("dev.codestory.namespace".into(), namespace.clone());
        if let Some(project_root) = project_root {
            let hash = project_hash(project_root);
            labels.insert("dev.codestory.project_hash".into(), hash.clone());
            labels.insert(
                "dev.codestory.project_id".into(),
                format!("codestory-{hash}"),
            );
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

    pub fn activate_embed_url_default(&self) {
        if std::env::var("CODESTORY_EMBED_LLAMACPP_URL").is_err() {
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

fn dynamic_agent_port(namespace: &str, salt: &str) -> u16 {
    dynamic_agent_port_excluding(namespace, salt, &BTreeSet::new())
}

fn dynamic_agent_ports(base: &Path, namespace: &str) -> SidecarPorts {
    allocate_agent_ports_in_registry(base, namespace).unwrap_or_else(|error| {
        warn!(
            namespace,
            error = %error,
            "falling back to unlocked dynamic agent sidecar port allocation"
        );
        fallback_dynamic_agent_ports(namespace)
    })
}

fn allocate_agent_ports_in_registry(base: &Path, namespace: &str) -> Result<SidecarPorts> {
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
    let mut registry = read_agent_port_registry(&registry_path);
    if let Some(ports) = registry.get(namespace) {
        return Ok(ports.clone());
    }

    let mut reserved = reserved_registry_ports(&registry);
    let zoekt_http = reserve_dynamic_agent_port(namespace, "zoekt", &mut reserved);
    let qdrant_http = reserve_dynamic_agent_port(namespace, "qdrant-http", &mut reserved);
    let qdrant_grpc = reserve_dynamic_agent_port(namespace, "qdrant-grpc", &mut reserved);
    let embed_http = reserve_dynamic_agent_port(namespace, "embed", &mut reserved);
    let ports = SidecarPorts {
        zoekt_http,
        qdrant_http,
        qdrant_grpc,
        embed_http,
        embed_url: SidecarLayout::embed_base_url(embed_http),
    };
    registry.insert(namespace.to_string(), ports.clone());
    std::fs::write(&registry_path, serde_json::to_vec_pretty(&registry)?).with_context(|| {
        format!(
            "write sidecar port allocation registry {}",
            registry_path.display()
        )
    })?;
    Ok(ports)
}

fn read_agent_port_registry(path: &Path) -> BTreeMap<String, SidecarPorts> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default()
}

fn reserved_registry_ports(registry: &BTreeMap<String, SidecarPorts>) -> BTreeSet<u16> {
    registry
        .values()
        .flat_map(|ports| {
            [
                ports.zoekt_http,
                ports.qdrant_http,
                ports.qdrant_grpc,
                ports.embed_http,
            ]
        })
        .collect()
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

fn fallback_dynamic_agent_ports(namespace: &str) -> SidecarPorts {
    let mut reserved = BTreeSet::new();
    let zoekt_http = reserve_dynamic_agent_port(namespace, "zoekt", &mut reserved);
    let qdrant_http = reserve_dynamic_agent_port(namespace, "qdrant-http", &mut reserved);
    let qdrant_grpc = reserve_dynamic_agent_port(namespace, "qdrant-grpc", &mut reserved);
    let embed_http = reserve_dynamic_agent_port(namespace, "embed", &mut reserved);
    SidecarPorts {
        zoekt_http,
        qdrant_http,
        qdrant_grpc,
        embed_http,
        embed_url: SidecarLayout::embed_base_url(embed_http),
    }
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
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|port| *port != 0)
        .or(Some(default).filter(|_| std::env::var(name).is_ok()))
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

        let first =
            allocate_agent_ports_in_registry(cache.path(), "codestory-agent-a").expect("first");
        let same =
            allocate_agent_ports_in_registry(cache.path(), "codestory-agent-a").expect("same");
        let other =
            allocate_agent_ports_in_registry(cache.path(), "codestory-agent-b").expect("other");

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
