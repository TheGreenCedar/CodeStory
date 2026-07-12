//! Project-local SQLite FTS lexical index.

use anyhow::{Context, Result, bail};
use codestory_store::{FileRole, Store, SymbolSearchDoc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const LEXICAL_INDEX_VERSION: &str = "sqlite-fts5-v1";
pub const LEXICAL_INDEX_FILE: &str = "lexical-index.sqlite3";
const LEGACY_INDEX_FILE: &str = "lexical-index.jsonl";
const LEGACY_META_FILE: &str = "shard-meta.json";
const LEGACY_STUB_MARKER: &str = ".zoekt-stub";
const MAX_FILE_BYTES: u64 = 1_000_000;
const MAX_CANDIDATES: usize = 4_096;
const COVERAGE_PATH_SAMPLE: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LexicalCoverage {
    pub discovered_files: u32,
    pub indexed_files: u32,
    pub omitted_oversized: u32,
    pub unreadable_files: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted_path_sample: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable_path_sample: Vec<String>,
}

impl LexicalCoverage {
    pub fn complete(&self) -> bool {
        self.omitted_oversized == 0 && self.unreadable_files == 0
    }

    pub fn detail(&self) -> String {
        let mut detail = format!(
            "sqlite fts5; discovered={} indexed={} omitted_oversized={} unreadable={}",
            self.discovered_files,
            self.indexed_files,
            self.omitted_oversized,
            self.unreadable_files
        );
        if !self.omitted_path_sample.is_empty() {
            detail.push_str(&format!(
                "; omitted_path_sample={}",
                self.omitted_path_sample.join(",")
            ));
        }
        if !self.unreadable_path_sample.is_empty() {
            detail.push_str(&format!(
                "; unreadable_path_sample={}",
                self.unreadable_path_sample.join(",")
            ));
        }
        detail
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalInputFingerprint {
    pub file_count: u32,
    pub hash: String,
    pub coverage: LexicalCoverage,
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

    fn parse(value: &str) -> Result<Self> {
        match value {
            "lexical_source" => Ok(Self::LexicalSource),
            "symbol_doc" => Ok(Self::SymbolDoc),
            "component_report" => Ok(Self::ComponentReport),
            _ => bail!("lexical shard contains unknown document source `{value}`"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LexicalDocument {
    path: String,
    content: String,
    source: LexicalDocumentSource,
    node_id: Option<String>,
    symbol_name: Option<String>,
    start_line: Option<u32>,
}

#[derive(Debug, Clone)]
struct LexicalShardMetadata {
    project_id: String,
    sidecar_input_hash: String,
    lexical_hash: String,
    file_count: u32,
    coverage: LexicalCoverage,
    binding_sha256: String,
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

pub fn lexical_input_fingerprint(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<LexicalInputFingerprint> {
    let mut hasher = lexical_documents_hasher();
    let mut file_count = 0_u32;
    let coverage = scan_lexical_documents(project_root, storage_path, &mut |document| {
        hash_lexical_document(&mut hasher, document);
        file_count = file_count.saturating_add(1);
        Ok(())
    })?;
    Ok(LexicalInputFingerprint {
        file_count,
        hash: finish_lexical_documents_hash(hasher, &coverage),
        coverage,
    })
}

pub fn build_lexical_shard(
    project_root: &Path,
    storage_path: Option<&Path>,
    lexical_data_dir: &Path,
    project_id: &str,
    expected: &LexicalInputFingerprint,
    sidecar_input_hash: &str,
) -> Result<LexicalInputFingerprint> {
    let shard_dir = shard_dir_for(lexical_data_dir, project_id);
    std::fs::create_dir_all(&shard_dir)
        .with_context(|| format!("create lexical shard directory {}", shard_dir.display()))?;
    let index_path = shard_dir.join(LEXICAL_INDEX_FILE);
    let (temp_path, reserved) =
        codestory_workspace::atomic_file::create_unique_temp_file(&index_path, "lexical-index")?;
    drop(reserved);
    let result: Result<LexicalInputFingerprint> = (|| {
        let rebuilt = write_lexical_database(
            &temp_path,
            project_id,
            sidecar_input_hash,
            expected,
            |visit| scan_lexical_documents(project_root, storage_path, visit),
        )?;
        validate_lexical_database(
            &temp_path,
            project_id,
            sidecar_input_hash,
            Some((expected.file_count, expected.hash.as_str())),
            true,
        )?;
        publish_immutable_lexical_database(&temp_path, &index_path)?;
        Ok(rebuilt)
    })();
    if result.is_err() {
        if let Ok(metadata) = std::fs::metadata(&temp_path) {
            let _ = make_file_owner_writable(&temp_path, &metadata.permissions());
        }
        let _ = std::fs::remove_file(&temp_path);
    }
    let rebuilt = result?;

    // Old JSONL generations are migration inputs only: the new reader never opens them.
    for legacy in [LEGACY_INDEX_FILE, LEGACY_META_FILE, LEGACY_STUB_MARKER] {
        let path = shard_dir.join(legacy);
        if path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(rebuilt)
}

fn publish_immutable_lexical_database(temp_path: &Path, index_path: &Path) -> Result<()> {
    let previous_permissions = match std::fs::metadata(index_path) {
        Ok(metadata) => {
            let permissions = metadata.permissions();
            if permissions.readonly() {
                make_file_owner_writable(index_path, &permissions)?;
            }
            Some(permissions)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let result =
        codestory_workspace::atomic_file::publish_existing_file_atomic(temp_path, index_path);
    match result {
        Ok(()) => {
            let mut permissions = std::fs::metadata(index_path)?.permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(index_path, permissions).with_context(|| {
                format!("protect immutable lexical shard {}", index_path.display())
            })
        }
        Err(error) => {
            if let Some(permissions) = previous_permissions {
                let _ = std::fs::set_permissions(index_path, permissions);
            }
            Err(error)
        }
    }
}

#[allow(clippy::permissions_set_readonly_false)]
fn make_file_owner_writable(path: &Path, permissions: &std::fs::Permissions) -> Result<()> {
    let mut writable = permissions.clone();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        writable.set_mode(writable.mode() | 0o200);
    }
    #[cfg(windows)]
    writable.set_readonly(false);
    std::fs::set_permissions(path, writable).with_context(|| {
        format!(
            "prepare immutable lexical shard replacement {}",
            path.display()
        )
    })
}

pub fn shard_has_lexical_index(shard_dir: &Path, expected_sidecar_input_hash: &str) -> bool {
    let Some(project_id) = shard_dir.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    validate_lexical_database(
        &shard_dir.join(LEXICAL_INDEX_FILE),
        project_id,
        expected_sidecar_input_hash,
        None,
        false,
    )
    .is_ok()
}

pub fn shard_matches_lexical_input(
    lexical_data_dir: &Path,
    sidecar_generation: &str,
    expected_file_count: u32,
    expected_hash: &str,
    expected_sidecar_input_hash: &str,
) -> bool {
    validate_lexical_database(
        &shard_dir_for(lexical_data_dir, sidecar_generation).join(LEXICAL_INDEX_FILE),
        sidecar_generation,
        expected_sidecar_input_hash,
        Some((expected_file_count, expected_hash)),
        true,
    )
    .is_ok()
}

pub fn lexical_shard_coverage(
    lexical_data_dir: &Path,
    sidecar_generation: &str,
    expected_sidecar_input_hash: &str,
) -> Result<LexicalCoverage> {
    Ok(validate_lexical_database(
        &shard_dir_for(lexical_data_dir, sidecar_generation).join(LEXICAL_INDEX_FILE),
        sidecar_generation,
        expected_sidecar_input_hash,
        None,
        false,
    )?
    .coverage)
}

#[cfg(test)]
pub fn search_lexical_index(
    shard_dir: &Path,
    expected_sidecar_input_hash: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<LexicalHit>> {
    search_lexical_index_with_cancel(shard_dir, expected_sidecar_input_hash, query, limit, || {
        false
    })
}

pub fn search_lexical_index_with_cancel<F>(
    shard_dir: &Path,
    expected_sidecar_input_hash: &str,
    query: &str,
    limit: usize,
    cancelled: F,
) -> Result<Vec<LexicalHit>>
where
    F: Fn() -> bool + Send + Sync + 'static,
{
    let cancelled: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(cancelled);
    let result = search_lexical_index_with_cancel_inner(
        shard_dir,
        expected_sidecar_input_hash,
        query,
        limit,
        Arc::clone(&cancelled),
    );
    if result.is_err() && cancelled() {
        bail!("lexical search cancelled");
    }
    result
}

fn search_lexical_index_with_cancel_inner(
    shard_dir: &Path,
    expected_sidecar_input_hash: &str,
    query: &str,
    limit: usize,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Vec<LexicalHit>> {
    if cancelled() {
        bail!("lexical search cancelled");
    }
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(project_id) = shard_dir.file_name().and_then(|name| name.to_str()) else {
        bail!("lexical shard path has no generation directory");
    };
    let index_path = shard_dir.join(LEXICAL_INDEX_FILE);
    let connection = open_read_only(&index_path)?;
    let progress_cancelled = Arc::clone(&cancelled);
    connection.progress_handler(1_000, Some(move || progress_cancelled()))?;
    let _metadata = validate_open_database(
        &connection,
        project_id,
        expected_sidecar_input_hash,
        None,
        false,
        cancelled.as_ref(),
    )?;
    let tokens = lexical_query_tokens(query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let candidate_terms = candidate_query_terms(&tokens);
    if candidate_terms.is_empty() {
        return Ok(Vec::new());
    }
    let fts_query = candidate_terms
        .iter()
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let candidate_limit = limit.saturating_mul(64).clamp(256, MAX_CANDIDATES);

    let command_tokens = command_query_tokens(query);
    let document_count: usize = connection.query_row(
        "SELECT file_count FROM lexical_metadata WHERE id = 1",
        [],
        |row| row.get::<_, u32>(0).map(|count| count as usize),
    )?;
    let mut token_frequencies = Vec::with_capacity(tokens.len());
    for token in &tokens {
        if cancelled() {
            bail!("lexical search cancelled");
        }
        token_frequencies.push(fts_document_frequency(&connection, token)?);
    }
    let token_weights = token_frequencies
        .iter()
        .zip(tokens.iter())
        .map(|(frequency, token)| {
            let mut weight = lexical_token_weight(*frequency, document_count);
            if command_tokens.iter().any(|command| command == token) {
                weight *= 2.0;
            }
            weight
        })
        .collect::<Vec<_>>();
    let total_weight = token_weights.iter().sum::<f32>();
    let required_weight = required_lexical_match_weight(tokens.len(), total_weight);

    let mut statement = connection.prepare(
        "SELECT d.path, d.content, d.source, d.node_id, d.symbol_name, d.start_line
         FROM lexical_fts
         JOIN lexical_documents d ON d.id = lexical_fts.rowid
         WHERE lexical_fts MATCH ?1
         ORDER BY bm25(lexical_fts, 8.0, 1.0)
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![fts_query, candidate_limit as i64], |row| {
        Ok(LexicalDocument {
            path: row.get(0)?,
            content: row.get(1)?,
            source: LexicalDocumentSource::parse(&row.get::<_, String>(2)?).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    error.into(),
                )
            })?,
            node_id: row.get(3)?,
            symbol_name: row.get(4)?,
            start_line: row.get(5)?,
        })
    })?;

    let mut hits = Vec::new();
    for (index, row) in rows.enumerate() {
        if index % 64 == 0 && cancelled() {
            bail!("lexical search cancelled");
        }
        let document = row?;
        let normalized_path = normalize_lexical_text(&document.path);
        let normalized_content = normalize_lexical_text(&document.content);
        let token_match = lexical_token_match(
            &tokens,
            &token_weights,
            &normalized_path,
            &normalized_content,
        );
        if token_match.matched_weight >= required_weight
            && broad_query_path_gate(tokens.len(), &token_match)
        {
            hits.push(LexicalHit {
                score: score_lexical_match(&document.path, document.source, &token_match),
                path: document.path,
                source: document.source,
                node_id: document.node_id,
                symbol_name: document.symbol_name,
                start_line: document.start_line,
            });
        }
    }
    if cancelled() {
        bail!("lexical search cancelled");
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

pub fn shard_dir_for(lexical_data_dir: &Path, project_id: &str) -> PathBuf {
    lexical_data_dir.join("shards").join(project_id)
}

fn write_lexical_database<F>(
    path: &Path,
    project_id: &str,
    sidecar_input_hash: &str,
    expected: &LexicalInputFingerprint,
    scan: F,
) -> Result<LexicalInputFingerprint>
where
    F: FnOnce(&mut dyn FnMut(&LexicalDocument) -> Result<()>) -> Result<LexicalCoverage>,
{
    let mut connection = Connection::open(path)
        .with_context(|| format!("create lexical SQLite shard {}", path.display()))?;
    connection.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = FULL;
         PRAGMA temp_store = MEMORY;
         PRAGMA user_version = 1;
         CREATE TABLE lexical_metadata (
             id INTEGER PRIMARY KEY CHECK (id = 1),
             version TEXT NOT NULL,
             project_id TEXT NOT NULL,
             sidecar_input_hash TEXT NOT NULL,
             lexical_hash TEXT NOT NULL,
             file_count INTEGER NOT NULL,
             coverage_json TEXT NOT NULL,
             binding_sha256 TEXT NOT NULL,
             indexed_at_epoch_ms INTEGER NOT NULL
         );
         CREATE TABLE lexical_documents (
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL,
             content TEXT NOT NULL,
             source TEXT NOT NULL,
             node_id TEXT,
             symbol_name TEXT,
             start_line INTEGER
         );
         CREATE VIRTUAL TABLE lexical_fts USING fts5(path, content);",
    )?;
    let transaction = connection.transaction()?;
    let mut hasher = lexical_documents_hasher();
    let mut file_count = 0_u32;
    let actual = {
        let mut insert_document = transaction.prepare(
            "INSERT INTO lexical_documents
             (id, path, content, source, node_id, symbol_name, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let mut insert_fts = transaction
            .prepare("INSERT INTO lexical_fts(rowid, path, content) VALUES (?1, ?2, ?3)")?;
        let coverage = scan(&mut |document| {
            file_count = file_count
                .checked_add(1)
                .context("lexical document count overflow")?;
            let id = i64::from(file_count);
            hash_lexical_document(&mut hasher, document);
            insert_document.execute(params![
                id,
                document.path,
                document.content,
                document.source.provenance_label(),
                document.node_id,
                document.symbol_name,
                document.start_line,
            ])?;
            insert_fts.execute(params![
                id,
                normalize_lexical_text(&document.path),
                normalize_lexical_text(&document.content),
            ])?;
            Ok(())
        })?;
        drop(insert_fts);
        drop(insert_document);

        let actual = LexicalInputFingerprint {
            file_count,
            hash: finish_lexical_documents_hash(hasher, &coverage),
            coverage,
        };
        if &actual != expected {
            bail!(
                "lexical input changed while building shard: expected {} documents with hash {}, collected {} documents with hash {}",
                expected.file_count,
                expected.hash,
                actual.file_count,
                actual.hash
            );
        }
        let coverage_json = serde_json::to_string(&actual.coverage)?;
        let binding = metadata_binding(
            project_id,
            sidecar_input_hash,
            &actual.hash,
            actual.file_count,
            &coverage_json,
        );
        transaction.execute(
            "INSERT INTO lexical_metadata
             (id, version, project_id, sidecar_input_hash, lexical_hash, file_count,
              coverage_json, binding_sha256, indexed_at_epoch_ms)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                LEXICAL_INDEX_VERSION,
                project_id,
                sidecar_input_hash,
                actual.hash,
                actual.file_count,
                coverage_json,
                binding,
                chrono::Utc::now().timestamp_millis(),
            ],
        )?;
        actual
    };
    transaction.execute_batch(
        "CREATE TRIGGER lexical_documents_no_insert BEFORE INSERT ON lexical_documents
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;
         CREATE TRIGGER lexical_documents_no_update BEFORE UPDATE ON lexical_documents
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;
         CREATE TRIGGER lexical_documents_no_delete BEFORE DELETE ON lexical_documents
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;
         CREATE TRIGGER lexical_metadata_no_insert BEFORE INSERT ON lexical_metadata
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;
         CREATE TRIGGER lexical_metadata_no_update BEFORE UPDATE ON lexical_metadata
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;
         CREATE TRIGGER lexical_metadata_no_delete BEFORE DELETE ON lexical_metadata
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;",
    )?;
    transaction.commit()?;
    connection.execute_batch("PRAGMA optimize;")?;
    connection.close().map_err(|(_, error)| error)?;
    Ok(actual)
}

fn validate_lexical_database(
    path: &Path,
    expected_project_id: &str,
    expected_sidecar_input_hash: &str,
    expected_lexical: Option<(u32, &str)>,
    quick_check: bool,
) -> Result<LexicalShardMetadata> {
    if !path.is_file() {
        bail!("lexical SQLite shard is missing");
    }
    let connection = open_read_only(path)?;
    validate_open_database(
        &connection,
        expected_project_id,
        expected_sidecar_input_hash,
        expected_lexical,
        quick_check,
        &|| false,
    )
}

fn validate_open_database(
    connection: &Connection,
    expected_project_id: &str,
    expected_sidecar_input_hash: &str,
    expected_lexical: Option<(u32, &str)>,
    quick_check: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<LexicalShardMetadata> {
    if cancelled() {
        bail!("lexical search cancelled");
    }
    let schema_version: i32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if schema_version != 1 {
        bail!("lexical SQLite shard schema version is not current");
    }
    let required_tables: u32 = connection.query_row(
        "SELECT count(*) FROM sqlite_master
         WHERE type IN ('table', 'view')
           AND name IN ('lexical_metadata', 'lexical_documents', 'lexical_fts')",
        [],
        |row| row.get(0),
    )?;
    if required_tables != 3 {
        bail!("lexical SQLite shard schema is incomplete");
    }
    if quick_check {
        let check: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
        if check != "ok" {
            bail!("lexical SQLite shard failed quick_check: {check}");
        }
    }
    let metadata = connection
        .query_row(
            "SELECT version, project_id, sidecar_input_hash, lexical_hash, file_count,
                    coverage_json, binding_sha256
             FROM lexical_metadata WHERE id = 1",
            [],
            |row| {
                let coverage_json: String = row.get(5)?;
                let coverage = serde_json::from_str(&coverage_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        error.into(),
                    )
                })?;
                Ok((
                    row.get::<_, String>(0)?,
                    LexicalShardMetadata {
                        project_id: row.get(1)?,
                        sidecar_input_hash: row.get(2)?,
                        lexical_hash: row.get(3)?,
                        file_count: row.get(4)?,
                        coverage,
                        binding_sha256: row.get(6)?,
                    },
                    coverage_json,
                ))
            },
        )
        .optional()?
        .context("lexical SQLite shard metadata is missing")?;
    let (version, metadata, coverage_json) = metadata;
    if version != LEXICAL_INDEX_VERSION {
        bail!("lexical SQLite shard version is not current");
    }
    if metadata.project_id != expected_project_id {
        bail!("lexical SQLite shard project id does not match its generation directory");
    }
    if metadata.sidecar_input_hash != expected_sidecar_input_hash {
        bail!("lexical SQLite shard does not match the sidecar input hash");
    }
    if metadata.binding_sha256
        != metadata_binding(
            &metadata.project_id,
            &metadata.sidecar_input_hash,
            &metadata.lexical_hash,
            metadata.file_count,
            &coverage_json,
        )
    {
        bail!("lexical SQLite shard metadata binding is invalid");
    }
    if let Some((file_count, lexical_hash)) = expected_lexical
        && (metadata.file_count != file_count || metadata.lexical_hash != lexical_hash)
    {
        bail!("lexical SQLite shard does not match current lexical input");
    }
    let actual_count: u32 =
        connection.query_row("SELECT count(*) FROM lexical_documents", [], |row| {
            row.get(0)
        })?;
    if actual_count != metadata.file_count {
        bail!(
            "lexical SQLite shard row count mismatch: metadata={}, actual={actual_count}",
            metadata.file_count
        );
    }
    let fts_count: u32 =
        connection.query_row("SELECT count(*) FROM lexical_fts", [], |row| row.get(0))?;
    if fts_count != actual_count {
        bail!(
            "lexical SQLite shard FTS row count mismatch: documents={actual_count}, fts={fts_count}"
        );
    }
    let mut rows = connection.prepare(
        "SELECT d.path, d.content, f.path, f.content
         FROM lexical_documents d
         LEFT JOIN lexical_fts f ON f.rowid = d.id
         ORDER BY d.id",
    )?;
    let mut rows = rows.query([])?;
    let mut row_index = 0_usize;
    while let Some(row) = rows.next()? {
        if row_index.is_multiple_of(64) && cancelled() {
            bail!("lexical search cancelled");
        }
        row_index += 1;
        let path: String = row.get(0)?;
        let content: String = row.get(1)?;
        let fts_path: Option<String> = row.get(2)?;
        let fts_content: Option<String> = row.get(3)?;
        if fts_path != Some(normalize_lexical_text(&path))
            || fts_content != Some(normalize_lexical_text(&content))
        {
            bail!("lexical SQLite shard FTS rows do not match immutable documents");
        }
    }
    if cancelled() {
        bail!("lexical search cancelled");
    }
    Ok(metadata)
}

fn open_read_only(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open lexical SQLite shard {}", path.display()))?;
    connection.execute_batch("PRAGMA query_only = ON;")?;
    Ok(connection)
}

fn metadata_binding(
    project_id: &str,
    sidecar_input_hash: &str,
    lexical_hash: &str,
    file_count: u32,
    coverage_json: &str,
) -> String {
    let mut hasher = Sha256::new();
    for value in [
        LEXICAL_INDEX_VERSION,
        project_id,
        sidecar_input_hash,
        lexical_hash,
        coverage_json,
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    hasher.update(file_count.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

fn scan_lexical_documents(
    project_root: &Path,
    storage_path: Option<&Path>,
    visit: &mut dyn FnMut(&LexicalDocument) -> Result<()>,
) -> Result<LexicalCoverage> {
    let workspace = codestory_workspace::WorkspaceManifest::open(project_root.to_path_buf())
        .context("open workspace for lexical discovery")?;
    let discovered = workspace
        .source_files()
        .context("discover canonical workspace files for lexical index")?;
    let mut coverage = LexicalCoverage {
        discovered_files: discovered.len().min(u32::MAX as usize) as u32,
        ..Default::default()
    };
    for path in discovered {
        let relative = lexical_relative_path(project_root, &path);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                coverage.unreadable_files = coverage.unreadable_files.saturating_add(1);
                push_coverage_sample(&mut coverage.unreadable_path_sample, relative);
                continue;
            }
        };
        if metadata.len() > MAX_FILE_BYTES {
            coverage.omitted_oversized = coverage.omitted_oversized.saturating_add(1);
            push_coverage_sample(&mut coverage.omitted_path_sample, relative);
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => {
                coverage.unreadable_files = coverage.unreadable_files.saturating_add(1);
                push_coverage_sample(&mut coverage.unreadable_path_sample, relative);
                continue;
            }
        };
        visit(&LexicalDocument {
            path: relative,
            content,
            source: LexicalDocumentSource::LexicalSource,
            node_id: None,
            symbol_name: None,
            start_line: None,
        })?;
        coverage.indexed_files = coverage.indexed_files.saturating_add(1);
    }
    scan_symbol_documents(project_root, storage_path, visit)?;
    Ok(coverage)
}

fn scan_symbol_documents(
    project_root: &Path,
    storage_path: Option<&Path>,
    visit: &mut dyn FnMut(&LexicalDocument) -> Result<()>,
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
        for doc in &batch {
            visit(&symbol_document(project_root, doc))?;
        }
    }
    Ok(())
}

fn symbol_document(project_root: &Path, doc: &SymbolSearchDoc) -> LexicalDocument {
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
                doc.display_name.replace([' ', '\t', '\r', '\n'], "_")
            )
        });
    LexicalDocument {
        path,
        content: doc.doc_text.clone(),
        source,
        node_id: Some(doc.node_id.0.to_string()),
        symbol_name: Some(doc.display_name.clone()),
        start_line: doc.start_line,
    }
}

fn lexical_relative_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalize_lexical_file_path(project_root: &Path, path: &str) -> Option<String> {
    let path = Path::new(path);
    if path.is_absolute() {
        path.strip_prefix(project_root)
            .ok()
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
    } else {
        Some(path.to_string_lossy().replace('\\', "/"))
    }
}

fn push_coverage_sample(sample: &mut Vec<String>, path: String) {
    if sample.len() < COVERAGE_PATH_SAMPLE {
        sample.push(path);
    }
}

fn lexical_documents_hasher() -> Sha256 {
    let mut hasher = Sha256::new();
    hasher.update(b"codestory-sqlite-lexical-v1");
    hasher.update(LEXICAL_INDEX_VERSION.as_bytes());
    hasher
}

fn hash_lexical_document(hasher: &mut Sha256, document: &LexicalDocument) {
    for value in [
        document.path.as_str(),
        document.content.as_str(),
        document.source.provenance_label(),
        document.node_id.as_deref().unwrap_or_default(),
        document.symbol_name.as_deref().unwrap_or_default(),
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    hasher.update(document.start_line.unwrap_or_default().to_le_bytes());
}

fn finish_lexical_documents_hash(mut hasher: Sha256, coverage: &LexicalCoverage) -> String {
    hasher.update(serde_json::to_vec(coverage).unwrap_or_default());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
fn lexical_documents_hash(documents: &[LexicalDocument], coverage: &LexicalCoverage) -> String {
    let mut hasher = lexical_documents_hasher();
    for document in documents {
        hash_lexical_document(&mut hasher, document);
    }
    finish_lexical_documents_hash(hasher, coverage)
}

fn normalize_lexical_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len() + value.len() / 8);
    let characters = value.chars().collect::<Vec<_>>();
    for (index, character) in characters.iter().copied().enumerate() {
        let previous = index.checked_sub(1).and_then(|index| characters.get(index));
        let next = characters.get(index + 1);
        if character.is_uppercase()
            && previous.is_some_and(|value| value.is_lowercase() || value.is_numeric())
            || character.is_uppercase()
                && previous.is_some_and(|value| value.is_uppercase())
                && next.is_some_and(|value| value.is_lowercase())
        {
            normalized.push(' ');
        }
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized
}

fn candidate_query_terms(tokens: &[String]) -> Vec<String> {
    let mut terms = Vec::new();
    for token in tokens {
        for part in token.split('_').filter(|part| part.len() >= 2) {
            if !terms.iter().any(|existing| existing == part) {
                terms.push(part.to_string());
            }
        }
    }
    terms
}

fn fts_document_frequency(connection: &Connection, token: &str) -> Result<usize> {
    let terms = candidate_query_terms(&[token.to_string()]);
    if terms.is_empty() {
        return Ok(0);
    }
    let query = terms
        .iter()
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    connection
        .query_row(
            "SELECT count(*) FROM lexical_fts WHERE lexical_fts MATCH ?1",
            [query],
            |row| row.get::<_, u32>(0).map(|count| count as usize),
        )
        .map_err(Into::into)
}

fn lexical_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let normalized = normalize_lexical_text(query);
    for token in normalized
        .split(|character: char| !(character.is_alphanumeric() || character == '_'))
        .filter(|token| token.len() >= 2)
        .filter(|token| !LEXICAL_STOP_WORDS.contains(token))
    {
        if !tokens.iter().any(|existing| existing == token) {
            tokens.push(token.to_string());
        }
    }
    tokens
}

fn command_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut in_backticks = false;
    let mut current = String::new();
    for character in query.chars() {
        if character == '`' {
            if in_backticks {
                for token in lexical_query_tokens(&current)
                    .into_iter()
                    .filter(|token| token != "codex")
                {
                    if !tokens.iter().any(|existing| existing == &token) {
                        tokens.push(token);
                    }
                }
                current.clear();
            }
            in_backticks = !in_backticks;
        } else if in_backticks {
            current.push(character);
        }
    }
    tokens
}

const LEXICAL_STOP_WORDS: &[&str] = &[
    "about", "after", "and", "are", "cite", "does", "explain", "file", "files", "flow", "flows",
    "for", "from", "how", "into", "level", "path", "source", "sources", "support", "that", "the",
    "through", "top", "what", "where", "which", "with",
];

fn lexical_token_weight(document_frequency: usize, document_count: usize) -> f32 {
    let rarity = ((document_count as f32 + 1.0) / (document_frequency as f32 + 1.0)).ln();
    (1.0 + rarity).clamp(0.25, 5.0)
}

fn required_lexical_match_weight(token_count: usize, total_weight: f32) -> f32 {
    if token_count <= 3 {
        total_weight
    } else {
        (total_weight * 0.28).max(2.5)
    }
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
    let mut result = LexicalTokenMatch {
        matched_weight: 0.0,
        path_weight: 0.0,
        content_weight: 0.0,
        total_weight: 0.0,
        meaningful_path_weight: 0.0,
    };
    for (token, weight) in tokens.iter().zip(token_weights.iter().copied()) {
        result.total_weight += weight;
        let path_factor = path_match_factor(path_lower, token);
        let content_match = content_lower.contains(token.as_str());
        if path_factor > 0.0 || content_match {
            result.matched_weight += weight;
        }
        if path_factor > 0.0 {
            result.path_weight += weight * path_factor;
            if path_factor >= 1.0 && weight >= 1.5 {
                result.meaningful_path_weight += weight;
            }
        }
        if content_match {
            result.content_weight += weight;
        }
    }
    result
}

fn broad_query_path_gate(token_count: usize, token_match: &LexicalTokenMatch) -> bool {
    token_count < 8 || token_match.meaningful_path_weight > 0.0
}

fn path_match_factor(normalized_path: &str, token: &str) -> f32 {
    if normalized_path.split_whitespace().any(|part| part == token) {
        1.0
    } else if normalized_path.contains(token) {
        0.35
    } else {
        0.0
    }
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
        score *= match FileRole::classify_path(Path::new(path)) {
            FileRole::Entrypoint => 1.08,
            FileRole::Source => 1.0,
            FileRole::Test => 0.68,
            FileRole::Docs => 0.72,
            FileRole::Benchmark => 0.64,
            FileRole::Generated => 0.55,
            FileRole::Vendor => 0.45,
        };
    }
    score.min(0.99)
}

#[cfg(test)]
#[allow(clippy::permissions_set_readonly_false)]
pub(crate) fn make_test_file_writable(path: &Path) {
    let mut permissions = std::fs::metadata(path)
        .expect("test file metadata")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(windows)]
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions).expect("make test file writable");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    fn build(project: &Path, data: &Path, generation: &str, input: &str) -> PathBuf {
        let fingerprint = lexical_input_fingerprint(project, None).expect("fingerprint");
        build_lexical_shard(project, None, data, generation, &fingerprint, input)
            .expect("build lexical shard");
        shard_dir_for(data, generation)
    }

    #[test]
    fn sqlite_fts_search_keeps_existing_scoring_and_project_isolation() {
        let project_a = TempDir::new().expect("project a");
        let project_b = TempDir::new().expect("project b");
        std::fs::create_dir_all(project_a.path().join("src")).expect("mkdir a");
        std::fs::create_dir_all(project_b.path().join("src")).expect("mkdir b");
        std::fs::write(project_a.path().join("src/a_weak.rs"), "handler once").expect("a weak");
        std::fs::write(
            project_a.path().join("src/z_strong_handler.rs"),
            "handler handler handler",
        )
        .expect("a strong");
        std::fs::write(project_b.path().join("src/handler.rs"), "project_b_handler").expect("b");
        let data = TempDir::new().expect("data");
        let shard_a = build(project_a.path(), data.path(), "a", "input-a");
        let _shard_b = build(project_b.path(), data.path(), "b", "input-b");

        let hits = search_lexical_index(&shard_a, "input-a", "handler", 1).expect("search");
        assert_eq!(hits[0].path, "src/z_strong_handler.rs");
        assert!(
            search_lexical_index(&shard_a, "input-a", "project_b_handler", 8)
                .expect("isolated search")
                .is_empty()
        );
        assert!(search_lexical_index(&shard_a, "wrong-input", "handler", 8).is_err());
    }

    #[test]
    fn sqlite_lexical_search_interrupts_the_sqlite_vm() {
        let project = TempDir::new().expect("project");
        std::fs::create_dir_all(project.path().join("src")).expect("src");
        for index in 0..256 {
            std::fs::write(
                project.path().join(format!("src/{index}.rs")),
                "fn cancellation_needle() {}",
            )
            .expect("source");
        }
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "cancel", "input");
        let polls = Arc::new(AtomicUsize::new(0));
        let search_polls = Arc::clone(&polls);

        let error = search_lexical_index_with_cancel(&shard, "input", "needle", 8, move || {
            search_polls.fetch_add(1, Ordering::Relaxed) > 20
        })
        .expect_err("SQLite execution should observe cancellation");

        assert!(error.to_string().contains("cancelled"));
        assert!(
            polls.load(Ordering::Relaxed) > 20,
            "the progress handler must poll inside SQLite beyond Rust loop checkpoints"
        );
    }

    #[test]
    fn broad_prompt_relevance_fixture_remains_equivalent() {
        let project = TempDir::new().expect("project");
        for (path, content) in [
            (
                "workspace/app/src/event_processor_with_jsonl_output.rs",
                "jsonl event output request runtime turn start",
            ),
            (
                "workspace/app/tests/event_processor_with_json_output.rs",
                "json event output test approval fixture",
            ),
            ("workspace/core/src/session.rs", "session bookkeeping"),
            (
                ".agents/skills/review/SKILL.md",
                "request json cli runtime thread turn start event output",
            ),
            (
                "workspace/app-protocol/schema/typescript/v2/CommandRequestParams.ts",
                "app server command request turn start request",
            ),
        ] {
            let path = project.path().join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, content).expect("fixture");
        }
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "broad", "input");

        let hits = search_lexical_index(
            &shard,
            "input",
            "Explain how `app request --json` flows from CLI into runtime thread turn start JSONL event output",
            4,
        )
        .expect("search");
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

    #[test]
    fn old_jsonl_is_not_a_query_engine_and_is_removed_on_rebuild() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "pub fn handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = shard_dir_for(data.path(), "generation");
        std::fs::create_dir_all(&shard).expect("shard");
        std::fs::write(shard.join(LEGACY_INDEX_FILE), "legacy").expect("legacy index");
        std::fs::write(shard.join(LEGACY_META_FILE), "{}").expect("legacy meta");
        assert!(search_lexical_index(&shard, "input", "handler", 4).is_err());

        let _rebuilt = build(project.path(), data.path(), "generation", "input");
        let rebuilt = build(project.path(), data.path(), "generation", "input");
        assert!(!rebuilt.join(LEGACY_INDEX_FILE).exists());
        assert!(!rebuilt.join(LEGACY_META_FILE).exists());
        assert_eq!(
            search_lexical_index(&rebuilt, "input", "handler", 4)
                .expect("rebuilt search")
                .len(),
            1
        );
    }

    #[test]
    fn malformed_sqlite_shard_fails_closed() {
        let root = TempDir::new().expect("root");
        let shard = shard_dir_for(root.path(), "generation");
        std::fs::create_dir_all(&shard).expect("shard");
        std::fs::write(shard.join(LEXICAL_INDEX_FILE), b"not sqlite").expect("malformed");
        assert!(!shard_has_lexical_index(&shard, "input"));
        assert!(search_lexical_index(&shard, "input", "handler", 4).is_err());
    }

    #[test]
    fn sqlite_build_skips_stale_temporary_file_collisions() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = shard_dir_for(data.path(), "collision");
        std::fs::create_dir_all(&shard).expect("shard");
        let index = shard.join(LEXICAL_INDEX_FILE);
        let probe = codestory_workspace::atomic_file::atomic_temp_path(&index, "lexical-index");
        let counter = probe
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.rsplit('.').nth(1))
            .and_then(|value| value.parse::<u64>().ok())
            .expect("temp counter");
        let stale = (counter + 1..=counter + 32)
            .map(|counter| {
                index.with_file_name(format!(
                    ".lexical-index.{}.{}.tmp",
                    std::process::id(),
                    counter
                ))
            })
            .collect::<Vec<_>>();
        for path in &stale {
            std::fs::write(path, b"stale").expect("stale temp");
        }
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");

        build_lexical_shard(
            project.path(),
            None,
            data.path(),
            "collision",
            &fingerprint,
            "input",
        )
        .expect("collision-safe build");

        for path in stale {
            assert_eq!(std::fs::read(path).expect("stale preserved"), b"stale");
        }
    }

    #[test]
    fn camel_case_and_acronym_queries_use_the_same_fts_normalization_as_documents() {
        let project = TempDir::new().expect("project");
        std::fs::write(
            project.path().join("server.rs"),
            "struct HTTPServer; fn parseJSONResponse() {}",
        )
        .expect("source");
        std::fs::create_dir_all(project.path().join("src")).expect("src");
        std::fs::write(
            project.path().join("src/TLSHandshakeCoordinator.rs"),
            "// path-only acronym fixture",
        )
        .expect("acronym path");
        std::fs::write(
            project.path().join("src/ÜberServiceRegistry.rs"),
            "// path-only unicode fixture",
        )
        .expect("unicode path");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "case", "input");

        for query in [
            "HTTPServer",
            "http server",
            "parseJSONResponse",
            "parse json response",
        ] {
            assert_eq!(
                search_lexical_index(&shard, "input", query, 4)
                    .expect("search")
                    .first()
                    .map(|hit| hit.path.as_str()),
                Some("server.rs"),
                "query={query}"
            );
        }
        for (query, expected) in [
            ("TLSHandshakeCoordinator", "src/TLSHandshakeCoordinator.rs"),
            (
                "tls handshake coordinator",
                "src/TLSHandshakeCoordinator.rs",
            ),
            ("ÜBERServiceRegistry", "src/ÜberServiceRegistry.rs"),
            ("über service registry", "src/ÜberServiceRegistry.rs"),
        ] {
            assert_eq!(
                search_lexical_index(&shard, "input", query, 4)
                    .expect("path search")
                    .first()
                    .map(|hit| hit.path.as_str()),
                Some(expected),
                "query={query}"
            );
        }
    }

    #[test]
    fn fts_rows_are_bound_to_the_immutable_document_rows() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "binding", "input");
        let index = shard.join(LEXICAL_INDEX_FILE);
        make_test_file_writable(&index);
        let connection = Connection::open(&index).expect("open writable");
        connection
            .execute(
                "UPDATE lexical_fts SET content = 'forged' WHERE rowid = 1",
                [],
            )
            .expect("forge FTS row");
        drop(connection);

        assert!(!shard_matches_lexical_input(
            data.path(),
            "binding",
            1,
            &lexical_input_fingerprint(project.path(), None)
                .expect("fingerprint")
                .hash,
            "input"
        ));
    }

    #[test]
    fn routine_readiness_and_search_reject_mutated_or_deleted_bound_rows() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        for (generation, mutation) in [
            (
                "mutated-fts",
                "UPDATE lexical_fts SET content = 'forged' WHERE rowid = 1;",
            ),
            ("deleted-fts", "DELETE FROM lexical_fts WHERE rowid = 1;"),
            (
                "mutated-document",
                "DROP TRIGGER lexical_documents_no_update;
                 UPDATE lexical_documents SET content = 'forged' WHERE id = 1;",
            ),
            (
                "deleted-document",
                "DROP TRIGGER lexical_documents_no_delete;
                 DELETE FROM lexical_documents WHERE id = 1;",
            ),
        ] {
            let shard = build(project.path(), data.path(), generation, "input");
            let index = shard.join(LEXICAL_INDEX_FILE);
            make_test_file_writable(&index);
            let connection = Connection::open(&index).expect("open writable");
            connection.execute_batch(mutation).expect("mutate shard");
            drop(connection);

            assert!(
                !shard_has_lexical_index(&shard, "input"),
                "readiness accepted {generation}"
            );
            assert!(
                search_lexical_index(&shard, "input", "handler", 4).is_err(),
                "search accepted {generation}"
            );
        }
    }

    #[test]
    fn canonical_discovery_coverage_and_large_corpus_are_preserved() {
        let project = TempDir::new().expect("project");
        let src = project.path().join("src");
        std::fs::create_dir_all(&src).expect("src");
        for index in 0..4_100 {
            std::fs::write(
                src.join(format!("file_{index:04}.kt")),
                format!("fun symbol_{index:04}() = {index}\n"),
            )
            .expect("source");
        }
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "large", "input");
        let coverage = lexical_shard_coverage(data.path(), "large", "input").expect("coverage");
        assert_eq!(coverage.discovered_files, 4_100);
        assert_eq!(coverage.indexed_files, 4_100);
        assert!(coverage.complete());
        assert_eq!(
            search_lexical_index(&shard, "input", "symbol_4099", 4)
                .expect("search")
                .first()
                .map(|hit| hit.path.as_str()),
            Some("src/file_4099.kt")
        );
    }

    #[test]
    fn lexical_source_set_is_canonical_workspace_discovery() {
        let project = TempDir::new().expect("project");
        for profile in codestory_contracts::language_support::LANGUAGE_SUPPORT_PROFILES {
            for extension in profile.extensions {
                std::fs::write(
                    project
                        .path()
                        .join(format!("sample_{}.{}", profile.language_name, extension)),
                    "sample source\n",
                )
                .expect("write supported source");
            }
        }
        std::fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname='sample'\n",
        )
        .expect("cargo manifest");
        std::fs::write(project.path().join("compose.yaml"), "services: {}\n")
            .expect("compose manifest");

        let workspace = codestory_workspace::WorkspaceManifest::open(project.path().to_path_buf())
            .expect("workspace");
        let expected = workspace
            .source_files()
            .expect("canonical discovery")
            .into_iter()
            .map(|path| lexical_relative_path(project.path(), &path))
            .collect::<std::collections::BTreeSet<_>>();
        let mut actual = std::collections::BTreeSet::new();
        let coverage = scan_lexical_documents(project.path(), None, &mut |document| {
            if document.source == LexicalDocumentSource::LexicalSource {
                actual.insert(document.path.clone());
            }
            Ok(())
        })
        .expect("lexical collection");

        assert_eq!(actual, expected);
        assert!(coverage.complete());
    }

    #[test]
    fn omitted_inputs_are_persisted_in_readiness_metadata() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn ok() {}").expect("source");
        std::fs::write(
            project.path().join("large.rs"),
            vec![b'x'; MAX_FILE_BYTES as usize + 1],
        )
        .expect("large");
        std::fs::write(project.path().join("invalid.rs"), [0xff, 0xfe, 0xfd])
            .expect("invalid utf-8");
        let data = TempDir::new().expect("data");
        build(project.path(), data.path(), "coverage", "input");
        let coverage = lexical_shard_coverage(data.path(), "coverage", "input").expect("coverage");
        assert_eq!(coverage.omitted_oversized, 1);
        assert_eq!(coverage.unreadable_files, 1);
        assert!(!coverage.complete());
        assert_eq!(coverage.omitted_path_sample, ["large.rs"]);
        assert_eq!(coverage.unreadable_path_sample, ["invalid.rs"]);
    }

    #[test]
    fn all_omitted_inputs_still_publish_coverage_metadata() {
        let project = TempDir::new().expect("project");
        std::fs::write(
            project.path().join("large.rs"),
            vec![b'x'; MAX_FILE_BYTES as usize + 1],
        )
        .expect("large");
        std::fs::write(project.path().join("invalid.rs"), [0xff, 0xfe, 0xfd])
            .expect("invalid utf-8");
        let data = TempDir::new().expect("data");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        assert_eq!(fingerprint.file_count, 0);

        let rebuilt = build_lexical_shard(
            project.path(),
            None,
            data.path(),
            "all-omitted",
            &fingerprint,
            "input",
        )
        .expect("build empty shard");
        let coverage =
            lexical_shard_coverage(data.path(), "all-omitted", "input").expect("coverage");

        assert_eq!(rebuilt, fingerprint);
        assert_eq!(coverage.discovered_files, 2);
        assert_eq!(coverage.indexed_files, 0);
        assert_eq!(coverage.omitted_oversized, 1);
        assert_eq!(coverage.unreadable_files, 1);
        assert!(
            search_lexical_index(
                &shard_dir_for(data.path(), "all-omitted"),
                "input",
                "handler",
                4,
            )
            .expect("empty search")
            .is_empty()
        );
    }

    #[test]
    #[ignore = "measurement fixture; run with --ignored --nocapture for PR corpus/query evidence"]
    fn report_jsonl_to_sqlite_corpus_and_query_delta() {
        let root = TempDir::new().expect("root");
        let shard = shard_dir_for(root.path(), "benchmark");
        std::fs::create_dir_all(&shard).expect("shard");
        let documents = (0..10_000)
            .map(|index| LexicalDocument {
                path: format!("src/file_{index:05}.rs"),
                content: format!("pub fn symbol_{index:05}() {{ handler_{index:05}(); }}"),
                source: LexicalDocumentSource::LexicalSource,
                node_id: None,
                symbol_name: None,
                start_line: None,
            })
            .collect::<Vec<_>>();
        let coverage = LexicalCoverage {
            discovered_files: documents.len() as u32,
            indexed_files: documents.len() as u32,
            ..Default::default()
        };
        let fingerprint = LexicalInputFingerprint {
            file_count: documents.len() as u32,
            hash: lexical_documents_hash(&documents, &coverage),
            coverage: coverage.clone(),
        };
        write_lexical_database(
            &shard.join(LEXICAL_INDEX_FILE),
            "benchmark",
            "benchmark-input",
            &fingerprint,
            |visit| {
                for document in &documents {
                    visit(document)?;
                }
                Ok(coverage.clone())
            },
        )
        .expect("write sqlite");
        let jsonl = documents
            .iter()
            .flat_map(|document| {
                let mut row = serde_json::to_vec(document).expect("serialize JSONL row");
                row.push(b'\n');
                row
            })
            .collect::<Vec<_>>();
        let jsonl_path = shard.join(LEGACY_INDEX_FILE);
        std::fs::write(&jsonl_path, &jsonl).expect("write JSONL");

        let query = "symbol_09999";
        let mut sqlite_micros = Vec::new();
        let mut jsonl_micros = Vec::new();
        let mut sqlite_top = None;
        let mut jsonl_top = None;
        for _ in 0..21 {
            let started = std::time::Instant::now();
            let hits =
                search_lexical_index(&shard, "benchmark-input", query, 8).expect("SQLite search");
            sqlite_micros.push(started.elapsed().as_micros() as u64);
            sqlite_top = hits.first().map(|hit| hit.path.clone());

            let started = std::time::Instant::now();
            let parsed = std::fs::read_to_string(&jsonl_path)
                .expect("read JSONL")
                .lines()
                .map(|line| serde_json::from_str::<LexicalDocument>(line).expect("parse row"))
                .collect::<Vec<_>>();
            let hits = legacy_full_scan_for_measurement(&parsed, query, 8);
            jsonl_micros.push(started.elapsed().as_micros() as u64);
            jsonl_top = hits.first().map(|hit| hit.path.clone());
        }
        sqlite_micros.sort_unstable();
        jsonl_micros.sort_unstable();
        assert_eq!(sqlite_top, jsonl_top);
        println!(
            "{}",
            serde_json::json!({
                "corpus_documents": documents.len(),
                "jsonl_bytes": std::fs::metadata(jsonl_path).expect("JSONL metadata").len(),
                "sqlite_bytes": std::fs::metadata(shard.join(LEXICAL_INDEX_FILE)).expect("SQLite metadata").len(),
                "jsonl_median_query_us": jsonl_micros[jsonl_micros.len() / 2],
                "sqlite_median_query_us": sqlite_micros[sqlite_micros.len() / 2],
            })
        );
    }

    fn legacy_full_scan_for_measurement(
        documents: &[LexicalDocument],
        query: &str,
        limit: usize,
    ) -> Vec<LexicalHit> {
        let tokens = lexical_query_tokens(query);
        let frequencies = tokens
            .iter()
            .map(|token| {
                documents
                    .iter()
                    .filter(|document| {
                        document.path.to_ascii_lowercase().contains(token.as_str())
                            || document
                                .content
                                .to_ascii_lowercase()
                                .contains(token.as_str())
                    })
                    .count()
            })
            .collect::<Vec<_>>();
        let weights = frequencies
            .iter()
            .map(|frequency| lexical_token_weight(*frequency, documents.len()))
            .collect::<Vec<_>>();
        let required = required_lexical_match_weight(tokens.len(), weights.iter().sum());
        let mut hits = documents
            .iter()
            .filter_map(|document| {
                let token_match = lexical_token_match(
                    &tokens,
                    &weights,
                    &document.path.to_ascii_lowercase(),
                    &document.content.to_ascii_lowercase(),
                );
                (token_match.matched_weight >= required).then(|| LexicalHit {
                    path: document.path.clone(),
                    source: document.source,
                    node_id: document.node_id.clone(),
                    symbol_name: document.symbol_name.clone(),
                    start_line: document.start_line,
                    score: score_lexical_match(&document.path, document.source, &token_match),
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.path.cmp(&right.path))
        });
        hits.truncate(limit);
        hits
    }
}
