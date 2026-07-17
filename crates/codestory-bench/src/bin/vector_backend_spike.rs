//! Evidence-only runner for #1202. It never changes CodeStory's production
//! retrieval route; it reads an immutable published `vectors.sqlite3` input.

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use codestory_retrieval::{InProcessEmbeddingClient, SidecarRuntimeConfig};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

const DIMENSIONS: usize = 768;
const TOP_K: usize = 20;
const INCREMENTAL_COUNT: usize = 100;
const SELECTION_SEED: &str = "codestory-1202-vector-spike-v1";
const VECTOR_DIGEST_DOMAIN: &[u8] = b"codestory-vector-digest-v1\0";

#[derive(Parser)]
#[command(about = "Evidence-only sqlite-vec vs USearch runner for CodeStory issue #1202")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Bind a vector-free source-truth catalog to one immutable production publication.
    Prepare {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Run the production embedded cosine scan over the exact frozen subset.
    Oracle {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        fixture: PathBuf,
        #[arg(long)]
        count: usize,
    },
    /// Build, load, query, increment, and fault-probe one candidate in one fresh process.
    Candidate {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        fixture: PathBuf,
        #[arg(long)]
        oracle: PathBuf,
        #[arg(long)]
        count: usize,
        #[arg(long, value_enum)]
        backend: Backend,
        #[arg(long)]
        workdir: PathBuf,
        #[arg(long, default_value_t = 5)]
        warmups: usize,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum Backend {
    SqliteVec,
    Usearch,
}

impl Backend {
    fn label(self) -> &'static str {
        match self {
            Self::SqliteVec => "sqlite-vec",
            Self::Usearch => "usearch",
        }
    }

    fn version(self) -> &'static str {
        match self {
            Self::SqliteVec => "0.1.9",
            Self::Usearch => "2.26.0",
        }
    }

    fn index_name(self) -> &'static str {
        match self {
            Self::SqliteVec => "index.sqlite3",
            Self::Usearch => "index.usearch",
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct SourceAttestation {
    database_sha256: String,
    schema_version: i64,
    generation: String,
    input_hash: String,
    embedding_backend: String,
    embedding_dim: usize,
    point_count: usize,
    producer_identity: String,
    evidence_contract_identity: String,
    vector_digest: String,
}

#[derive(Deserialize)]
struct Catalog {
    schema_version: u32,
    corpus_commit: String,
    queries: Vec<CatalogQuery>,
}

#[derive(Deserialize)]
struct CatalogQuery {
    id: String,
    kind: String,
    text: String,
    file_path: String,
    symbol: String,
}

#[derive(Clone, Deserialize, Serialize)]
struct FrozenQuery {
    id: String,
    kind: String,
    text: String,
    expected_node_id: String,
    expected_document_hash: String,
    vector: Vec<f32>,
}

#[derive(Deserialize, Serialize)]
struct Fixture {
    schema_version: u32,
    source: SourceAttestation,
    source_database_path: String,
    corpus_commit: String,
    catalog_sha256: String,
    selection_seed: String,
    selected_node_ids: Vec<String>,
    incremental_node_ids: Vec<String>,
    queries: Vec<FrozenQuery>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Oracle {
    schema_version: u32,
    source_database_sha256: String,
    fixture_sha256: String,
    count: usize,
    cold_query_ms: f64,
    warm_query_p50_ms: f64,
    warm_query_p95_ms: f64,
    top_k_ordinals: Vec<Vec<u64>>,
    source_truth_hit_at_20: f64,
}

#[derive(Serialize)]
struct CandidateResult {
    schema_version: u32,
    generated_at_unix_seconds: u64,
    backend: &'static str,
    backend_version: &'static str,
    count: usize,
    source_database_sha256: String,
    fixture_sha256: String,
    build_ms: f64,
    load_ms: f64,
    cold_query_ms: f64,
    warm_query_p50_ms: f64,
    warm_query_p95_ms: f64,
    rss_bytes_after_warm_queries: u64,
    disk_bytes: u64,
    incremental_reuse_ms: f64,
    ann_recall_at_20: f64,
    source_truth_hit_at_20: f64,
    concurrent_reader_consistency: bool,
    pinned_old_reader_after_publication: bool,
    new_current_reader_observed_incremental: bool,
    old_generation_unchanged: bool,
    corrupt_candidate_rejected: bool,
    failed_candidate_preserved_current_pointer: bool,
    rollback_pointer_readable: bool,
    referenced_generation_tamper_rejected: bool,
    pinned_reader_after_referenced_tamper: bool,
}

#[derive(Serialize, Deserialize)]
struct GenerationManifest {
    schema_version: u32,
    backend: String,
    backend_version: String,
    source_database_sha256: String,
    fixture_sha256: String,
    count: usize,
    index_sha256: String,
}

#[derive(Serialize, Deserialize)]
struct Pointer {
    schema_version: u32,
    current: String,
    rollback: Option<String>,
}

#[derive(Clone, Copy)]
struct FixtureBinding<'a> {
    source_database_sha256: &'a str,
    fixture_sha256: &'a str,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Prepare {
            source,
            catalog,
            output,
        } => prepare(&source, &catalog, &output),
        Command::Oracle {
            source,
            fixture,
            count,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&run_oracle(&source, &fixture, count)?)?
            );
            Ok(())
        }
        Command::Candidate {
            source,
            fixture,
            oracle,
            count,
            backend,
            workdir,
            warmups,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&run_candidate(
                    &source, &fixture, &oracle, count, backend, &workdir, warmups
                )?)?
            );
            Ok(())
        }
    }
}

