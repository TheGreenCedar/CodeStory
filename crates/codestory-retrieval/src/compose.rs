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
const DOCKER_ADDRESS_POOL_EXHAUSTED_REASON: &str = "docker_address_pool_exhausted";
const DOCKER_ADDRESS_POOL_EXHAUSTED_NEEDLE: &str =
    "all predefined address pools have been fully subnetted";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct EmbedModelInventory {
    pub model_dir: Option<String>,
    pub required_gguf: String,
    pub required_gguf_present: bool,
    pub candidate_dirs: Vec<String>,
}

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
    bootstrap_sidecars_with_runtime(
        &runtime,
        repo_root,
        storage_scope,
        compose_file,
        skip_compose,
        wait_timeout,
    )
}

pub fn bootstrap_sidecars_with_runtime(
    runtime: &SidecarRuntimeConfig,
    repo_root: Option<&Path>,
    storage_scope: &BootstrapStorageScope,
    compose_file: Option<&Path>,
    skip_compose: bool,
    wait_timeout: Duration,
) -> Result<BootstrapReport> {
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
        docker_compose_up(path, repo_root, runtime)?;
        true
    } else {
        false
    };

    let state = sidecar_up_with_runtime(runtime, resolved_compose.as_deref())?;
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
            docker_bind_path(&embed_model_dir(repo_root, layout)?),
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
            "{}",
            docker_compose_up_failure_message(output.status.code(), &stdout, &stderr, repo_root)
        );
    }
    Ok(())
}

fn docker_compose_up_failure_message(
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    repo_root: Option<&Path>,
) -> String {
    if docker_address_pool_exhausted(stdout) || docker_address_pool_exhausted(stderr) {
        let project = repo_root
            .map(|path| quoted_project_arg(&path.display().to_string()))
            .unwrap_or_else(|| "<repo>".to_string());
        return format!(
            "docker compose up failed (exit {exit_code:?}): reason={DOCKER_ADDRESS_POOL_EXHAUSTED_REASON}\n\
Docker's predefined address pools are exhausted. Run read-only inventory: \
`codestory-cli sidecar inventory --project {project} --format markdown` \
or `codestory-cli sidecar inventory --project {project} --format json`.\n\
Raw docker compose output:\n{stdout}{stderr}"
        );
    }

    format!("docker compose up failed (exit {exit_code:?}):\n{stdout}{stderr}")
}

fn quoted_project_arg(project: &str) -> String {
    format!("\"{}\"", project.replace('"', "\\\""))
}

