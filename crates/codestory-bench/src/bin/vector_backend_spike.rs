//! Evidence-only runner for #1202. It never changes CodeStory's production
//! retrieval route; it reads an immutable published `vectors.sqlite3` input.

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use codestory_retrieval::{
    CODERANK_QUERY_PREFIX_DEFAULT, InProcessEmbeddingClient, PinnedQuerySession,
    RetrievalPublicationIdentity, SidecarRuntimeConfig, embedding_runtime_id_for_runtime,
    process_embedding_identity, semantic_vector_dim,
};
use codestory_store::RetrievalIndexManifest;
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command as ProcessCommand;
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
        project_root: PathBuf,
        #[arg(long)]
        storage: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Run the production embedded cosine scan over the exact frozen subset.
    Oracle {
        #[arg(long)]
        inputs: PathBuf,
        #[arg(long)]
        expected_input_sha256: String,
        #[arg(long)]
        count: usize,
    },
    /// Build, load, query, increment, and fault-probe one candidate in one fresh process.
    Candidate {
        #[arg(long)]
        inputs: PathBuf,
        #[arg(long)]
        expected_input_sha256: String,
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
    /// Measure current resident memory in a fresh candidate-reader process.
    ResidentRss {
        #[arg(long)]
        inputs: PathBuf,
        #[arg(long)]
        expected_input_sha256: String,
        #[arg(long, value_enum)]
        backend: Backend,
        #[arg(long)]
        workdir: PathBuf,
        #[arg(long)]
        generation: String,
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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
    generation_manifest_sha256: String,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
struct ProductPublicationBinding {
    retrieval_manifest: RetrievalIndexManifest,
    publication: RetrievalPublicationIdentity,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
struct QueryEmbedderIdentity {
    runtime_id: String,
    embedding_dim: usize,
    query_prefix: String,
    model_digest: String,
    ggml_build_identity: String,
    backend: String,
    policy: String,
    execution_device_names: Vec<String>,
    execution_backend_names: Vec<String>,
    accelerator_execution_verified: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenArtifact {
    path: String,
    sha256: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenInputManifest {
    schema_version: u32,
    source: FrozenArtifact,
    source_generation_manifest: FrozenArtifact,
    fixture: FrozenArtifact,
    binary_sha256: String,
    host_evidence: FrozenArtifact,
}

#[derive(Deserialize)]
struct HostEvidence {
    schema_version: u32,
    os: String,
    arch: String,
    binary: HostBinaryEvidence,
}

#[derive(Deserialize)]
struct HostBinaryEvidence {
    sha256: String,
}

struct FrozenInputPaths {
    input_path: PathBuf,
    source: PathBuf,
    source_generation_manifest: PathBuf,
    fixture: PathBuf,
    host_evidence: PathBuf,
    input_manifest_sha256: String,
}

struct VerifiedInputs {
    paths: FrozenInputPaths,
    fixture: Fixture,
    fixture_sha256: String,
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
    publication: ProductPublicationBinding,
    query_embedder: QueryEmbedderIdentity,
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
    input_manifest_sha256: String,
    source_database_sha256: String,
    source_generation_manifest_sha256: String,
    fixture_sha256: String,
    query_embedder: QueryEmbedderIdentity,
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
    input_manifest_sha256: String,
    source_database_sha256: String,
    source_generation_manifest_sha256: String,
    fixture_sha256: String,
    query_embedder: QueryEmbedderIdentity,
    build_ms: f64,
    load_ms: f64,
    cold_query_ms: f64,
    warm_query_p50_ms: f64,
    warm_query_p95_ms: f64,
    rss_baseline_after_input_verification_bytes: u64,
    rss_bytes_after_load: u64,
    rss_bytes_after_warm_queries: u64,
    rss_bytes_after_warm_queries_delta: i64,
    rss_measurement_scope: &'static str,
    rss_measurement_warmups: usize,
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

#[derive(Deserialize, Serialize)]
struct ResidentRss {
    schema_version: u32,
    input_manifest_sha256: String,
    backend: String,
    generation: String,
    source_database_sha256: String,
    source_generation_manifest_sha256: String,
    fixture_sha256: String,
    query_embedder: QueryEmbedderIdentity,
    warmups: usize,
    rss_baseline_after_input_verification_bytes: u64,
    rss_bytes_after_load: u64,
    rss_bytes_after_warm_queries: u64,
    rss_bytes_after_warm_queries_delta: i64,
}

#[derive(Serialize, Deserialize)]
struct GenerationManifest {
    schema_version: u32,
    backend: String,
    backend_version: String,
    input_manifest_sha256: String,
    source_database_sha256: String,
    source_generation_manifest_sha256: String,
    fixture_sha256: String,
    query_embedder: QueryEmbedderIdentity,
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
    input_manifest_sha256: &'a str,
    source_database_sha256: &'a str,
    source_generation_manifest_sha256: &'a str,
    fixture_sha256: &'a str,
    query_embedder: &'a QueryEmbedderIdentity,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Prepare {
            project_root,
            storage,
            catalog,
            output,
        } => prepare(&project_root, &storage, &catalog, &output),
        Command::Oracle {
            inputs,
            expected_input_sha256,
            count,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &run_oracle(&inputs, &expected_input_sha256, count,)?
                )?
            );
            Ok(())
        }
        Command::Candidate {
            inputs,
            expected_input_sha256,
            oracle,
            count,
            backend,
            workdir,
            warmups,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&run_candidate(
                    &inputs,
                    &expected_input_sha256,
                    &oracle,
                    count,
                    backend,
                    &workdir,
                    warmups,
                )?)?
            );
            Ok(())
        }
        Command::ResidentRss {
            inputs,
            expected_input_sha256,
            backend,
            workdir,
            generation,
            warmups,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&run_resident_rss(
                    &inputs,
                    &expected_input_sha256,
                    backend,
                    &workdir,
                    &generation,
                    warmups,
                )?)?
            );
            Ok(())
        }
    }
}