fn prepare(source: &Path, catalog_path: &Path, output: &Path) -> Result<()> {
    ensure!(
        output.parent().is_some(),
        "fixture output needs a parent directory"
    );
    ensure!(
        !output.exists(),
        "fixture output already exists: {}",
        output.display()
    );
    let source = source
        .canonicalize()
        .context("canonicalize source database")?;
    reject_sidecars(&source)?;
    let attestation = attest_source(&source)?;
    ensure!(
        attestation.point_count > 100_000 + INCREMENTAL_COUNT,
        "production publication needs more than 100100 dense anchors"
    );
    let catalog_bytes = fs::read(catalog_path)
        .with_context(|| format!("read catalog {}", catalog_path.display()))?;
    let catalog: Catalog =
        serde_json::from_slice(&catalog_bytes).context("parse source-truth catalog")?;
    ensure!(
        catalog.schema_version == 1,
        "unsupported source-truth catalog schema"
    );
    ensure!(
        catalog.queries.len() == 30,
        "catalog needs exactly 30 queries"
    );
    let identities = resolve_catalog(&source, &catalog)?;
    let all_ids = list_node_ids(&source)?;
    let forced = identities
        .iter()
        .map(|query| query.expected_node_id.clone())
        .collect::<HashSet<_>>();
    let mut selected = forced.into_iter().collect::<Vec<_>>();
    selected.sort();
    let mut ranked = all_ids
        .into_iter()
        .filter(|id| !selected.contains(id))
        .collect::<Vec<_>>();
    ranked.sort_by_key(|id| selection_key(id));
    let remaining = 100_000 + INCREMENTAL_COUNT - selected.len();
    selected.extend(ranked.into_iter().take(remaining));
    ensure!(
        selected.len() == 100_000 + INCREMENTAL_COUNT,
        "not enough real anchors for nested fixture"
    );
    let incremental_node_ids = selected.split_off(100_000);
    let runtime = SidecarRuntimeConfig::local();
    let embedder = InProcessEmbeddingClient::new(&runtime);
    let queries = identities
        .into_iter()
        .map(|mut query| {
            query.vector = embedder.embed_query(&query.text)?;
            validate_vector(&query.id, &query.vector)?;
            Ok(query)
        })
        .collect::<Result<Vec<_>>>()?;
    let fixture = Fixture {
        schema_version: 1,
        source: attestation,
        source_database_path: source.display().to_string(),
        corpus_commit: catalog.corpus_commit,
        catalog_sha256: sha256_bytes(&catalog_bytes),
        selection_seed: SELECTION_SEED.into(),
        selected_node_ids: selected,
        incremental_node_ids,
        queries,
    };
    atomic_write_json(output, &fixture)
}

fn run_oracle(source: &Path, fixture_path: &Path, count: usize) -> Result<Oracle> {
    let (source, fixture, fixture_sha) = verified_fixture(source, fixture_path)?;
    let selected = selected_ordinals(&fixture, count)?;
    let started = Instant::now();
    let top_k_ordinals = exact_scan(&source, &selected, &fixture.queries)?;
    let cold_query_ms = elapsed_ms(started);
    let mut warm = Vec::new();
    for query in &fixture.queries {
        let started = Instant::now();
        let _ = exact_scan(&source, &selected, std::slice::from_ref(query))?;
        warm.push(elapsed_ms(started));
    }
    let source_truth_hit_at_20 = source_truth_hit(&fixture.queries, &top_k_ordinals, &selected);
    Ok(Oracle {
        schema_version: 1,
        source_database_sha256: fixture.source.database_sha256.clone(),
        fixture_sha256: fixture_sha,
        count,
        cold_query_ms,
        warm_query_p50_ms: percentile(&mut warm.clone(), 0.50),
        warm_query_p95_ms: percentile(&mut warm, 0.95),
        top_k_ordinals,
        source_truth_hit_at_20,
    })
}

