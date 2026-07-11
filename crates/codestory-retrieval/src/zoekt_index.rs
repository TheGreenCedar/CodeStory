//! Build per-project Zoekt shard directories (lexical index + optional remote index).

use crate::config::ZOEKT_REAL_VERSION_PIN;
use anyhow::{Context, Result, bail};
use codestory_store::{FileRole, Store, SymbolSearchDoc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

const LEXICAL_INDEX_FILE: &str = "lexical-index.jsonl";
const SHARD_META_FILE: &str = "shard-meta.json";
const STUB_MARKER: &str = ".zoekt-stub";

const MAX_FILE_BYTES: usize = 256 * 1024;
const VALIDATED_SHARD_CACHE_CAPACITY: usize = 128;
const VALIDATED_SHARD_CACHE_TTL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified_nanos: u128,
    change_token: i128,
    device: u64,
    file_id: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShardValidationCacheKey {
    index_path: PathBuf,
    expected_sidecar_input_hash: String,
    binding_sha256: String,
    data_identity: FileIdentity,
    meta_identity: FileIdentity,
}

#[derive(Debug, Clone)]
struct CachedShardValidation {
    key: ShardValidationCacheKey,
    validated_at: Instant,
}

static VALIDATED_SHARD_CACHE: OnceLock<Mutex<VecDeque<CachedShardValidation>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LexicalIndexEntry {
    path: String,
    content: String,
    #[serde(default)]
    source: LexicalDocumentSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    symbol_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LexicalDocumentSource {
    #[default]
    LexicalSource,
    SymbolDoc,
    ComponentReport,
}

impl LexicalDocumentSource {
    pub(crate) fn provenance_label(self) -> &'static str {
        match self {
            Self::LexicalSource => "lexical_source",
            Self::SymbolDoc => "symbol_doc",
            Self::ComponentReport => "component_report",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ShardMeta {
    version: String,
    project_id: String,
    file_count: u32,
    lexical_hash: Option<String>,
    #[serde(default)]
    sidecar_input_hash: Option<String>,
    #[serde(default)]
    data_sha256: Option<String>,
    #[serde(default)]
    data_bytes: Option<u64>,
    #[serde(default)]
    binding_sha256: Option<String>,
    indexed_at_epoch_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalInputFingerprint {
    pub file_count: u32,
    pub hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShardPublishPhase {
    DataWrite,
    DataPublish,
    MetadataWrite,
    MetadataPublish,
}

/// Populate `shards/<project_id>/` with a searchable lexical index; remove stub marker on success.
pub fn build_zoekt_shard(
    project_root: &Path,
    storage_path: Option<&Path>,
    zoekt_data_dir: &Path,
    project_id: &str,
    expected: &LexicalInputFingerprint,
    sidecar_input_hash: &str,
) -> Result<bool> {
    build_zoekt_shard_with_checkpoint(
        project_root,
        storage_path,
        zoekt_data_dir,
        project_id,
        expected,
        sidecar_input_hash,
        |_| Ok(()),
    )
}

fn build_zoekt_shard_with_checkpoint(
    project_root: &Path,
    storage_path: Option<&Path>,
    zoekt_data_dir: &Path,
    project_id: &str,
    expected: &LexicalInputFingerprint,
    sidecar_input_hash: &str,
    mut checkpoint: impl FnMut(ShardPublishPhase) -> Result<()>,
) -> Result<bool> {
    let entries = collect_lexical_entries(project_root, storage_path)?;
    if entries.is_empty() {
        return Ok(false);
    }
    let lexical_hash = lexical_entries_hash(&entries);
    if entries.len().min(u32::MAX as usize) as u32 != expected.file_count
        || lexical_hash != expected.hash
    {
        bail!(
            "lexical input changed while building shard: expected {} entries with hash {}, collected {} entries with hash {lexical_hash}",
            expected.file_count,
            expected.hash,
            entries.len()
        );
    }

    let shard_dir = zoekt_data_dir.join("shards").join(project_id);
    std::fs::create_dir_all(&shard_dir)
        .with_context(|| format!("create zoekt shard dir {}", shard_dir.display()))?;

    let index_path = shard_dir.join(LEXICAL_INDEX_FILE);
    let version = ZOEKT_REAL_VERSION_PIN.to_string();
    let mut meta = ShardMeta {
        version: version.clone(),
        project_id: project_id.to_string(),
        file_count: entries.len() as u32,
        lexical_hash: Some(lexical_hash),
        sidecar_input_hash: Some(sidecar_input_hash.to_string()),
        data_sha256: None,
        data_bytes: None,
        binding_sha256: None,
        indexed_at_epoch_ms: chrono::Utc::now().timestamp_millis(),
    };
    checkpoint(ShardPublishPhase::DataWrite)?;
    codestory_workspace::atomic_file::write_file_atomic(
        &index_path,
        "lexical-index",
        |file| write_lexical_entries(file, &entries),
        |temp_path| {
            validate_lexical_index_file(temp_path, &meta)?;
            checkpoint(ShardPublishPhase::DataPublish)
        },
    )
    .context("publish lexical index")?;

    let (data_bytes, data_sha256) = lexical_index_file_fingerprint(&index_path)?;
    meta.data_bytes = Some(data_bytes);
    meta.data_sha256 = Some(data_sha256);
    meta.binding_sha256 = Some(shard_meta_binding(&meta));

    let meta_path = shard_dir.join(SHARD_META_FILE);
    checkpoint(ShardPublishPhase::MetadataWrite)?;
    let meta_json = serde_json::to_vec_pretty(&meta).context("serialize shard meta")?;
    codestory_workspace::atomic_file::write_file_atomic(
        &meta_path,
        "shard-meta",
        |file| file.write_all(&meta_json).context("write shard metadata"),
        |temp_path| {
            let candidate = read_shard_meta(temp_path)?;
            if candidate != meta {
                bail!("temporary shard metadata differs from the expected metadata");
            }
            validate_lexical_index_file(&index_path, &candidate)?;
            checkpoint(ShardPublishPhase::MetadataPublish)
        },
    )
    .context("publish shard metadata")?;

    let stub = shard_dir.join(STUB_MARKER);
    if stub.is_file() {
        std::fs::remove_file(&stub).context("remove zoekt stub marker")?;
    }

    Ok(true)
}

pub fn shard_has_lexical_index(shard_dir: &Path, expected_sidecar_input_hash: &str) -> bool {
    let Ok(Some((index_path, meta))) =
        validated_shard_files(shard_dir, expected_sidecar_input_hash)
    else {
        return false;
    };
    validate_lexical_index_bytes_cached(shard_dir, &index_path, &meta, expected_sidecar_input_hash)
        .is_ok()
}

pub fn shard_matches_lexical_input(
    zoekt_data_dir: &Path,
    sidecar_generation: &str,
    expected_file_count: u32,
    expected_hash: &str,
    expected_sidecar_input_hash: &str,
) -> bool {
    let shard_dir = shard_dir_for(zoekt_data_dir, sidecar_generation);
    let Ok(Some((meta, _entries))) = load_validated_shard(&shard_dir, expected_sidecar_input_hash)
    else {
        return false;
    };
    meta.version == ZOEKT_REAL_VERSION_PIN
        && meta.project_id == sidecar_generation
        && meta.file_count == expected_file_count
        && meta.lexical_hash.as_deref() == Some(expected_hash)
}

fn write_lexical_entries(file: &mut std::fs::File, entries: &[LexicalIndexEntry]) -> Result<()> {
    let mut writer = BufWriter::new(file);
    for entry in entries {
        serde_json::to_writer(&mut writer, entry).context("serialize lexical index entry")?;
        writer
            .write_all(b"\n")
            .context("write lexical index newline")?;
    }
    writer.flush().context("flush lexical index")
}

fn load_validated_shard(
    shard_dir: &Path,
    expected_sidecar_input_hash: &str,
) -> Result<Option<(ShardMeta, Vec<LexicalIndexEntry>)>> {
    let Some((index_path, meta)) = validated_shard_files(shard_dir, expected_sidecar_input_hash)?
    else {
        return Ok(None);
    };
    let entries = validate_lexical_index_file(&index_path, &meta)?;
    Ok(Some((meta, entries)))
}

fn validated_shard_files(
    shard_dir: &Path,
    expected_sidecar_input_hash: &str,
) -> Result<Option<(PathBuf, ShardMeta)>> {
    if shard_dir.join(STUB_MARKER).is_file() {
        return Ok(None);
    }
    let index_path = shard_dir.join(LEXICAL_INDEX_FILE);
    let meta_path = shard_dir.join(SHARD_META_FILE);
    match (index_path.is_file(), meta_path.is_file()) {
        (false, false) => return Ok(None),
        (true, true) => {}
        _ => bail!("lexical shard is incomplete: data and metadata must both exist"),
    }
    let meta = read_shard_meta(&meta_path)?;
    if meta.version != ZOEKT_REAL_VERSION_PIN {
        bail!("lexical shard version is not current");
    }
    let expected_project_id = shard_dir.file_name().and_then(|name| name.to_str());
    if expected_project_id != Some(meta.project_id.as_str()) {
        bail!("lexical shard metadata project id does not match its directory");
    }
    if meta.sidecar_input_hash.as_deref() != Some(expected_sidecar_input_hash) {
        bail!("lexical shard metadata does not match the sidecar input hash");
    }
    if meta.data_sha256.is_none() || meta.data_bytes.is_none() {
        bail!("lexical shard metadata is missing the published data fingerprint");
    }
    if meta.binding_sha256.as_deref() != Some(shard_meta_binding(&meta).as_str()) {
        bail!("lexical shard metadata binding is invalid");
    }
    Ok(Some((index_path, meta)))
}

fn read_shard_meta(path: &Path) -> Result<ShardMeta> {
    let body =
        std::fs::read(path).with_context(|| format!("read shard metadata {}", path.display()))?;
    serde_json::from_slice(&body)
        .with_context(|| format!("parse shard metadata {}", path.display()))
}

fn shard_meta_binding(meta: &ShardMeta) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"codestory-lexical-shard-meta-v1");
    hasher.update(meta.version.as_bytes());
    hasher.update([0]);
    hasher.update(meta.project_id.as_bytes());
    hasher.update([0]);
    hasher.update(meta.file_count.to_le_bytes());
    hasher.update(meta.lexical_hash.as_deref().unwrap_or_default().as_bytes());
    hasher.update([0]);
    hasher.update(
        meta.sidecar_input_hash
            .as_deref()
            .unwrap_or_default()
            .as_bytes(),
    );
    hasher.update([0]);
    hasher.update(meta.data_sha256.as_deref().unwrap_or_default().as_bytes());
    hasher.update([0]);
    hasher.update(meta.data_bytes.unwrap_or_default().to_le_bytes());
    hasher.update(meta.indexed_at_epoch_ms.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

fn validate_lexical_index_file(path: &Path, meta: &ShardMeta) -> Result<Vec<LexicalIndexEntry>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("open lexical index {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut raw_hasher = sha2::Sha256::default();
    let mut data_bytes = 0_u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .with_context(|| format!("read lexical index {}", path.display()))?;
        if read == 0 {
            break;
        }
        use sha2::Digest;
        raw_hasher.update(&line);
        data_bytes = data_bytes.saturating_add(read as u64);
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        let line_number = entries.len() + 1;
        if line.is_empty() {
            bail!(
                "lexical index {} contains blank line {}",
                path.display(),
                line_number
            );
        }
        let line = std::str::from_utf8(&line).with_context(|| {
            format!(
                "decode lexical index {} line {}",
                path.display(),
                line_number
            )
        })?;
        entries.push(serde_json::from_str(line).with_context(|| {
            format!(
                "parse lexical index {} line {}",
                path.display(),
                line_number
            )
        })?);
    }
    let actual_count = entries.len().min(u32::MAX as usize) as u32;
    if actual_count != meta.file_count {
        bail!(
            "lexical index row count mismatch: metadata={}, actual={actual_count}",
            meta.file_count
        );
    }
    let Some(expected_hash) = meta.lexical_hash.as_deref() else {
        bail!("lexical shard metadata is missing lexical_hash");
    };
    let actual_hash = lexical_entries_hash(&entries);
    if actual_hash != expected_hash {
        bail!("lexical index hash mismatch: metadata={expected_hash}, actual={actual_hash}");
    }
    if let Some(expected_bytes) = meta.data_bytes
        && data_bytes != expected_bytes
    {
        bail!("lexical index byte count mismatch: metadata={expected_bytes}, actual={data_bytes}");
    }
    if let Some(expected_sha256) = meta.data_sha256.as_deref() {
        use sha2::Digest;
        let actual_sha256 = format!("{:x}", raw_hasher.finalize());
        if actual_sha256 != expected_sha256 {
            bail!(
                "lexical index data hash mismatch: metadata={expected_sha256}, actual={actual_sha256}"
            );
        }
    }
    Ok(entries)
}

fn validate_lexical_index_bytes(path: &Path, meta: &ShardMeta) -> Result<()> {
    let (actual_bytes, actual_sha256) = lexical_index_file_fingerprint(path)?;
    if meta.data_bytes != Some(actual_bytes)
        || meta.data_sha256.as_deref() != Some(actual_sha256.as_str())
    {
        bail!("lexical index published data fingerprint does not match metadata");
    }
    Ok(())
}

fn validate_lexical_index_bytes_cached(
    shard_dir: &Path,
    index_path: &Path,
    meta: &ShardMeta,
    expected_sidecar_input_hash: &str,
) -> Result<()> {
    let key = shard_validation_cache_key(shard_dir, index_path, meta, expected_sidecar_input_hash)?;
    let cache = VALIDATED_SHARD_CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
    {
        let mut cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|entry| entry.validated_at.elapsed() <= VALIDATED_SHARD_CACHE_TTL);
        if cache.iter().any(|entry| entry.key == key) {
            return Ok(());
        }
    }

    validate_lexical_index_bytes(index_path, meta)?;
    let validated_key =
        shard_validation_cache_key(shard_dir, index_path, meta, expected_sidecar_input_hash)?;
    if validated_key != key {
        bail!("lexical shard changed while its published data was being validated");
    }

    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|entry| entry.key.index_path != key.index_path);
    cache.push_back(CachedShardValidation {
        key,
        validated_at: Instant::now(),
    });
    while cache.len() > VALIDATED_SHARD_CACHE_CAPACITY {
        cache.pop_front();
    }
    Ok(())
}

fn shard_validation_cache_key(
    shard_dir: &Path,
    index_path: &Path,
    meta: &ShardMeta,
    expected_sidecar_input_hash: &str,
) -> Result<ShardValidationCacheKey> {
    Ok(ShardValidationCacheKey {
        index_path: index_path.to_path_buf(),
        expected_sidecar_input_hash: expected_sidecar_input_hash.to_string(),
        binding_sha256: meta.binding_sha256.clone().unwrap_or_default(),
        data_identity: file_identity(index_path)?,
        meta_identity: file_identity(&shard_dir.join(SHARD_META_FILE))?,
    })
}

fn file_identity(path: &Path) -> Result<FileIdentity> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("open file for identity {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("read file metadata {}", path.display()))?;
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let (change_token, device, file_id) = platform_file_identity(&file, &metadata)?;
    Ok(FileIdentity {
        len: metadata.len(),
        modified_nanos,
        change_token,
        device,
        file_id,
    })
}

#[cfg(unix)]
fn platform_file_identity(
    _file: &std::fs::File,
    metadata: &std::fs::Metadata,
) -> Result<(i128, u64, u128)> {
    use std::os::unix::fs::MetadataExt;

    let change_token = i128::from(metadata.ctime())
        .saturating_mul(1_000_000_000)
        .saturating_add(i128::from(metadata.ctime_nsec()));
    Ok((change_token, metadata.dev(), u128::from(metadata.ino())))
}

#[cfg(windows)]
fn platform_file_identity(
    file: &std::fs::File,
    _metadata: &std::fs::Metadata,
) -> Result<(i128, u64, u128)> {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    #[derive(Default)]
    struct FileBasicInfo {
        creation_time: i64,
        last_access_time: i64,
        last_write_time: i64,
        change_time: i64,
        file_attributes: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct FileIdInfo {
        volume_serial_number: u64,
        file_id: [u8; 16],
    }

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut c_void,
            file_information_class: i32,
            file_information: *mut c_void,
            buffer_size: u32,
        ) -> i32;
    }

    const FILE_BASIC_INFO_CLASS: i32 = 0;
    const FILE_ID_INFO_CLASS: i32 = 18;

    fn query<T: Default>(file: &std::fs::File, class: i32) -> Result<T> {
        let mut info = T::default();
        let result = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle().cast(),
                class,
                std::ptr::from_mut(&mut info).cast(),
                size_of::<T>() as u32,
            )
        };
        if result == 0 {
            return Err(std::io::Error::last_os_error()).context("query Windows file identity");
        }
        Ok(info)
    }

    let basic: FileBasicInfo = query(file, FILE_BASIC_INFO_CLASS)?;
    let id: FileIdInfo = query(file, FILE_ID_INFO_CLASS)?;
    Ok((
        i128::from(basic.change_time),
        id.volume_serial_number,
        u128::from_le_bytes(id.file_id),
    ))
}

