use anyhow::{Context, Result};
use directories::ProjectDirs;
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

impl SidecarLayout {
    pub fn from_env() -> Self {
        let base = user_cache_root();
        Self {
            zoekt_http_port: env_port("CODESTORY_ZOEKT_PORT", DEFAULT_ZOEKT_HTTP_PORT),
            qdrant_http_port: env_port("CODESTORY_QDRANT_HTTP_PORT", DEFAULT_QDRANT_HTTP_PORT),
            qdrant_grpc_port: env_port("CODESTORY_QDRANT_GRPC_PORT", DEFAULT_QDRANT_GRPC_PORT),
            zoekt_data_dir: base.join("zoekt"),
            qdrant_data_dir: base.join("qdrant"),
            scip_artifacts_root: base.join("scip"),
            state_file: base.join("retrieval-sidecars.json"),
        }
    }

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

fn env_port(name: &str, default: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
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