fn run_candidate(
    source: &Path,
    fixture_path: &Path,
    oracle_path: &Path,
    count: usize,
    backend: Backend,
    workdir: &Path,
    warmups: usize,
) -> Result<CandidateResult> {
    ensure!(
        !workdir.exists(),
        "candidate workdir must be new: {}",
        workdir.display()
    );
    let (source, fixture, fixture_sha) = verified_fixture(source, fixture_path)?;
    let source_database_sha256 = fixture.source.database_sha256.clone();
    let binding = FixtureBinding {
        source_database_sha256: &source_database_sha256,
        fixture_sha256: &fixture_sha,
    };
    let oracle: Oracle = serde_json::from_slice(&fs::read(oracle_path)?)?;
    ensure!(
        oracle.count == count
            && oracle.fixture_sha256 == fixture_sha
            && oracle.source_database_sha256 == fixture.source.database_sha256,
        "oracle does not bind this candidate input"
    );
    let selected = selected_ordinals(&fixture, count)?;
    fs::create_dir_all(workdir.join("generations"))?;
    let generation_one = workdir.join("generations").join("generation-1");
    let started = Instant::now();
    build_generation(backend, &source, &selected, &generation_one, binding)?;
    publish_pointer(workdir, "generation-1", None)?;
    let build_ms = elapsed_ms(started);
    let old_hash = directory_hash(&generation_one)?;
    let disk_bytes = directory_size(&generation_one)?;
    let started = Instant::now();
    let pinned_old = open_generation(backend, workdir, "generation-1", binding)?;
    let load_ms = elapsed_ms(started);
    let cold_started = Instant::now();
    let cold = pinned_old.search(&fixture.queries[0].vector)?;
    let cold_query_ms = elapsed_ms(cold_started);
    for query in &fixture.queries {
        for _ in 0..warmups {
            let _ = pinned_old.search(&query.vector)?;
        }
    }
    let mut warm = Vec::new();
    let mut candidate_hits = Vec::new();
    for query in &fixture.queries {
        let started = Instant::now();
        let hits = pinned_old.search(&query.vector)?;
        warm.push(elapsed_ms(started));
        candidate_hits.push(hits);
    }
    let rss_bytes_after_warm_queries = current_rss_bytes();
    let ann_recall_at_20 = recall(&candidate_hits, &oracle.top_k_ordinals);
    let source_truth_hit_at_20 = source_truth_hit(&fixture.queries, &candidate_hits, &selected);
    let concurrent_reader_consistency = concurrent_reader_consistency(
        backend,
        workdir,
        "generation-1",
        binding,
        &fixture.queries[0].vector,
        &cold,
    )?;
    let generation_two = workdir.join("generations").join("generation-2");
    let started = Instant::now();
    build_incremental_generation(
        backend,
        &generation_one,
        &generation_two,
        count,
        &source,
        &fixture.incremental_node_ids,
        binding,
    )?;
    publish_pointer(workdir, "generation-2", Some("generation-1"))?;
    let incremental_reuse_ms = elapsed_ms(started);
    let pinned_old_reader_after_publication =
        pinned_old.search(&fixture.queries[0].vector)? == cold;
    let current = open_current_generation(backend, workdir, binding)?;
    let new_current_reader_observed_incremental = current.count() == count + INCREMENTAL_COUNT;
    let old_generation_unchanged = directory_hash(&generation_one)? == old_hash;
    let pointer_before_corrupt = fs::read(workdir.join("publication.json"))?;
    let corrupt = workdir.join("generations").join("generation-corrupt");
    fs::create_dir_all(&corrupt)?;
    fs::write(corrupt.join(backend.index_name()), b"not an index")?;
    let corrupt_candidate_rejected = validate_generation(&corrupt, backend, binding).is_err();
    let failed_candidate_preserved_current_pointer =
        fs::read(workdir.join("publication.json"))? == pointer_before_corrupt;
    publish_pointer(workdir, "generation-1", Some("generation-2"))?;
    let rollback_pointer_readable =
        open_current_generation(backend, workdir, binding)?.count() == count;
    tamper_file(&generation_one.join(backend.index_name()))?;
    let referenced_generation_tamper_rejected =
        open_current_generation(backend, workdir, binding).is_err();
    let pinned_reader_after_referenced_tamper =
        pinned_old.search(&fixture.queries[0].vector)? == cold;
    Ok(CandidateResult {
        schema_version: 1,
        generated_at_unix_seconds: now_unix_seconds(),
        backend: backend.label(),
        backend_version: backend.version(),
        count,
        source_database_sha256,
        fixture_sha256: fixture_sha,
        build_ms,
        load_ms,
        cold_query_ms,
        warm_query_p50_ms: percentile(&mut warm.clone(), 0.50),
        warm_query_p95_ms: percentile(&mut warm, 0.95),
        rss_bytes_after_warm_queries,
        disk_bytes,
        incremental_reuse_ms,
        ann_recall_at_20,
        source_truth_hit_at_20,
        concurrent_reader_consistency,
        pinned_old_reader_after_publication,
        new_current_reader_observed_incremental,
        old_generation_unchanged,
        corrupt_candidate_rejected,
        failed_candidate_preserved_current_pointer,
        rollback_pointer_readable,
        referenced_generation_tamper_rejected,
        pinned_reader_after_referenced_tamper,
    })
}

