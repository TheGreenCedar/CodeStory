use crate::candidate::{CandidateHit, CandidateSource};
use crate::config::SidecarLayout;
use crate::embeddings::InProcessEmbeddingClient;
use crate::sidecar_search::SearchExecutionContext;
use anyhow::{Context, Result, bail};
use codestory_store::FileRole;
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::Instant;

const VECTOR_INDEX_SCHEMA_VERSION: i64 = 1;
const VECTOR_INDEX_FILE: &str = "vectors.sqlite3";
type ScoredHit = (
    f32,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmbeddedVectorHealth {
    pub ready: bool,
    pub point_count: u64,
    pub latency_ms: u64,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SemanticPoint {
    pub display_name: String,
    pub node_id: String,
    pub file_path: Option<String>,
    pub file_role: Option<FileRole>,
    pub dense_reason: Option<String>,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone)]
pub(crate) struct EmbeddedVectorIndex {
    path: PathBuf,
    generation: String,
    input_hash: String,
    embedding: InProcessEmbeddingClient,
}

impl EmbeddedVectorIndex {
    pub(crate) fn open(
        layout: &SidecarLayout,
        collection: &str,
        generation: &str,
        input_hash: &str,
        embedding: InProcessEmbeddingClient,
    ) -> Self {
        Self {
            path: index_path(layout, collection),
            generation: generation.to_string(),
            input_hash: input_hash.to_string(),
            embedding,
        }
    }

    pub(crate) fn build_with_points(
        layout: &SidecarLayout,
        collection: &str,
        generation: &str,
        input_hash: &str,
        embedding_backend: &str,
        embedding_dim: usize,
        produce: impl FnOnce(&mut dyn FnMut(SemanticPoint) -> Result<()>) -> Result<()>,
    ) -> Result<u64> {
        let path = index_path(layout, collection);
        let parent = path
            .parent()
            .context("embedded vector index has no parent")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create embedded vector directory {}", parent.display()))?;
        let (temp_path, reserved) =
            codestory_workspace::atomic_file::create_unique_temp_file(&path, "vector-index")?;
        drop(reserved);
        let result = (|| {
            let point_count = write_database(
                &temp_path,
                generation,
                input_hash,
                embedding_backend,
                embedding_dim,
                produce,
            )?;
            validate_database(
                &temp_path,
                generation,
                input_hash,
                point_count,
                embedding_backend,
                embedding_dim,
            )?;
            codestory_workspace::atomic_file::publish_existing_file_atomic(&temp_path, &path)?;
            Ok(point_count)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp_path);
        }
        result
    }

    pub(crate) fn health(
        layout: &SidecarLayout,
        collection: &str,
        generation: &str,
        input_hash: &str,
        expected_points: u64,
        embedding_backend: &str,
        embedding_dim: usize,
    ) -> EmbeddedVectorHealth {
        let started = Instant::now();
        let result = validate_database(
            &index_path(layout, collection),
            generation,
            input_hash,
            expected_points,
            embedding_backend,
            embedding_dim,
        );
        EmbeddedVectorHealth {
            ready: result.is_ok(),
            point_count: result.as_ref().map_or(0, |count| *count),
            latency_ms: started.elapsed().as_millis() as u64,
            detail: result.map_or_else(
                |error| format!("embedded vector index unavailable: {error:#}"),
                |count| format!("embedded SQLite vectors ready points_count={count}"),
            ),
        }
    }

    pub(crate) fn search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        let vector = self.embedding.embed_query(query)?;
        search_database(
            &self.path,
            &self.generation,
            &self.input_hash,
            &vector,
            limit,
            || false,
        )
    }

    pub(crate) fn search_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        context.timeout(std::time::Duration::from_secs(2))?;
        let vector = self.embedding.embed_query(query)?;
        context.check_cancelled()?;
        let context = context.clone();
        search_database(
            &self.path,
            &self.generation,
            &self.input_hash,
            &vector,
            limit,
            move || context.is_cancelled(),
        )
    }
}

pub(crate) fn index_path(layout: &SidecarLayout, collection: &str) -> PathBuf {
    layout
        .semantic_data_dir
        .join("collections")
        .join(collection)
        .join(VECTOR_INDEX_FILE)
}

