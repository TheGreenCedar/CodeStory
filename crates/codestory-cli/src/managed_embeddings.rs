use anyhow::{Context, Result, anyhow, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::args::{CliEmbeddingQuant, CliLlamaVariant};

pub(crate) const MANAGED_LLAMACPP_URL: &str = "http://127.0.0.1:8080/v1/embeddings";
const MANAGED_HOST: &str = "127.0.0.1";
const MANAGED_PORT: u16 = 8080;
const MANAGED_EMBEDDING_DIM: usize = 768;
const LLAMA_RELEASE_TAG: &str = "b9058";
const MANAGED_DIR_NAME: &str = "managed-embeddings";
const DISABLE_AUTOSTART_ENV: &str = "CODESTORY_MANAGED_EMBEDDINGS_DISABLE_AUTOSTART";
const ENDPOINT_PROBE_TEXT: &str = "codestory managed embeddings health probe";
const ENDPOINT_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

type HttpHeaders = Vec<(String, String)>;
type RawHttpResponse = (u16, HttpHeaders, Vec<u8>);

#[derive(Debug, Clone, Copy)]
struct LlamaAsset {
    os: &'static str,
    arch: &'static str,
    variant: CliLlamaVariant,
    name: &'static str,
    url: &'static str,
    sha256: &'static str,
    size_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
struct ModelAsset {
    quant: CliEmbeddingQuant,
    name: &'static str,
    url: &'static str,
    sha256: &'static str,
    size_bytes: u64,
}

#[derive(Debug, Clone)]
struct HttpEndpoint {
    host: String,
    port: u16,
    path: String,
}

const LLAMA_ASSETS: &[LlamaAsset] = &[
    LlamaAsset {
        os: "windows",
        arch: "x86_64",
        variant: CliLlamaVariant::Cpu,
        name: "llama-b9058-bin-win-cpu-x64.zip",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b9058/llama-b9058-bin-win-cpu-x64.zip",
        sha256: "965ac174332a7edc60d9f9d1c6e5cc8243cb623c3c94d95e833a85172628ba06",
        size_bytes: 16_045_952,
    },
    LlamaAsset {
        os: "windows",
        arch: "x86_64",
        variant: CliLlamaVariant::Vulkan,
        name: "llama-b9058-bin-win-vulkan-x64.zip",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b9058/llama-b9058-bin-win-vulkan-x64.zip",
        sha256: "d0a52a50c021d80d49acfbdae38faafed2dd1e4923790bafb5e701f13548d893",
        size_bytes: 33_677_020,
    },
    LlamaAsset {
        os: "linux",
        arch: "x86_64",
        variant: CliLlamaVariant::Cpu,
        name: "llama-b9058-bin-ubuntu-x64.tar.gz",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b9058/llama-b9058-bin-ubuntu-x64.tar.gz",
        sha256: "2cf277637b18e4d30c95f5703dc82bb288245bbf3a16c0800738764ab219a76f",
        size_bytes: 14_113_690,
    },
    LlamaAsset {
        os: "linux",
        arch: "x86_64",
        variant: CliLlamaVariant::Vulkan,
        name: "llama-b9058-bin-ubuntu-vulkan-x64.tar.gz",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b9058/llama-b9058-bin-ubuntu-vulkan-x64.tar.gz",
        sha256: "570d0a61897dc9c154d200dd5efd20bdd255f82f0342ca457f0188108c073fae",
        size_bytes: 32_518_737,
    },
    LlamaAsset {
        os: "macos",
        arch: "x86_64",
        variant: CliLlamaVariant::Cpu,
        name: "llama-b9058-bin-macos-x64.tar.gz",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b9058/llama-b9058-bin-macos-x64.tar.gz",
        sha256: "01b1aeec8a7262f11ff136b310172aa86efebbb44f8dc0d3ba93bf5e097b97aa",
        size_bytes: 8_673_301,
    },
    LlamaAsset {
        os: "macos",
        arch: "aarch64",
        variant: CliLlamaVariant::Cpu,
        name: "llama-b9058-bin-macos-arm64.tar.gz",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b9058/llama-b9058-bin-macos-arm64.tar.gz",
        sha256: "3ad6db7e02af619afbb026afe0d05cc16f7a1d969c4c8483215a7c3bd92dd9a2",
        size_bytes: 8_641_646,
    },
];

const MODEL_ASSETS: &[ModelAsset] = &[
    ModelAsset {
        quant: CliEmbeddingQuant::Q8_0,
        name: "bge-base-en-v1.5-q8_0.gguf",
        url: "https://huggingface.co/CompendiumLabs/bge-base-en-v1.5-gguf/resolve/main/bge-base-en-v1.5-q8_0.gguf",
        sha256: "ad1afe72cd6654a558667a3db10878b049a75bfd72912e1dabb91310d671173c",
        size_bytes: 117_974_304,
    },
    ModelAsset {
        quant: CliEmbeddingQuant::Q4KM,
        name: "bge-base-en-v1.5-q4_k_m.gguf",
        url: "https://huggingface.co/CompendiumLabs/bge-base-en-v1.5-gguf/resolve/main/bge-base-en-v1.5-q4_k_m.gguf",
        sha256: "74aebb552ea73b271d3b9c709923b4b7633b304fbc897a0498e52a180c3a9da9",
        size_bytes: 68_348_448,
    },
];

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ManagedAssetOutput {
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) sha256: String,
    pub(crate) size_bytes: u64,
    pub(crate) path: String,
    pub(crate) installed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ManagedEmbeddingsStatus {
    pub(crate) state: String,
    pub(crate) message: String,
    pub(crate) root: String,
    pub(crate) endpoint: String,
    pub(crate) llama_server: Option<String>,
    pub(crate) model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManagedManifest {
    llama_asset: Option<String>,
    llama_path: Option<String>,
    llama_variant: Option<CliLlamaVariant>,
    model_asset: Option<String>,
    model_path: Option<String>,
    model_quant: Option<CliEmbeddingQuant>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ManagedEmbeddingsSetupOutput {
    pub(crate) dry_run: bool,
    pub(crate) root: String,
    pub(crate) endpoint: String,
    pub(crate) llama_release: String,
    pub(crate) llama_variant: CliLlamaVariant,
    pub(crate) model_quant: CliEmbeddingQuant,
    pub(crate) llama: ManagedAssetOutput,
    pub(crate) model: ManagedAssetOutput,
    pub(crate) status: ManagedEmbeddingsStatus,
    pub(crate) next_commands: Vec<String>,
}

pub(crate) fn managed_root(cache_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(cache_override) = cache_override {
        return Ok(cache_override.join(MANAGED_DIR_NAME));
    }
    Ok(ProjectDirs::from("dev", "codestory", "codestory")
        .map(|dirs| dirs.cache_dir().join(MANAGED_DIR_NAME))
        .unwrap_or_else(|| {
            std::env::temp_dir()
                .join("codestory")
                .join(MANAGED_DIR_NAME)
        }))
}

pub(crate) fn setup_embeddings(
    root: &Path,
    quant: CliEmbeddingQuant,
    variant: CliLlamaVariant,
    dry_run: bool,
    start_server: bool,
) -> Result<ManagedEmbeddingsSetupOutput> {
    let llama = select_llama_asset(variant)?;
    let model = select_model_asset(quant);
    let llama_archive = downloads_dir(root).join(llama.name);
    let model_path = models_dir(root).join(model.name);
    let llama_extract_dir = llama_extract_dir(root, llama);

    if !dry_run {
        fs::create_dir_all(downloads_dir(root))
            .with_context(|| format!("Failed to create {}", downloads_dir(root).display()))?;
        fs::create_dir_all(models_dir(root))
            .with_context(|| format!("Failed to create {}", models_dir(root).display()))?;
        install_asset(llama.url, &llama_archive, llama.sha256)?;
        extract_llama_archive(&llama_archive, &llama_extract_dir)?;
        install_asset(model.url, &model_path, model.sha256)?;
        write_manifest(root, llama, model, &llama_extract_dir, &model_path)?;
    }

    let status = if dry_run {
        inspect_status(root)
    } else if start_server {
        start_or_reuse_managed_server(root)
    } else {
        inspect_status(root)
    };

    Ok(ManagedEmbeddingsSetupOutput {
        dry_run,
        root: clean_path(root),
        endpoint: MANAGED_LLAMACPP_URL.to_string(),
        llama_release: LLAMA_RELEASE_TAG.to_string(),
        llama_variant: llama.variant,
        model_quant: model.quant,
        llama: ManagedAssetOutput {
            name: llama.name.to_string(),
            url: llama.url.to_string(),
            sha256: llama.sha256.to_string(),
            size_bytes: llama.size_bytes,
            path: clean_path(&llama_extract_dir),
            installed: llama_server_path(root).is_some(),
        },
        model: ManagedAssetOutput {
            name: model.name.to_string(),
            url: model.url.to_string(),
            sha256: model.sha256.to_string(),
            size_bytes: model.size_bytes,
            path: clean_path(&model_path),
            installed: model_path.exists(),
        },
        status,
        next_commands: vec![
            "codestory-cli doctor --project .".to_string(),
            "codestory-cli index --project . --refresh full".to_string(),
        ],
    })
}

pub(crate) fn inspect_status(root: &Path) -> ManagedEmbeddingsStatus {
    if disabled_by_embedding_env() {
        return ManagedEmbeddingsStatus {
            state: "disabled_by_config".to_string(),
            message:
                "Managed llama is skipped because embedding env config selects a non-llama backend."
                    .to_string(),
            root: clean_path(root),
            endpoint: MANAGED_LLAMACPP_URL.to_string(),
            llama_server: None,
            model: None,
        };
    }
    if let Some(url) = explicit_llama_url() {
        let state = if embedding_endpoint_ready(&url, None) {
            "external_llama_configured"
        } else {
            "external_llama_unreachable"
        };
        let display_url = redact_url_for_display(&url);
        let message = if state == "external_llama_configured" {
            format!(
                "External llama.cpp endpoint is configured and accepted an embeddings probe at {display_url}."
            )
        } else {
            format!(
                "External llama.cpp endpoint is configured but did not accept an embeddings probe at {display_url}."
            )
        };
        return ManagedEmbeddingsStatus {
            state: state.to_string(),
            message,
            root: clean_path(root),
            endpoint: display_url,
            llama_server: None,
            model: None,
        };
    }

    let server = llama_server_path(root);
    let model = default_model_path(root);
    if server.is_none() || model.is_none() {
        return ManagedEmbeddingsStatus {
            state: "missing_managed_assets".to_string(),
            message:
                "Managed llama assets are not installed. Run `codestory-cli setup embeddings` (defaults to Vulkan; pass `--variant cpu` for the CPU fallback)."
                    .to_string(),
            root: clean_path(root),
            endpoint: MANAGED_LLAMACPP_URL.to_string(),
            llama_server: server.as_ref().map(|path| clean_path(path)),
            model: model.as_ref().map(|path| clean_path(path)),
        };
    }

    let running = managed_endpoint_ready();
    ManagedEmbeddingsStatus {
        state: if running {
            "managed_server_running"
        } else {
            "managed_server_stopped"
        }
        .to_string(),
        message: managed_status_message(root, running),
        root: clean_path(root),
        endpoint: MANAGED_LLAMACPP_URL.to_string(),
        llama_server: server.as_ref().map(|path| clean_path(path)),
        model: model.as_ref().map(|path| clean_path(path)),
    }
}

pub(crate) fn prepare_runtime_if_installed(root: &Path) {
    if disabled_by_embedding_env()
        || explicit_llama_url().is_some()
        || std::env::var_os(DISABLE_AUTOSTART_ENV).is_some()
    {
        return;
    }
    if llama_server_path(root).is_none() || default_model_path(root).is_none() {
        return;
    }
    if managed_endpoint_ready() {
        set_managed_endpoint_env();
        return;
    }
    let status = start_or_reuse_managed_server(root);
    if status.state == "managed_server_running" {
        set_managed_endpoint_env();
    } else {
        eprintln!("{}", status.message);
    }
}

pub(crate) fn render_setup_embeddings_markdown(output: &ManagedEmbeddingsSetupOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Managed Embeddings Setup");
    let _ = writeln!(markdown, "root: `{}`", output.root);
    let _ = writeln!(markdown, "endpoint: `{}`", output.endpoint);
    let _ = writeln!(markdown, "dry_run: `{}`", output.dry_run);
    let _ = writeln!(markdown);
    let _ = writeln!(
        markdown,
        "- llama: `{}` ({:?}, {} bytes)",
        output.llama.name, output.llama_variant, output.llama.size_bytes
    );
    let _ = writeln!(markdown, "- llama_path: `{}`", output.llama.path);
    let _ = writeln!(
        markdown,
        "- model: `{}` ({:?}, {} bytes)",
        output.model.name, output.model_quant, output.model.size_bytes
    );
    let _ = writeln!(markdown, "- model_path: `{}`", output.model.path);
    let _ = writeln!(markdown, "- status: `{}`", output.status.state);
    let _ = writeln!(markdown, "- message: {}", output.status.message);
    if !output.next_commands.is_empty() {
        let _ = writeln!(markdown);
        let _ = writeln!(markdown, "next_commands:");
        for command in &output.next_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }
    markdown
}

fn start_or_reuse_managed_server(root: &Path) -> ManagedEmbeddingsStatus {
    if managed_endpoint_ready() {
        set_managed_endpoint_env();
        return inspect_status(root);
    }

    let Some(server) = llama_server_path(root) else {
        return inspect_status(root);
    };
    let Some(model) = default_model_path(root) else {
        return inspect_status(root);
    };
    if let Err(error) = spawn_llama_server(root, &server, &model) {
        return ManagedEmbeddingsStatus {
            state: "managed_server_stopped".to_string(),
            message: format!(
                "Failed to start managed llama-server: {error:#}. Check logs in `{}`; if Vulkan startup fails on this machine, rerun setup with `--variant cpu`.",
                clean_path(&logs_dir(root))
            ),
            root: clean_path(root),
            endpoint: MANAGED_LLAMACPP_URL.to_string(),
            llama_server: Some(clean_path(&server)),
            model: Some(clean_path(&model)),
        };
    }

    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if managed_endpoint_ready() {
            set_managed_endpoint_env();
            return inspect_status(root);
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    ManagedEmbeddingsStatus {
        state: "managed_server_stopped".to_string(),
        message: format!(
            "Managed llama-server was started, but {MANAGED_LLAMACPP_URL} did not become reachable within 20 seconds. Check logs in `{}`; if Vulkan startup fails on this machine, rerun setup with `--variant cpu`.",
            clean_path(&logs_dir(root))
        ),
        root: clean_path(root),
        endpoint: MANAGED_LLAMACPP_URL.to_string(),
        llama_server: Some(clean_path(&server)),
        model: Some(clean_path(&model)),
    }
}

fn spawn_llama_server(root: &Path, server: &Path, model: &Path) -> Result<()> {
    fs::create_dir_all(logs_dir(root))
        .with_context(|| format!("Failed to create {}", logs_dir(root).display()))?;
    let server = canonical_child_path(root, server).with_context(|| {
        format!(
            "Managed llama-server path is not trusted: {}",
            server.display()
        )
    })?;
    let model = canonical_child_path(root, model)
        .with_context(|| format!("Managed model path is not trusted: {}", model.display()))?;
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_dir(root).join("llama-server.out.log"))
        .context("Failed to open managed llama stdout log")?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_dir(root).join("llama-server.err.log"))
        .context("Failed to open managed llama stderr log")?;
    let mut command = Command::new(&server);
    command
        .args(llama_server_args(&model))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if let Some(parent) = server.parent() {
        command.current_dir(parent);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .with_context(|| format!("Failed to spawn {}", server.display()))?;
    Ok(())
}

fn llama_server_args(model: &Path) -> Vec<String> {
    vec![
        "--embedding".to_string(),
        "--model".to_string(),
        model.to_string_lossy().to_string(),
        "--pooling".to_string(),
        "cls".to_string(),
        "--host".to_string(),
        MANAGED_HOST.to_string(),
        "--port".to_string(),
        MANAGED_PORT.to_string(),
    ]
}

fn install_asset(url: &str, destination: &Path, expected_sha256: &str) -> Result<()> {
    if destination.exists() {
        verify_sha256(destination, expected_sha256)?;
        return Ok(());
    }
    let Some(parent) = destination.parent() else {
        bail!(
            "Managed asset destination has no parent: {}",
            destination.display()
        );
    };
    fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    let partial = destination.with_extension("download");
    let curl = trusted_tool_path("curl", &[parent])?;
    let status = Command::new(&curl)
        .arg("--fail")
        .arg("--location")
        .arg("--retry")
        .arg("3")
        .arg("--output")
        .arg(&partial)
        .arg(url)
        .status()
        .with_context(|| {
            format!(
                "Failed to run trusted curl at {} while downloading {url}",
                curl.display()
            )
        })?;
    if !status.success() {
        bail!("curl failed while downloading {url}");
    }
    verify_sha256(&partial, expected_sha256)?;
    fs::rename(&partial, destination).with_context(|| {
        format!(
            "Failed to move downloaded asset {} to {}",
            partial.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        bail!(
            "Checksum mismatch for {}: expected {}, got {}",
            path.display(),
            expected,
            actual
        );
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn extract_llama_archive(archive: &Path, destination: &Path) -> Result<()> {
    let marker = destination.join(".codestory-extracted");
    if marker.exists() && llama_server_in_dir(destination).is_some() {
        return Ok(());
    }
    let entries = archive_entries(archive)?;
    validate_archive_entries(&entries)?;
    let staging = destination.with_extension("staging");
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .with_context(|| format!("Failed to remove {}", staging.display()))?;
    }
    fs::create_dir_all(&staging)
        .with_context(|| format!("Failed to create {}", staging.display()))?;
    let tar = trusted_tool_path(
        "tar",
        &[
            archive.parent().unwrap_or_else(|| Path::new(".")),
            destination,
        ],
    )?;
    let status = Command::new(&tar)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .with_context(|| {
            format!(
                "Failed to run trusted tar at {} while extracting {}",
                tar.display(),
                archive.display()
            )
        })?;
    if !status.success() {
        bail!("tar failed while extracting {}", archive.display());
    }
    if destination.exists() {
        fs::remove_dir_all(destination)
            .with_context(|| format!("Failed to remove {}", destination.display()))?;
    }
    fs::rename(&staging, destination).with_context(|| {
        format!(
            "Failed to move extracted llama archive {} to {}",
            staging.display(),
            destination.display()
        )
    })?;
    fs::write(marker, b"ok").context("Failed to write managed extraction marker")?;
    Ok(())
}

fn archive_entries(archive: &Path) -> Result<Vec<String>> {
    let tar = trusted_tool_path("tar", &[archive.parent().unwrap_or_else(|| Path::new("."))])?;
    let output = Command::new(&tar)
        .arg("-tf")
        .arg(archive)
        .output()
        .with_context(|| {
            format!(
                "Failed to run trusted tar at {} while listing {}",
                tar.display(),
                archive.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "tar failed while listing {}\nstderr:\n{}",
            archive.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub(crate) fn validate_archive_entries(entries: &[String]) -> Result<()> {
    if entries.is_empty() {
        bail!("Managed llama archive did not contain any entries");
    }
    for entry in entries {
        if !archive_entry_is_safe(entry) {
            bail!("Unsafe archive entry in managed llama asset: {entry}");
        }
    }
    Ok(())
}

pub(crate) fn archive_entry_is_safe(entry: &str) -> bool {
    let trimmed = entry.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.starts_with('\\')
        || trimmed.contains(':')
    {
        return false;
    }
    let path = Path::new(trimmed);
    path.components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn trusted_tool_path(tool: &str, disallowed_roots: &[&Path]) -> Result<PathBuf> {
    let Some(path_var) = std::env::var_os("PATH") else {
        bail!("PATH is not set; cannot locate trusted `{tool}`");
    };
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for candidate_name in command_candidate_names(tool) {
            let candidate = dir.join(&candidate_name);
            if !candidate.is_file() {
                continue;
            }
            let canonical = fs::canonicalize(&candidate)
                .with_context(|| format!("Failed to resolve {}", candidate.display()))?;
            if path_is_under_disallowed_root(&canonical, disallowed_roots) {
                continue;
            }
            return Ok(canonical);
        }
    }
    bail!("Could not find trusted `{tool}` on PATH outside the project/cache roots")
}

fn command_candidate_names(tool: &str) -> Vec<String> {
    if Path::new(tool).extension().is_some() {
        return vec![tool.to_string()];
    }
    #[cfg(target_os = "windows")]
    {
        let mut names = vec![tool.to_string()];
        let path_ext = std::env::var_os("PATHEXT")
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
        for extension in path_ext.split(';') {
            if extension.trim().is_empty() {
                continue;
            }
            names.push(format!("{tool}{extension}"));
            names.push(format!("{tool}{}", extension.to_ascii_lowercase()));
        }
        names
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![tool.to_string()]
    }
}

fn path_is_under_disallowed_root(path: &Path, disallowed_roots: &[&Path]) -> bool {
    let current_dir = std::env::current_dir()
        .ok()
        .and_then(|path| fs::canonicalize(path).ok());
    if current_dir
        .as_ref()
        .is_some_and(|root| path.starts_with(root))
    {
        return true;
    }
    disallowed_roots
        .iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .any(|root| path.starts_with(root))
}

fn write_manifest(
    root: &Path,
    llama: LlamaAsset,
    model: ModelAsset,
    llama_extract_dir: &Path,
    model_path: &Path,
) -> Result<()> {
    fs::create_dir_all(root).with_context(|| format!("Failed to create {}", root.display()))?;
    let manifest = serde_json::json!({
        "llama_release": LLAMA_RELEASE_TAG,
        "llama_asset": llama.name,
        "llama_sha256": llama.sha256,
        "llama_path": clean_path(llama_extract_dir),
        "llama_variant": llama.variant,
        "model_asset": model.name,
        "model_sha256": model.sha256,
        "model_path": clean_path(model_path),
        "model_quant": model.quant,
        "endpoint": MANAGED_LLAMACPP_URL,
    });
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("serialize managed embeddings manifest"),
    )
    .with_context(|| format!("Failed to write {}", root.join("manifest.json").display()))?;
    Ok(())
}

fn select_llama_asset(variant: CliLlamaVariant) -> Result<LlamaAsset> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    if let Some(asset) = LLAMA_ASSETS
        .iter()
        .copied()
        .find(|asset| asset.os == os && asset.arch == arch && asset.variant == variant)
    {
        return Ok(asset);
    }
    if variant == CliLlamaVariant::Vulkan
        && let Some(asset) = LLAMA_ASSETS.iter().copied().find(|asset| {
            asset.os == os && asset.arch == arch && asset.variant == CliLlamaVariant::Cpu
        })
    {
        return Ok(asset);
    }
    Err(anyhow!(
        "No pinned llama.cpp {variant:?} asset for {os}/{arch}. Use --variant cpu or an external llama.cpp endpoint via CODESTORY_EMBED_LLAMACPP_URL."
    ))
}

fn select_model_asset(quant: CliEmbeddingQuant) -> ModelAsset {
    MODEL_ASSETS
        .iter()
        .copied()
        .find(|asset| asset.quant == quant)
        .expect("all CLI model quant values have a pinned asset")
}

fn downloads_dir(root: &Path) -> PathBuf {
    root.join("downloads")
}

fn models_dir(root: &Path) -> PathBuf {
    root.join("models").join("bge-base-en-v1.5-gguf")
}

fn logs_dir(root: &Path) -> PathBuf {
    root.join("logs")
}

fn llama_extract_dir(root: &Path, asset: LlamaAsset) -> PathBuf {
    let archive_label = asset
        .name
        .strip_suffix(".tar.gz")
        .or_else(|| asset.name.strip_suffix(".zip"))
        .unwrap_or(asset.name);
    root.join("llama")
        .join(LLAMA_RELEASE_TAG)
        .join(archive_label)
}

fn manifest_child_path(root: &Path, raw_path: &str) -> Option<PathBuf> {
    let path = PathBuf::from(raw_path);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    canonical_child_path(root, &candidate).ok()
}

fn canonical_child_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("Failed to resolve managed root {}", root.display()))?;
    let path = fs::canonicalize(path)
        .with_context(|| format!("Failed to resolve managed path {}", path.display()))?;
    if !path.starts_with(&root) {
        bail!(
            "Managed path {} is outside managed root {}",
            path.display(),
            root.display()
        );
    }
    Ok(path)
}

fn default_model_path(root: &Path) -> Option<PathBuf> {
    if let Some(manifest) = read_manifest(root)
        && let Some(model_path) = manifest.model_path
        && let Some(path) = manifest_child_path(root, &model_path)
    {
        if path.exists() {
            return Some(path);
        }
    }
    MODEL_ASSETS
        .iter()
        .filter_map(|asset| canonical_child_path(root, &models_dir(root).join(asset.name)).ok())
        .find(|path| path.exists())
}

fn llama_server_path(root: &Path) -> Option<PathBuf> {
    if let Some(manifest) = read_manifest(root)
        && let Some(llama_path) = manifest.llama_path
        && let Some(dir) = manifest_child_path(root, &llama_path)
        && let Some(server) =
            llama_server_in_dir(&dir).and_then(|path| canonical_child_path(root, &path).ok())
    {
        return Some(server);
    }
    let llama_root = root.join("llama").join(LLAMA_RELEASE_TAG);
    llama_server_in_dir(&llama_root).and_then(|path| canonical_child_path(root, &path).ok())
}

fn read_manifest(root: &Path) -> Option<ManagedManifest> {
    let path = root.join("manifest.json");
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn llama_server_in_dir(root: &Path) -> Option<PathBuf> {
    let executable = if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    find_file_named(root, executable)
}

fn find_file_named(root: &Path, name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_file_named(&path, name)
        {
            return Some(found);
        }
    }
    None
}

fn disabled_by_embedding_env() -> bool {
    env_value_is_hash("CODESTORY_EMBED_RUNTIME_MODE")
        || env_value_is_hash("CODESTORY_EMBED_BACKEND")
        || matches!(
            std::env::var("CODESTORY_HYBRID_RETRIEVAL_ENABLED")
                .ok()
                .map(|value| value.trim().to_ascii_lowercase()),
            Some(value) if value == "0" || value == "false" || value == "off"
        )
}

fn env_value_is_hash(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_ascii_lowercase()),
        Some(value) if value == "hash" || value == "hash_projection"
    )
}

fn explicit_llama_url() -> Option<String> {
    std::env::var("CODESTORY_EMBED_LLAMACPP_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value != MANAGED_LLAMACPP_URL)
}

fn set_managed_endpoint_env() {
    unsafe {
        std::env::set_var("CODESTORY_EMBED_LLAMACPP_URL", MANAGED_LLAMACPP_URL);
    }
}

fn managed_endpoint_ready() -> bool {
    embedding_endpoint_ready(MANAGED_LLAMACPP_URL, Some(MANAGED_EMBEDDING_DIM))
}

pub(crate) fn embedding_endpoint_ready(url: &str, expected_dimension: Option<usize>) -> bool {
    probe_embedding_endpoint(url, expected_dimension).is_ok()
}

fn probe_embedding_endpoint(url: &str, expected_dimension: Option<usize>) -> Result<usize> {
    let endpoint = parse_http_endpoint(url)
        .ok_or_else(|| anyhow!("Managed embedding endpoint must be an http:// URL"))?;
    let request = serde_json::json!({
        "input": [ENDPOINT_PROBE_TEXT],
        "model": "codestory-local-embedding",
    });
    let response = post_json_to_endpoint(&endpoint, &request)?;
    parse_embedding_probe_response(response, expected_dimension)
}

fn post_json_to_endpoint(endpoint: &HttpEndpoint, request: &JsonValue) -> Result<JsonValue> {
    let body =
        serde_json::to_vec(request).context("failed to serialize embedding probe request")?;
    let mut addrs = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve embedding endpoint {}", endpoint.url()))?;
    let mut stream = addrs
        .find_map(|addr| TcpStream::connect_timeout(&addr, ENDPOINT_PROBE_TIMEOUT).ok())
        .ok_or_else(|| anyhow!("failed to connect to embedding endpoint {}", endpoint.url()))?;
    stream.set_read_timeout(Some(ENDPOINT_PROBE_TIMEOUT))?;
    stream.set_write_timeout(Some(ENDPOINT_PROBE_TIMEOUT))?;
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        endpoint.path,
        endpoint.host,
        endpoint.port,
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let (status_code, headers, body) = split_http_response(&response)?;
    if !(200..300).contains(&status_code) {
        bail!(
            "embedding endpoint {} returned HTTP {status_code}: {}",
            endpoint.url(),
            String::from_utf8_lossy(&body)
        );
    }
    let body = if headers
        .iter()
        .any(|(key, value)| key == "transfer-encoding" && value.contains("chunked"))
    {
        decode_chunked_http_body(&body)?
    } else {
        body
    };
    serde_json::from_slice(&body).with_context(|| {
        format!(
            "failed to parse JSON response from embedding endpoint {}",
            endpoint.url()
        )
    })
}

fn parse_embedding_probe_response(
    response: JsonValue,
    expected_dimension: Option<usize>,
) -> Result<usize> {
    let data = response
        .get("data")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| anyhow!("embedding probe response missing `data` array"))?;
    if data.len() != 1 {
        bail!(
            "embedding probe response returned {} vectors for one input",
            data.len()
        );
    }
    let embedding = data[0]
        .get("embedding")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| anyhow!("embedding probe response item missing `embedding`"))?;
    if embedding.is_empty() {
        bail!("embedding probe response returned an empty embedding");
    }
    if embedding.iter().any(|value| value.as_f64().is_none()) {
        bail!("embedding probe response contained a non-numeric embedding value");
    }
    let dimension = embedding.len();
    if let Some(expected_dimension) = expected_dimension
        && dimension != expected_dimension
    {
        bail!(
            "embedding probe response dimension mismatch: expected {}, got {}",
            expected_dimension,
            dimension
        );
    }
    Ok(dimension)
}

fn split_http_response(response: &[u8]) -> Result<RawHttpResponse> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid HTTP response from embedding endpoint"))?;
    let header_text = String::from_utf8_lossy(&response[..header_end]);
    let mut lines = header_text.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| anyhow!("missing HTTP status line from embedding endpoint"))?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("missing HTTP status code from embedding endpoint"))?
        .parse::<u16>()
        .context("invalid HTTP status code from embedding endpoint")?;
    let headers = lines
        .filter_map(|line| {
            line.split_once(':').map(|(key, value)| {
                (
                    key.trim().to_ascii_lowercase(),
                    value.trim().to_ascii_lowercase(),
                )
            })
        })
        .collect::<Vec<_>>();
    Ok((status_code, headers, response[header_end + 4..].to_vec()))
}

fn decode_chunked_http_body(body: &[u8]) -> Result<Vec<u8>> {
    let mut offset = 0;
    let mut decoded = Vec::new();
    while offset < body.len() {
        let line_end = body[offset..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| anyhow!("invalid chunked response from embedding endpoint"))?
            + offset;
        let size_text = String::from_utf8_lossy(&body[offset..line_end]);
        let size_hex = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_hex, 16)
            .context("invalid chunk size from embedding endpoint")?;
        offset = line_end + 2;
        if size == 0 {
            break;
        }
        if offset + size > body.len() {
            bail!("truncated chunked response from embedding endpoint");
        }
        decoded.extend_from_slice(&body[offset..offset + size]);
        offset += size + 2;
    }
    Ok(decoded)
}