#[cfg(not(any(unix, windows)))]
fn platform_file_identity(
    _file: &std::fs::File,
    _metadata: &std::fs::Metadata,
) -> Result<(i128, u64, u128)> {
    Ok((0, 0, 0))
}

fn lexical_index_file_fingerprint(path: &Path) -> Result<(u64, String)> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("open lexical index {}", path.display()))?;
    let mut hasher = sha2::Sha256::default();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("read lexical index {}", path.display()))?;
        if read == 0 {
            break;
        }
        use sha2::Digest;
        hasher.update(&buffer[..read]);
        bytes = bytes.saturating_add(read as u64);
    }
    use sha2::Digest;
    Ok((bytes, format!("{:x}", hasher.finalize())))
}

pub fn lexical_input_fingerprint(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<LexicalInputFingerprint> {
    let mut hasher = new_lexical_entries_hasher();
    let mut file_count = 0_usize;
    hash_lexical_entries_inner(project_root, project_root, &mut hasher, &mut file_count)?;
    hash_symbol_doc_entries(project_root, storage_path, &mut hasher, &mut file_count)?;
    Ok(LexicalInputFingerprint {
        file_count: file_count.min(u32::MAX as usize) as u32,
        hash: finalize_lexical_entries_hash(hasher),
    })
}

fn lexical_entries_hash(entries: &[LexicalIndexEntry]) -> String {
    let mut hasher = new_lexical_entries_hasher();
    for entry in entries {
        update_lexical_entries_hash(&mut hasher, entry);
    }
    finalize_lexical_entries_hash(hasher)
}

fn new_lexical_entries_hasher() -> sha2::Sha256 {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"codestory-zoekt-lexical-v1");
    hasher.update(ZOEKT_REAL_VERSION_PIN.as_bytes());
    hasher
}