fn write_database(
    path: &Path,
    generation: &str,
    input_hash: &str,
    embedding_backend: &str,
    embedding_dim: usize,
    produce: impl FnOnce(&mut dyn FnMut(SemanticPoint) -> Result<()>) -> Result<()>,
) -> Result<u64> {
    let mut connection = Connection::open(path)
        .with_context(|| format!("create embedded vector index {}", path.display()))?;
    connection.execute_batch(
        "PRAGMA journal_mode=DELETE;
         PRAGMA synchronous=FULL;
         CREATE TABLE metadata (
             schema_version INTEGER NOT NULL,
             generation TEXT NOT NULL,
             input_hash TEXT NOT NULL,
             embedding_backend TEXT NOT NULL,
             embedding_dim INTEGER NOT NULL,
             point_count INTEGER NOT NULL
         );
         CREATE TABLE vectors (
             node_id TEXT PRIMARY KEY NOT NULL,
             display_name TEXT NOT NULL,
             file_path TEXT,
             file_role TEXT,
             dense_reason TEXT,
             vector BLOB NOT NULL
         ) WITHOUT ROWID;",
    )?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut point_count = 0_u64;
    {
        let mut insert = transaction.prepare(
            "INSERT INTO vectors (
                 node_id, display_name, file_path, file_role, dense_reason, vector
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut visit = |point: SemanticPoint| -> Result<()> {
            if point.vector.len() != embedding_dim {
                bail!(
                    "embedded vector dimension mismatch for node {}: expected {embedding_dim}, found {}",
                    point.node_id,
                    point.vector.len()
                );
            }
            insert.execute(params![
                point.node_id,
                point.display_name,
                point.file_path,
                point.file_role.map(|role| role.as_str()),
                point.dense_reason,
                vector_bytes(&point.vector),
            ])?;
            point_count = point_count.saturating_add(1);
            Ok(())
        };
        produce(&mut visit)?;
    }
    transaction.execute(
        "INSERT INTO metadata VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            VECTOR_INDEX_SCHEMA_VERSION,
            generation,
            input_hash,
            embedding_backend,
            embedding_dim as i64,
            point_count as i64,
        ],
    )?;
    transaction.commit()?;
    connection.execute_batch("PRAGMA optimize;")?;
    drop(connection);
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("open embedded vector index for sync {}", path.display()))?
        .sync_all()
        .with_context(|| format!("sync embedded vector index {}", path.display()))?;
    Ok(point_count)
}

fn validate_database(
    path: &Path,
    generation: &str,
    input_hash: &str,
    expected_points: u64,
    embedding_backend: &str,
    embedding_dim: usize,
) -> Result<u64> {
    let connection = open_read_only(path)?;
    let metadata = connection.query_row(
        "SELECT schema_version, generation, input_hash, embedding_backend,
                embedding_dim, point_count
         FROM metadata",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        },
    )?;
    if metadata.0 != VECTOR_INDEX_SCHEMA_VERSION
        || metadata.1 != generation
        || metadata.2 != input_hash
        || metadata.3 != embedding_backend
        || metadata.4 != embedding_dim as i64
        || metadata.5 < 0
        || metadata.5 as u64 != expected_points
    {
        bail!("embedded vector metadata does not match the published generation");
    }
    let actual: i64 = connection.query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))?;
    if actual < 0 || actual as u64 != expected_points {
        bail!(
            "embedded vector count mismatch: expected {expected_points}, found {}",
            actual.max(0)
        );
    }
    Ok(actual as u64)
}

fn search_database(
    path: &Path,
    generation: &str,
    input_hash: &str,
    query: &[f32],
    limit: usize,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<CandidateHit>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let connection = open_read_only(path)?;
    let (stored_generation, stored_hash, stored_dim): (String, String, i64) = connection
        .query_row(
            "SELECT generation, input_hash, embedding_dim FROM metadata",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
    if stored_generation != generation
        || stored_hash != input_hash
        || stored_dim != query.len() as i64
    {
        bail!("embedded vector index publication identity changed");
    }
    let mut statement = connection.prepare(
        "SELECT node_id, display_name, file_path, file_role, dense_reason, vector FROM vectors",
    )?;
    let mut rows = statement.query([])?;
    let mut scored = Vec::with_capacity(limit);
    let query_norm = query.iter().map(|value| value * value).sum::<f32>().sqrt();
    while let Some(row) = rows.next()? {
        if cancelled() {
            bail!("embedded vector search cancelled");
        }
        let bytes: Vec<u8> = row.get(5)?;
        let score = cosine_similarity_bytes(query, query_norm, &bytes)?;
        let candidate = (
            score,
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
        );
        if scored.len() < limit {
            scored.push(candidate);
            continue;
        }
        let (worst_index, worst) = scored
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| compare_scored_hits(left, right))
            .expect("non-empty bounded score set");
        if compare_scored_hits(&candidate, worst) == Ordering::Less {
            scored[worst_index] = candidate;
        }
    }
    scored.sort_unstable_by(compare_scored_hits);
    Ok(scored
        .into_iter()
        .map(
            |(score, node_id, display_name, file_path, file_role, dense_reason)| {
                let file_path = file_path.unwrap_or_else(|| display_name.clone());
                let mut hit = CandidateHit::with_source(
                    file_path,
                    Some(display_name),
                    score,
                    CandidateSource::Semantic,
                );
                hit.node_id = Some(node_id);
                hit.file_role = file_role
                    .as_deref()
                    .map(codestory_store::FileRole::from_db_value);
                hit.add_provenance(if dense_reason.as_deref() == Some("component_report") {
                    "component_report"
                } else {
                    "dense_anchor"
                });
                hit
            },
        )
        .collect())
}