fn verified_fixture(source: &Path, fixture_path: &Path) -> Result<(PathBuf, Fixture, String)> {
    let source = source.canonicalize()?;
    reject_sidecars(&source)?;
    let fixture_bytes = fs::read(fixture_path)?;
    let fixture: Fixture = serde_json::from_slice(&fixture_bytes)?;
    ensure!(fixture.schema_version == 1, "unsupported fixture schema");
    ensure!(
        fixture.source.embedding_dim == DIMENSIONS && fixture.queries.len() == 30,
        "fixture does not meet the declared profile"
    );
    ensure!(
        fixture.selected_node_ids.len() == 100_000
            && fixture.incremental_node_ids.len() == INCREMENTAL_COUNT,
        "fixture does not contain the declared nested real-anchor input"
    );
    let attestation = attest_source(&source)?;
    ensure!(
        attestation.database_sha256 == fixture.source.database_sha256
            && attestation.vector_digest == fixture.source.vector_digest
            && attestation.generation == fixture.source.generation,
        "source publication no longer matches fixture attestation"
    );
    for query in &fixture.queries {
        validate_vector(&query.id, &query.vector)?;
    }
    Ok((source, fixture, sha256_bytes(&fixture_bytes)))
}

fn resolve_catalog(source: &Path, catalog: &Catalog) -> Result<Vec<FrozenQuery>> {
    let conn = open_read_only(source)?;
    let mut output = Vec::new();
    for query in &catalog.queries {
        ensure!(
            !query.id.is_empty() && !query.text.is_empty(),
            "catalog query identity and text are required"
        );
        let mut statement = conn.prepare(
            "SELECT node_id, document_hash FROM vectors WHERE file_path = ?1 AND display_name = ?2",
        )?;
        let matches = statement
            .query_map(params![query.file_path, query.symbol], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ensure!(
            matches.len() == 1,
            "catalog query {} must resolve exactly one source-truth vector for {}:{}; found {}",
            query.id,
            query.file_path,
            query.symbol,
            matches.len()
        );
        output.push(FrozenQuery {
            id: query.id.clone(),
            kind: query.kind.clone(),
            text: query.text.clone(),
            expected_node_id: matches[0].0.clone(),
            expected_document_hash: matches[0].1.clone(),
            vector: Vec::new(),
        });
    }
    Ok(output)
}

fn list_node_ids(source: &Path) -> Result<Vec<String>> {
    let conn = open_read_only(source)?;
    let mut statement = conn.prepare("SELECT node_id FROM vectors ORDER BY node_id")?;
    Ok(statement
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}

fn selected_ordinals(fixture: &Fixture, count: usize) -> Result<HashMap<String, u64>> {
    ensure!(
        [1_000, 10_000, 25_000, 100_000].contains(&count),
        "undeclared vector count {count}"
    );
    let mut output = HashMap::with_capacity(count);
    for (index, id) in fixture.selected_node_ids.iter().take(count).enumerate() {
        output.insert(id.clone(), index as u64 + 1);
    }
    for query in &fixture.queries {
        ensure!(
            output.contains_key(&query.expected_node_id),
            "selected {} workload omits catalog target {}",
            count,
            query.id
        );
    }
    Ok(output)
}

fn exact_scan(
    source: &Path,
    selected: &HashMap<String, u64>,
    queries: &[FrozenQuery],
) -> Result<Vec<Vec<u64>>> {
    let conn = open_read_only(source)?;
    let mut top = vec![Vec::<(f32, u64)>::new(); queries.len()];
    let mut statement = conn.prepare("SELECT node_id, vector FROM vectors ORDER BY node_id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let Some(ordinal) = selected.get(&id).copied() else {
            continue;
        };
        let vector = decode_vector(&row.get::<_, Vec<u8>>(1)?)?;
        for (index, query) in queries.iter().enumerate() {
            insert_scored(&mut top[index], (cosine(&query.vector, &vector)?, ordinal));
        }
    }
    Ok(top
        .into_iter()
        .map(|hits| hits.into_iter().map(|(_, ordinal)| ordinal).collect())
        .collect())
}

fn build_generation(
    backend: Backend,
    source: &Path,
    selected: &HashMap<String, u64>,
    dir: &Path,
    binding: FixtureBinding<'_>,
) -> Result<()> {
    fs::create_dir_all(dir)?;
    let index = dir.join(backend.index_name());
    match backend {
        Backend::SqliteVec => build_sqlite_vec(&index, source, selected)?,
        Backend::Usearch => build_usearch(&index, source, selected)?,
    }
    write_manifest(dir, backend, selected.len(), binding)
}

fn build_incremental_generation(
    backend: Backend,
    base: &Path,
    dir: &Path,
    base_count: usize,
    source: &Path,
    tail: &[String],
    binding: FixtureBinding<'_>,
) -> Result<()> {
    fs::create_dir_all(dir)?;
    let source_index = base.join(backend.index_name());
    let index = dir.join(backend.index_name());
    match backend {
        Backend::SqliteVec => {
            fs::copy(&source_index, &index)?;
            let conn = Connection::open(&index)?;
            let mut added = 0usize;
            stream_vectors(
                source,
                &tail
                    .iter()
                    .enumerate()
                    .map(|(i, id)| (id.clone(), base_count as u64 + i as u64 + 1))
                    .collect(),
                |ordinal, vector| {
                    conn.execute(
                        "INSERT INTO vectors(rowid, embedding) VALUES (?1, ?2)",
                        params![ordinal as i64, vector_bytes(&vector)],
                    )?;
                    added += 1;
                    Ok(())
                },
            )?;
            ensure!(
                added == tail.len(),
                "incremental sqlite-vec source rows missing"
            );
            conn.execute_batch("PRAGMA optimize;")?;
            sync_file(&index)?;
        }
        Backend::Usearch => {
            let index_handle = Index::restore(&source_index.to_string_lossy())?;
            let tail_map = tail
                .iter()
                .enumerate()
                .map(|(i, id)| (id.clone(), base_count as u64 + i as u64 + 1))
                .collect::<HashMap<_, _>>();
            let mut added = 0usize;
            stream_vectors(source, &tail_map, |ordinal, vector| {
                index_handle.add(ordinal, &vector)?;
                added += 1;
                Ok(())
            })?;
            ensure!(
                added == tail.len(),
                "incremental USearch source rows missing"
            );
            index_handle.save(&index.to_string_lossy())?;
            sync_file(&index)?;
        }
    }
    write_manifest(dir, backend, base_count + tail.len(), binding)
}

fn build_sqlite_vec(index: &Path, source: &Path, selected: &HashMap<String, u64>) -> Result<()> {
    register_sqlite_vec()?;
    let mut conn = Connection::open(index)?;
    conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; CREATE VIRTUAL TABLE vectors USING vec0(embedding float[768] distance_metric=cosine);")?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut insert = tx.prepare("INSERT INTO vectors(rowid, embedding) VALUES (?1, ?2)")?;
    let mut added = 0usize;
    stream_vectors(source, selected, |ordinal, vector| {
        insert.execute(params![ordinal as i64, vector_bytes(&vector)])?;
        added += 1;
        Ok(())
    })?;
    ensure!(added == selected.len(), "sqlite-vec source rows missing");
    drop(insert);
    tx.commit()?;
    conn.execute_batch("PRAGMA optimize;")?;
    drop(conn);
    sync_file(index)
}

fn build_usearch(index: &Path, source: &Path, selected: &HashMap<String, u64>) -> Result<()> {
    let options = IndexOptions {
        dimensions: DIMENSIONS,
        metric: MetricKind::Cos,
        quantization: ScalarKind::F32,
        ..Default::default()
    };
    let index_handle = Index::new(&options)?;
    index_handle.reserve(selected.len())?;
    let mut added = 0usize;
    stream_vectors(source, selected, |ordinal, vector| {
        index_handle.add(ordinal, &vector)?;
        added += 1;
        Ok(())
    })?;
    ensure!(added == selected.len(), "USearch source rows missing");
    index_handle.save(&index.to_string_lossy())?;
    sync_file(index)
}

fn stream_vectors(
    source: &Path,
    selection: &HashMap<String, u64>,
    mut visit: impl FnMut(u64, Vec<f32>) -> Result<()>,
) -> Result<()> {
    let conn = open_read_only(source)?;
    let mut statement = conn.prepare("SELECT node_id, vector FROM vectors ORDER BY node_id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        if let Some(ordinal) = selection.get(&id).copied() {
            visit(ordinal, decode_vector(&row.get::<_, Vec<u8>>(1)?)?)?;
        }
    }
    Ok(())
}