fn update_lexical_entries_hash(hasher: &mut sha2::Sha256, entry: &LexicalIndexEntry) {
    use sha2::Digest;
    hasher.update(entry.path.as_bytes());
    hasher.update([0]);
    hasher.update(entry.content.as_bytes());
    hasher.update([0]);
    hasher.update(entry.source.provenance_label().as_bytes());
    hasher.update([0]);
    if let Some(node_id) = entry.node_id.as_deref() {
        hasher.update(node_id.as_bytes());
    }
    hasher.update([0]);
    if let Some(symbol_name) = entry.symbol_name.as_deref() {
        hasher.update(symbol_name.as_bytes());
    }
    hasher.update([0]);
    if let Some(start_line) = entry.start_line {
        hasher.update(start_line.to_le_bytes());
    }
    hasher.update([0]);
}

fn finalize_lexical_entries_hash(hasher: sha2::Sha256) -> String {
    use sha2::Digest;
    format!("{:x}", hasher.finalize())
}

pub fn search_lexical_index(
    shard_dir: &Path,
    expected_sidecar_input_hash: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<LexicalHit>> {
    let Some((_meta, entries)) = load_validated_shard(shard_dir, expected_sidecar_input_hash)?
    else {
        return Ok(Vec::new());
    };
    let tokens = lexical_query_tokens(query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let command_tokens = command_query_tokens(query);
    let token_frequencies = token_document_frequencies(&entries, &tokens);
    let token_weights = token_frequencies
        .iter()
        .zip(tokens.iter())
        .map(|(frequency, token)| {
            let mut weight = lexical_token_weight(*frequency, entries.len());
            if command_tokens
                .iter()
                .any(|command_token| command_token == token)
            {
                weight *= 2.0;
            }
            weight
        })
        .collect::<Vec<_>>();
    let total_weight = token_weights.iter().sum::<f32>();
    let required_weight = required_lexical_match_weight(tokens.len(), total_weight);
    let mut hits = Vec::new();
    for entry in entries {
        let path_lower = entry.path.to_ascii_lowercase();
        let content_lower = entry.content.to_ascii_lowercase();
        let token_match = lexical_token_match(&tokens, &token_weights, &path_lower, &content_lower);
        if token_match.matched_weight >= required_weight
            && broad_query_path_gate(tokens.len(), &token_match)
        {
            let score = score_lexical_match(&entry.path, entry.source, &token_match);
            hits.push(LexicalHit {
                path: entry.path,
                source: entry.source,
                node_id: entry.node_id,
                symbol_name: entry.symbol_name,
                start_line: entry.start_line,
                score,
            });
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
    });
    hits.truncate(limit);
    Ok(hits)
}

fn lexical_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for token in query
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|token| token.len() >= 2)
        .map(str::to_ascii_lowercase)
        .filter(|token| !LEXICAL_STOP_WORDS.contains(&token.as_str()))
    {
        if !tokens.iter().any(|existing| existing == &token) {
            tokens.push(token);
        }
    }
    tokens
}

