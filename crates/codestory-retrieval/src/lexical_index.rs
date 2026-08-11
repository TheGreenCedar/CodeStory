//! Project-local SQLite FTS lexical index.

use anyhow::{Context, Result, bail};
use codestory_contracts::api::SearchTargetDto;
use codestory_contracts::owned_artifacts::sqlite_file_with_sidecars;
use codestory_contracts::validation_receipts::SealedReceiptCache;
#[cfg(test)]
use codestory_store::FileRole;
use codestory_store::{SourcePolicyExclusionPolicyIdentity, Store, SymbolSearchDoc};
use codestory_workspace::paths::sqlite_open_path;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const LEXICAL_INDEX_VERSION: &str = "sqlite-fts5-v1";
pub const LEXICAL_INDEX_FILE: &str = "lexical-index.sqlite3";
const LEGACY_INDEX_FILE: &str = "lexical-index.jsonl";
const LEGACY_META_FILE: &str = "shard-meta.json";
const LEGACY_STUB_MARKER: &str = ".zoekt-stub";
/// Default lexical source-file cap for scans without a pinned core publication.
/// Product scans use the active cap recorded with that publication.
pub(crate) const MAX_FILE_BYTES: u64 = codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP;
const MAX_CANDIDATES: usize = 4_096;
const COVERAGE_PATH_SAMPLE: usize = 32;

/// How many published lexical generations may hold a sealed health receipt at
/// once.
///
/// A runtime works on one project generation at a time and keeps at most the
/// outgoing one alive beside it, so the live cardinality is a handful; the
/// bound exists to make the memory ceiling hard, not to force turnover. A
/// receipt is one metadata row, so the whole cache at capacity is tens of
/// kilobytes. Reaching the bound clears the cache, which costs a re-scan and
/// never changes a verdict.
const LEXICAL_SHARD_RECEIPT_CAPACITY: usize = 256;

/// Sealed deep-verification receipts for immutable lexical shards.
///
/// The receipt records what a shard *is* — its self-consistent metadata row —
/// after a full integrity pass. It never records whether that shard satisfies
/// a particular caller's expectations; those comparisons are cheap and re-run
/// on every probe. The seal covers the shard database and every SQLite sidecar
/// identity the registry owns, so an in-place rewrite, a replacement, or a
/// stray write-ahead log invalidates the receipt instead of hiding behind it.
static LEXICAL_SHARD_RECEIPTS: SealedReceiptCache<PathBuf, LexicalShardMetadata> =
    SealedReceiptCache::new(LEXICAL_SHARD_RECEIPT_CAPACITY);

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
    pub target: Option<SearchTargetDto>,
    pub source_excerpt: Option<String>,
    pub score: f32,
}

pub(crate) struct LexicalSourceInput {
    hasher: Sha256,
    file_count: u32,
    coverage: LexicalCoverage,
}

