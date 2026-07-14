use anyhow::{Context, Result, bail};
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const EMBEDDING_MODELS_JSON: &str = include_str!("../assets/embedding-models.json");
const ASSET_LOCK_FILE: &str = "managed-embeddings/assets.lock";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct ManagedModelAsset {
    pub(crate) id: String,
    pub(crate) filename: String,
    pub(crate) artifact_bytes: u64,
    pub(crate) sha256: String,
    pub(crate) urls: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ManagedModelManifest {
    models: Vec<ManagedModelAsset>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManagedModelInstallManifest {
    id: String,
    filename: String,
    artifact_bytes: u64,
    sha256: String,
    source_url: String,
}

pub(crate) struct ManagedAssetLock {
    _file: File,
}

impl ManagedAssetLock {
    pub(crate) fn acquire(cache_root: &Path) -> Result<Self> {
        let path = cache_root.join(ASSET_LOCK_FILE);
        fs::create_dir_all(path.parent().expect("asset lock has a parent"))
            .with_context(|| format!("create managed asset cache at {}", path.display()))?;
        let file = open_regular_lock_file(&path)?;
        FileExt::lock_exclusive(&file)
            .with_context(|| format!("lock managed asset cache {}", path.display()))?;
        Ok(Self { _file: file })
    }
}

fn open_regular_lock_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).truncate(false);
    let file = match options.create_new(true).open(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            options.create_new(false).open(path)
        }
        Err(error) => Err(error),
    }
    .with_context(|| format!("open managed asset lock {}", path.display()))?;
    let path_metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect managed asset lock {}", path.display()))?;
    let file_metadata = file
        .metadata()
        .with_context(|| format!("inspect opened managed asset lock {}", path.display()))?;
    if !path_metadata.file_type().is_file() || !file_metadata.file_type().is_file() {
        bail!(
            "managed asset lock is not a regular file: {}",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if path_metadata.dev() != file_metadata.dev() || path_metadata.ino() != file_metadata.ino()
        {
            bail!(
                "managed asset lock changed while opening: {}",
                path.display()
            );
        }
    }
    Ok(file)
}

pub(crate) fn pinned_embedding_model() -> ManagedModelAsset {
    let manifest: ManagedModelManifest =
        serde_json::from_str(EMBEDDING_MODELS_JSON).expect("embedded model manifest must be valid");
    let [model] = manifest.models.as_slice() else {
        panic!("embedded model manifest must contain exactly one product model");
    };
    assert_eq!(
        model.filename,
        crate::embeddings::BGE_BASE_EN_V1_5_GGUF,
        "managed model filename must match the product embedding contract"
    );
    model.clone()
}

pub(crate) fn managed_embedding_model_dir(cache_root: &Path) -> PathBuf {
    let model = pinned_embedding_model();
    cache_root
        .join("managed-embeddings/models/sha256")
        .join(&model.sha256)
}

pub(crate) fn managed_embedding_model_is_published(cache_root: &Path) -> bool {
    let model = pinned_embedding_model();
    let dir = managed_embedding_model_dir(cache_root);
    let path = dir.join(&model.filename);
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return false;
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != model.artifact_bytes
    {
        return false;
    }
    let Ok(manifest) = fs::read(dir.join("install-manifest.json")) else {
        return false;
    };
    serde_json::from_slice::<ManagedModelInstallManifest>(&manifest).is_ok_and(|manifest| {
        manifest.id == model.id
            && manifest.filename == model.filename
            && manifest.artifact_bytes == model.artifact_bytes
            && manifest.sha256.eq_ignore_ascii_case(&model.sha256)
    })
}