fn command_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut in_backticks = false;
    let mut current = String::new();
    for ch in query.chars() {
        if ch == '`' {
            if in_backticks {
                push_command_tokens(&current, &mut tokens);
                current.clear();
            }
            in_backticks = !in_backticks;
            continue;
        }
        if in_backticks {
            current.push(ch);
        }
    }
    tokens
}

fn push_command_tokens(command: &str, tokens: &mut Vec<String>) {
    for token in command
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .map(|token| token.trim_start_matches('-').to_ascii_lowercase())
        .filter(|token| token.len() >= 2)
        .filter(|token| token != "codex")
    {
        if !tokens.iter().any(|existing| existing == &token) {
            tokens.push(token);
        }
    }
}

const LEXICAL_STOP_WORDS: &[&str] = &[
    "about", "after", "and", "are", "cite", "does", "explain", "file", "files", "flow", "flows",
    "for", "from", "how", "into", "level", "path", "source", "sources", "support", "that", "the",
    "through", "top", "what", "where", "which", "with",
];

fn token_document_frequencies(entries: &[LexicalIndexEntry], tokens: &[String]) -> Vec<usize> {
    tokens
        .iter()
        .map(|token| {
            entries
                .iter()
                .filter(|entry| {
                    let path_lower = entry.path.to_ascii_lowercase();
                    let content_lower = entry.content.to_ascii_lowercase();
                    path_lower.contains(token.as_str()) || content_lower.contains(token.as_str())
                })
                .count()
        })
        .collect()
}

fn lexical_token_weight(document_frequency: usize, document_count: usize) -> f32 {
    let rarity = ((document_count as f32 + 1.0) / (document_frequency as f32 + 1.0)).ln();
    (1.0 + rarity).clamp(0.25, 5.0)
}

fn required_lexical_match_weight(token_count: usize, total_weight: f32) -> f32 {
    if token_count <= 3 {
        return total_weight;
    }
    (total_weight * 0.28).max(2.5)
}

#[derive(Debug, Clone, Copy)]
struct LexicalTokenMatch {
    matched_weight: f32,
    path_weight: f32,
    content_weight: f32,
    total_weight: f32,
    meaningful_path_weight: f32,
}

fn lexical_token_match(
    tokens: &[String],
    token_weights: &[f32],
    path_lower: &str,
    content_lower: &str,
) -> LexicalTokenMatch {
    let mut matched_weight = 0.0;
    let mut path_weight = 0.0;
    let mut content_weight = 0.0;
    let mut total_weight = 0.0;
    let mut meaningful_path_weight = 0.0;
    for (token, weight) in tokens.iter().zip(token_weights.iter().copied()) {
        total_weight += weight;
        let path_factor = path_match_factor(path_lower, token);
        let path_match = path_factor > 0.0;
        let content_match = content_lower.contains(token.as_str());
        if path_match || content_match {
            matched_weight += weight;
        }
        if path_match {
            path_weight += weight * path_factor;
            if path_factor >= 1.0 && weight >= 1.5 {
                meaningful_path_weight += weight;
            }
        }
        if content_match {
            content_weight += weight;
        }
    }
    LexicalTokenMatch {
        matched_weight,
        path_weight,
        content_weight,
        total_weight,
        meaningful_path_weight,
    }
}