enum Handle {
    Sqlite(Connection),
    Usearch(Index),
}

impl Handle {
    fn search(&self, query: &[f32]) -> Result<Vec<u64>> {
        match self {
            Self::Sqlite(conn) => {
                let mut statement = conn.prepare(
                    "SELECT rowid FROM vectors WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
                )?;
                Ok(statement
                    .query_map(params![vector_bytes(query), TOP_K as i64], |row| {
                        row.get::<_, i64>(0).map(|value| value as u64)
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?)
            }
            Self::Usearch(index) => Ok(index.search(query, TOP_K)?.keys),
        }
    }
    fn count(&self) -> usize {
        match self {
            Self::Sqlite(conn) => conn
                .query_row("SELECT count(*) FROM vectors", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap_or_default()
                .max(0) as usize,
            Self::Usearch(index) => index.size(),
        }
    }
}

fn open_generation(
    backend: Backend,
    root: &Path,
    generation: &str,
    binding: FixtureBinding<'_>,
) -> Result<Handle> {
    let dir = root.join("generations").join(generation);
    validate_generation(&dir, backend, binding)?;
    let index = dir.join(backend.index_name());
    match backend {
        Backend::SqliteVec => {
            register_sqlite_vec()?;
            Ok(Handle::Sqlite(Connection::open_with_flags(
                index,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?))
        }
        Backend::Usearch => Ok(Handle::Usearch(Index::restore(&index.to_string_lossy())?)),
    }
}

fn open_current_generation(
    backend: Backend,
    root: &Path,
    binding: FixtureBinding<'_>,
) -> Result<Handle> {
    let pointer: Pointer = serde_json::from_slice(&fs::read(root.join("publication.json"))?)?;
    ensure!(
        pointer.schema_version == 1,
        "unsupported publication pointer"
    );
    open_generation(backend, root, &pointer.current, binding)
}

fn write_manifest(
    dir: &Path,
    backend: Backend,
    count: usize,
    binding: FixtureBinding<'_>,
) -> Result<()> {
    let index = dir.join(backend.index_name());
    sync_file(&index)?;
    atomic_write_json(
        &dir.join("manifest.json"),
        &GenerationManifest {
            schema_version: 1,
            backend: backend.label().into(),
            backend_version: backend.version().into(),
            source_database_sha256: binding.source_database_sha256.into(),
            fixture_sha256: binding.fixture_sha256.into(),
            count,
            index_sha256: sha256_file(&index)?,
        },
    )
}

fn validate_generation(dir: &Path, backend: Backend, binding: FixtureBinding<'_>) -> Result<()> {
    let manifest: GenerationManifest =
        serde_json::from_slice(&fs::read(dir.join("manifest.json"))?)
            .context("read generation manifest")?;
    ensure!(
        manifest.schema_version == 1
            && manifest.backend == backend.label()
            && manifest.source_database_sha256 == binding.source_database_sha256
            && manifest.fixture_sha256 == binding.fixture_sha256,
        "generation manifest identity mismatch"
    );
    ensure!(
        sha256_file(&dir.join(backend.index_name()))? == manifest.index_sha256,
        "generation index digest mismatch"
    );
    Ok(())
}

fn publish_pointer(root: &Path, current: &str, rollback: Option<&str>) -> Result<()> {
    atomic_write_json(
        &root.join("publication.json"),
        &Pointer {
            schema_version: 1,
            current: current.into(),
            rollback: rollback.map(str::to_owned),
        },
    )
}

fn concurrent_reader_consistency(
    backend: Backend,
    root: &Path,
    generation: &str,
    binding: FixtureBinding<'_>,
    query: &[f32],
    expected: &[u64],
) -> Result<bool> {
    let mut workers = Vec::new();
    for _ in 0..4 {
        let root = root.to_path_buf();
        let query = query.to_vec();
        let expected = expected.to_vec();
        let source_database_sha256 = binding.source_database_sha256.to_owned();
        let fixture_sha256 = binding.fixture_sha256.to_owned();
        let generation = generation.to_owned();
        workers.push(std::thread::spawn(move || {
            open_generation(
                backend,
                &root,
                &generation,
                FixtureBinding {
                    source_database_sha256: &source_database_sha256,
                    fixture_sha256: &fixture_sha256,
                },
            )
            .and_then(|handle| Ok(handle.search(&query)? == expected))
        }));
    }
    Ok(workers
        .into_iter()
        .all(|worker| worker.join().ok().and_then(Result::ok) == Some(true)))
}

fn attest_source(source: &Path) -> Result<SourceAttestation> {
    let conn = open_read_only(source)?;
    let quick: String = conn.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    ensure!(
        quick == "ok",
        "production vectors.sqlite3 quick_check failed: {quick}"
    );
    let metadata = conn.query_row("SELECT schema_version, generation, input_hash, embedding_backend, embedding_dim, point_count, producer_identity, evidence_contract_identity, vector_digest FROM metadata", [], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, i64>(4)?, row.get::<_, i64>(5)?, row.get::<_, String>(6)?, row.get::<_, String>(7)?, row.get::<_, String>(8)?)))?;
    ensure!(
        metadata.0 > 0 && metadata.4 == DIMENSIONS as i64 && metadata.5 >= 0,
        "unexpected production vector metadata"
    );
    let digest = canonical_digest(&conn, DIMENSIONS)?;
    ensure!(
        digest == metadata.8,
        "production vector canonical digest mismatch"
    );
    Ok(SourceAttestation {
        database_sha256: sha256_file(source)?,
        schema_version: metadata.0,
        generation: metadata.1,
        input_hash: metadata.2,
        embedding_backend: metadata.3,
        embedding_dim: metadata.4 as usize,
        point_count: metadata.5 as usize,
        producer_identity: metadata.6,
        evidence_contract_identity: metadata.7,
        vector_digest: digest,
    })
}

fn canonical_digest(conn: &Connection, dimensions: usize) -> Result<String> {
    let mut statement =
        conn.prepare("SELECT node_id, document_hash, vector FROM vectors ORDER BY node_id")?;
    let mut rows = statement.query([])?;
    let mut digest = Sha256::new();
    digest.update(VECTOR_DIGEST_DOMAIN);
    let mut count = 0usize;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let doc: String = row.get(1)?;
        let bytes: Vec<u8> = row.get(2)?;
        let _ = decode_vector_with_dimensions(&bytes, dimensions)?;
        hash_len(&mut digest, id.as_bytes());
        hash_len(&mut digest, doc.as_bytes());
        hash_len(&mut digest, &bytes);
        count += 1;
    }
    ensure!(count > 0, "production vector database has no rows");
    Ok(hex::encode(digest.finalize()))
}

fn reject_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        ensure!(
            !PathBuf::from(format!("{}{}", path.display(), suffix)).exists(),
            "unbound SQLite sidecar exists beside {}",
            path.display()
        );
    }
    Ok(())
}
fn open_read_only(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}
fn register_sqlite_vec() -> Result<()> {
    let init = unsafe {
        std::mem::transmute::<*const (), rusqlite::auto_extension::RawAutoExtension>(
            sqlite_vec::sqlite3_vec_init as *const (),
        )
    };
    unsafe { rusqlite::auto_extension::register_auto_extension(init) }?;
    Ok(())
}
fn decode_vector(bytes: &[u8]) -> Result<Vec<f32>> {
    decode_vector_with_dimensions(bytes, DIMENSIONS)
}
fn decode_vector_with_dimensions(bytes: &[u8], dimensions: usize) -> Result<Vec<f32>> {
    ensure!(bytes.len() == dimensions * 4, "vector byte width mismatch");
    let vector = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_bits(u32::from_le_bytes(chunk.try_into().expect("four bytes"))))
        .collect::<Vec<_>>();
    validate_vector("stored", &vector)?;
    Ok(vector)
}
fn validate_vector(label: &str, vector: &[f32]) -> Result<()> {
    ensure!(
        vector.len() == DIMENSIONS && vector.iter().all(|value| value.is_finite()),
        "{label} vector is malformed"
    );
    let norm = vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    ensure!(
        (norm - 1.0).abs() <= 1e-3,
        "{label} vector is not normalized"
    );
    Ok(())
}
fn vector_bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_bits().to_le_bytes())
        .collect()
}
fn cosine(left: &[f32], right: &[f32]) -> Result<f32> {
    validate_vector("query", left)?;
    validate_vector("candidate", right)?;
    Ok(left.iter().zip(right).map(|(a, b)| a * b).sum())
}
fn insert_scored(hits: &mut Vec<(f32, u64)>, hit: (f32, u64)) {
    hits.push(hit);
    hits.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    hits.truncate(TOP_K);
}
fn recall(actual: &[Vec<u64>], expected: &[Vec<u64>]) -> f64 {
    actual
        .iter()
        .zip(expected)
        .map(|(a, e)| a.iter().filter(|id| e.contains(id)).count() as f64 / TOP_K as f64)
        .sum::<f64>()
        / actual.len() as f64
}
fn source_truth_hit(
    queries: &[FrozenQuery],
    actual: &[Vec<u64>],
    selected: &HashMap<String, u64>,
) -> f64 {
    queries
        .iter()
        .zip(actual)
        .filter(|(query, hits)| {
            selected
                .get(&query.expected_node_id)
                .is_some_and(|expected| hits.contains(expected))
        })
        .count() as f64
        / queries.len() as f64
}
fn percentile(values: &mut [f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    values[((values.len() - 1) as f64 * fraction).ceil() as usize]
}
fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}
fn selection_key(id: &str) -> String {
    sha256_bytes(format!("{SELECTION_SEED}\0{id}").as_bytes())
}
fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}
fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn hash_len(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}
fn directory_size(dir: &Path) -> Result<u64> {
    Ok(fs::read_dir(dir)?.try_fold(0u64, |size, entry| {
        let entry = entry?;
        Ok::<_, std::io::Error>(size + entry.metadata()?.len())
    })?)
}
fn directory_hash(dir: &Path) -> Result<String> {
    let mut names = fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    names.sort();
    let mut digest = Sha256::new();
    for name in names {
        let path = dir.join(&name);
        digest.update(name.to_string_lossy().as_bytes());
        digest.update(sha256_file(&path)?.as_bytes());
    }
    Ok(hex::encode(digest.finalize()))
}
fn tamper_file(path: &Path) -> Result<()> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut byte = [0];
    file.read_exact(&mut byte)?;
    byte[0] ^= 0x01;
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(0))?;
    file.write_all(&byte)?;
    file.sync_all()?;
    Ok(())
}
fn sync_file(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
fn atomic_write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("output path has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("output"),
        now_unix_seconds()
    ));
    ensure!(!temporary.exists(), "temporary output collision");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