pub(crate) fn ensure_managed_embedding_model(cache_root: &Path) -> Result<PathBuf> {
    let model = pinned_embedding_model();
    let _lock = ManagedAssetLock::acquire(cache_root)?;
    let dir = managed_embedding_model_dir(cache_root);
    let path = dir.join(&model.filename);
    let source_url =
        ensure_cached_asset_locked(&path, &model.urls, model.artifact_bytes, &model.sha256)?;
    if source_url == "cache" && managed_embedding_model_is_published(cache_root) {
        return Ok(dir);
    }
    let manifest = ManagedModelInstallManifest {
        id: model.id,
        filename: model.filename,
        artifact_bytes: model.artifact_bytes,
        sha256: model.sha256,
        source_url,
    };
    codestory_workspace::atomic_file::write_bytes_atomic(
        &dir.join("install-manifest.json"),
        "managed-model-install-manifest",
        &serde_json::to_vec_pretty(&manifest).expect("serialize managed model manifest"),
    )?;
    Ok(dir)
}

pub(crate) fn ensure_cached_asset_locked(
    destination: &Path,
    urls: &[String],
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<String> {
    validate_asset_metadata(urls, expected_bytes, expected_sha256)?;
    fs::create_dir_all(
        destination
            .parent()
            .context("managed asset has no parent")?,
    )
    .with_context(|| {
        format!(
            "create managed asset directory for {}",
            destination.display()
        )
    })?;
    quarantine_interrupted_partials(destination)?;
    if destination.is_file() && file_matches(destination, expected_bytes, expected_sha256) {
        return Ok("cache".to_string());
    }
    if fs::symlink_metadata(destination).is_ok() {
        quarantine_path(destination, "invalid")?;
    }

    let mut errors = Vec::new();
    for url in urls {
        let partial = unique_partial_path(destination);
        let downloaded = download_exact(url, &partial, expected_bytes)
            .and_then(|()| verify_file(&partial, expected_bytes, expected_sha256));
        match downloaded {
            Ok(()) => {
                codestory_workspace::atomic_file::publish_existing_file_atomic(
                    &partial,
                    destination,
                )?;
                return Ok(url.clone());
            }
            Err(error) => {
                if partial.symlink_metadata().is_ok() {
                    let _ = quarantine_path(&partial, "download");
                }
                errors.push(format!("{url}: {error:#}"));
            }
        }
    }
    bail!(
        "managed asset download failed after {} source(s): {}",
        urls.len(),
        errors.join("; ")
    )
}

pub(crate) fn quarantine_path(path: &Path, reason: &str) -> Result<PathBuf> {
    let parent = path.parent().context("quarantine path has no parent")?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("quarantine path has no portable filename")?;
    let quarantine = parent.join(format!(
        ".quarantine-{name}-{reason}-{}-{}",
        std::process::id(),
        now_nanos()
    ));
    fs::rename(path, &quarantine).with_context(|| {
        format!(
            "quarantine managed asset {} as {}",
            path.display(),
            quarantine.display()
        )
    })?;
    Ok(quarantine)
}

fn validate_asset_metadata(urls: &[String], expected_bytes: u64, sha256: &str) -> Result<()> {
    if urls.is_empty() || expected_bytes == 0 || !is_sha256(sha256) {
        bail!("managed asset metadata is incomplete");
    }
    if urls
        .iter()
        .any(|url| !url.starts_with("https://") && !cfg!(test))
    {
        bail!("managed asset URLs must use HTTPS");
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn quarantine_interrupted_partials(destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .context("managed asset has no parent")?;
    let prefix = format!(
        ".{}.partial-",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .context("managed asset has no portable filename")?
    );
    for entry in fs::read_dir(parent).with_context(|| format!("read {}", parent.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", parent.display()))?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            quarantine_path(&entry.path(), "interrupted")?;
        }
    }
    Ok(())
}

fn unique_partial_path(destination: &Path) -> PathBuf {
    let parent = destination.parent().expect("managed asset has a parent");
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .expect("managed asset has a portable filename");
    parent.join(format!(
        ".{name}.partial-{}-{}",
        std::process::id(),
        now_nanos()
    ))
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn download_exact(url: &str, destination: &Path, expected_bytes: u64) -> Result<()> {
    let response = ureq::get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .call()
        .with_context(|| format!("download {url}"))?;
    if let Some(content_length) = response.header("content-length") {
        let announced = content_length
            .parse::<u64>()
            .with_context(|| format!("invalid content-length from {url}"))?;
        if announced != expected_bytes {
            bail!("download size mismatch from {url}: expected {expected_bytes}, got {announced}");
        }
    }
    let mut source = response.into_reader();
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("create {}", destination.display()))?;
    let mut received = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source
            .read(&mut buffer)
            .with_context(|| format!("read {url}"))?;
        if read == 0 {
            break;
        }
        received = received.saturating_add(read as u64);
        if received > expected_bytes {
            bail!("download exceeds declared size from {url}");
        }
        target
            .write_all(&buffer[..read])
            .with_context(|| format!("write {}", destination.display()))?;
    }
    if received != expected_bytes {
        bail!("download size mismatch from {url}: expected {expected_bytes}, got {received}");
    }
    target
        .sync_all()
        .with_context(|| format!("sync {}", destination.display()))?;
    Ok(())
}

fn verify_file(path: &Path, expected_bytes: u64, expected_sha256: &str) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("metadata {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("managed asset is not a regular file: {}", path.display());
    }
    if metadata.len() != expected_bytes {
        bail!(
            "managed asset size mismatch for {}: expected {}, got {}",
            path.display(),
            expected_bytes,
            metadata.len()
        );
    }
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        bail!(
            "managed asset checksum mismatch for {}: expected {}, got {}",
            path.display(),
            expected_sha256,
            actual
        );
    }
    Ok(())
}