fn broad_query_path_gate(token_count: usize, token_match: &LexicalTokenMatch) -> bool {
    token_count < 8 || token_match.meaningful_path_weight > 0.0
}

fn path_match_factor(path_lower: &str, token: &str) -> f32 {
    if path_lower.split('/').any(|segment| segment == token) {
        return 1.8;
    }
    if path_lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .flat_map(|part| part.split('_'))
        .any(|part| part == token)
    {
        return 1.0;
    }
    if path_lower.contains(token) {
        return 0.35;
    }
    0.0
}

#[derive(Debug, Clone)]
pub struct LexicalHit {
    pub path: String,
    pub source: LexicalDocumentSource,
    pub node_id: Option<String>,
    pub symbol_name: Option<String>,
    pub start_line: Option<u32>,
    pub score: f32,
}

fn score_lexical_match(
    path: &str,
    source: LexicalDocumentSource,
    token_match: &LexicalTokenMatch,
) -> f32 {
    let coverage = if token_match.total_weight <= 0.0 {
        0.0
    } else {
        token_match.matched_weight / token_match.total_weight
    };
    let mut score = 0.20_f32
        + coverage * 0.25
        + token_match.path_weight * 0.09
        + token_match.content_weight * 0.035;
    let path_lower = path.replace('\\', "/").to_ascii_lowercase();
    if path_lower.contains("/src/") || path_lower.starts_with("src/") {
        score += 0.04;
    }
    if source == LexicalDocumentSource::ComponentReport {
        score += 0.08;
    } else {
        score *= lexical_file_role_multiplier(FileRole::classify_path(Path::new(path)));
    }
    score.min(0.99)
}

fn lexical_file_role_multiplier(file_role: FileRole) -> f32 {
    match file_role {
        FileRole::Entrypoint => 1.08,
        FileRole::Source => 1.0,
        FileRole::Test => 0.68,
        FileRole::Docs => 0.72,
        FileRole::Benchmark => 0.64,
        FileRole::Generated => 0.55,
        FileRole::Vendor => 0.45,
    }
}

fn collect_lexical_entries(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<Vec<LexicalIndexEntry>> {
    let mut entries = Vec::new();
    collect_lexical_entries_inner(project_root, project_root, &mut entries)?;
    collect_symbol_doc_entries(project_root, storage_path, &mut entries)?;
    Ok(entries)
}

fn collect_lexical_entries_inner(
    project_root: &Path,
    dir: &Path,
    entries: &mut Vec<LexicalIndexEntry>,
) -> Result<()> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(_) => return Ok(()),
    };
    let mut dir_entries = read_dir.flatten().collect::<Vec<_>>();
    dir_entries.sort_by_key(|entry| entry.path());

    for entry in dir_entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if should_skip_dir(&name) {
                continue;
            }
            collect_lexical_entries_inner(project_root, &path, entries)?;
            continue;
        }
        if !should_index_file(&name) {
            continue;
        }
        let metadata = entry.metadata().ok();
        if metadata
            .as_ref()
            .and_then(|meta| meta.len().try_into().ok())
            .is_some_and(|len: usize| len > MAX_FILE_BYTES)
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(project_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        entries.push(LexicalIndexEntry {
            path: rel,
            content,
            source: LexicalDocumentSource::LexicalSource,
            node_id: None,
            symbol_name: None,
            start_line: None,
        });
    }
    Ok(())
}

fn hash_lexical_entries_inner(
    project_root: &Path,
    dir: &Path,
    hasher: &mut sha2::Sha256,
    file_count: &mut usize,
) -> Result<()> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(_) => return Ok(()),
    };
    let mut dir_entries = read_dir.flatten().collect::<Vec<_>>();
    dir_entries.sort_by_key(|entry| entry.path());

    for entry in dir_entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if should_skip_dir(&name) {
                continue;
            }
            hash_lexical_entries_inner(project_root, &path, hasher, file_count)?;
            continue;
        }
        if !should_index_file(&name) {
            continue;
        }
        let metadata = entry.metadata().ok();
        if metadata
            .as_ref()
            .and_then(|meta| meta.len().try_into().ok())
            .is_some_and(|len: usize| len > MAX_FILE_BYTES)
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(project_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        update_lexical_entries_hash(
            hasher,
            &LexicalIndexEntry {
                path: rel,
                content,
                source: LexicalDocumentSource::LexicalSource,
                node_id: None,
                symbol_name: None,
                start_line: None,
            },
        );
        *file_count = file_count.saturating_add(1);
    }
    Ok(())
}

fn collect_symbol_doc_entries(
    project_root: &Path,
    storage_path: Option<&Path>,
    entries: &mut Vec<LexicalIndexEntry>,
) -> Result<()> {
    let Some(storage_path) = storage_path.filter(|path| path.is_file()) else {
        return Ok(());
    };
    let storage = Store::open(storage_path).context("open storage for lexical symbol docs")?;
    let mut after = None;
    loop {
        let batch = storage
            .get_symbol_search_docs_batch_after(after, 4096)
            .context("load symbol search docs for lexical shard")?;
        if batch.is_empty() {
            break;
        }
        after = batch.last().map(|doc| doc.node_id);
        for doc in batch {
            entries.push(symbol_doc_lexical_entry(project_root, &doc));
        }
    }
    Ok(())
}

fn hash_symbol_doc_entries(
    project_root: &Path,
    storage_path: Option<&Path>,
    hasher: &mut sha2::Sha256,
    file_count: &mut usize,
) -> Result<()> {
    let Some(storage_path) = storage_path.filter(|path| path.is_file()) else {
        return Ok(());
    };
    let storage = Store::open(storage_path).context("open storage for lexical symbol docs")?;
    let mut after = None;
    loop {
        let batch = storage
            .get_symbol_search_docs_batch_after(after, 4096)
            .context("load symbol search docs for lexical shard")?;
        if batch.is_empty() {
            break;
        }
        after = batch.last().map(|doc| doc.node_id);
        for doc in batch {
            update_lexical_entries_hash(hasher, &symbol_doc_lexical_entry(project_root, &doc));
            *file_count = file_count.saturating_add(1);
        }
    }
    Ok(())
}

