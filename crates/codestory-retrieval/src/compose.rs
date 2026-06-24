use crate::config::{
    SidecarLayout, SidecarProfile, SidecarRuntimeConfig, retrieval_compose_profile, user_cache_root,
};
use crate::health::{InfrastructureHealth, probe_infrastructure_health};
use crate::qdrant_storage::{
    BootstrapStorageScope, DEFAULT_QDRANT_COLLECTION_RETENTION, QdrantStorageRepairReport,
    repair_qdrant_storage,
};
use crate::sidecar::{SidecarStateFile, sidecar_up_with_runtime};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Relative path from repository root to the retrieval compose file.
pub const DEFAULT_COMPOSE_REL_PATH: &str = "docker/retrieval-compose.yml";
const BUNDLED_RETRIEVAL_COMPOSE: &str = include_str!("../../../docker/retrieval-compose.yml");

#[derive(Debug, Clone)]
pub struct BootstrapReport {
    pub state: SidecarStateFile,
    pub infrastructure: InfrastructureHealth,
    pub compose_started: bool,
    pub compose_file: Option<PathBuf>,
    pub storage_repair: QdrantStorageRepairReport,
}

/// Prepare cache dirs, optionally start Docker Compose, write sidecar state, wait for probes.
pub fn bootstrap_sidecars(
    repo_root: Option<&Path>,
    storage_scope: &BootstrapStorageScope,
    compose_file: Option<&Path>,
    skip_compose: bool,
    wait_timeout: Duration,
) -> Result<BootstrapReport> {
    bootstrap_sidecars_with_profile(
        repo_root,
        storage_scope,
        compose_file,
        skip_compose,
        wait_timeout,
        SidecarProfile::Local,
    )
}

pub fn bootstrap_sidecars_with_profile(
    repo_root: Option<&Path>,
    storage_scope: &BootstrapStorageScope,
    compose_file: Option<&Path>,
    skip_compose: bool,
    wait_timeout: Duration,
    profile: SidecarProfile,
) -> Result<BootstrapReport> {
    let runtime = SidecarRuntimeConfig::for_project_profile(repo_root, profile);
    let layout = runtime.layout.clone();
    runtime.activate_embed_url_default();
    layout.ensure_data_dirs()?;
    let storage_repair =
        repair_qdrant_storage(&layout, storage_scope, DEFAULT_QDRANT_COLLECTION_RETENTION)?;

    let resolved_compose = if skip_compose {
        None
    } else {
        Some(resolve_compose_file(repo_root, compose_file)?)
    };

    let compose_started = if let Some(path) = resolved_compose.as_ref() {
        docker_compose_up(path, repo_root, &runtime)?;
        true
    } else {
        false
    };

    let state = sidecar_up_with_runtime(&runtime, resolved_compose.as_deref())?;
    let infrastructure = if wait_timeout.is_zero() {
        probe_infrastructure_health(&layout)
    } else {
        wait_for_infrastructure(&layout, wait_timeout)?
    };

    Ok(BootstrapReport {
        state,
        infrastructure,
        compose_started,
        compose_file: resolved_compose,
        storage_repair,
    })
}

/// Deprecated: pass [`BootstrapStorageScope::from_parts`] as the second argument to [`bootstrap_sidecars`].
#[deprecated(
    since = "0.4.0",
    note = "use bootstrap_sidecars(repo_root, &BootstrapStorageScope::from_parts(repo_root, None, None), compose_file, skip_compose, wait_timeout)"
)]
pub fn bootstrap_sidecars_without_storage_scope(
    repo_root: Option<&Path>,
    compose_file: Option<&Path>,
    skip_compose: bool,
    wait_timeout: Duration,
) -> Result<BootstrapReport> {
    let storage_scope = BootstrapStorageScope::from_parts(repo_root, None, None);
    bootstrap_sidecars(
        repo_root,
        &storage_scope,
        compose_file,
        skip_compose,
        wait_timeout,
    )
}