fn file_matches(path: &Path, expected_bytes: u64, expected_sha256: &str) -> bool {
    verify_file(path, expected_bytes, expected_sha256).is_ok()
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use tempfile::tempdir;

    fn serve_once(body: Vec<u8>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let url = format!(
            "http://{}/asset",
            listener.local_addr().expect("server addr")
        );
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).expect("read request");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("write response headers");
            stream.write_all(&body).expect("write response body");
        });
        (url, handle)
    }

    #[test]
    fn product_model_metadata_resolves_to_one_machine_content_address() {
        let first = tempdir().expect("cache");
        let second_project = tempdir().expect("second project");
        let model = pinned_embedding_model();
        let path = managed_embedding_model_dir(first.path());

        assert_eq!(model.filename, crate::embeddings::BGE_BASE_EN_V1_5_GGUF);
        assert!(path.starts_with(first.path()));
        assert!(path.ends_with(&model.sha256));
        assert!(!path.starts_with(second_project.path()));
    }

    #[test]
    fn corrupt_and_interrupted_assets_are_quarantined_before_atomic_publication() {
        let cache = tempdir().expect("cache");
        let destination = cache.path().join("sha256/test/asset.bin");
        fs::create_dir_all(destination.parent().unwrap()).expect("asset dir");
        fs::write(&destination, b"corrupt").expect("corrupt asset");
        fs::write(
            destination
                .parent()
                .unwrap()
                .join(".asset.bin.partial-dead"),
            b"partial",
        )
        .expect("partial asset");
        let body = b"verified asset".to_vec();
        let sha = format!("{:x}", Sha256::digest(&body));
        let (url, server) = serve_once(body.clone());
        let _lock = ManagedAssetLock::acquire(cache.path()).expect("asset lock");

        ensure_cached_asset_locked(&destination, &[url], body.len() as u64, &sha)
            .expect("publish asset");
        server.join().expect("server thread");

        assert_eq!(fs::read(&destination).expect("published asset"), body);
        let quarantines = fs::read_dir(destination.parent().unwrap())
            .expect("asset dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".quarantine-")
            })
            .count();
        assert_eq!(quarantines, 2);
    }
}