#[cfg(test)]
pub fn lexical_input_fingerprint(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<LexicalInputFingerprint> {
    let mut hasher = lexical_documents_hasher();
    let mut file_count = 0_u32;
    let coverage =
        scan_lexical_documents(project_root, storage_path, storage_path, &mut |document| {
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

pub(crate) fn lexical_source_input(
    project_root: &Path,
    storage_path: &Path,
) -> Result<LexicalSourceInput> {
    let mut hasher = lexical_documents_hasher();
    let mut file_count = 0_u32;
    let coverage =
        scan_lexical_documents(project_root, Some(storage_path), None, &mut |document| {
            hash_lexical_document(&mut hasher, document);
            file_count = file_count.saturating_add(1);
            Ok(())
        })?;
    Ok(LexicalSourceInput {
        hasher,
        file_count,
        coverage,
    })
}

pub(crate) fn finish_lexical_input_for_store(
    mut source: LexicalSourceInput,
    project_root: &Path,
    storage: &Store,
) -> Result<LexicalInputFingerprint> {
    scan_symbol_documents_from_store(project_root, storage, &mut |document| {
        hash_lexical_document(&mut source.hasher, document);
        source.file_count = source.file_count.saturating_add(1);
        Ok(())
    })?;
    Ok(LexicalInputFingerprint {
        file_count: source.file_count,
        hash: finish_lexical_documents_hash(source.hasher, &source.coverage),
        coverage: source.coverage,
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
            |visit| scan_lexical_documents(project_root, storage_path, storage_path, visit),
        )?;
        // The staged file is about to be renamed away, so its verdict is not
        // receiptable: seal the published identity, never the temporary one.
        let staged = verify_lexical_database_contents(&temp_path)?;
        match_lexical_shard_expectations(
            &staged,
            project_id,
            sidecar_input_hash,
            Some((expected.file_count, expected.hash.as_str())),
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
    )?
    .coverage)
}

/// Sealed-receipt accounting for one shard, for tests that must prove the deep
/// verification ran exactly as often as the seal allowed.
#[cfg(test)]
pub(crate) fn lexical_shard_receipt_stats(
    lexical_data_dir: &Path,
    sidecar_generation: &str,
) -> Option<codestory_contracts::validation_receipts::ReceiptStats> {
    LEXICAL_SHARD_RECEIPTS
        .stats(&shard_dir_for(lexical_data_dir, sidecar_generation).join(LEXICAL_INDEX_FILE))
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
    let _metadata = validate_open_database_metadata(
        &connection,
        project_id,
        expected_sidecar_input_hash,
        None,
        cancelled.as_ref(),
    )?;
    let document_count: usize = connection.query_row(
        "SELECT file_count FROM lexical_metadata WHERE id = 1",
        [],
        |row| row.get::<_, u32>(0).map(|count| count as usize),
    )?;
    search_lexical_index_on_connection(
        &connection,
        query,
        limit,
        document_count,
        &mut HashMap::new(),
        cancelled.as_ref(),
    )
}

pub(crate) fn search_lexical_index_batch_with_cancel(
    shard_dir: &Path,
    expected_sidecar_input_hash: &str,
    queries: &[(String, usize)],
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Vec<Vec<LexicalHit>>> {
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    if cancelled() {
        bail!("lexical search cancelled");
    }
    let Some(project_id) = shard_dir.file_name().and_then(|name| name.to_str()) else {
        bail!("lexical shard path has no generation directory");
    };
    let index_path = shard_dir.join(LEXICAL_INDEX_FILE);
    let connection = open_read_only(&index_path)?;
    let progress_cancelled = Arc::clone(&cancelled);
    connection.progress_handler(1_000, Some(move || progress_cancelled()))?;
    let _metadata = validate_open_database_metadata(
        &connection,
        project_id,
        expected_sidecar_input_hash,
        None,
        cancelled.as_ref(),
    )?;
    let document_count: usize = connection.query_row(
        "SELECT file_count FROM lexical_metadata WHERE id = 1",
        [],
        |row| row.get::<_, u32>(0).map(|count| count as usize),
    )?;
    let mut token_frequencies = HashMap::new();
    queries
        .iter()
        .map(|(query, limit)| {
            search_lexical_index_on_connection(
                &connection,
                query,
                *limit,
                document_count,
                &mut token_frequencies,
                cancelled.as_ref(),
            )
        })
        .collect()
}

fn search_lexical_index_on_connection(
    connection: &Connection,
    query: &str,
    limit: usize,
    document_count: usize,
    frequency_cache: &mut HashMap<String, usize>,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Result<Vec<LexicalHit>> {
    if cancelled() {
        bail!("lexical search cancelled");
    }
    if limit == 0 {
        return Ok(Vec::new());
    }
    let tokens = lexical_query_tokens(query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let fts_query = tokens
        .iter()
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let candidate_limit = limit.saturating_mul(64).clamp(256, MAX_CANDIDATES);

    let mandatory_tokens = quoted_query_tokens(query);
    let mut token_frequencies = Vec::with_capacity(tokens.len());
    for token in &tokens {
        if cancelled() {
            bail!("lexical search cancelled");
        }
        let frequency = match frequency_cache.get(token) {
            Some(frequency) => *frequency,
            None => {
                let frequency = fts_document_frequency(connection, token)?;
                frequency_cache.insert(token.clone(), frequency);
                frequency
            }
        };
        token_frequencies.push(frequency);
    }
    let token_weights = token_frequencies
        .iter()
        .zip(tokens.iter())
        .map(|(frequency, token)| {
            let mut weight = lexical_token_weight(*frequency, document_count);
            if mandatory_tokens.iter().any(|mandatory| mandatory == token) {
                weight *= 2.0;
            }
            weight
        })
        .collect::<Vec<_>>();
    let total_weight = token_weights.iter().sum::<f32>();
    let required_weight = required_lexical_match_weight(tokens.len(), total_weight);

    let exact_candidates = query_exact_candidates(connection, query, candidate_limit)?;
    let path_candidates = query_fts_candidates(
        connection,
        &fts_query,
        candidate_limit,
        LexicalCandidateOrder::Path,
    )?;
    let content_candidates = query_fts_candidates(
        connection,
        &fts_query,
        candidate_limit,
        LexicalCandidateOrder::Content,
    )?;
    let mut symbol_candidates = query_fts_candidates(
        connection,
        &fts_query,
        candidate_limit,
        LexicalCandidateOrder::SymbolDocument,
    )?;
    rank_symbol_candidates_by_identifier_overlap(&mut symbol_candidates, &tokens, &token_weights);

    // Exact, path-BM25, content-BM25, and focused symbol documents are
    // independent lexical recall lanes. Preserve every deterministic rank a
    // document earned and fuse those ranks instead of comparing incompatible
    // raw BM25 scores or collapsing them to one best lane. The coverage rule
    // below remains the admission threshold.
    let query_shape = crate::query_features::classify_query(query).shape;
    let mut best_sublane_ranks = HashMap::new();
    for (lane, candidates) in [
        (LexicalSublane::Exact, exact_candidates.as_slice()),
        (LexicalSublane::Path, path_candidates.as_slice()),
        (LexicalSublane::Content, content_candidates.as_slice()),
        (LexicalSublane::SymbolDocument, symbol_candidates.as_slice()),
    ] {
        record_lexical_sublane_ranks(&mut best_sublane_ranks, candidates, lane);
    }

    let mut candidates = exact_candidates;
    candidates.extend(interleave_candidate_lanes(vec![
        path_candidates,
        content_candidates,
        symbol_candidates,
    ]));

    let mut hits = Vec::new();
    let mut seen = HashSet::new();
    for (index, (document, normalized_path, normalized_content)) in
        candidates.into_iter().enumerate()
    {
        if index % 64 == 0 && cancelled() {
            bail!("lexical search cancelled");
        }
        let identity = lexical_candidate_identity(&document);
        let sublane_ranks = best_sublane_ranks
            .get(&identity)
            .copied()
            .unwrap_or_default();
        if seen.contains(&identity) {
            continue;
        }
        if seen.len() == MAX_CANDIDATES {
            break;
        }
        seen.insert(identity);
        let token_match = lexical_token_match(
            &tokens,
            &token_weights,
            &normalized_path,
            &normalized_content,
        );
        if token_match.matched_weight >= required_weight
            && mandatory_tokens_match(&mandatory_tokens, &normalized_path, &normalized_content)
        {
            let (target, matched_line, source_excerpt) =
                if document.source == LexicalDocumentSource::LexicalSource {
                    lexical_source_target(
                        &document.path,
                        &document.content,
                        &tokens,
                        token_match.content_weight > 0.0,
                    )
                } else {
                    (None, None, None)
                };
            hits.push(LexicalHit {
                score: lexical_sublane_score(sublane_ranks, query_shape),
                path: document.path,
                source: document.source,
                node_id: document.node_id,
                symbol_name: document.symbol_name,
                start_line: document.start_line.or(matched_line),
                target,
                source_excerpt,
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
            .then_with(|| lexical_source_rank(left.source).cmp(&lexical_source_rank(right.source)))
            .then_with(|| left.node_id.cmp(&right.node_id))
            .then_with(|| left.symbol_name.cmp(&right.symbol_name))
            .then_with(|| left.start_line.cmp(&right.start_line))
    });
    hits.truncate(limit);
    Ok(hits)
}

fn lexical_source_rank(source: LexicalDocumentSource) -> u8 {
    match source {
        LexicalDocumentSource::SymbolDoc => 0,
        LexicalDocumentSource::LexicalSource => 1,
        LexicalDocumentSource::ComponentReport => 2,
    }
}

#[derive(Debug, Clone, Copy)]
enum LexicalCandidateOrder {
    Path,
    Content,
    SymbolDocument,
}

type LexicalCandidate = (LexicalDocument, String, String);

fn query_fts_candidates(
    connection: &Connection,
    fts_query: &str,
    candidate_limit: usize,
    order: LexicalCandidateOrder,
) -> Result<Vec<LexicalCandidate>> {
    let scoped_query = match order {
        LexicalCandidateOrder::Path => format!("path : ({fts_query})"),
        LexicalCandidateOrder::Content | LexicalCandidateOrder::SymbolDocument => {
            format!("content : ({fts_query})")
        }
    };
    let sql = match order {
        LexicalCandidateOrder::Path => {
            "SELECT d.path, d.content, lexical_fts.path, lexical_fts.content,
                    d.source, d.node_id, d.symbol_name, d.start_line
             FROM lexical_fts
             JOIN lexical_documents d ON d.id = lexical_fts.rowid
             WHERE lexical_fts MATCH ?1
             ORDER BY bm25(lexical_fts, 8.0, 1.0), d.path, d.id
             LIMIT ?2"
        }
        LexicalCandidateOrder::Content => {
            "SELECT d.path, d.content, lexical_fts.path, lexical_fts.content,
                    d.source, d.node_id, d.symbol_name, d.start_line
             FROM lexical_fts
             JOIN lexical_documents d ON d.id = lexical_fts.rowid
             WHERE lexical_fts MATCH ?1
             ORDER BY bm25(lexical_fts, 1.0, 4.0), d.path, d.id
             LIMIT ?2"
        }
        LexicalCandidateOrder::SymbolDocument => {
            "SELECT d.path, d.content, lexical_fts.path, lexical_fts.content,
                    d.source, d.node_id, d.symbol_name, d.start_line
             FROM lexical_fts
             JOIN lexical_documents d ON d.id = lexical_fts.rowid
             WHERE lexical_fts MATCH ?1 AND d.source = 'symbol_doc'
             ORDER BY bm25(lexical_fts, 1.0, 4.0), d.path, d.id
             LIMIT ?2"
        }
    };
    let mut statement = connection.prepare_cached(sql)?;
    let rows = statement.query_map(params![scoped_query, candidate_limit as i64], |row| {
        let document = LexicalDocument {
            path: row.get(0)?,
            content: row.get(1)?,
            source: LexicalDocumentSource::parse(&row.get::<_, String>(4)?).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    error.into(),
                )
            })?,
            node_id: row.get(5)?,
            symbol_name: row.get(6)?,
            start_line: row.get(7)?,
        };
        Ok((document, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn query_exact_candidates(
    connection: &Connection,
    query: &str,
    candidate_limit: usize,
) -> Result<Vec<LexicalCandidate>> {
    let mut needles = quoted_query_tokens(query);
    let intent = crate::query_features::classify_query(query).intent;
    needles.extend(
        intent
            .exact_symbols
            .into_iter()
            .chain(intent.paths)
            .map(|needle| needle.to_ascii_lowercase()),
    );
    needles.sort();
    needles.dedup();

    let mut candidates = Vec::new();
    let mut statement = connection.prepare_cached(
        "SELECT d.path, d.content, lower(lexical_fts.path), lower(lexical_fts.content),
                d.source, d.node_id, d.symbol_name, d.start_line
         FROM lexical_documents d
         JOIN lexical_fts ON lexical_fts.rowid = d.id
         WHERE lower(d.path) = ?1
            OR lower(d.symbol_name) = ?1
            OR lower(d.symbol_name) LIKE '%::' || ?1
            OR lower(d.symbol_name) LIKE '%.' || ?1
         ORDER BY d.path, d.source, d.node_id, d.symbol_name, d.start_line, d.id
         LIMIT ?2",
    )?;
    for needle in needles {
        let rows = statement.query_map(params![needle, candidate_limit as i64], |row| {
            let document = LexicalDocument {
                path: row.get(0)?,
                content: row.get(1)?,
                source: LexicalDocumentSource::parse(&row.get::<_, String>(4)?).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            error.into(),
                        )
                    },
                )?,
                node_id: row.get(5)?,
                symbol_name: row.get(6)?,
                start_line: row.get(7)?,
            };
            Ok((document, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
        })?;
        candidates.extend(rows.collect::<std::result::Result<Vec<_>, _>>()?);
        if candidates.len() >= candidate_limit {
            break;
        }
    }
    candidates.truncate(candidate_limit);
    Ok(candidates)
}

fn interleave_candidate_lanes(lanes: Vec<Vec<LexicalCandidate>>) -> Vec<LexicalCandidate> {
    let mut lanes = lanes.into_iter().map(Vec::into_iter).collect::<Vec<_>>();
    let mut candidates = Vec::new();
    loop {
        let mut added = false;
        for lane in &mut lanes {
            if let Some(candidate) = lane.next() {
                candidates.push(candidate);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    candidates
}

type LexicalCandidateIdentity = (String, u8, Option<String>, Option<String>, Option<u32>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexicalSublane {
    Exact,
    Path,
    Content,
    SymbolDocument,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LexicalSublaneRanks {
    exact: Option<u32>,
    path: Option<u32>,
    content: Option<u32>,
    symbol_document: Option<u32>,
}

impl LexicalSublaneRanks {
    fn record(&mut self, lane: LexicalSublane, rank: u32) {
        let slot = match lane {
            LexicalSublane::Exact => &mut self.exact,
            LexicalSublane::Path => &mut self.path,
            LexicalSublane::Content => &mut self.content,
            LexicalSublane::SymbolDocument => &mut self.symbol_document,
        };
        *slot = Some(slot.map_or(rank, |current| current.min(rank)));
    }
}

fn record_lexical_sublane_ranks(
    ranks: &mut HashMap<LexicalCandidateIdentity, LexicalSublaneRanks>,
    candidates: &[LexicalCandidate],
    lane: LexicalSublane,
) {
    for (index, (document, _, _)) in candidates.iter().enumerate() {
        let candidate_rank = u32::try_from(index + 1).unwrap_or(u32::MAX);
        ranks
            .entry(lexical_candidate_identity(document))
            .or_default()
            .record(lane, candidate_rank);
    }
}

fn rank_symbol_candidates_by_identifier_overlap(
    candidates: &mut [LexicalCandidate],
    tokens: &[String],
    token_weights: &[f32],
) {
    let original_order = candidates
        .iter()
        .enumerate()
        .map(|(index, (document, _, _))| (lexical_candidate_identity(document), index))
        .collect::<HashMap<_, _>>();
    candidates.sort_by(|left, right| {
        symbol_identifier_overlap(&right.0, tokens, token_weights)
            .partial_cmp(&symbol_identifier_overlap(&left.0, tokens, token_weights))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                original_order[&lexical_candidate_identity(&left.0)]
                    .cmp(&original_order[&lexical_candidate_identity(&right.0)])
            })
    });
}

fn symbol_identifier_overlap(
    document: &LexicalDocument,
    tokens: &[String],
    token_weights: &[f32],
) -> f32 {
    let Some(symbol_name) = document.symbol_name.as_deref() else {
        return 0.0;
    };
    let normalized = normalize_lexical_text(symbol_name);
    let symbol_tokens = normalized.split_whitespace().collect::<HashSet<_>>();
    tokens
        .iter()
        .zip(token_weights.iter().copied())
        .filter_map(|(token, weight)| symbol_tokens.contains(token.as_str()).then_some(weight))
        .sum()
}

fn lexical_sublane_score(
    ranks: LexicalSublaneRanks,
    shape: crate::query_features::QueryShape,
) -> f32 {
    // Fuse only deterministic ranks. BM25 values from different FTS column
    // weightings are not comparable, and aggregate file coverage must not
    // erase a focused symbol-document signal. k=20 matches the outer hybrid
    // ranker; the denominator normalizes a candidate ranked first in every
    // lexical sublane to 1.0.
    const RRF_K: f32 = 20.0;
    let (exact_weight, path_weight, content_weight, symbol_weight) = match shape {
        crate::query_features::QueryShape::PathLike => (2.0, 1.25, 1.0, 1.0),
        crate::query_features::QueryShape::SymbolLike => (2.0, 0.75, 1.0, 1.5),
        crate::query_features::QueryShape::NaturalLanguage
        | crate::query_features::QueryShape::Mixed => (2.0, 1.0, 1.0, 1.25),
    };
    let weighted_rank =
        |rank: Option<u32>, weight: f32| rank.map_or(0.0, |rank| weight / (RRF_K + rank as f32));
    let score = weighted_rank(ranks.exact, exact_weight)
        + weighted_rank(ranks.path, path_weight)
        + weighted_rank(ranks.content, content_weight)
        + weighted_rank(ranks.symbol_document, symbol_weight);
    let maximum = (exact_weight + path_weight + content_weight + symbol_weight) / (RRF_K + 1.0);
    (score / maximum).clamp(0.0, 1.0)
}

fn lexical_candidate_identity(document: &LexicalDocument) -> LexicalCandidateIdentity {
    let source = match document.source {
        LexicalDocumentSource::LexicalSource => 0,
        LexicalDocumentSource::SymbolDoc => 1,
        LexicalDocumentSource::ComponentReport => 2,
    };
    (
        document.path.clone(),
        source,
        document.node_id.clone(),
        document.symbol_name.clone(),
        document.start_line,
    )
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
    let mut connection = Connection::open(sqlite_open_path(path))
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
            return Err(crate::index::SidecarInputChanged::new(
                "lexical shard build",
                format!(
                    "{} documents with hash {}",
                    expected.file_count, expected.hash
                ),
                format!("{} documents with hash {}", actual.file_count, actual.hash),
            )
            .into());
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

/// Deep-verify one immutable lexical shard, reusing a sealed receipt when the
/// shard's native identity is unchanged, then check the caller's expectations
/// against the receipted metadata.
///
/// The two halves are deliberately separate. The deep half scans the whole FTS
/// mirror and is a fact about the artifact, so it is receiptable. The
/// expectation half is three string comparisons that depend on the caller, so
/// it runs every time and can never be answered from a receipt.
fn validate_lexical_database(
    path: &Path,
    expected_project_id: &str,
    expected_sidecar_input_hash: &str,
    expected_lexical: Option<(u32, &str)>,
) -> Result<LexicalShardMetadata> {
    let metadata = LEXICAL_SHARD_RECEIPTS.validate_sealed(
        path.to_path_buf(),
        &sqlite_file_with_sidecars(path),
        || verify_lexical_database_contents(path),
    )?;
    match_lexical_shard_expectations(
        &metadata,
        expected_project_id,
        expected_sidecar_input_hash,
        expected_lexical,
    )?;
    Ok(metadata)
}

/// The receiptable half: everything that depends only on the shard's own bytes.
///
/// `quick_check` is unconditional here. It used to be opt-in so that the cheap
/// callers could skip it, but with the verdict sealed to native identity the
/// page-level check is paid once per generation, and the strongest verdict is
/// the only one worth sealing.
fn verify_lexical_database_contents(path: &Path) -> Result<LexicalShardMetadata> {
    if !path.is_file() {
        bail!("lexical SQLite shard is missing");
    }
    let connection = open_read_only(path)?;
    verify_open_database_contents(&connection, &|| false)
}

fn verify_open_database_contents(
    connection: &Connection,
    cancelled: &dyn Fn() -> bool,
) -> Result<LexicalShardMetadata> {
    let metadata = read_open_database_metadata(connection, cancelled)?;
    let check: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if check != "ok" {
        bail!("lexical SQLite shard failed quick_check: {check}");
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
            bail!("lexical validation cancelled");
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
        bail!("lexical validation cancelled");
    }
    Ok(metadata)
}

fn validate_open_database_metadata(
    connection: &Connection,
    expected_project_id: &str,
    expected_sidecar_input_hash: &str,
    expected_lexical: Option<(u32, &str)>,
    cancelled: &dyn Fn() -> bool,
) -> Result<LexicalShardMetadata> {
    let metadata = read_open_database_metadata(connection, cancelled)?;
    match_lexical_shard_expectations(
        &metadata,
        expected_project_id,
        expected_sidecar_input_hash,
        expected_lexical,
    )?;
    Ok(metadata)
}

/// Read the shard's own metadata row and check it is internally consistent.
///
/// Nothing here depends on the caller, which is what makes the verdict
/// receiptable.
fn read_open_database_metadata(
    connection: &Connection,
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
    Ok(metadata)
}

/// The caller-dependent half: never receipted, always re-checked.
fn match_lexical_shard_expectations(
    metadata: &LexicalShardMetadata,
    expected_project_id: &str,
    expected_sidecar_input_hash: &str,
    expected_lexical: Option<(u32, &str)>,
) -> Result<()> {
    if metadata.project_id != expected_project_id {
        bail!("lexical SQLite shard project id does not match its generation directory");
    }
    if metadata.sidecar_input_hash != expected_sidecar_input_hash {
        bail!("lexical SQLite shard does not match the sidecar input hash");
    }
    if let Some((file_count, lexical_hash)) = expected_lexical
        && (metadata.file_count != file_count || metadata.lexical_hash != lexical_hash)
    {
        bail!("lexical SQLite shard does not match current lexical input");
    }
    Ok(())
}

fn open_read_only(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(
        sqlite_open_path(path),
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
    source_storage_path: Option<&Path>,
    symbol_storage_path: Option<&Path>,
    visit: &mut dyn FnMut(&LexicalDocument) -> Result<()>,
) -> Result<LexicalCoverage> {
    let source_policy = lexical_source_policy(project_root, source_storage_path)?;
    let workspace = match source_storage_path {
        Some(storage_path) => {
            codestory_workspace::WorkspaceManifest::open_with_storage_owned_exclusions(
                project_root.to_path_buf(),
                storage_path,
            )
        }
        None => codestory_workspace::WorkspaceManifest::open(project_root.to_path_buf()),
    }
    .context("open workspace for lexical discovery")?;
    let discovered = workspace
        .source_files()
        .context("discover canonical workspace files for lexical index")?;
    let mut coverage = LexicalCoverage::default();
    for path in discovered {
        let relative = lexical_relative_path(project_root, &path);
        if source_policy.excluded_paths.contains(&relative) {
            continue;
        }
        coverage.discovered_files = coverage.discovered_files.saturating_add(1);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                coverage.unreadable_files = coverage.unreadable_files.saturating_add(1);
                push_coverage_sample(&mut coverage.unreadable_path_sample, relative);
                continue;
            }
        };
        if metadata.len() > source_policy.max_file_bytes {
            coverage.omitted_oversized = coverage.omitted_oversized.saturating_add(1);
            push_coverage_sample(&mut coverage.omitted_path_sample, relative);
            continue;
        }
        let content = match read_lexical_file_text_limited(&path, source_policy.max_file_bytes) {
            Ok(Some(content)) => content,
            Ok(None) => {
                coverage.omitted_oversized = coverage.omitted_oversized.saturating_add(1);
                push_coverage_sample(&mut coverage.omitted_path_sample, relative);
                continue;
            }
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
    scan_symbol_documents(project_root, symbol_storage_path, visit)?;
    Ok(coverage)
}

fn read_lexical_file_text_limited(path: &Path, max_bytes: u64) -> std::io::Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    read_lexical_text_limited(file, max_bytes)
}

fn read_lexical_text_limited(reader: impl Read, max_bytes: u64) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    let mut reader = reader.take(max_bytes.saturating_add(1));
    reader.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Ok(None);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

struct LexicalSourcePolicy {
    max_file_bytes: u64,
    excluded_paths: HashSet<String>,
}

fn lexical_source_policy(
    project_root: &Path,
    source_storage_path: Option<&Path>,
) -> Result<LexicalSourcePolicy> {
    let Some(storage_path) = source_storage_path else {
        return Ok(LexicalSourcePolicy {
            max_file_bytes: MAX_FILE_BYTES,
            excluded_paths: HashSet::new(),
        });
    };
    if !storage_path.is_file() {
        bail!(
            "pinned core storage for lexical source policy is missing: {}",
            storage_path.display()
        );
    }
    let storage =
        Store::open_read_only(storage_path).context("open storage for lexical source policy")?;
    let publication = storage
        .get_complete_index_publication()
        .context("load complete core publication for lexical source policy")?
        .context("complete core publication for lexical source policy is missing")?;
    let manifest = storage
        .get_source_policy_exclusion_manifest()
        .context("load lexical source policy manifest")?
        .context("lexical source policy manifest is missing")?;
    let records = storage
        .get_source_policy_exclusions()
        .context("load lexical source policy exclusions")?;
    let project_identity = codestory_workspace::project_identity_v3(project_root);
    let validated = storage
        .validate_source_policy_exclusion_publication(
            &publication,
            &project_identity.project_id,
            &project_identity.workspace_id,
            SourcePolicyExclusionPolicyIdentity::new(
                &manifest.policy_version,
                manifest.byte_cap,
                manifest.structural_unit_cap,
            ),
        )
        .context("validate lexical source policy publication")?;
    let confirmed_publication = storage
        .get_complete_index_publication()
        .context("confirm complete core publication for lexical source policy")?;
    let confirmed_manifest = storage
        .get_source_policy_exclusion_manifest()
        .context("confirm lexical source policy manifest")?;
    let confirmed_records = storage
        .get_source_policy_exclusions()
        .context("confirm lexical source policy exclusions")?;
    if validated != manifest
        || confirmed_publication.as_ref() != Some(&publication)
        || confirmed_manifest.as_ref() != Some(&manifest)
        || confirmed_records != records
    {
        bail!("lexical source policy publication changed while it was being pinned");
    }

    Ok(LexicalSourcePolicy {
        max_file_bytes: validated.byte_cap,
        excluded_paths: records
            .into_iter()
            .map(|record| record.normalized_path)
            .collect(),
    })
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
    scan_symbol_documents_from_store(project_root, &storage, visit)
}

fn scan_symbol_documents_from_store(
    project_root: &Path,
    storage: &Store,
    visit: &mut dyn FnMut(&LexicalDocument) -> Result<()>,
) -> Result<()> {
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
    let mut characters = value.chars().peekable();
    let mut previous: Option<char> = None;
    while let Some(character) = characters.next() {
        let next = characters.peek().copied();
        if character.is_uppercase()
            && previous.is_some_and(|value: char| value.is_lowercase() || value.is_numeric())
            || character.is_uppercase()
                && previous.is_some_and(|value: char| value.is_uppercase())
                && next.is_some_and(|value| value.is_lowercase())
        {
            normalized.push(' ');
        }
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else {
            normalized.push(' ');
        }
        previous = Some(character);
    }
    normalized
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LexicalSourceToken {
    normalized: String,
    start_byte: usize,
    end_byte: usize,
}

fn lexical_source_match(value: &str, query_tokens: &[String]) -> Option<LexicalSourceToken> {
    let mut characters = value.char_indices().peekable();
    let mut current = String::new();
    let mut start_byte = None;
    let mut previous: Option<char> = None;

    while let Some((byte_index, character)) = characters.next() {
        let next = characters.peek().map(|(_, character)| *character);
        let camel_boundary = character.is_uppercase()
            && previous.is_some_and(|value| value.is_lowercase() || value.is_numeric())
            || character.is_uppercase()
                && previous.is_some_and(|value| value.is_uppercase())
                && next.is_some_and(|value| value.is_lowercase());
        if camel_boundary && !current.is_empty() {
            let token = LexicalSourceToken {
                normalized: std::mem::take(&mut current),
                start_byte: start_byte.take().expect("non-empty token has a start"),
                end_byte: byte_index,
            };
            if query_tokens
                .iter()
                .any(|query| token.normalized.starts_with(query))
            {
                return Some(token);
            }
        }
        if character.is_alphanumeric() {
            start_byte.get_or_insert(byte_index);
            current.extend(character.to_lowercase());
        } else if !current.is_empty() {
            let token = LexicalSourceToken {
                normalized: std::mem::take(&mut current),
                start_byte: start_byte.take().expect("non-empty token has a start"),
                end_byte: byte_index,
            };
            if query_tokens
                .iter()
                .any(|query| token.normalized.starts_with(query))
            {
                return Some(token);
            }
        }
        previous = Some(character);
    }
    if !current.is_empty() {
        let token = LexicalSourceToken {
            normalized: current,
            start_byte: start_byte.expect("non-empty token has a start"),
            end_byte: value.len(),
        };
        if query_tokens
            .iter()
            .any(|query| token.normalized.starts_with(query))
        {
            return Some(token);
        }
    }
    None
}

fn lexical_source_target(
    file_path: &str,
    content: &str,
    query_tokens: &[String],
    content_matched: bool,
) -> (Option<SearchTargetDto>, Option<u32>, Option<String>) {
    if content_matched && let Some(matched) = lexical_source_match(content, query_tokens) {
        let start_line = content[..matched.start_byte]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            .saturating_add(1) as u32;
        let line_start = content[..matched.start_byte]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let line_end = content[matched.end_byte..]
            .find('\n')
            .map_or(content.len(), |index| matched.end_byte + index);
        let excerpt_prefix = content[line_start..matched.start_byte]
            .chars()
            .rev()
            .take(192)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>();
        let excerpt_match = content[matched.start_byte..matched.end_byte]
            .chars()
            .take(128)
            .collect::<String>();
        let excerpt_suffix = content[matched.end_byte..line_end]
            .chars()
            .take(192)
            .collect::<String>();
        let Ok(start_byte) = u32::try_from(matched.start_byte) else {
            return (
                Some(SearchTargetDto::File {
                    file_path: file_path.to_string(),
                }),
                None,
                None,
            );
        };
        let Ok(end_byte) = u32::try_from(matched.end_byte) else {
            return (
                Some(SearchTargetDto::File {
                    file_path: file_path.to_string(),
                }),
                None,
                None,
            );
        };
        return (
            Some(SearchTargetDto::FileRange {
                file_path: file_path.to_string(),
                start_byte,
                end_byte,
            }),
            Some(start_line),
            Some(format!("{excerpt_prefix}{excerpt_match}{excerpt_suffix}")),
        );
    }

    (
        Some(SearchTargetDto::File {
            file_path: file_path.to_string(),
        }),
        None,
        None,
    )
}

fn fts_document_frequency(connection: &Connection, token: &str) -> Result<usize> {
    let query = format!("\"{}\"*", token.replace('"', "\"\""));
    connection
        .prepare_cached("SELECT count(*) FROM lexical_fts WHERE lexical_fts MATCH ?1")?
        .query_row([query], |row| {
            row.get::<_, u32>(0).map(|count| count as usize)
        })
        .map_err(Into::into)
}

fn lexical_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let normalized = normalize_lexical_text(query);
    for token in normalized
        .split_whitespace()
        .filter(|token| token.len() >= 2)
        .filter(|token| !LEXICAL_STOP_WORDS.contains(token))
    {
        if !tokens.iter().any(|existing| existing == token) {
            tokens.push(token.to_string());
        }
    }
    tokens
}

fn quoted_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut delimiter = None;
    let mut current = String::new();
    for character in query.chars() {
        if delimiter == Some(character) {
            if !current.is_empty() {
                for token in lexical_query_tokens(&current) {
                    if !tokens.iter().any(|existing| existing == &token) {
                        tokens.push(token);
                    }
                }
                current.clear();
            }
            delimiter = None;
        } else if delimiter.is_none() && matches!(character, '`' | '"') {
            delimiter = Some(character);
        } else if delimiter.is_some() {
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
    match token_count {
        0 => 0.0,
        1 => total_weight,
        2 | 3 => total_weight * 0.60,
        _ => total_weight * 0.40,
    }
}

#[derive(Debug, Clone, Copy)]
struct LexicalTokenMatch {
    matched_weight: f32,
    path_weight: f32,
    content_weight: f32,
    total_weight: f32,
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
        }
        if content_match {
            result.content_weight += weight;
        }
    }
    result
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

fn mandatory_tokens_match(
    mandatory_tokens: &[String],
    normalized_path: &str,
    normalized_content: &str,
) -> bool {
    mandatory_tokens.iter().all(|token| {
        normalized_path.contains(token.as_str()) || normalized_content.contains(token.as_str())
    })
}

#[cfg(test)]
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
    let path_coverage = if token_match.total_weight <= 0.0 {
        0.0
    } else {
        token_match.path_weight / token_match.total_weight
    };
    let content_coverage = if token_match.total_weight <= 0.0 {
        0.0
    } else {
        token_match.content_weight / token_match.total_weight
    };
    let role_prior = match lexical_file_role(path) {
        FileRole::Entrypoint => 1.0,
        FileRole::Source => 0.9,
        FileRole::Test => 0.55,
        FileRole::Docs => 0.50,
        FileRole::Benchmark => 0.45,
        FileRole::Generated => 0.35,
        FileRole::Vendor => 0.25,
    };
    let source_quality = match source {
        LexicalDocumentSource::SymbolDoc => 1.0,
        LexicalDocumentSource::LexicalSource => 0.9,
        LexicalDocumentSource::ComponentReport => 0.8,
    };
    (0.55 * coverage
        + 0.20 * content_coverage.clamp(0.0, 1.0)
        + 0.15 * path_coverage.clamp(0.0, 1.0)
        + 0.05 * role_prior
        + 0.05 * source_quality)
        .clamp(0.0, 1.0)
}

#[cfg(test)]
fn lexical_file_role(path: &str) -> FileRole {
    let path = Path::new(path);
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "mdx" | "rst"
            )
        })
    {
        FileRole::Docs
    } else {
        FileRole::classify_path(path)
    }
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

/// Keep the fallback aligned if the default source policy changes. Product
/// scans resolve the active cap from the pinned core publication instead.
const _: () = assert!(
    MAX_FILE_BYTES >= codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP,
    "the lexical lane must admit every file the indexer does"
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    struct CountingReader {
        remaining: usize,
        bytes_read: Arc<AtomicUsize>,
    }

    impl Read for CountingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = buffer.len().min(self.remaining);
            buffer[..count].fill(b'x');
            self.remaining -= count;
            self.bytes_read.fetch_add(count, Ordering::SeqCst);
            Ok(count)
        }
    }

    fn build(project: &Path, data: &Path, generation: &str, input: &str) -> PathBuf {
        let fingerprint = lexical_input_fingerprint(project, None).expect("fingerprint");
        build_lexical_shard(project, None, data, generation, &fingerprint, input)
            .expect("build lexical shard");
        shard_dir_for(data, generation)
    }

    fn publish_test_source_policy(
        storage: &mut Store,
        project_root: &Path,
        byte_cap: u64,
        candidates: &[codestory_workspace::OversizedSourceExclusionCandidate],
    ) {
        let publication = codestory_store::IndexPublicationRecord {
            generation: 1,
            generation_id: "test-generation".to_string(),
            run_id: "test-run".to_string(),
            mode: codestory_store::IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        storage
            .put_index_publication(&publication)
            .expect("publish test core identity");
        let identity = codestory_workspace::project_identity_v3(project_root);
        storage
            .publish_source_policy_exclusion_generation(
                &publication,
                &identity.project_id,
                &identity.workspace_id,
                SourcePolicyExclusionPolicyIdentity::new(
                    codestory_contracts::workspace::OVERSIZED_SOURCE_POLICY_VERSION,
                    byte_cap,
                    codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
                ),
                candidates,
            )
            .expect("publish test source policy");
    }

    #[test]
    fn lexical_text_reader_stops_after_cap_overflow_sentinel() {
        let bytes_read = Arc::new(AtomicUsize::new(0));
        let reader = CountingReader {
            remaining: 4_096,
            bytes_read: Arc::clone(&bytes_read),
        };

        let contents = read_lexical_text_limited(reader, 64).expect("bounded read");

        assert!(contents.is_none());
        assert_eq!(bytes_read.load(Ordering::SeqCst), 65);
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
        assert_eq!(hits[0].start_line, Some(1));
        assert_eq!(
            hits[0].source_excerpt.as_deref(),
            Some("handler handler handler")
        );
        assert_eq!(
            hits[0].target,
            Some(SearchTargetDto::FileRange {
                file_path: "src/z_strong_handler.rs".to_string(),
                start_byte: 0,
                end_byte: 7,
            })
        );
        assert!(
            search_lexical_index(&shard_a, "input-a", "project_b_handler", 8)
                .expect("isolated search")
                .is_empty()
        );
        assert!(search_lexical_index(&shard_a, "wrong-input", "handler", 8).is_err());
    }

    #[test]
    fn lexical_batch_is_serial_equivalent() {
        let project = TempDir::new().expect("project");
        std::fs::create_dir_all(project.path().join("src")).expect("src");
        std::fs::write(
            project.path().join("src/alpha_handler.rs"),
            "fn alpha_handler() { beta_router(); }",
        )
        .expect("alpha");
        std::fs::write(
            project.path().join("src/beta_router.rs"),
            "fn beta_router() {}",
        )
        .expect("beta");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "batch", "input");
        let requests = vec![
            ("alpha handler".to_string(), 4),
            ("beta router".to_string(), 2),
        ];

        let batched =
            search_lexical_index_batch_with_cancel(&shard, "input", &requests, Arc::new(|| false))
                .expect("batch search");
        let serial = requests
            .iter()
            .map(|(query, limit)| search_lexical_index(&shard, "input", query, *limit))
            .collect::<Result<Vec<_>>>()
            .expect("serial searches");
        let identity = |hits: &[LexicalHit]| {
            hits.iter()
                .map(|hit| {
                    (
                        hit.path.clone(),
                        hit.node_id.clone(),
                        hit.symbol_name.clone(),
                        hit.start_line,
                        hit.score.to_bits(),
                    )
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(
            batched
                .iter()
                .map(|hits| identity(hits))
                .collect::<Vec<_>>(),
            serial.iter().map(|hits| identity(hits)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn lexical_union_retains_an_independent_content_candidate() {
        let project = TempDir::new().expect("project");
        let path_matches = project.path().join("needle");
        std::fs::create_dir_all(&path_matches).expect("path matches");
        for index in 0..300 {
            std::fs::write(
                path_matches.join(format!("path_match_{index:03}.rs")),
                "fn unrelated() {}",
            )
            .expect("path candidate");
        }
        let content_match = project.path().join("src/content_match.rs");
        std::fs::create_dir_all(content_match.parent().expect("parent")).expect("src");
        std::fs::write(&content_match, "fn content_match() { /* needle */ }")
            .expect("content candidate");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "union", "input");

        let hits = search_lexical_index(&shard, "input", "needle", 8).expect("search");

        assert!(
            hits.iter().any(|hit| hit.path == "src/content_match.rs"),
            "the content lane must survive a large independent path lane: {hits:#?}"
        );
    }

    #[test]
    fn focused_symbol_lane_beats_scattered_whole_file_term_coverage() {
        let root = TempDir::new().expect("root");
        let generation = "focused-symbol";
        let shard = shard_dir_for(root.path(), generation);
        std::fs::create_dir_all(&shard).expect("shard");
        let documents = vec![
            LexicalDocument {
                path: "src/broad.rs".to_string(),
                content: "request dispatch output evidence indexed symbol hits retrieval shadow"
                    .to_string(),
                source: LexicalDocumentSource::LexicalSource,
                node_id: None,
                symbol_name: None,
                start_line: None,
            },
            LexicalDocument {
                path: "src/primary.rs".to_string(),
                content:
                    "fn request_dispatch_output_evidence_indexed_symbol_hits_retrieval_shadow()"
                        .to_string(),
                source: LexicalDocumentSource::SymbolDoc,
                node_id: Some("symbol:primary-request-dispatch".to_string()),
                symbol_name: Some(
                    "request_dispatch_output_evidence_indexed_symbol_hits_retrieval_shadow"
                        .to_string(),
                ),
                start_line: Some(4),
            },
            LexicalDocument {
                path: "src/output.rs".to_string(),
                content: "fn append_request_evidence_symbol() // indexed output dispatch result"
                    .to_string(),
                source: LexicalDocumentSource::SymbolDoc,
                node_id: Some("symbol:append-request-evidence".to_string()),
                symbol_name: Some("append_request_evidence_symbol".to_string()),
                start_line: Some(12),
            },
        ];
        let coverage = LexicalCoverage {
            discovered_files: 3,
            indexed_files: 3,
            ..Default::default()
        };
        let fingerprint = LexicalInputFingerprint {
            file_count: documents.len() as u32,
            hash: lexical_documents_hash(&documents, &coverage),
            coverage: coverage.clone(),
        };
        write_lexical_database(
            &shard.join(LEXICAL_INDEX_FILE),
            generation,
            "focused-symbol-input",
            &fingerprint,
            |visit| {
                for document in &documents {
                    visit(document)?;
                }
                Ok(coverage.clone())
            },
        )
        .expect("write focused lexical shard");

        let hits = search_lexical_index(
            &shard,
            "focused-symbol-input",
            "request dispatch output evidence indexed symbol hits retrieval shadow",
            3,
        )
        .expect("search");

        let focused_position = hits
            .iter()
            .position(|hit| hit.symbol_name.as_deref() == Some("append_request_evidence_symbol"))
            .expect("second-ranked focused symbol is retained");
        let broad_position = hits
            .iter()
            .position(|hit| hit.path == "src/broad.rs")
            .expect("broad source candidate is retained");
        assert!(
            focused_position < broad_position,
            "a focused symbol-document rank must survive independent sublane fusion"
        );
    }

    #[test]
    fn lexical_sublane_fusion_does_not_collapse_to_the_minimum_rank() {
        let broad_file = LexicalSublaneRanks {
            content: Some(1),
            ..Default::default()
        };
        let focused_symbol = LexicalSublaneRanks {
            symbol_document: Some(2),
            ..Default::default()
        };

        assert!(
            lexical_sublane_score(focused_symbol, crate::query_features::QueryShape::Mixed)
                > lexical_sublane_score(broad_file, crate::query_features::QueryShape::Mixed)
        );
    }

    #[test]
    fn lexical_recall_requires_quoted_entities_without_a_long_query_path_gate() {
        let project = TempDir::new().expect("project");
        std::fs::create_dir_all(project.path().join("src")).expect("src");
        std::fs::write(
            project.path().join("src/flow.rs"),
            "packet search dispatch builds ranked results through semantic graph retrieval",
        )
        .expect("complete source");
        std::fs::write(
            project.path().join("src/distractor.rs"),
            "packet search dispatch builds ranked results through semantic graph",
        )
        .expect("missing quoted entity");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "quoted", "input");

        let hits = search_lexical_index(
            &shard,
            "input",
            "explain packet search dispatch builds ranked results through semantic graph `retrieval`",
            8,
        )
        .expect("search");

        assert_eq!(
            hits.iter().map(|hit| hit.path.as_str()).collect::<Vec<_>>(),
            vec!["src/flow.rs"]
        );
    }

    #[test]
    fn lexical_long_query_requires_forty_percent_of_non_stopwords() {
        assert_eq!(required_lexical_match_weight(4, 10.0), 4.0);
        assert_eq!(required_lexical_match_weight(12, 10.0), 4.0);
    }

    #[test]
    fn whole_file_path_only_match_has_an_explicit_file_target() {
        let project = TempDir::new().expect("project");
        std::fs::create_dir_all(project.path().join("src")).expect("src");
        std::fs::write(
            project.path().join("src/request_handler.rs"),
            "fn run() {}\n",
        )
        .expect("source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "path-target", "input");

        let hits = search_lexical_index(&shard, "input", "request_handler", 1).expect("search");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start_line, None);
        assert_eq!(hits[0].source_excerpt, None);
        assert_eq!(
            hits[0].target,
            Some(SearchTargetDto::File {
                file_path: "src/request_handler.rs".to_string(),
            })
        );
    }

    #[test]
    fn sqlite_lexical_search_observes_cancellation() {
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
            search_polls.fetch_add(1, Ordering::Relaxed) > 5
        })
        .expect_err("lexical execution should observe cancellation");

        assert!(error.to_string().contains("cancelled"));
        assert!(
            polls.load(Ordering::Relaxed) > 5,
            "lexical execution must poll cancellation during query work"
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
    fn lexical_shard_survives_data_dirs_beyond_max_path() {
        // Regression companion to the embedded vector deep-root test: the
        // lexical shard is built at the same sidecar depth, so its SQLite
        // opens must also survive cache roots beyond the Windows MAX_PATH
        // cap for non-longPathAware processes.
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn deep_handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        let mut deep_data = data.path().to_path_buf();
        let segment = "max-path-regression-padding-segment".repeat(2);
        while deep_data.as_os_str().len() < 320 {
            deep_data.push(&segment);
        }
        std::fs::create_dir_all(&deep_data).expect("create deep lexical data dir");

        let shard = build(project.path(), &deep_data, "longpath", "input");
        assert!(
            shard.join(LEXICAL_INDEX_FILE).as_os_str().len() > 260,
            "regression shard no longer exceeds MAX_PATH: {}",
            shard.display()
        );
        assert_eq!(
            search_lexical_index(&shard, "input", "deep_handler", 4)
                .expect("search lexical shard under a deep data dir")
                .first()
                .map(|hit| hit.path.as_str()),
            Some("lib.rs")
        );
    }

    #[test]
    fn deep_validation_rejects_forged_rows_without_scanning_them_on_search() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn handler() {}").expect("source");
        std::fs::write(project.path().join("other.rs"), "fn unrelated() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "binding", "input");
        let index = shard.join(LEXICAL_INDEX_FILE);
        make_test_file_writable(&index);
        let connection = Connection::open(&index).expect("open writable");
        connection
            .execute(
                "UPDATE lexical_fts SET content = 'forged'
                 WHERE rowid = (SELECT id FROM lexical_documents WHERE path = 'other.rs')",
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
        assert_eq!(
            search_lexical_index(&shard, "input", "handler", 4)
                .expect("metadata-valid search")
                .first()
                .map(|hit| hit.path.as_str()),
            Some("lib.rs")
        );
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
        let coverage = scan_lexical_documents(project.path(), None, None, &mut |document| {
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
    fn storage_owned_json_does_not_change_fingerprint_or_shard_materialization() {
        let project = TempDir::new().expect("project");
        let storage_path = project.path().join("cache").join("custom-core.db");
        std::fs::create_dir_all(storage_path.parent().expect("storage parent"))
            .expect("storage parent");
        let mut storage = Store::open(&storage_path).expect("custom in-worktree store");
        publish_test_source_policy(&mut storage, project.path(), MAX_FILE_BYTES, &[]);
        drop(storage);
        let sibling = project
            .path()
            .join("cache")
            .join("custom-core.search-generations-user");
        std::fs::create_dir_all(&sibling).expect("sibling user directory");
        std::fs::write(
            sibling.join("user-config.json"),
            "{\"user_token\":\"admitted_user_json\"}\n",
        )
        .expect("user json");

        let before =
            lexical_input_fingerprint(project.path(), Some(&storage_path)).expect("fingerprint");

        let legacy = codestory_workspace::legacy_search_directory_for_storage(&storage_path);
        let generations =
            codestory_workspace::search_generation_directory_for_storage(&storage_path);
        std::fs::create_dir_all(&legacy).expect("legacy search directory");
        std::fs::create_dir_all(generations.join("generation-1"))
            .expect("search generation directory");
        std::fs::write(
            legacy.join("meta.json"),
            "{\"generated_token\":\"legacy_owned_json\"}\n",
        )
        .expect("legacy metadata");
        std::fs::write(
            generations.join("generation-1").join("meta.json"),
            "{\"generated_token\":\"generation_owned_json\"}\n",
        )
        .expect("generation metadata");

        let after = lexical_input_fingerprint(project.path(), Some(&storage_path))
            .expect("post-generation fingerprint");
        assert_eq!(after, before);

        let data = TempDir::new().expect("lexical data");
        let rebuilt = build_lexical_shard(
            project.path(),
            Some(&storage_path),
            data.path(),
            "owned-exclusions",
            &before,
            "input",
        )
        .expect("materialize shard");
        assert_eq!(rebuilt, before);
        let shard = shard_dir_for(data.path(), "owned-exclusions");
        assert!(
            search_lexical_index(&shard, "input", "generation_owned_json", 8)
                .expect("search owned token")
                .is_empty()
        );
        assert_eq!(
            search_lexical_index(&shard, "input", "admitted_user_json", 8)
                .expect("search user token")
                .first()
                .map(|hit| hit.path.as_str()),
            Some("cache/custom-core.search-generations-user/user-config.json")
        );
    }

    #[test]
    fn pinned_policy_exclusions_filter_structural_files_without_narrowing_parser_recall() {
        let project = TempDir::new().expect("project");
        let src = project.path().join("src");
        let data = project.path().join("data");
        std::fs::create_dir_all(&src).expect("src");
        std::fs::create_dir_all(&data).expect("data");

        let widened_cap = MAX_FILE_BYTES + 1_024;
        let parser_path = src.join("widened.rs");
        let mut parser_source = b"fn widened_parser_token() {}\n".to_vec();
        parser_source.resize(MAX_FILE_BYTES as usize + 1, b' ');
        std::fs::write(&parser_path, &parser_source).expect("widened parser source");

        let json_path = data.join("config.json");
        let mut json_source = br#"{"excluded_structural_token":"x"}"#.to_vec();
        json_source.resize(
            codestory_contracts::workspace::DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP as usize + 1,
            b' ',
        );
        std::fs::write(&json_path, &json_source).expect("excluded structural source");

        let storage_root = TempDir::new().expect("storage root");
        let storage_path = storage_root.path().join("core.db");
        let mut storage = Store::open(&storage_path).expect("core storage");
        publish_test_source_policy(
            &mut storage,
            project.path(),
            widened_cap,
            &[codestory_workspace::OversizedSourceExclusionCandidate {
                normalized_path: "data/config.json".to_string(),
                content_hash: format!("{:x}", Sha256::digest(&json_source)),
                observed_size: json_source.len() as u64,
                observed_unit_count: 0,
                policy_version: codestory_contracts::workspace::OVERSIZED_SOURCE_POLICY_VERSION
                    .to_string(),
                byte_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP,
                structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            }],
        );
        drop(storage);

        let mut source_paths = std::collections::BTreeSet::new();
        let coverage =
            scan_lexical_documents(project.path(), Some(&storage_path), None, &mut |document| {
                if document.source == LexicalDocumentSource::LexicalSource {
                    source_paths.insert(document.path.clone());
                }
                Ok(())
            })
            .expect("scan pinned lexical sources");

        assert_eq!(
            source_paths,
            std::collections::BTreeSet::from(["src/widened.rs".to_string()])
        );
        assert_eq!(coverage.discovered_files, 1);
        assert_eq!(coverage.indexed_files, 1);
        assert!(coverage.complete());
    }

    #[test]
    fn pinned_lexical_scan_fails_closed_without_source_policy_publication() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn source() {}\n").expect("source");
        let storage_root = TempDir::new().expect("storage root");
        let storage_path = storage_root.path().join("core.db");
        drop(Store::open(&storage_path).expect("bare core storage"));

        let error = lexical_input_fingerprint(project.path(), Some(&storage_path))
            .expect_err("missing publication must fail closed");

        assert!(
            format!("{error:#}")
                .contains("complete core publication for lexical source policy is missing")
        );
    }

    #[test]
    fn pinned_lexical_scan_rejects_a_foreign_project_policy() {
        let project_a = TempDir::new().expect("project a");
        let project_b = TempDir::new().expect("project b");
        std::fs::create_dir_all(project_a.path().join("data")).expect("project a data");
        std::fs::create_dir_all(project_b.path().join("data")).expect("project b data");
        std::fs::write(
            project_a.path().join("data/config.json"),
            br#"{"selected_project_token":"a"}"#,
        )
        .expect("project a source");

        let mut excluded_source = br#"{"foreign_project_token":"b"}"#.to_vec();
        excluded_source.resize(
            codestory_contracts::workspace::DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP as usize + 1,
            b' ',
        );
        std::fs::write(project_b.path().join("data/config.json"), &excluded_source)
            .expect("project b source");

        let identity_a = codestory_workspace::project_identity_v3(project_a.path());
        let identity_b = codestory_workspace::project_identity_v3(project_b.path());
        assert_ne!(identity_a.workspace_id, identity_b.workspace_id);

        let storage_root = TempDir::new().expect("foreign storage root");
        let storage_path = storage_root.path().join("core.db");
        let mut storage = Store::open(&storage_path).expect("foreign core storage");
        publish_test_source_policy(
            &mut storage,
            project_b.path(),
            MAX_FILE_BYTES,
            &[codestory_workspace::OversizedSourceExclusionCandidate {
                normalized_path: "data/config.json".to_string(),
                content_hash: format!("{:x}", Sha256::digest(&excluded_source)),
                observed_size: excluded_source.len() as u64,
                observed_unit_count: 0,
                policy_version: codestory_contracts::workspace::OVERSIZED_SOURCE_POLICY_VERSION
                    .to_string(),
                byte_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP,
                structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            }],
        );
        drop(storage);

        let error = lexical_input_fingerprint(project_a.path(), Some(&storage_path))
            .expect_err("foreign core policy must fail closed");

        assert!(format!("{error:#}").contains(
            "source policy exclusion manifest does not match the complete core publication"
        ));
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
        let mut reports = Vec::new();
        for corpus_documents in [1_000_usize, 10_000] {
            let generation = format!("benchmark-{corpus_documents}");
            let shard = shard_dir_for(root.path(), &generation);
            std::fs::create_dir_all(&shard).expect("shard");
            let documents = (0..corpus_documents)
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
            let index_path = shard.join(LEXICAL_INDEX_FILE);
            write_lexical_database(
                &index_path,
                &generation,
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

            let mut deep_validation_micros = Vec::new();
            for _ in 0..7 {
                let started = std::time::Instant::now();
                // Measure the uncached scan on purpose: the sealed receipt
                // would answer every repeat and report the cost of a HashMap
                // lookup instead of the corpus pass this fixture reports.
                let metadata =
                    verify_lexical_database_contents(&index_path).expect("deep validation");
                match_lexical_shard_expectations(
                    &metadata,
                    &generation,
                    "benchmark-input",
                    Some((fingerprint.file_count, fingerprint.hash.as_str())),
                )
                .expect("deep validation expectations");
                deep_validation_micros.push(started.elapsed().as_micros() as u64);
            }

            let query = format!("symbol_{:05}", corpus_documents - 1);
            let mut sqlite_micros = Vec::new();
            let mut jsonl_micros = Vec::new();
            let mut sqlite_top = None;
            let mut jsonl_top = None;
            for _ in 0..21 {
                let started = std::time::Instant::now();
                let hits = search_lexical_index(&shard, "benchmark-input", &query, 8)
                    .expect("SQLite search");
                sqlite_micros.push(started.elapsed().as_micros() as u64);
                sqlite_top = hits.first().map(|hit| hit.path.clone());

                let started = std::time::Instant::now();
                let parsed = std::fs::read_to_string(&jsonl_path)
                    .expect("read JSONL")
                    .lines()
                    .map(|line| serde_json::from_str::<LexicalDocument>(line).expect("parse row"))
                    .collect::<Vec<_>>();
                let hits = legacy_full_scan_for_measurement(&parsed, &query, 8);
                jsonl_micros.push(started.elapsed().as_micros() as u64);
                jsonl_top = hits.first().map(|hit| hit.path.clone());
            }
            deep_validation_micros.sort_unstable();
            sqlite_micros.sort_unstable();
            jsonl_micros.sort_unstable();
            assert_eq!(sqlite_top, jsonl_top);
            reports.push(serde_json::json!({
                "corpus_documents": documents.len(),
                "jsonl_bytes": std::fs::metadata(jsonl_path).expect("JSONL metadata").len(),
                "sqlite_bytes": std::fs::metadata(index_path).expect("SQLite metadata").len(),
                "deep_validation_median_us": deep_validation_micros[deep_validation_micros.len() / 2],
                "jsonl_median_query_us": jsonl_micros[jsonl_micros.len() / 2],
                "sqlite_warm_median_query_us": sqlite_micros[sqlite_micros.len() / 2],
            }));
        }
        println!("{}", serde_json::json!({ "corpora": reports }));
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
                (token_match.matched_weight >= required).then(|| {
                    let (target, matched_line, source_excerpt) =
                        if document.source == LexicalDocumentSource::LexicalSource {
                            lexical_source_target(
                                &document.path,
                                &document.content,
                                &tokens,
                                token_match.content_weight > 0.0,
                            )
                        } else {
                            (None, None, None)
                        };
                    LexicalHit {
                        path: document.path.clone(),
                        source: document.source,
                        node_id: document.node_id.clone(),
                        symbol_name: document.symbol_name.clone(),
                        start_line: document.start_line.or(matched_line),
                        target,
                        source_excerpt,
                        score: score_lexical_match(&document.path, document.source, &token_match),
                    }
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

    fn shard_modified_nanos(index: &Path) -> u64 {
        std::fs::metadata(index)
            .expect("shard metadata")
            .modified()
            .expect("shard mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("shard mtime after epoch")
            .as_nanos() as u64
    }

    fn restore_shard_modified_nanos(index: &Path, nanos: u64) {
        let file = std::fs::File::options()
            .write(true)
            .open(index)
            .expect("open shard to restore times");
        let restored = std::time::UNIX_EPOCH + std::time::Duration::from_nanos(nanos);
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(restored)
                .set_accessed(restored),
        )
        .expect("restore shard times");
    }

    #[test]
    fn repeated_probes_of_one_generation_deep_scan_the_shard_once() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "pub fn handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "receipt-reuse", "input");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        assert_eq!(
            lexical_shard_receipt_stats(data.path(), "receipt-reuse"),
            None,
            "publishing a shard must not seal a receipt; only a read path may"
        );

        // Exactly the pair a single health probe performs, then the finalize
        // fence's stricter check over the same generation.
        assert!(shard_has_lexical_index(&shard, "input"));
        lexical_shard_coverage(data.path(), "receipt-reuse", "input").expect("coverage");
        assert!(shard_matches_lexical_input(
            data.path(),
            "receipt-reuse",
            fingerprint.file_count,
            &fingerprint.hash,
            "input",
        ));

        let stats = lexical_shard_receipt_stats(data.path(), "receipt-reuse")
            .expect("a successful deep verification seals a receipt");
        assert_eq!(
            (stats.validations, stats.reuses, stats.invalidations),
            (1, 2, 0),
            "three probes of one unchanged generation must scan the FTS mirror once"
        );
    }

    #[test]
    fn a_sealed_receipt_cannot_hide_in_place_corruption_of_its_shard() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn handler() {}").expect("source");
        std::fs::write(project.path().join("other.rs"), "fn unrelated() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "receipt-seal", "input");
        let index = shard.join(LEXICAL_INDEX_FILE);

        lexical_shard_coverage(data.path(), "receipt-seal", "input")
            .expect("healthy generation verifies");
        assert!(
            lexical_shard_receipt_stats(data.path(), "receipt-seal").is_some(),
            "the healthy verdict must be sealed, or corruption below proves nothing"
        );

        // Rewrite the shard where it lies, keeping the forged text the same
        // length as the document it shadows, then put the modification time
        // back: exactly what a restore-in-place or a torn write leaves behind,
        // and invisible to a length-and-mtime check.
        let published = std::fs::metadata(&index).expect("shard metadata");
        let modified = shard_modified_nanos(&index);
        let length = published.len();
        let permissions = published.permissions();
        make_test_file_writable(&index);
        let connection = Connection::open(&index).expect("open writable");
        connection
            .execute(
                "UPDATE lexical_fts SET content = 'fn unrelated() {;'
                 WHERE rowid = (SELECT id FROM lexical_documents WHERE path = 'other.rs')",
                [],
            )
            .expect("forge FTS row");
        drop(connection);
        restore_shard_modified_nanos(&index, modified);
        std::fs::set_permissions(&index, permissions.clone()).expect("restore shard permissions");
        let corrupted = std::fs::metadata(&index).expect("shard metadata");
        assert_eq!(
            shard_modified_nanos(&index),
            modified,
            "the corruption must be invisible to a modification-time check"
        );
        assert_eq!(
            corrupted.len(),
            length,
            "the corruption must be invisible to a file-length check"
        );
        assert_eq!(
            corrupted.permissions().readonly(),
            permissions.readonly(),
            "the corruption must be invisible to a permission check"
        );

        let after = lexical_shard_coverage(data.path(), "receipt-seal", "input");

        assert!(
            after.is_err(),
            "the sealed receipt answered for bytes that no longer exist: {after:?}"
        );
        assert!(
            format!("{:#}", after.expect_err("corrupt shard"))
                .contains("FTS rows do not match immutable documents"),
            "the re-run deep scan must report the corruption it found"
        );
        assert!(!shard_has_lexical_index(&shard, "input"));
        assert_eq!(
            lexical_shard_receipt_stats(data.path(), "receipt-seal"),
            None,
            "a failed verification must leave no receipt behind"
        );
    }

    #[test]
    fn rebuilding_a_damaged_generation_in_place_reseals_it_as_healthy() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "receipt-repair", "input");
        let index = shard.join(LEXICAL_INDEX_FILE);
        lexical_shard_coverage(data.path(), "receipt-repair", "input").expect("healthy");

        make_test_file_writable(&index);
        std::fs::write(&index, b"not sqlite").expect("corrupt shard");
        assert!(lexical_shard_coverage(data.path(), "receipt-repair", "input").is_err());

        // The same generation id rebuilt in place: this is the self-repair the
        // finalize fall-through reaches.
        let _repaired = build(project.path(), data.path(), "receipt-repair", "input");

        lexical_shard_coverage(data.path(), "receipt-repair", "input")
            .expect("the repaired generation verifies again");
        assert!(shard_has_lexical_index(&shard, "input"));
    }
}