pub fn resolve_compose_file(
    repo_root: Option<&Path>,
    override_path: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(path) = override_path {
        let path = path.to_path_buf();
        if path.is_file() {
            return Ok(path);
        }
        bail!("compose file not found: {}", path.display());
    }
    if let Ok(path) = std::env::var("CODESTORY_RETRIEVAL_COMPOSE_FILE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "CODESTORY_RETRIEVAL_COMPOSE_FILE is set but not a file: {}",
            path.display()
        );
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(root) = repo_root {
        roots.push(root.to_path_buf());
    }
    if let Ok(dir) = std::env::current_dir()
        && !roots.iter().any(|existing| existing == &dir)
    {
        roots.push(dir);
    }
    for root in roots {
        let candidate = root.join(DEFAULT_COMPOSE_REL_PATH);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    write_bundled_compose_file(&user_cache_root())
}

fn write_bundled_compose_file(cache_root: &Path) -> Result<PathBuf> {
    let compose_path = cache_root.join("retrieval-compose.yml");
    std::fs::create_dir_all(cache_root)
        .with_context(|| format!("create CodeStory cache dir {}", cache_root.display()))?;
    if compose_path.is_file()
        && std::fs::read_to_string(&compose_path)
            .map(|contents| contents == BUNDLED_RETRIEVAL_COMPOSE)
            .unwrap_or(false)
    {
        return Ok(compose_path);
    }
    std::fs::write(&compose_path, BUNDLED_RETRIEVAL_COMPOSE)
        .with_context(|| format!("write bundled retrieval compose {}", compose_path.display()))?;
    Ok(compose_path)
}

pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn docker_compose_up(
    compose_file: &Path,
    repo_root: Option<&Path>,
    runtime: &SidecarRuntimeConfig,
) -> Result<()> {
    let layout = &runtime.layout;
    if !docker_available() {
        bail!(
            "docker is not available on PATH. Install Docker Desktop (Windows) or Docker Engine, \
             then re-run bootstrap. Manual Qdrant: docker run -p 6333:6333 -p 6334:6334 \
             -v \"{}:/qdrant/storage\" {}",
            layout.qdrant_data_dir.display(),
            crate::config::QDRANT_IMAGE_PIN
        );
    }

    std::fs::create_dir_all(&layout.qdrant_data_dir).with_context(|| {
        format!(
            "create Qdrant data dir {}",
            layout.qdrant_data_dir.display()
        )
    })?;
    std::fs::create_dir_all(&layout.zoekt_data_dir)
        .with_context(|| format!("create Zoekt data dir {}", layout.zoekt_data_dir.display()))?;

    let workdir = repo_root
        .or_else(|| compose_file.parent().and_then(|p| p.parent()))
        .unwrap_or_else(|| Path::new("."));

    let mut command = docker_compose_command()?;
    let compose_profile = retrieval_compose_profile();
    if !compose_profile.trim().eq_ignore_ascii_case("real") {
        bail!(
            "CODESTORY_RETRIEVAL_COMPOSE_PROFILE={compose_profile} is unsupported; real sidecars are mandatory"
        );
    }
    remove_container_if_present("codestory-zoekt-stub")?;
    command
        .arg("compose")
        .arg("-p")
        .arg(&runtime.compose_project)
        .arg("-f")
        .arg(compose_file)
        .arg("up")
        .arg("-d")
        .current_dir(workdir)
        .env(
            "CODESTORY_QDRANT_DATA_DIR",
            docker_bind_path(&layout.qdrant_data_dir),
        )
        .env(
            "CODESTORY_QDRANT_HTTP_PORT",
            layout.qdrant_http_port.to_string(),
        )
        .env(
            "CODESTORY_QDRANT_GRPC_PORT",
            layout.qdrant_grpc_port.to_string(),
        )
        .env("CODESTORY_ZOEKT_PORT", layout.zoekt_http_port.to_string())
        .env(
            "CODESTORY_ZOEKT_DATA_DIR",
            docker_bind_path(&layout.zoekt_data_dir),
        )
        .env(
            "CODESTORY_EMBED_MODEL_DIR",
            docker_bind_path(&embed_model_dir(repo_root, layout)),
        )
        .env("CODESTORY_EMBED_PORT", runtime.embed_http_port.to_string())
        .env(
            "CODESTORY_EMBED_LLAMACPP_URL",
            SidecarLayout::embed_base_url(runtime.embed_http_port),
        )
        .env("CODESTORY_SIDECAR_NAMESPACE", &runtime.namespace)
        .env("CODESTORY_SIDECAR_PROFILE", runtime.profile.as_str())
        .env("CODESTORY_SIDECAR_OWNER", "codestory")
        .env("COMPOSE_PROFILES", compose_profile);

    let output = command.output().context("spawn docker compose")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "docker compose up failed (exit {:?}):\n{stdout}{stderr}",
            output.status.code()
        );
    }
    Ok(())
}

