use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Phase 2 lexical shard pin (local index + optional Zoekt webserver).
pub const ZOEKT_REAL_VERSION_PIN: &str = "zoekt-20250506123554";

/// Zoekt webserver image for `COMPOSE_PROFILES=real`.
pub const ZOEKT_WEBSERVER_IMAGE_PIN: &str =
    "sourcegraph/zoekt-webserver:0.0.0-20250506123554-490422d1adb4";

/// Qdrant container image pin for local dev and CI smoke.
pub const QDRANT_IMAGE_PIN: &str = "qdrant/qdrant:v1.12.5";

/// llama.cpp server image for `COMPOSE_PROFILES=real` embed service (see `docker/retrieval-compose.yml`).
#[allow(dead_code)]
pub const LLAMACPP_SERVER_IMAGE_PIN: &str = "ghcr.io/ggml-org/llama.cpp:server";

pub const DEFAULT_ZOEKT_HTTP_PORT: u16 = 6070;
pub const DEFAULT_QDRANT_HTTP_PORT: u16 = 6333;
pub const DEFAULT_QDRANT_GRPC_PORT: u16 = 6334;
pub const DEFAULT_EMBED_HTTP_PORT: u16 = 8080;

pub const ZOEKT_HEALTH_BUDGET: Duration = Duration::from_millis(100);
pub const QDRANT_HEALTH_BUDGET: Duration = Duration::from_millis(200);

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
        let profile = if env_profile() == Some(SidecarProfile::Agent)
            || agent_state_file(project_root).is_file()
            || running_in_ci_agent()
        {
            SidecarProfile::Agent
        } else {
            SidecarProfile::Local
        };
        Self::for_project_profile(Some(project_root), profile)
    }

    pub fn for_project_profile(project_root: Option<&Path>, profile: SidecarProfile) -> Self {
        let base = user_cache_root();
        let namespace = namespace_for(project_root, profile);
        let state_file = match profile {
            SidecarProfile::Local => base.join("retrieval-sidecars.json"),
            SidecarProfile::Agent => base
                .join("sidecars")
                .join(&namespace)
                .join("retrieval-sidecars.json"),
        };
        let stored = read_ports_from_state(&state_file);
        let dynamic = profile == SidecarProfile::Agent && stored.is_none();
        let zoekt_http_port = env_port("CODESTORY_ZOEKT_PORT", DEFAULT_ZOEKT_HTTP_PORT)
            .or_else(|| stored.as_ref().map(|ports| ports.zoekt_http))
            .unwrap_or_else(|| {
                if dynamic {
                    free_local_port()
                } else {
                    DEFAULT_ZOEKT_HTTP_PORT
                }
            });
        let qdrant_http_port = env_port("CODESTORY_QDRANT_HTTP_PORT", DEFAULT_QDRANT_HTTP_PORT)
            .or_else(|| stored.as_ref().map(|ports| ports.qdrant_http))
            .unwrap_or_else(|| {
                if dynamic {
                    free_local_port()
                } else {
                    DEFAULT_QDRANT_HTTP_PORT
                }
            });
        let qdrant_grpc_port = env_port("CODESTORY_QDRANT_GRPC_PORT", DEFAULT_QDRANT_GRPC_PORT)
            .or_else(|| stored.as_ref().map(|ports| ports.qdrant_grpc))
            .unwrap_or_else(|| {
                if dynamic {
                    free_local_port()
                } else {
                    DEFAULT_QDRANT_GRPC_PORT
                }
            });
        let embed_http_port = env_port("CODESTORY_EMBED_PORT", DEFAULT_EMBED_HTTP_PORT)
            .or_else(|| stored.as_ref().map(|ports| ports.embed_http))
            .unwrap_or_else(|| {
                if dynamic {
                    free_local_port()
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
            .map(|path| {
                format!(
                    "codestory-cli retrieval down --project {} --profile {}",
                    quote_command_path(path),
                    profile.as_str()
                )
            })
            .unwrap_or_else(|| "codestory-cli retrieval down".to_string());
        let mut labels = BTreeMap::new();
        labels.insert("dev.codestory.owner".into(), "codestory".into());
        labels.insert("dev.codestory.profile".into(), profile.as_str().into());
        labels.insert("dev.codestory.namespace".into(), namespace.clone());
        Self {
            layout,
            profile,
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
            }
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

fn running_in_ci_agent() -> bool {
    env_flag("CODESTORY_AGENT", false)
        || env_flag("CODESTORY_AGENT_RUN", false)
        || env_flag("CI", false)
        || env_flag("GITHUB_ACTIONS", false)
}

fn namespace_for(project_root: Option<&Path>, profile: SidecarProfile) -> String {
    match (profile, project_root) {
        (SidecarProfile::Local, _) => "codestory".into(),
        (SidecarProfile::Agent, Some(path)) => {
            format!(
                "codestory-agent-{}",
                fnv1a_hex(path.to_string_lossy().as_bytes())
            )
        }
        (SidecarProfile::Agent, None) => format!("codestory-agent-{}", std::process::id()),
    }
}

fn agent_state_file(project_root: &Path) -> PathBuf {
    user_cache_root()
        .join("sidecars")
        .join(namespace_for(Some(project_root), SidecarProfile::Agent))
        .join("retrieval-sidecars.json")
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

pub fn user_cache_root() -> PathBuf {
    ProjectDirs::from("dev", "codestory", "codestory")
        .map(|dirs| dirs.cache_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("codestory").join("cache"))
}

pub fn zoekt_enabled() -> bool {
    env_flag("CODESTORY_ZOEKT_ENABLED", true)
}

pub fn qdrant_enabled() -> bool {
    env_flag("CODESTORY_QDRANT_ENABLED", true)
}

/// Sidecar retrieval is mandatory; Qdrant uses 768-d semantic vectors by default.
/// `CODESTORY_RETRIEVAL_REAL_EMBEDDINGS=0` is unsupported for product indexing.
pub fn qdrant_semantic_vectors_enabled() -> bool {
    env_flag("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", true)
}

/// Docker compose profile: `real` by default. Other profiles are rejected by product bootstrap.
pub fn retrieval_compose_profile() -> String {
    std::env::var("CODESTORY_RETRIEVAL_COMPOSE_PROFILE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "real".to_string())
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