fn compare_scored_hits(left: &ScoredHit, right: &ScoredHit) -> Ordering {
    right
        .0
        .total_cmp(&left.0)
        .then_with(|| left.1.cmp(&right.1))
}

fn open_read_only(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open embedded vector index {}", path.display()))
}

fn vector_bytes(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(vector));
    for value in vector {
        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    bytes
}

fn cosine_similarity_bytes(query: &[f32], query_norm: f32, bytes: &[u8]) -> Result<f32> {
    if bytes.len() != std::mem::size_of_val(query) {
        bail!("embedded vector blob has an invalid width");
    }
    let mut dot = 0.0_f32;
    let mut vector_norm = 0.0_f32;
    for (query_value, chunk) in query.iter().zip(bytes.chunks_exact(4)) {
        let value = f32::from_bits(u32::from_le_bytes(chunk.try_into().expect("four bytes")));
        dot += query_value * value;
        vector_norm += value * value;
    }
    let denominator = query_norm * vector_norm.sqrt();
    Ok(if denominator > f32::EPSILON {
        dot / denominator
    } else {
        0.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SidecarLayout;
    use codestory_store::FileRole;
    use tempfile::tempdir;

    fn layout(root: &Path) -> SidecarLayout {
        SidecarLayout {
            lexical_data_dir: root.join("lexical"),
            semantic_data_dir: root.join("semantic"),
            scip_artifacts_root: root.join("scip"),
            state_file: root.join("state.json"),
        }
    }

    fn point(node_id: &str, vector: Vec<f32>) -> SemanticPoint {
        SemanticPoint {
            display_name: format!("symbol_{node_id}"),
            node_id: node_id.into(),
            file_path: Some(format!("src/{node_id}.rs")),
            file_role: Some(FileRole::Source),
            dense_reason: Some("public_api".into()),
            vector,
        }
    }

    #[test]
    fn immutable_index_is_generation_bound_and_ranks_cosine_similarity() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let points = [point("1", vec![1.0, 0.0]), point("2", vec![0.0, 1.0])];
        EmbeddedVectorIndex::build_with_points(
            &layout,
            "codestory_test_deadbeefdeadbeef",
            "test-deadbeefdeadbeef",
            "input",
            "backend",
            2,
            |visit| {
                for point in points {
                    visit(point)?;
                }
                Ok(())
            },
        )
        .expect("build");

        let path = index_path(&layout, "codestory_test_deadbeefdeadbeef");
        let hits = search_database(
            &path,
            "test-deadbeefdeadbeef",
            "input",
            &[0.9, 0.1],
            1,
            || false,
        )
        .expect("search");
        assert_eq!(hits[0].node_id.as_deref(), Some("1"));
        assert!(
            !EmbeddedVectorIndex::health(
                &layout,
                "codestory_test_deadbeefdeadbeef",
                "other-generation",
                "input",
                2,
                "backend",
                2,
            )
            .ready
        );
    }

    #[test]
    #[ignore = "measurement lane; run with --release --ignored --nocapture"]
    fn embedded_vector_scan_measurement() {
        const DIMENSION: usize = 768;
        const SEARCH_RUNS: usize = 10;

        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let mut measurements = Vec::new();
        for point_count in [1_000_usize, 10_000, 25_000] {
            let collection = format!("codestory_measurement_{point_count}");
            let build_started = Instant::now();
            EmbeddedVectorIndex::build_with_points(
                &layout,
                &collection,
                "measurement-generation",
                "measurement-input",
                crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
                DIMENSION,
                |visit| {
                    for index in 0..point_count {
                        let mut vector = vec![0.0_f32; DIMENSION];
                        vector[index % DIMENSION] = 1.0;
                        vector[(index.wrapping_mul(31) + 7) % DIMENSION] = 0.5;
                        visit(point(&index.to_string(), vector))?;
                    }
                    Ok(())
                },
            )
            .expect("build measurement index");
            let build_ms = build_started.elapsed().as_millis();

            let mut query = vec![0.0_f32; DIMENSION];
            query[0] = 1.0;
            query[7] = 0.5;
            let path = index_path(&layout, &collection);
            let mut search_us = Vec::with_capacity(SEARCH_RUNS);
            for _ in 0..SEARCH_RUNS {
                let started = Instant::now();
                let hits = search_database(
                    &path,
                    "measurement-generation",
                    "measurement-input",
                    &query,
                    20,
                    || false,
                )
                .expect("measure search");
                assert_eq!(hits.len(), 20);
                search_us.push(started.elapsed().as_micros());
            }
            search_us.sort_unstable();
            measurements.push(serde_json::json!({
                "points": point_count,
                "dimension": DIMENSION,
                "database_bytes": std::fs::metadata(&path).expect("index metadata").len(),
                "build_ms": build_ms,
                "warm_search_p50_us": search_us[SEARCH_RUNS / 2],
                "warm_search_p95_us": search_us[SEARCH_RUNS - 1],
            }));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&measurements).expect("serialize measurements")
        );
    }
}