fn prepare(
    project_root: &Path,
    storage_path: &Path,
    catalog_path: &Path,
    output: &Path,
) -> Result<()> {
    require_approved_execution_target()?;
    ensure!(
        output.parent().is_some(),
        "fixture output needs a parent directory"
    );
    ensure!(
        !output.exists(),
        "fixture output already exists: {}",
        output.display()
    );
    let project_root = project_root
        .canonicalize()
        .context("canonicalize production project root")?;
    let storage_path = storage_path
        .canonicalize()
        .context("canonicalize production storage")?;
    let runtime = SidecarRuntimeConfig::for_project_auto(&project_root);
    let session = PinnedQuerySession::begin(&project_root, &storage_path, &runtime)
        .context("admit the production retrieval publication before freezing vector evidence")?;
    let publication = ProductPublicationBinding {
        retrieval_manifest: session.manifest().clone(),
        publication: session.publication_identity().clone(),
    };
    let source = source_for_pinned_publication(&runtime, &publication)?;
    let attestation = attest_source(&source)?;
    verify_source_matches_pinned_publication(&source, &attestation, &publication)?;
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
    let embedder = InProcessEmbeddingClient::new(&runtime);
    let queries = identities
        .into_iter()
        .map(|mut query| {
            query.vector = embedder.embed_query(&query.text)?;
            validate_vector(&query.id, &query.vector)?;
            Ok(query)
        })
        .collect::<Result<Vec<_>>>()?;
    let query_embedder = current_query_embedder_identity(&runtime)?;
    ensure!(
        query_embedder.runtime_id == attestation.embedding_backend
            && query_embedder.embedding_dim == attestation.embedding_dim,
        "production vectors are not bound to the current query embedder identity"
    );
    let ending_attestation = attest_source(&source)?;
    ensure!(
        ending_attestation == attestation,
        "production vector publication changed while freezing the fixture"
    );
    let final_session = PinnedQuerySession::begin(&project_root, &storage_path, &runtime)
        .context("re-admit the production retrieval publication before writing the fixture")?;
    ensure!(
        final_session.manifest() == session.manifest()
            && final_session.publication_identity() == session.publication_identity(),
        "production retrieval publication changed while freezing the fixture"
    );
    let final_source = source_for_pinned_publication(&runtime, &publication)?;
    ensure!(
        final_source == source,
        "production vector source changed while freezing the fixture"
    );
    ensure!(
        attest_source(&final_source)? == attestation,
        "production vector publication changed while freezing the fixture"
    );
    final_session
        .revalidate()
        .context("production retrieval publication changed while freezing the fixture")?;
    let fixture = Fixture {
        schema_version: 2,
        source: attestation,
        publication,
        query_embedder,
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

fn run_oracle(inputs: &Path, expected_input_sha256: &str, count: usize) -> Result<Oracle> {
    let verified = verified_inputs(inputs, expected_input_sha256)?;
    let selected = selected_ordinals(&verified.fixture, count)?;
    let started = Instant::now();
    let top_k_ordinals = exact_scan(&verified.paths.source, &selected, &verified.fixture.queries)?;
    let cold_query_ms = elapsed_ms(started);
    let mut warm = Vec::new();
    for query in &verified.fixture.queries {
        let started = Instant::now();
        let _ = exact_scan(
            &verified.paths.source,
            &selected,
            std::slice::from_ref(query),
        )?;
        warm.push(elapsed_ms(started));
    }
    let source_truth_hit_at_20 =
        source_truth_hit(&verified.fixture.queries, &top_k_ordinals, &selected);
    verify_frozen_input_paths(&verified.paths)?;
    Ok(Oracle {
        schema_version: 2,
        input_manifest_sha256: verified.paths.input_manifest_sha256.clone(),
        source_database_sha256: verified.fixture.source.database_sha256.clone(),
        source_generation_manifest_sha256: verified
            .fixture
            .source
            .generation_manifest_sha256
            .clone(),
        fixture_sha256: verified.fixture_sha256,
        query_embedder: verified.fixture.query_embedder.clone(),
        count,
        cold_query_ms,
        warm_query_p50_ms: percentile(&mut warm.clone(), 0.50),
        warm_query_p95_ms: percentile(&mut warm, 0.95),
        top_k_ordinals,
        source_truth_hit_at_20,
    })
}

fn run_candidate(
    inputs: &Path,
    expected_input_sha256: &str,
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
    let verified = verified_inputs(inputs, expected_input_sha256)?;
    let binding = fixture_binding(&verified);
    let oracle: Oracle =
        serde_json::from_slice(&fs::read(oracle_path)?).context("read exact-scan oracle")?;
    ensure!(
        oracle.schema_version == 2
            && oracle.count == count
            && oracle.input_manifest_sha256 == verified.paths.input_manifest_sha256
            && oracle.fixture_sha256 == verified.fixture_sha256
            && oracle.source_database_sha256 == verified.fixture.source.database_sha256
            && oracle.source_generation_manifest_sha256
                == verified.fixture.source.generation_manifest_sha256
            && oracle.query_embedder == verified.fixture.query_embedder,
        "oracle does not bind this frozen candidate input"
    );
    let selected = selected_ordinals(&verified.fixture, count)?;
    fs::create_dir_all(workdir.join("generations"))?;
    let generation_one = workdir.join("generations").join("generation-1");
    let started = Instant::now();
    build_generation(
        backend,
        &verified.paths.source,
        &selected,
        &generation_one,
        binding,
    )?;
    publish_pointer(workdir, "generation-1", None)?;
    let build_ms = elapsed_ms(started);
    let old_hash = directory_hash(&generation_one)?;
    let disk_bytes = directory_size(&generation_one)?;
    let started = Instant::now();
    let pinned_old = open_generation(backend, workdir, "generation-1", binding)?;
    let load_ms = elapsed_ms(started);
    let cold_started = Instant::now();
    let cold = pinned_old.search(&verified.fixture.queries[0].vector)?;
    let cold_query_ms = elapsed_ms(cold_started);
    for query in &verified.fixture.queries {
        for _ in 0..warmups {
            let _ = pinned_old.search(&query.vector)?;
        }
    }
    let mut warm = Vec::new();
    let mut candidate_hits = Vec::new();
    for query in &verified.fixture.queries {
        let started = Instant::now();
        let hits = pinned_old.search(&query.vector)?;
        warm.push(elapsed_ms(started));
        candidate_hits.push(hits);
    }
    let resident_rss = measure_resident_rss(
        inputs,
        expected_input_sha256,
        backend,
        workdir,
        "generation-1",
        warmups,
        &verified,
    )?;
    let ann_recall_at_20 = recall(&candidate_hits, &oracle.top_k_ordinals);
    let source_truth_hit_at_20 =
        source_truth_hit(&verified.fixture.queries, &candidate_hits, &selected);
    let concurrent_reader_consistency = concurrent_reader_consistency(
        backend,
        workdir,
        "generation-1",
        binding,
        &verified.fixture.queries[0].vector,
        &cold,
    )?;
    let generation_two = workdir.join("generations").join("generation-2");
    let started = Instant::now();
    build_incremental_generation(
        backend,
        &generation_one,
        &generation_two,
        count,
        &verified.paths.source,
        &verified.fixture.incremental_node_ids,
        binding,
    )?;
    publish_pointer(workdir, "generation-2", Some("generation-1"))?;
    let incremental_reuse_ms = elapsed_ms(started);
    let pinned_old_reader_after_publication =
        pinned_old.search(&verified.fixture.queries[0].vector)? == cold;
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
        pinned_old.search(&verified.fixture.queries[0].vector)? == cold;
    verify_frozen_input_paths(&verified.paths)?;
    Ok(CandidateResult {
        schema_version: 2,
        generated_at_unix_seconds: now_unix_seconds(),
        backend: backend.label(),
        backend_version: backend.version(),
        count,
        input_manifest_sha256: verified.paths.input_manifest_sha256.clone(),
        source_database_sha256: verified.fixture.source.database_sha256.clone(),
        source_generation_manifest_sha256: verified
            .fixture
            .source
            .generation_manifest_sha256
            .clone(),
        fixture_sha256: verified.fixture_sha256,
        query_embedder: verified.fixture.query_embedder.clone(),
        build_ms,
        load_ms,
        cold_query_ms,
        warm_query_p50_ms: percentile(&mut warm.clone(), 0.50),
        warm_query_p95_ms: percentile(&mut warm, 0.95),
        rss_baseline_after_input_verification_bytes: resident_rss
            .rss_baseline_after_input_verification_bytes,
        rss_bytes_after_load: resident_rss.rss_bytes_after_load,
        rss_bytes_after_warm_queries: resident_rss.rss_bytes_after_warm_queries,
        rss_bytes_after_warm_queries_delta: resident_rss.rss_bytes_after_warm_queries_delta,
        rss_measurement_scope: "fresh_candidate_reader_current_resident_after_warm_queries",
        rss_measurement_warmups: warmups,
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

fn run_resident_rss(
    inputs: &Path,
    expected_input_sha256: &str,
    backend: Backend,
    workdir: &Path,
    generation: &str,
    warmups: usize,
) -> Result<ResidentRss> {
    let paths = load_frozen_input_paths(inputs, expected_input_sha256)?;
    let (fixture, fixture_sha256) = frozen_fixture(&paths)?;
    verify_fixture_contract(&fixture)?;
    verify_source_matches_pinned_publication(&paths.source, &fixture.source, &fixture.publication)?;
    let binding = FixtureBinding {
        input_manifest_sha256: &paths.input_manifest_sha256,
        source_database_sha256: &fixture.source.database_sha256,
        source_generation_manifest_sha256: &fixture.source.generation_manifest_sha256,
        fixture_sha256: &fixture_sha256,
        query_embedder: &fixture.query_embedder,
    };
    let rss_baseline_after_input_verification_bytes = current_resident_bytes()?;
    let reader = open_generation(backend, workdir, generation, binding)?;
    let rss_bytes_after_load = current_resident_bytes()?;
    for query in &fixture.queries {
        for _ in 0..warmups {
            let _ = reader.search(&query.vector)?;
        }
    }
    let rss_bytes_after_warm_queries = current_resident_bytes()?;
    verify_frozen_input_paths(&paths)?;
    Ok(ResidentRss {
        schema_version: 1,
        input_manifest_sha256: paths.input_manifest_sha256,
        backend: backend.label().into(),
        generation: generation.into(),
        source_database_sha256: fixture.source.database_sha256,
        source_generation_manifest_sha256: fixture.source.generation_manifest_sha256,
        fixture_sha256,
        query_embedder: fixture.query_embedder,
        warmups,
        rss_baseline_after_input_verification_bytes,
        rss_bytes_after_load,
        rss_bytes_after_warm_queries,
        rss_bytes_after_warm_queries_delta: signed_delta(
            rss_bytes_after_warm_queries,
            rss_baseline_after_input_verification_bytes,
        ),
    })
}

fn measure_resident_rss(
    inputs: &Path,
    expected_input_sha256: &str,
    backend: Backend,
    workdir: &Path,
    generation: &str,
    warmups: usize,
    verified: &VerifiedInputs,
) -> Result<ResidentRss> {
    let executable = std::env::current_exe().context("locate vector spike executable")?;
    let output = ProcessCommand::new(&executable)
        .arg("resident-rss")
        .arg("--inputs")
        .arg(inputs)
        .arg("--expected-input-sha256")
        .arg(expected_input_sha256)
        .arg("--backend")
        .arg(backend.label())
        .arg("--workdir")
        .arg(workdir)
        .arg("--generation")
        .arg(generation)
        .arg("--warmups")
        .arg(warmups.to_string())
        .output()
        .context("launch fresh resident-memory reader")?;
    ensure!(
        output.status.success(),
        "fresh resident-memory reader failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let measured: ResidentRss = serde_json::from_slice(&output.stdout)
        .context("parse fresh resident-memory reader result")?;
    ensure!(
        measured.schema_version == 1
            && measured.backend == backend.label()
            && measured.generation == generation
            && measured.input_manifest_sha256 == verified.paths.input_manifest_sha256
            && measured.source_database_sha256 == verified.fixture.source.database_sha256
            && measured.source_generation_manifest_sha256
                == verified.fixture.source.generation_manifest_sha256
            && measured.fixture_sha256 == verified.fixture_sha256
            && measured.query_embedder == verified.fixture.query_embedder
            && measured.warmups == warmups,
        "fresh resident-memory reader did not bind the candidate evidence"
    );
    Ok(measured)
}

fn verified_inputs(inputs: &Path, expected_input_sha256: &str) -> Result<VerifiedInputs> {
    let paths = load_frozen_input_paths(inputs, expected_input_sha256)?;
    let (fixture, fixture_sha256) = frozen_fixture(&paths)?;
    verify_fixture_contract(&fixture)?;
    let expected_generation_manifest = paths
        .source
        .parent()
        .context("frozen source has no parent directory")?
        .join("vector-generation-manifest.json")
        .canonicalize()
        .context("canonicalize frozen source generation manifest")?;
    ensure!(
        paths.source_generation_manifest == expected_generation_manifest,
        "frozen source generation manifest is not the source publication sibling"
    );
    let attestation = attest_source(&paths.source)?;
    ensure!(
        attestation == fixture.source,
        "frozen source publication no longer matches fixture attestation"
    );
    ensure!(
        attestation.generation_manifest_sha256 == sha256_file(&paths.source_generation_manifest)?,
        "frozen source generation manifest no longer matches fixture attestation"
    );
    verify_source_matches_pinned_publication(&paths.source, &attestation, &fixture.publication)?;
    verify_frozen_input_paths(&paths)?;
    Ok(VerifiedInputs {
        paths,
        fixture,
        fixture_sha256,
    })
}

fn fixture_binding(verified: &VerifiedInputs) -> FixtureBinding<'_> {
    FixtureBinding {
        input_manifest_sha256: &verified.paths.input_manifest_sha256,
        source_database_sha256: &verified.fixture.source.database_sha256,
        source_generation_manifest_sha256: &verified.fixture.source.generation_manifest_sha256,
        fixture_sha256: &verified.fixture_sha256,
        query_embedder: &verified.fixture.query_embedder,
    }
}

fn frozen_fixture(paths: &FrozenInputPaths) -> Result<(Fixture, String)> {
    let bytes = fs::read(&paths.fixture)
        .with_context(|| format!("read frozen fixture {}", paths.fixture.display()))?;
    let sha256 = sha256_bytes(&bytes);
    let fixture: Fixture = serde_json::from_slice(&bytes).context("parse frozen fixture")?;
    Ok((fixture, sha256))
}

fn verify_fixture_contract(fixture: &Fixture) -> Result<()> {
    ensure!(fixture.schema_version == 2, "unsupported fixture schema");
    ensure!(
        fixture.source.embedding_dim == DIMENSIONS && fixture.queries.len() == 30,
        "fixture does not meet the declared profile"
    );
    ensure!(
        fixture.selected_node_ids.len() == 100_000
            && fixture.incremental_node_ids.len() == INCREMENTAL_COUNT,
        "fixture does not contain the declared nested real-anchor input"
    );
    ensure!(
        fixture.query_embedder.runtime_id == fixture.source.embedding_backend
            && fixture.query_embedder.embedding_dim == fixture.source.embedding_dim
            && fixture.query_embedder.query_prefix == CODERANK_QUERY_PREFIX_DEFAULT
            && !fixture.query_embedder.model_digest.is_empty()
            && !fixture.query_embedder.ggml_build_identity.is_empty()
            && !fixture.query_embedder.backend.is_empty()
            && !fixture.query_embedder.policy.is_empty(),
        "fixture query embedder identity is incomplete or incompatible with the source publication"
    );
    for query in &fixture.queries {
        validate_vector(&query.id, &query.vector)?;
    }
    Ok(())
}

fn load_frozen_input_paths(inputs: &Path, expected_input_sha256: &str) -> Result<FrozenInputPaths> {
    require_approved_execution_target()?;
    reject_symlinked_path(inputs, "frozen input manifest")?;
    require_regular_file(inputs, "frozen input manifest")?;
    let input_path = inputs
        .canonicalize()
        .with_context(|| format!("canonicalize frozen input manifest {}", inputs.display()))?;
    let input_bytes = fs::read(&input_path)
        .with_context(|| format!("read frozen input manifest {}", input_path.display()))?;
    let input_manifest_sha256 = sha256_bytes(&input_bytes);
    ensure!(
        input_manifest_sha256 == expected_input_sha256,
        "frozen input manifest digest mismatch: expected {expected_input_sha256}, observed {input_manifest_sha256}"
    );
    let input: FrozenInputManifest =
        serde_json::from_slice(&input_bytes).context("parse frozen input manifest")?;
    ensure!(
        input.schema_version == 1,
        "unsupported frozen input manifest schema"
    );
    let source = resolve_frozen_artifact(&input_path, &input.source, "source database")?;
    reject_sidecars(&source)?;
    let source_generation_manifest = resolve_frozen_artifact(
        &input_path,
        &input.source_generation_manifest,
        "source generation manifest",
    )?;
    let fixture = resolve_frozen_artifact(&input_path, &input.fixture, "fixture")?;
    let host_evidence =
        resolve_frozen_artifact(&input_path, &input.host_evidence, "host evidence")?;
    let executable = std::env::current_exe().context("locate vector spike executable")?;
    reject_symlinked_path(&executable, "vector spike executable")?;
    require_regular_file(&executable, "vector spike executable")?;
    let executable = executable
        .canonicalize()
        .context("canonicalize vector spike executable")?;
    ensure!(
        sha256_file(&executable)? == input.binary_sha256,
        "vector spike executable digest does not match frozen input manifest"
    );
    let host: HostEvidence = serde_json::from_slice(
        &fs::read(&host_evidence)
            .with_context(|| format!("read host evidence {}", host_evidence.display()))?,
    )
    .context("parse host evidence")?;
    ensure!(
        host.schema_version == 1
            && host.os == "darwin"
            && host.arch == "arm64"
            && host.binary.sha256 == input.binary_sha256,
        "host evidence does not attest the approved macOS arm64 executable"
    );
    Ok(FrozenInputPaths {
        input_path,
        source,
        source_generation_manifest,
        fixture,
        host_evidence,
        input_manifest_sha256,
    })
}

fn resolve_frozen_artifact(
    input_path: &Path,
    artifact: &FrozenArtifact,
    label: &str,
) -> Result<PathBuf> {
    validate_relative_evidence_path(&artifact.path, label)?;
    let root = input_path
        .parent()
        .context("frozen input manifest has no parent directory")?
        .canonicalize()
        .context("canonicalize frozen input root")?;
    let unresolved = root.join(&artifact.path);
    let artifact_label = format!("frozen {label}");
    reject_symlinked_path(&unresolved, &artifact_label)?;
    require_regular_file(&unresolved, &artifact_label)?;
    let path = unresolved
        .canonicalize()
        .with_context(|| format!("canonicalize frozen {label} {}", artifact.path))?;
    ensure!(
        path.starts_with(&root),
        "frozen {label} escapes the evidence root"
    );
    let observed = sha256_file(&path)?;
    ensure!(
        observed == artifact.sha256,
        "frozen {label} digest mismatch: expected {}, observed {observed}",
        artifact.sha256
    );
    Ok(path)
}

fn validate_relative_evidence_path(path: &str, label: &str) -> Result<()> {
    let candidate = Path::new(path);
    ensure!(
        candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_))),
        "frozen {label} path must be a plain relative path"
    );
    Ok(())
}

fn reject_symlinked_path(path: &Path, label: &str) -> Result<()> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("resolve current directory for frozen evidence")?
            .join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(Path::new("/")),
            Component::Normal(segment) => {
                current.push(segment);
                let metadata = fs::symlink_metadata(&current).with_context(|| {
                    format!("inspect {label} path component {}", current.display())
                })?;
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "{label} traverses a symbolic link: {}",
                    current.display()
                );
            }
            Component::CurDir | Component::ParentDir => {
                anyhow::bail!("{label} path must not contain dot path components")
            }
        }
    }
    Ok(())
}