fn docker_address_pool_exhausted(details: &str) -> bool {
    details
        .to_ascii_lowercase()
        .contains(DOCKER_ADDRESS_POOL_EXHAUSTED_NEEDLE)
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

fn embed_model_dir(repo_root: Option<&Path>, layout: &SidecarLayout) -> Result<PathBuf> {
    let inventory = embed_model_inventory(repo_root, layout);
    if let Some(model_dir) = inventory
        .required_gguf_present
        .then(|| inventory.model_dir.as_ref())
        .flatten()
    {
        return Ok(PathBuf::from(model_dir));
    }
    if std::env::var("CODESTORY_EMBED_MODEL_DIR").is_ok() {
        anyhow::bail!(
            "CODESTORY_EMBED_MODEL_DIR does not contain {}; run `node scripts/setup-retrieval-env.mjs --fetch-embed-model` or set CODESTORY_EMBED_MODEL_DIR",
            crate::embeddings::BGE_BASE_EN_V1_5_GGUF
        );
    }
    anyhow::bail!(
        "No llama.cpp embedding model directory contains {}; run `node scripts/setup-retrieval-env.mjs --fetch-embed-model` or set CODESTORY_EMBED_MODEL_DIR",
        crate::embeddings::BGE_BASE_EN_V1_5_GGUF
    )
}

pub fn embed_model_inventory(
    repo_root: Option<&Path>,
    layout: &SidecarLayout,
) -> EmbedModelInventory {
    let candidates = embed_model_candidates(repo_root, layout);
    let model_dir = candidates
        .iter()
        .find(|candidate| embed_model_dir_ready(candidate))
        .or_else(|| candidates.first())
        .map(|path| path.display().to_string());
    let required_gguf_present = model_dir
        .as_ref()
        .is_some_and(|path| embed_model_dir_ready(Path::new(path)));
    EmbedModelInventory {
        model_dir,
        required_gguf: crate::embeddings::BGE_BASE_EN_V1_5_GGUF.to_string(),
        required_gguf_present,
        candidate_dirs: candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
    }
}

fn embed_model_candidates(repo_root: Option<&Path>, layout: &SidecarLayout) -> Vec<PathBuf> {
    if let Ok(path) = std::env::var("CODESTORY_EMBED_MODEL_DIR") {
        return vec![PathBuf::from(path)];
    }
    let workdir = repo_root
        .or_else(|| Some(Path::new(".")))
        .unwrap_or(Path::new("."));
    let fallback = layout
        .qdrant_data_dir
        .parent()
        .map(|parent| parent.join("embed-models"))
        .unwrap_or_else(|| layout.qdrant_data_dir.join("embed-models"));
    let mut candidates = Vec::new();
    for candidate in [
        workdir.join("target").join("retrieval-models"),
        workdir.join("models").join("gguf").join("bge-base-en-v1.5"),
        user_cache_root().join("embed-models"),
        fallback,
    ] {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

#[cfg(test)]
fn embed_model_dir_from_candidates(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<PathBuf> {
    for candidate in candidates {
        if embed_model_dir_ready(&candidate) {
            return Ok(candidate);
        }
    }
    bail!(
        "No llama.cpp embedding model directory contains {}; run `node scripts/setup-retrieval-env.mjs --fetch-embed-model` or set CODESTORY_EMBED_MODEL_DIR",
        crate::embeddings::BGE_BASE_EN_V1_5_GGUF
    )
}

fn embed_model_dir_ready(path: &Path) -> bool {
    path.join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF)
        .is_file()
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

        assert_eq!(
            embed_model_dir(Some(project.path()), &layout).expect("model dir"),
            model_dir
        );
    }

    #[test]
    fn embed_model_dir_uses_first_candidate_with_model() {
        let empty = tempdir().expect("empty");
        let cache = tempdir().expect("cache");
        let model_dir = cache.path().join("embed-models");
        std::fs::create_dir_all(&model_dir).expect("model dir");
        std::fs::write(
            model_dir.join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF),
            b"model placeholder",
        )
        .expect("model file");

        assert_eq!(
            embed_model_dir_from_candidates([empty.path().to_path_buf(), model_dir.clone()])
                .expect("fallback model dir"),
            model_dir
        );
    }

    #[test]
    fn embed_model_dir_fails_before_empty_fallback_container() {
        let empty = tempdir().expect("empty");

        let error = embed_model_dir_from_candidates([empty.path().to_path_buf()])
            .expect_err("missing model must fail before docker compose");

        assert!(
            error
                .to_string()
                .contains(crate::embeddings::BGE_BASE_EN_V1_5_GGUF)
        );
    }

    #[test]
    fn docker_bind_path_removes_windows_verbatim_prefix() {
        assert_eq!(
            docker_bind_path(Path::new(r"\\?\C:\Users\alber\codestory\models")),
            "C:/Users/alber/codestory/models"
        );
    }

    #[test]
    fn compose_failure_classifies_docker_address_pool_exhaustion() {
        let project = Path::new("C:/repo/example project");
        let stdout = "compose stdout\n";
        let stderr =
            "failed to create network: all predefined address pools have been fully subnetted";

        let message = docker_compose_up_failure_message(Some(1), stdout, stderr, Some(project));

        assert!(message.contains("reason=docker_address_pool_exhausted"));
        assert!(message.contains(stdout));
        assert!(message.contains(stderr));
        assert!(message.contains(
            "codestory-cli sidecar inventory --project \"C:/repo/example project\" --format markdown"
        ));
        assert!(message.contains(
            "codestory-cli sidecar inventory --project \"C:/repo/example project\" --format json"
        ));
        for forbidden in [" prune", " remove", " down", " delete", " restart"] {
            assert!(
                !message.to_ascii_lowercase().contains(forbidden),
                "guidance must stay non-destructive: {message}"
            );
        }
    }

    #[test]
    fn compose_failure_preserves_generic_stderr_without_reason() {
        let stderr = "compose service failed for another reason";

        let message = docker_compose_up_failure_message(Some(17), "stdout\n", stderr, None);

        assert!(message.contains("docker compose up failed (exit Some(17))"));
        assert!(message.contains("stdout\n"));
        assert!(message.contains(stderr));
        assert!(!message.contains("docker_address_pool_exhausted"));
        assert!(!message.contains("sidecar inventory"));
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