pub fn docker_compose_down_for_state(state: &SidecarStateFile) -> Result<()> {
    if state.owner != "codestory" || state.profile != SidecarProfile::Agent.as_str() {
        return Ok(());
    }
    let Some(compose_file) = state.compose_file.as_ref().map(PathBuf::from) else {
        return Ok(());
    };
    if !compose_file.is_file() || !docker_available() {
        return Ok(());
    }
    let output = docker_compose_command()?
        .arg("compose")
        .arg("-p")
        .arg(&state.compose_project)
        .arg("-f")
        .arg(&compose_file)
        .arg("down")
        .arg("--remove-orphans")
        .output()
        .context("spawn docker compose down")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "docker compose down failed for owned sidecar namespace {} (exit {:?}):\n{stdout}{stderr}",
            state.namespace,
            output.status.code()
        );
    }
    Ok(())
}

fn docker_compose_command() -> Result<Command> {
    Ok(Command::new("docker"))
}

fn remove_container_if_present(name: &str) -> Result<()> {
    let inspect = Command::new("docker")
        .args(["container", "inspect", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("inspect docker container {name}"))?;
    if !inspect.success() {
        return Ok(());
    }
    let output = Command::new("docker")
        .args(["rm", "-f", name])
        .output()
        .with_context(|| format!("remove stale docker container {name}"))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "failed to remove stale docker container {name} (exit {:?}):\n{stdout}{stderr}",
            output.status.code()
        );
    }
    Ok(())
}

fn docker_bind_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let without_verbatim = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.to_string()
    };
    without_verbatim.replace('\\', "/")
}

fn embed_model_dir(repo_root: Option<&Path>, layout: &SidecarLayout) -> PathBuf {
    if let Ok(path) = std::env::var("CODESTORY_EMBED_MODEL_DIR") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return path;
        }
    }
    let workdir = repo_root
        .or_else(|| Some(Path::new(".")))
        .unwrap_or(Path::new("."));
    for candidate in [
        workdir.join("target").join("retrieval-models"),
        workdir.join("models").join("gguf").join("bge-base-en-v1.5"),
    ] {
        if candidate
            .join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF)
            .is_file()
        {
            return candidate;
        }
    }
    layout
        .qdrant_data_dir
        .parent()
        .map(|parent| parent.join("embed-models"))
        .unwrap_or_else(|| layout.qdrant_data_dir.join("embed-models"))
}

fn wait_for_infrastructure(
    layout: &SidecarLayout,
    timeout: Duration,
) -> Result<InfrastructureHealth> {
    let started = Instant::now();
    let poll = Duration::from_millis(500);
    let mut last = probe_infrastructure_health(layout);
    while started.elapsed() < timeout {
        if last.zoekt_reachable && last.qdrant_reachable && last.embed_reachable {
            return Ok(last);
        }
        thread::sleep(poll);
        last = probe_infrastructure_health(layout);
    }
    Ok(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn embed_model_dir_discovers_repo_models_layout() {
        let project = tempdir().expect("project");
        let model_dir = project
            .path()
            .join("models")
            .join("gguf")
            .join("bge-base-en-v1.5");
        std::fs::create_dir_all(&model_dir).expect("model dir");
        std::fs::write(
            model_dir.join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF),
            b"model placeholder",
        )
        .expect("model file");
        let layout = SidecarLayout::from_env();

        assert_eq!(embed_model_dir(Some(project.path()), &layout), model_dir);
    }

    #[test]
    fn docker_bind_path_removes_windows_verbatim_prefix() {
        assert_eq!(
            docker_bind_path(Path::new(r"\\?\C:\Users\alber\codestory\models")),
            "C:/Users/alber/codestory/models"
        );
    }

    #[test]
    fn bundled_compose_file_is_written_to_cache() {
        let cache = tempdir().expect("cache");
        let path = write_bundled_compose_file(cache.path()).expect("write bundled compose");

        assert_eq!(path, cache.path().join("retrieval-compose.yml"));
        let contents = std::fs::read_to_string(path).expect("read bundled compose");
        assert!(contents.contains("name: ${CODESTORY_SIDECAR_NAMESPACE:-codestory-retrieval}"));
        assert!(contents.contains("qdrant/qdrant:v1.12.5"));
    }

    #[test]
    fn repo_compose_file_still_wins_over_bundled_fallback() {
        let project = tempdir().expect("project");
        let compose = project.path().join(DEFAULT_COMPOSE_REL_PATH);
        std::fs::create_dir_all(compose.parent().expect("compose parent"))
            .expect("create compose parent");
        std::fs::write(&compose, "services: {}\n").expect("write compose");

        let resolved = resolve_compose_file(Some(project.path()), None).expect("resolve compose");

        assert_eq!(resolved, compose);
    }
}