fn require_regular_file(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "{label} must be an ordinary regular file: {}",
        path.display()
    );
    Ok(())
}

fn verify_frozen_input_paths(paths: &FrozenInputPaths) -> Result<()> {
    let reloaded = load_frozen_input_paths(&paths.input_path, &paths.input_manifest_sha256)?;
    ensure!(
        reloaded.source == paths.source
            && reloaded.source_generation_manifest == paths.source_generation_manifest
            && reloaded.fixture == paths.fixture
            && reloaded.host_evidence == paths.host_evidence,
        "frozen input manifest resolved to different evidence paths"
    );
    Ok(())
}

fn source_for_pinned_publication(
    runtime: &SidecarRuntimeConfig,
    publication: &ProductPublicationBinding,
) -> Result<PathBuf> {
    let collection = publication.retrieval_manifest.semantic_generation.trim();
    ensure!(
        !collection.is_empty(),
        "pinned retrieval publication has no semantic collection"
    );
    let source = runtime
        .layout
        .semantic_data_dir
        .join("collections")
        .join(collection)
        .join("vectors.sqlite3")
        .canonicalize()
        .context("canonicalize pinned production vectors.sqlite3")?;
    reject_sidecars(&source)?;
    Ok(source)
}

fn verify_source_matches_pinned_publication(
    source: &Path,
    attestation: &SourceAttestation,
    publication: &ProductPublicationBinding,
) -> Result<()> {
    let manifest = &publication.retrieval_manifest;
    let identity = &publication.publication;
    ensure!(
        !manifest.project_id.is_empty()
            && !manifest.semantic_generation.is_empty()
            && !identity.core_generation_id.is_empty()
            && !identity.core_run_id.is_empty(),
        "fixture publication binding is incomplete"
    );
    let expected_count = manifest
        .dense_projection_count
        .or(manifest.projection_count)
        .context("pinned retrieval publication has no dense anchor count")?;
    ensure!(
        expected_count >= 0 && expected_count as usize == attestation.point_count,
        "pinned retrieval publication anchor count does not match vectors.sqlite3"
    );
    ensure!(
        manifest.sidecar_generation.as_deref() == Some(attestation.generation.as_str())
            && manifest.sidecar_input_hash.as_deref() == Some(attestation.input_hash.as_str())
            && manifest.embedding_backend.as_deref()
                == Some(attestation.embedding_backend.as_str())
            && manifest.embedding_dim == Some(attestation.embedding_dim as i32),
        "pinned retrieval manifest does not match vectors.sqlite3 attestation"
    );
    ensure!(
        identity.sidecar_generation == attestation.generation
            && identity.sidecar_input_hash == attestation.input_hash
            && identity.semantic_generation == manifest.semantic_generation,
        "pinned retrieval publication identity does not match vectors.sqlite3"
    );
    let collection = source
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    ensure!(
        collection == Some(manifest.semantic_generation.as_str()),
        "vectors.sqlite3 is not located in the pinned semantic generation"
    );
    Ok(())
}