fn symbol_doc_lexical_entry(project_root: &Path, doc: &SymbolSearchDoc) -> LexicalIndexEntry {
    let source = if doc.display_name.starts_with("component_report:") {
        LexicalDocumentSource::ComponentReport
    } else {
        LexicalDocumentSource::SymbolDoc
    };
    let path = doc
        .file_path
        .as_deref()
        .and_then(|path| normalize_lexical_file_path(project_root, path))
        .unwrap_or_else(|| {
            format!(
                "codestory://{}",
                doc.display_name
                    .replace('\\', "/")
                    .replace([' ', '\t', '\r', '\n'], "_")
            )
        });
    LexicalIndexEntry {
        path,
        content: doc.doc_text.clone(),
        source,
        node_id: Some(doc.node_id.0.to_string()),
        symbol_name: Some(doc.display_name.clone()),
        start_line: doc.start_line,
    }
}

fn normalize_lexical_file_path(project_root: &Path, path: &str) -> Option<String> {
    let path = Path::new(path);
    if path.is_absolute() {
        path.strip_prefix(project_root)
            .ok()
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
    } else {
        Some(path.to_string_lossy().replace('\\', "/"))
    }
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "node_modules" | "dist" | "build" | ".codestory" | "__pycache__"
    )
}

fn should_index_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(lower.as_str(), "lib.rs" | "mod.rs" | "main.rs")
        || lower.ends_with(".rs")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".py")
        || lower.ends_with(".go")
        || lower.ends_with(".java")
        || lower.ends_with(".c")
        || lower.ends_with(".cpp")
        || lower.ends_with(".h")
        || lower.ends_with(".hpp")
        || lower.ends_with(".cs")
        || lower.ends_with(".md")
}