fn current_rss_bytes() -> u64 {
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0 {
            #[cfg(target_os = "macos")]
            {
                return usage.ru_maxrss.max(0) as u64;
            }
            #[cfg(not(target_os = "macos"))]
            {
                return (usage.ru_maxrss.max(0) as u64) * 1024;
            }
        }
    }
    0
}
fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_backends_build_and_bind_the_source_manifest() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source.sqlite3");
        let conn = Connection::open(&source)?;
        conn.execute_batch(
            "PRAGMA journal_mode=DELETE;
             CREATE TABLE vectors (node_id TEXT PRIMARY KEY, vector BLOB NOT NULL);",
        )?;
        for ordinal in 1..=32u64 {
            let mut vector = vec![0.0; DIMENSIONS];
            vector[ordinal as usize - 1] = 1.0;
            conn.execute(
                "INSERT INTO vectors(node_id, vector) VALUES (?1, ?2)",
                params![format!("node-{ordinal:04}"), vector_bytes(&vector)],
            )?;
        }
        drop(conn);

        let selection = (1..=32u64)
            .map(|ordinal| (format!("node-{ordinal:04}"), ordinal))
            .collect::<HashMap<_, _>>();
        let source_sha = sha256_file(&source)?;
        let binding = FixtureBinding {
            source_database_sha256: &source_sha,
            fixture_sha256: "fixture-test",
        };
        let query = {
            let mut vector = vec![0.0; DIMENSIONS];
            vector[0] = 1.0;
            vector
        };

        for backend in [Backend::SqliteVec, Backend::Usearch] {
            let root = temp.path().join(backend.label());
            let generation = root.join("generations").join("generation-1");
            build_generation(backend, &source, &selection, &generation, binding)?;
            publish_pointer(&root, "generation-1", None)?;

            let handle = open_current_generation(backend, &root, binding)?;
            assert_eq!(handle.count(), selection.len());
            assert_eq!(handle.search(&query)?.first(), Some(&1));
            drop(handle);

            tamper_file(&generation.join(backend.index_name()))?;
            assert!(open_current_generation(backend, &root, binding).is_err());
        }
        Ok(())
    }
}