fn current_query_embedder_identity(
    runtime: &SidecarRuntimeConfig,
) -> Result<QueryEmbedderIdentity> {
    let identity = process_embedding_identity(&runtime.cache_root, runtime.embedding.allow_cpu)
        .context("observe live query embedder identity")?;
    ensure!(
        identity.worker_alive && identity.load_error.is_none(),
        "query embedder is not live while freezing the fixture"
    );
    Ok(QueryEmbedderIdentity {
        runtime_id: embedding_runtime_id_for_runtime(runtime),
        embedding_dim: semantic_vector_dim(),
        query_prefix: CODERANK_QUERY_PREFIX_DEFAULT.into(),
        model_digest: identity.model_digest.into(),
        ggml_build_identity: identity.ggml_build_identity.into(),
        backend: identity.backend,
        policy: identity.policy.into(),
        execution_device_names: identity.execution_device_names,
        execution_backend_names: identity.execution_backend_names,
        accelerator_execution_verified: identity.accelerator_execution_verified,
    })
}

fn require_approved_execution_target() -> Result<()> {
    ensure!(
        cfg!(target_os = "macos") && cfg!(target_arch = "aarch64"),
        "vector backend spike is approved only on a native macOS arm64 executable"
    );
    Ok(())
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
            input_manifest_sha256: binding.input_manifest_sha256.into(),
            source_database_sha256: binding.source_database_sha256.into(),
            source_generation_manifest_sha256: binding.source_generation_manifest_sha256.into(),
            fixture_sha256: binding.fixture_sha256.into(),
            query_embedder: binding.query_embedder.clone(),
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
            && manifest.input_manifest_sha256 == binding.input_manifest_sha256
            && manifest.source_database_sha256 == binding.source_database_sha256
            && manifest.source_generation_manifest_sha256
                == binding.source_generation_manifest_sha256
            && manifest.fixture_sha256 == binding.fixture_sha256
            && manifest.query_embedder == *binding.query_embedder,
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
        let input_manifest_sha256 = binding.input_manifest_sha256.to_owned();
        let source_database_sha256 = binding.source_database_sha256.to_owned();
        let source_generation_manifest_sha256 =
            binding.source_generation_manifest_sha256.to_owned();
        let fixture_sha256 = binding.fixture_sha256.to_owned();
        let query_embedder = binding.query_embedder.clone();
        let generation = generation.to_owned();
        workers.push(std::thread::spawn(move || {
            open_generation(
                backend,
                &root,
                &generation,
                FixtureBinding {
                    input_manifest_sha256: &input_manifest_sha256,
                    source_database_sha256: &source_database_sha256,
                    source_generation_manifest_sha256: &source_generation_manifest_sha256,
                    fixture_sha256: &fixture_sha256,
                    query_embedder: &query_embedder,
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
    let source = source
        .canonicalize()
        .with_context(|| format!("canonicalize vectors source {}", source.display()))?;
    reject_sidecars(&source)?;
    let generation_manifest = source
        .parent()
        .context("vectors source has no parent directory")?
        .join("vector-generation-manifest.json");
    ensure!(
        generation_manifest.is_file(),
        "published vectors source is missing vector-generation-manifest.json"
    );
    let conn = open_read_only(&source)?;
    let quick: String = conn.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    ensure!(
        quick == "ok",
        "production vectors.sqlite3 quick_check failed: {quick}"
    );
    let metadata_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM metadata", [], |row| row.get(0))?;
    ensure!(
        metadata_count == 1,
        "production vectors.sqlite3 must contain exactly one metadata row; found {metadata_count}"
    );
    let metadata = conn.query_row("SELECT schema_version, generation, input_hash, embedding_backend, embedding_dim, point_count, producer_identity, evidence_contract_identity, vector_digest FROM metadata", [], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, i64>(4)?, row.get::<_, i64>(5)?, row.get::<_, String>(6)?, row.get::<_, String>(7)?, row.get::<_, String>(8)?)))?;
    ensure!(
        metadata.0 > 0 && metadata.4 == DIMENSIONS as i64 && metadata.5 >= 0,
        "unexpected production vector metadata"
    );
    let (digest, row_count) = canonical_digest(&conn, DIMENSIONS)?;
    ensure!(
        digest == metadata.8,
        "production vector canonical digest mismatch"
    );
    ensure!(
        row_count == metadata.5 as usize,
        "production vector row count does not match metadata point_count"
    );
    Ok(SourceAttestation {
        database_sha256: sha256_file(&source)?,
        schema_version: metadata.0,
        generation: metadata.1,
        input_hash: metadata.2,
        embedding_backend: metadata.3,
        embedding_dim: metadata.4 as usize,
        point_count: metadata.5 as usize,
        producer_identity: metadata.6,
        evidence_contract_identity: metadata.7,
        vector_digest: digest,
        generation_manifest_sha256: sha256_file(&generation_manifest)?,
    })
}

fn canonical_digest(conn: &Connection, dimensions: usize) -> Result<(String, usize)> {
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
    Ok((hex::encode(digest.finalize()), count))
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
#[cfg(target_os = "macos")]
fn current_resident_bytes() -> Result<u64> {
    unsafe {
        let mut info: mach2::task_info::mach_task_basic_info = std::mem::zeroed();
        let mut count = mach2::task_info::MACH_TASK_BASIC_INFO_COUNT;
        let status = mach2::task::task_info(
            mach2::traps::mach_task_self(),
            mach2::task_info::MACH_TASK_BASIC_INFO,
            (&mut info as *mut mach2::task_info::mach_task_basic_info).cast(),
            &mut count,
        );
        ensure!(
            status == mach2::kern_return::KERN_SUCCESS,
            "read current resident memory with task_info failed: {status}"
        );
        Ok(std::ptr::addr_of!(info.resident_size).read_unaligned() as u64)
    }
}

#[cfg(not(target_os = "macos"))]
fn current_resident_bytes() -> Result<u64> {
    anyhow::bail!("current resident-memory measurement is approved only on macOS")
}

fn signed_delta(after: u64, before: u64) -> i64 {
    if after >= before {
        i64::try_from(after - before).unwrap_or(i64::MAX)
    } else {
        -i64::try_from(before - after).unwrap_or(i64::MAX)
    }
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
        let query_embedder = QueryEmbedderIdentity {
            runtime_id: "runtime-test".into(),
            embedding_dim: DIMENSIONS,
            query_prefix: CODERANK_QUERY_PREFIX_DEFAULT.into(),
            model_digest: "model-test".into(),
            ggml_build_identity: "ggml-test".into(),
            backend: "backend-test".into(),
            policy: "accelerated".into(),
            execution_device_names: vec!["device-test".into()],
            execution_backend_names: vec!["backend-test".into()],
            accelerator_execution_verified: true,
        };
        let binding = FixtureBinding {
            input_manifest_sha256: "input-test",
            source_database_sha256: &source_sha,
            source_generation_manifest_sha256: "source-generation-manifest-test",
            fixture_sha256: "fixture-test",
            query_embedder: &query_embedder,
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

    #[test]
    fn source_attestation_rejects_multiple_metadata_rows() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("vectors.sqlite3");
        fs::write(temp.path().join("vector-generation-manifest.json"), b"{}")?;
        let conn = Connection::open(&source)?;
        conn.execute_batch(
            "CREATE TABLE metadata (
                schema_version INTEGER,
                generation TEXT,
                input_hash TEXT,
                embedding_backend TEXT,
                embedding_dim INTEGER,
                point_count INTEGER,
                producer_identity TEXT,
                evidence_contract_identity TEXT,
                vector_digest TEXT
            );
            CREATE TABLE vectors (node_id TEXT, document_hash TEXT, vector BLOB);",
        )?;
        for _ in 0..2 {
            conn.execute(
                "INSERT INTO metadata VALUES (1, 'generation', 'input', 'runtime', 768, 0, 'producer', 'contract', 'digest')",
                [],
            )?;
        }
        drop(conn);
        let error = attest_source(&source).expect_err("multiple metadata rows must fail");
        assert!(error.to_string().contains("exactly one metadata row"));
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn frozen_input_manifest_rejects_mutated_artifact() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let temp_root = temp.path().canonicalize()?;
        let source = temp_root.join("source.sqlite3");
        let source_manifest = temp_root.join("vector-generation-manifest.json");
        let fixture = temp_root.join("fixture.json");
        let host_evidence = temp_root.join("host-evidence.json");
        fs::write(&source, b"source")?;
        fs::write(&source_manifest, b"manifest")?;
        fs::write(&fixture, b"fixture")?;
        let binary = std::env::current_exe()?.canonicalize()?;
        let binary_sha256 = sha256_file(&binary)?;
        atomic_write_json(
            &host_evidence,
            &serde_json::json!({
                "schema_version": 1,
                "os": "darwin",
                "arch": "arm64",
                "binary": { "sha256": binary_sha256 },
            }),
        )?;
        let input = serde_json::json!({
            "schema_version": 1,
            "source": { "path": "source.sqlite3", "sha256": sha256_file(&source)? },
            "source_generation_manifest": {
                "path": "vector-generation-manifest.json",
                "sha256": sha256_file(&source_manifest)?,
            },
            "fixture": { "path": "fixture.json", "sha256": sha256_file(&fixture)? },
            "binary_sha256": binary_sha256,
            "host_evidence": {
                "path": "host-evidence.json",
                "sha256": sha256_file(&host_evidence)?,
            },
        });
        let input_path = temp_root.join("input.json");
        atomic_write_json(&input_path, &input)?;
        let input_sha256 = sha256_file(&input_path)?;
        assert_eq!(
            load_frozen_input_paths(&input_path, &input_sha256)?.source,
            source.canonicalize()?
        );
        fs::write(&source, b"changed")?;
        assert!(load_frozen_input_paths(&input_path, &input_sha256).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn frozen_paths_reject_symbolic_links() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let temp_root = temp.path().canonicalize()?;
        let target = temp_root.join("target");
        let link = temp_root.join("link");
        fs::write(&target, b"target")?;
        std::os::unix::fs::symlink(&target, &link)?;
        assert!(reject_symlinked_path(&link, "fixture").is_err());
        Ok(())
    }
}