fn parse_http_endpoint(url: &str) -> Option<HttpEndpoint> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = rest
        .split_once('/')
        .map(|(authority, path)| (authority, format!("/{path}")))
        .unwrap_or((rest, "/v1/embeddings".to_string()));
    let (host, port) = if let Some((host, raw_port)) = authority.rsplit_once(':') {
        (host.to_string(), raw_port.parse::<u16>().ok()?)
    } else {
        (authority.to_string(), 80)
    };
    (!host.trim().is_empty()).then_some(HttpEndpoint { host, port, path })
}

impl HttpEndpoint {
    fn url(&self) -> String {
        format!("http://{}:{}{}", self.host, self.port, self.path)
    }
}

fn clean_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn redact_url_for_display(value: &str) -> String {
    let trimmed = value.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return "set".to_string();
    };
    let without_fragment = rest.split('#').next().unwrap_or(rest);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    let host_and_path = without_query
        .rsplit_once('@')
        .map(|(_, after_userinfo)| after_userinfo)
        .unwrap_or(without_query);
    format!("{scheme}://{host_and_path}")
}

fn managed_status_message(root: &Path, running: bool) -> String {
    let install = managed_install_label(root)
        .map(|label| format!(" using {label}"))
        .unwrap_or_default();
    if running {
        format!(
            "Managed llama.cpp endpoint accepted an embeddings probe at {MANAGED_LLAMACPP_URL}{install}."
        )
    } else {
        format!(
            "Managed llama assets are installed{install}, but llama-server is not reachable. Run `codestory-cli setup embeddings` to start it; if Vulkan startup fails on this machine, rerun setup with `--variant cpu`."
        )
    }
}