pub fn shard_dir_for(zoekt_data_dir: &Path, project_id: &str) -> PathBuf {
    zoekt_data_dir.join("shards").join(project_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use codestory_store::{FileInfo, FileRole};
    use tempfile::TempDir;

    fn entry(path: &str, content: &str) -> LexicalIndexEntry {
        LexicalIndexEntry {
            path: path.into(),
            content: content.into(),
            source: LexicalDocumentSource::LexicalSource,
            node_id: None,
            symbol_name: None,
            start_line: None,
        }
    }

    fn write_test_shard(shard: &Path, entries: &[LexicalIndexEntry]) {
        std::fs::create_dir_all(shard).expect("create shard");
        let mut file = std::fs::File::create(shard.join(LEXICAL_INDEX_FILE)).expect("index file");
        write_lexical_entries(&mut file, entries).expect("write index");
        file.sync_all().expect("sync index");
        drop(file);
        let (data_bytes, data_sha256) =
            lexical_index_file_fingerprint(&shard.join(LEXICAL_INDEX_FILE))
                .expect("fingerprint index");
        let meta = ShardMeta {
            version: ZOEKT_REAL_VERSION_PIN.to_string(),
            project_id: shard
                .file_name()
                .and_then(|name| name.to_str())
                .expect("shard name")
                .to_string(),
            file_count: entries.len() as u32,
            lexical_hash: Some(lexical_entries_hash(entries)),
            sidecar_input_hash: Some("test-input".to_string()),
            data_sha256: Some(data_sha256),
            data_bytes: Some(data_bytes),
            binding_sha256: None,
            indexed_at_epoch_ms: 1,
        };
        let mut meta = meta;
        meta.binding_sha256 = Some(shard_meta_binding(&meta));
        std::fs::write(
            shard.join(SHARD_META_FILE),
            serde_json::to_vec_pretty(&meta).expect("serialize meta"),
        )
        .expect("write meta");
    }

    #[test]
    fn lexical_index_finds_repo_relative_paths() {
        let project = TempDir::new().expect("project");
        std::fs::write(
            project.path().join("lib.rs"),
            "pub fn extension_service() {}",
        )
        .expect("write");
        let zoekt_root = TempDir::new().expect("zoekt");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        build_zoekt_shard(
            project.path(),
            None,
            zoekt_root.path(),
            "abc123",
            &fingerprint,
            "test-input",
        )
        .expect("build");
        let shard = shard_dir_for(zoekt_root.path(), "abc123");
        assert!(shard_has_lexical_index(&shard, "test-input"));
        let hits = search_lexical_index(&shard, "test-input", "extension", 8).expect("search");
        assert!(hits.iter().any(|hit| hit.path == "lib.rs"));
    }

    #[test]
    fn lexical_index_includes_symbol_search_docs_with_node_provenance() {
        let project = TempDir::new().expect("project");
        let src = project.path().join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        let source_path = src.join("lib.rs");
        std::fs::write(&source_path, "fn private_helper() {}\n").expect("write source");

        let storage_path = project.path().join("codestory.db");
        let mut storage = Store::open(&storage_path).expect("open store");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: source_path.clone(),
                language: "rust".into(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: NodeId(1),
                    kind: NodeKind::FILE,
                    serialized_name: "src/lib.rs".into(),
                    qualified_name: None,
                    canonical_id: None,
                    file_node_id: None,
                    start_line: Some(1),
                    start_col: Some(0),
                    end_line: Some(1),
                    end_col: Some(0),
                },
                Node {
                    id: NodeId(2),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "private_helper".into(),
                    qualified_name: Some("private_helper".into()),
                    canonical_id: None,
                    file_node_id: Some(NodeId(1)),
                    start_line: Some(1),
                    start_col: Some(0),
                    end_line: Some(1),
                    end_col: Some(22),
                },
            ])
            .expect("insert nodes");
        storage
            .upsert_symbol_search_docs_batch(&[SymbolSearchDoc {
                node_id: NodeId(2),
                file_node_id: Some(NodeId(1)),
                kind: NodeKind::FUNCTION,
                display_name: "private_helper".into(),
                qualified_name: Some("private_helper".into()),
                file_path: Some(source_path.to_string_lossy().to_string()),
                start_line: Some(1),
                doc_text: "symbol private_helper deterministic cache skip logic".into(),
                doc_version: 4,
                doc_hash: "doc-hash".into(),
                policy_version: "graph_first_v1".into(),
                source_provenance: "extracted".into(),
                updated_at_epoch_ms: 1,
            }])
            .expect("upsert symbol doc");
        drop(storage);

        let collected_entries =
            collect_lexical_entries(project.path(), Some(&storage_path)).expect("collect entries");
        let streaming_fingerprint =
            lexical_input_fingerprint(project.path(), Some(&storage_path)).expect("fingerprint");
        assert_eq!(
            streaming_fingerprint.file_count as usize,
            collected_entries.len()
        );
        assert_eq!(
            streaming_fingerprint.hash,
            lexical_entries_hash(&collected_entries),
            "streaming lexical fingerprint must match collected-entry hash"
        );

        let zoekt_root = TempDir::new().expect("zoekt");
        build_zoekt_shard(
            project.path(),
            Some(&storage_path),
            zoekt_root.path(),
            "symbols",
            &streaming_fingerprint,
            "test-input",
        )
        .expect("build");
        let shard = shard_dir_for(zoekt_root.path(), "symbols");
        let hits =
            search_lexical_index(&shard, "test-input", "cache skip logic", 4).expect("search");
        let hit = hits
            .iter()
            .find(|hit| hit.symbol_name.as_deref() == Some("private_helper"))
            .expect("symbol doc hit");
        assert_eq!(hit.source, LexicalDocumentSource::SymbolDoc);
        assert_eq!(hit.node_id.as_deref(), Some("2"));
        assert_eq!(hit.start_line, Some(1));
    }

    #[test]
    fn shard_match_requires_current_lexical_hash_metadata() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "pub fn alpha() {}").expect("write");
        let zoekt_root = TempDir::new().expect("zoekt");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        build_zoekt_shard(
            project.path(),
            None,
            zoekt_root.path(),
            "generation",
            &fingerprint,
            "test-input",
        )
        .expect("build");

        assert!(shard_matches_lexical_input(
            zoekt_root.path(),
            "generation",
            fingerprint.file_count,
            &fingerprint.hash,
            "test-input"
        ));
        assert!(!shard_matches_lexical_input(
            zoekt_root.path(),
            "generation",
            fingerprint.file_count,
            "not-the-current-hash",
            "test-input"
        ));
        assert!(!shard_matches_lexical_input(
            zoekt_root.path(),
            "generation",
            fingerprint.file_count,
            &fingerprint.hash,
            "wrong-input"
        ));
        assert!(
            search_lexical_index(
                &shard_dir_for(zoekt_root.path(), "generation"),
                "wrong-input",
                "alpha",
                4
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_or_truncated_rows_fail_closed() {
        let shard_root = TempDir::new().expect("shard root");
        let shard = shard_root.path().join("generation");
        let entries = [entry("src/a.rs", "alpha"), entry("src/b.rs", "beta")];
        write_test_shard(&shard, &entries);
        let first = serde_json::to_string(&entries[0]).expect("serialize first");
        let second = serde_json::to_string(&entries[1]).expect("serialize second");

        std::fs::write(
            shard.join(LEXICAL_INDEX_FILE),
            format!("{first}\n{{not-json}}\n{second}\n"),
        )
        .expect("write malformed index");
        assert!(!shard_has_lexical_index(&shard, "test-input"));
        assert!(search_lexical_index(&shard, "test-input", "alpha", 4).is_err());

        std::fs::write(
            shard.join(LEXICAL_INDEX_FILE),
            format!("{first}\n{}", &second[..second.len() - 1]),
        )
        .expect("write truncated index");
        assert!(!shard_has_lexical_index(&shard, "test-input"));
        assert!(search_lexical_index(&shard, "test-input", "alpha", 4).is_err());
    }

    #[test]
    fn readiness_cache_invalidates_when_published_data_changes() {
        let shard_root = TempDir::new().expect("shard root");
        let shard = shard_root.path().join("generation");
        write_test_shard(&shard, &[entry("src/a.rs", "alpha")]);
        assert!(shard_has_lexical_index(&shard, "test-input"));

        let index_path = shard.join(LEXICAL_INDEX_FILE);
        let original_modified = std::fs::metadata(&index_path)
            .expect("index metadata")
            .modified()
            .expect("index modified time");
        let mut bytes = std::fs::read(&index_path).expect("read index");
        bytes[0] = b'[';
        std::fs::write(&index_path, bytes).expect("replace index");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&index_path)
            .expect("open index to restore timestamp")
            .set_times(std::fs::FileTimes::new().set_modified(original_modified))
            .expect("restore index timestamp");

        assert!(!shard_has_lexical_index(&shard, "test-input"));
    }

    #[test]
    fn readiness_cache_revalidates_expired_matching_identity() {
        let shard_root = TempDir::new().expect("shard root");
        let shard = shard_root.path().join("generation");
        write_test_shard(&shard, &[entry("src/a.rs", "alpha")]);

        let index_path = shard.join(LEXICAL_INDEX_FILE);
        let mut bytes = std::fs::read(&index_path).expect("read index");
        bytes[0] = b'[';
        std::fs::write(&index_path, bytes).expect("replace index");
        let meta = read_shard_meta(&shard.join(SHARD_META_FILE)).expect("read shard metadata");
        let key = shard_validation_cache_key(&shard, &index_path, &meta, "test-input")
            .expect("build cache key");
        let cache = VALIDATED_SHARD_CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
        let mut cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|entry| entry.key.index_path != index_path);
        cache.push_back(CachedShardValidation {
            key,
            validated_at: Instant::now()
                .checked_sub(VALIDATED_SHARD_CACHE_TTL + Duration::from_millis(1))
                .expect("expired timestamp"),
        });
        drop(cache);

        assert!(!shard_has_lexical_index(&shard, "test-input"));
    }

    #[test]
    fn row_count_and_hash_mismatches_fail_closed() {
        let shard_root = TempDir::new().expect("shard root");
        let shard = shard_root.path().join("generation");
        let entries = [entry("src/a.rs", "alpha")];
        write_test_shard(&shard, &entries);
        let meta_path = shard.join(SHARD_META_FILE);
        let mut meta = read_shard_meta(&meta_path).expect("meta");

        meta.file_count += 1;
        std::fs::write(
            &meta_path,
            serde_json::to_vec_pretty(&meta).expect("serialize count mismatch"),
        )
        .expect("write count mismatch");
        assert!(!shard_has_lexical_index(&shard, "test-input"));

        meta.file_count = entries.len() as u32;
        meta.lexical_hash = Some("wrong-hash".to_string());
        std::fs::write(
            &meta_path,
            serde_json::to_vec_pretty(&meta).expect("serialize hash mismatch"),
        )
        .expect("write hash mismatch");
        assert!(!shard_has_lexical_index(&shard, "test-input"));
    }

    #[test]
    fn changed_input_does_not_replace_last_known_good_shard() {
        let project = TempDir::new().expect("project");
        let source = project.path().join("lib.rs");
        std::fs::write(&source, "pub fn alpha() {}").expect("write alpha");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        let zoekt_root = TempDir::new().expect("zoekt");
        build_zoekt_shard(
            project.path(),
            None,
            zoekt_root.path(),
            "generation",
            &fingerprint,
            "test-input",
        )
        .expect("initial build");

        std::fs::write(&source, "pub fn beta() {}").expect("write beta");
        let error = build_zoekt_shard(
            project.path(),
            None,
            zoekt_root.path(),
            "generation",
            &fingerprint,
            "test-input",
        )
        .expect_err("changed input must not publish");
        assert!(error.to_string().contains("lexical input changed"));

        let shard = shard_dir_for(zoekt_root.path(), "generation");
        assert!(shard_has_lexical_index(&shard, "test-input"));
        assert_eq!(
            search_lexical_index(&shard, "test-input", "alpha", 4)
                .expect("search last-known-good")
                .len(),
            1
        );
        assert!(
            search_lexical_index(&shard, "test-input", "beta", 4)
                .expect("search old shard")
                .is_empty()
        );
    }

    #[test]
    fn publish_phase_failures_preserve_last_known_good_shard() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "pub fn alpha() {}").expect("write source");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        let zoekt_root = TempDir::new().expect("zoekt");
        build_zoekt_shard(
            project.path(),
            None,
            zoekt_root.path(),
            "generation",
            &fingerprint,
            "test-input",
        )
        .expect("initial build");

        for failure_phase in [
            ShardPublishPhase::DataWrite,
            ShardPublishPhase::DataPublish,
            ShardPublishPhase::MetadataWrite,
            ShardPublishPhase::MetadataPublish,
        ] {
            let error = build_zoekt_shard_with_checkpoint(
                project.path(),
                None,
                zoekt_root.path(),
                "generation",
                &fingerprint,
                "test-input",
                |phase| {
                    if phase == failure_phase {
                        bail!("injected {phase:?} failure");
                    }
                    Ok(())
                },
            )
            .expect_err("injected phase must fail");
            assert!(format!("{error:#}").contains("injected"));

            let shard = shard_dir_for(zoekt_root.path(), "generation");
            assert!(
                shard_has_lexical_index(&shard, "test-input"),
                "{failure_phase:?} must preserve readiness"
            );
            assert_eq!(
                search_lexical_index(&shard, "test-input", "alpha", 4)
                    .expect("search last-known-good")
                    .len(),
                1,
                "{failure_phase:?} must preserve search"
            );
        }
    }

    #[test]
    fn lexical_search_scores_all_matches_before_truncating() {
        let zoekt_root = TempDir::new().expect("zoekt");
        let shard = zoekt_root.path();
        let weak = entry("src/a_weak.rs", "handler mentioned once");
        let strong = entry("src/z_strong_handler.rs", "handler handler handler");
        write_test_shard(shard, &[weak, strong]);

        let hits = search_lexical_index(shard, "test-input", "handler", 1).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "src/z_strong_handler.rs");
    }

    #[test]
    fn lexical_index_does_not_stop_at_legacy_smoke_cap() {
        let project = TempDir::new().expect("project");
        let src = project.path().join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        for index in 0..4_100 {
            std::fs::write(
                src.join(format!("file_{index:04}.ts")),
                format!("export const symbol_{index:04} = {index};\n"),
            )
            .expect("write source file");
        }

        let zoekt_root = TempDir::new().expect("zoekt");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        build_zoekt_shard(
            project.path(),
            None,
            zoekt_root.path(),
            "large",
            &fingerprint,
            "test-input",
        )
        .expect("build");
        let shard = shard_dir_for(zoekt_root.path(), "large");
        let hits = search_lexical_index(&shard, "test-input", "symbol_4099", 4).expect("search");

        assert!(
            hits.iter().any(|hit| hit.path == "src/file_4099.ts"),
            "large-repo lexical shard should include files after the old 4096-file cap"
        );
    }

    #[test]
    fn lexical_search_tie_breaks_by_path() {
        let zoekt_root = TempDir::new().expect("zoekt");
        let shard = zoekt_root.path();
        let later = entry("src/b.rs", "handler");
        let earlier = entry("src/a.rs", "handler");
        write_test_shard(shard, &[later, earlier]);

        let hits = search_lexical_index(shard, "test-input", "handler", 2).expect("search");
        assert_eq!(
            hits.iter().map(|hit| hit.path.as_str()).collect::<Vec<_>>(),
            vec!["src/a.rs", "src/b.rs",]
        );
    }

    #[test]
    fn lexical_search_uses_partial_matching_for_broad_prompts() {
        let zoekt_root = TempDir::new().expect("zoekt");
        let shard = zoekt_root.path();
        let source = entry(
            "workspace/app/src/event_processor_with_jsonl_output.rs",
            "jsonl event output request runtime turn start",
        );
        let test = entry(
            "workspace/app/tests/event_processor_with_json_output.rs",
            "json event output test approval fixture",
        );
        let unrelated = entry("workspace/core/src/session.rs", "session bookkeeping");
        let generic_agent_doc = entry(
            ".agents/skills/review/SKILL.md",
            "request json cli runtime thread turn start event output",
        );
        let generated_schema = entry(
            "workspace/app-protocol/schema/typescript/v2/CommandRequestParams.ts",
            "app server command request turn start request",
        );
        write_test_shard(
            shard,
            &[test, unrelated, generic_agent_doc, generated_schema, source],
        );

        let hits = search_lexical_index(
            shard,
            "test-input",
            "Explain how `app request --json` flows from CLI into runtime thread turn start JSONL event output",
            4,
        )
        .expect("search");

        assert!(!hits.is_empty());
        assert_eq!(
            hits.first().map(|hit| hit.path.as_str()),
            Some("workspace/app/src/event_processor_with_jsonl_output.rs")
        );
        assert!(
            hits.iter()
                .all(|hit| hit.path != "workspace/core/src/session.rs")
        );
        assert!(
            hits.iter()
                .all(|hit| hit.path != ".agents/skills/review/SKILL.md")
        );
    }
}