fn managed_install_label(root: &Path) -> Option<String> {
    let manifest = read_manifest(root)?;
    let llama = manifest
        .llama_asset
        .as_deref()
        .or_else(|| manifest.llama_variant.map(llama_variant_label))?;
    let model = manifest
        .model_asset
        .as_deref()
        .or_else(|| manifest.model_quant.map(model_quant_label))?;
    Some(format!("`{llama}` and `{model}`"))
}

fn llama_variant_label(variant: CliLlamaVariant) -> &'static str {
    match variant {
        CliLlamaVariant::Cpu => "cpu",
        CliLlamaVariant::Vulkan => "vulkan",
    }
}

fn model_quant_label(quant: CliEmbeddingQuant) -> &'static str {
    match quant {
        CliEmbeddingQuant::Q8_0 => "q8_0",
        CliEmbeddingQuant::Q4KM => "q4_k_m",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;
    use tempfile::tempdir;

    fn endpoint_serving_response(response: String) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind probe server");
        let port = listener.local_addr().expect("probe server addr").port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept probe request");
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            loop {
                let read = stream.read(&mut buffer).expect("read probe request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request_is_complete(&request) {
                    break;
                }
            }
            stream
                .write_all(response.as_bytes())
                .expect("write probe response");
        });
        format!("http://127.0.0.1:{port}/v1/embeddings")
    }

    fn request_is_complete(request: &[u8]) -> bool {
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        request.len() >= header_end + 4 + content_length
    }

    fn json_http_response(body: serde_json::Value) -> String {
        let body = serde_json::to_string(&body).expect("json body");
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    #[test]
    fn archive_entry_safety_rejects_traversal_and_absolute_paths() {
        assert!(archive_entry_is_safe("bin/llama-server.exe"));
        assert!(archive_entry_is_safe("./bin/llama-server"));
        assert!(!archive_entry_is_safe("../escape"));
        assert!(!archive_entry_is_safe("/absolute"));
        assert!(!archive_entry_is_safe("C:/absolute"));
        assert!(!archive_entry_is_safe("dir/../../escape"));
    }

    #[test]
    fn checksum_verification_detects_mismatch() {
        let temp = tempdir().expect("temp dir");
        let file = temp.path().join("asset.bin");
        fs::write(&file, b"asset").expect("write asset");
        let actual = sha256_file(&file).expect("sha256");
        assert!(verify_sha256(&file, &actual).is_ok());
        assert!(
            verify_sha256(
                &file,
                "0000000000000000000000000000000000000000000000000000000000000000"
            )
            .is_err()
        );
    }

    #[test]
    fn setup_dry_run_reports_pinned_assets_without_writing() {
        let temp = tempdir().expect("temp dir");
        let output = setup_embeddings(
            temp.path(),
            CliEmbeddingQuant::Q8_0,
            CliLlamaVariant::Cpu,
            true,
            false,
        )
        .expect("dry-run setup");
        assert!(output.dry_run);
        assert!(output.llama.url.contains("ggml-org/llama.cpp"));
        assert!(
            output
                .model
                .url
                .contains("CompendiumLabs/bge-base-en-v1.5-gguf")
        );
        assert!(!temp.path().join("downloads").exists());
    }

    #[test]
    fn vulkan_selection_falls_back_to_cpu_when_platform_has_no_vulkan_asset() {
        let asset = select_llama_asset(CliLlamaVariant::Vulkan).expect("default asset");
        if std::env::consts::OS == "macos" {
            assert_eq!(asset.variant, CliLlamaVariant::Cpu);
        } else if std::env::consts::ARCH == "x86_64" {
            assert_eq!(asset.variant, CliLlamaVariant::Vulkan);
        }
    }

    #[test]
    fn manifest_selected_server_path_wins_when_multiple_variants_exist() {
        let temp = tempdir().expect("temp dir");
        let root = temp.path();
        let executable = if cfg!(target_os = "windows") {
            "llama-server.exe"
        } else {
            "llama-server"
        };
        let cpu_dir = root.join("llama").join(LLAMA_RELEASE_TAG).join("cpu");
        let vulkan_dir = root.join("llama").join(LLAMA_RELEASE_TAG).join("vulkan");
        fs::create_dir_all(&cpu_dir).expect("create cpu dir");
        fs::create_dir_all(&vulkan_dir).expect("create vulkan dir");
        fs::write(cpu_dir.join(executable), b"cpu").expect("write cpu server");
        fs::write(vulkan_dir.join(executable), b"vulkan").expect("write vulkan server");

        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "llama_asset": "llama-b9058-bin-win-vulkan-x64.zip",
                "llama_path": clean_path(&vulkan_dir),
                "llama_variant": "vulkan",
                "model_asset": "bge-base-en-v1.5-q8_0.gguf",
                "model_path": clean_path(&root.join("models/bge-base-en-v1.5-q8_0.gguf")),
                "model_quant": "q8_0",
            }))
            .expect("manifest json"),
        )
        .expect("write manifest");

        let selected = llama_server_path(root).expect("selected server");
        assert_eq!(
            selected,
            fs::canonicalize(vulkan_dir.join(executable)).expect("canonical vulkan server")
        );
    }

    #[test]
    fn server_command_uses_embedding_endpoint_contract() {
        let args = llama_server_args(Path::new("model.gguf"));
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--model" && pair[1] == "model.gguf")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--pooling" && pair[1] == "cls")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--host" && pair[1] == MANAGED_HOST)
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--port" && pair[1] == MANAGED_PORT.to_string())
        );
        assert!(args.iter().any(|arg| arg == "--embedding"));
    }

    #[test]
    fn embedding_endpoint_probe_accepts_openai_embeddings_response() {
        let url = endpoint_serving_response(json_http_response(serde_json::json!({
            "data": [{
                "index": 0,
                "embedding": vec![0.125_f32; MANAGED_EMBEDDING_DIM],
            }]
        })));

        probe_embedding_endpoint(&url, Some(MANAGED_EMBEDDING_DIM))
            .expect("valid embedding endpoint should pass");
    }

    #[test]
    fn embedding_endpoint_probe_rejects_plain_http_response() {
        let url = endpoint_serving_response(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok"
                .to_string(),
        );

        assert!(!embedding_endpoint_ready(&url, Some(MANAGED_EMBEDDING_DIM)));
    }

    #[test]
    fn embedding_endpoint_probe_rejects_wrong_managed_dimension() {
        let url = endpoint_serving_response(json_http_response(serde_json::json!({
            "data": [{
                "index": 0,
                "embedding": [0.1, 0.2, 0.3],
            }]
        })));

        assert!(!embedding_endpoint_ready(&url, Some(MANAGED_EMBEDDING_DIM)));
    }

    #[test]
    fn external_embedding_endpoint_probe_does_not_require_managed_dimension() {
        let url = endpoint_serving_response(json_http_response(serde_json::json!({
            "data": [{
                "index": 0,
                "embedding": [0.1, 0.2, 0.3],
            }]
        })));

        probe_embedding_endpoint(&url, None).expect("external endpoint should not require dim");
    }

    #[test]
    fn manifest_paths_are_under_managed_root() {
        let temp = tempdir().expect("temp dir");
        let root = managed_root(Some(temp.path())).expect("managed root");
        assert!(root.starts_with(temp.path()));
        assert!(root.ends_with(MANAGED_DIR_NAME));
    }

    #[test]
    fn manifest_paths_outside_managed_root_are_ignored() {
        let temp = tempdir().expect("temp dir");
        let root = temp.path().join("managed");
        let outside = temp.path().join("outside");
        let executable = if cfg!(target_os = "windows") {
            "llama-server.exe"
        } else {
            "llama-server"
        };
        fs::create_dir_all(&root).expect("create managed root");
        fs::create_dir_all(&outside).expect("create outside dir");
        fs::write(outside.join(executable), b"not trusted").expect("write outside server");
        fs::write(outside.join("model.gguf"), b"not trusted").expect("write outside model");
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "llama_path": clean_path(&outside),
                "model_path": clean_path(&outside.join("model.gguf")),
            }))
            .expect("manifest json"),
        )
        .expect("write manifest");

        assert!(llama_server_path(&root).is_none());
        assert!(default_model_path(&root).is_none());
    }

    #[test]
    fn trusted_tool_lookup_rejects_disallowed_roots() {
        let temp = tempdir().expect("temp dir");
        let disallowed = temp.path().join("repo");
        let other = temp.path().join("tools");
        fs::create_dir_all(&disallowed).expect("create disallowed");
        fs::create_dir_all(&other).expect("create tools");
        let tool = if cfg!(target_os = "windows") {
            "codestory-test-tool.exe"
        } else {
            "codestory-test-tool"
        };
        let disallowed_tool = disallowed.join(tool);
        let trusted_tool = other.join(tool);
        fs::write(&disallowed_tool, b"bad").expect("write disallowed tool");
        fs::write(&trusted_tool, b"ok").expect("write trusted tool");

        assert!(path_is_under_disallowed_root(
            &fs::canonicalize(disallowed_tool).expect("canonical disallowed"),
            &[&disallowed]
        ));
        assert!(!path_is_under_disallowed_root(
            &fs::canonicalize(trusted_tool).expect("canonical trusted"),
            &[&disallowed]
        ));
    }
}
