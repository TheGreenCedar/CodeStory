//! Evidence-only runner for #1202. It never changes CodeStory's production
//! retrieval route; it reads an immutable published `vectors.sqlite3` input.

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use codestory_retrieval::{
    CODERANK_QUERY_PREFIX_DEFAULT, PerUserEmbeddingClient, PinnedQuerySession,
    ProductEmbeddingClient, RetrievalPublicationIdentity, SidecarRuntimeConfig,
    embedding_runtime_id_for_runtime, semantic_vector_dim,
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
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

const DIMENSIONS: usize = 768;
const TOP_K: usize = 20;
const INCREMENTAL_COUNT: usize = 100;
const SELECTION_SEED: &str = "codestory-1202-vector-spike-v1";
const VECTOR_DIGEST_DOMAIN: &[u8] = b"codestory-vector-digest-v1\0";
const EMBEDDING_AUTHORITY_DIR_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_DIR";
const EMBEDDING_AUTHORITY_NONCE_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_NONCE";

#[derive(Parser)]
#[command(about = "Evidence-only sqlite-vec vs USearch runner for CodeStory issue #1202")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Internal native embedding-server entrypoint used by the shared CLI transport.
    #[command(name = "internal-embedding-server", hide = true)]
    InternalEmbeddingServer,
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
    /// Re-prove a frozen fixture against its catalog, source publication, and live embedder.
    #[command(hide = true)]
    VerifyFixture {
        #[arg(long)]
        project_root: PathBuf,
        #[arg(long)]
        storage: PathBuf,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        source_generation_manifest: PathBuf,
        #[arg(long)]
        fixture: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
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
    catalog: FrozenArtifact,
    fixture_verification: FrozenArtifact,
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
    catalog: PathBuf,
    fixture_verification: PathBuf,
    host_evidence: PathBuf,
    binary_sha256: String,
    input_manifest_sha256: String,
}

struct VerifiedInputs {
    paths: FrozenInputPaths,
    fixture: Fixture,
    fixture_sha256: String,
}

#[derive(Clone, Deserialize, Serialize)]
struct Catalog {
    schema_version: u32,
    corpus_commit: String,
    queries: Vec<CatalogQuery>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
struct CatalogQuery {
    id: String,
    kind: String,
    text: String,
    file_path: String,
    symbol: String,
}

/// Git identity of the source tree from which the catalog was resolved.
///
/// The catalog is source truth only when it is resolved against the exact,
/// clean checkout it names. Recording both the commit and tree makes that
/// identity independently inspectable without treating a path spelling as a
/// source identity.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
struct VerifiedCorpusIdentity {
    commit: String,
    tree: String,
    worktree_clean: bool,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
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
    verified_corpus: VerifiedCorpusIdentity,
    catalog_sha256: String,
    selection_seed: String,
    selected_node_ids: Vec<String>,
    incremental_node_ids: Vec<String>,
    queries: Vec<FrozenQuery>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
struct FixtureVerification {
    schema_version: u32,
    source_database_sha256: String,
    source_generation_manifest_sha256: String,
    fixture_sha256: String,
    catalog_sha256: String,
    binary_sha256: String,
    corpus: VerifiedCorpusIdentity,
    publication: ProductPublicationBinding,
    query_embedder: QueryEmbedderIdentity,
    selection_seed: String,
    query_vector_digest: String,
    expected_document_digest: String,
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
    first_query_after_load_ms: f64,
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
    reader_publish_barrier_old_readers_pinned: bool,
    reader_publish_barrier_post_publish_reader_matches_truth: bool,
    pinned_old_reader_after_publication: bool,
    old_generation_unchanged: bool,
    corrupt_candidate_publish_rejected: bool,
    incomplete_candidate_publish_rejected: bool,
    failed_candidate_preserved_current_pointer: bool,
    cancellation_signal: &'static str,
    cancellation_observed_after_vectors: usize,
    cancelled_candidate_publish_rejected: bool,
    cancelled_candidate_preserved_current_pointer: bool,
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

#[derive(Clone, Copy)]
struct BuildControl<'a> {
    processed: &'a AtomicUsize,
    cancel_after: Option<usize>,
}

impl BuildControl<'_> {
    fn before_next(self) -> Result<()> {
        let processed = self.processed.load(Ordering::Acquire);
        if self.cancel_after.is_some_and(|limit| processed >= limit) {
            anyhow::bail!("candidate_build_cancelled_after_vectors:{processed}");
        }
        Ok(())
    }

    fn record_processed(self) {
        self.processed.fetch_add(1, Ordering::AcqRel);
    }
}

fn main() -> Result<()> {
    let command = Cli::parse().command;
    match command {
        Command::InternalEmbeddingServer => {
            require_isolated_embedding_authority()?;
            codestory_cli::run_native_embedding_server()
        }
        Command::Prepare {
            project_root,
            storage,
            catalog,
            output,
        } => {
            require_isolated_embedding_authority()?;
            codestory_cli::install_native_embedding_client_transport()
                .context("install native embedding server transport")?;
            prepare(&project_root, &storage, &catalog, &output)
        }
        Command::VerifyFixture {
            project_root,
            storage,
            source,
            source_generation_manifest,
            fixture,
            catalog,
        } => {
            require_isolated_embedding_authority()?;
            codestory_cli::install_native_embedding_client_transport()
                .context("install native embedding server transport")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&verify_fixture_for_measurement(
                    &project_root,
                    &storage,
                    &source,
                    &source_generation_manifest,
                    &fixture,
                    &catalog,
                )?)?
            );
            Ok(())
        }
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
    let verified_corpus = verify_catalog_corpus_identity(&project_root, &catalog.corpus_commit)?;
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
    let embedder = ProductEmbeddingClient::new(&runtime);
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
    ensure!(
        verify_catalog_corpus_identity(&project_root, &catalog.corpus_commit)? == verified_corpus,
        "catalog source checkout changed while freezing the fixture"
    );
    let fixture = Fixture {
        schema_version: 4,
        source: attestation,
        publication,
        query_embedder,
        source_database_path: source.display().to_string(),
        corpus_commit: catalog.corpus_commit.clone(),
        verified_corpus,
        catalog_sha256: sha256_bytes(&catalog_bytes),
        selection_seed: SELECTION_SEED.into(),
        selected_node_ids: selected,
        incremental_node_ids,
        queries,
    };
    verify_fixture_contract(&fixture, &catalog, &sha256_bytes(&catalog_bytes), &source)?;
    atomic_write_json(output, &fixture)
}

fn verify_fixture_for_measurement(
    project_root: &Path,
    storage: &Path,
    source: &Path,
    source_generation_manifest: &Path,
    fixture_path: &Path,
    catalog_path: &Path,
) -> Result<FixtureVerification> {
    require_approved_execution_target()?;
    reject_symlinked_path(project_root, "fixture-verification project root")?;
    let project_root = project_root
        .canonicalize()
        .context("canonicalize fixture-verification project root")?;
    let storage = frozen_regular_path(storage, "fixture-verification core storage")?;
    let source = frozen_regular_path(source, "fixture-verification source database")?;
    let source_generation_manifest = frozen_regular_path(
        source_generation_manifest,
        "fixture-verification source generation manifest",
    )?;
    let fixture_path = frozen_regular_path(fixture_path, "fixture-verification fixture")?;
    let catalog_path = frozen_regular_path(catalog_path, "fixture-verification catalog")?;
    let expected_manifest = source
        .parent()
        .context("fixture-verification source has no parent")?
        .join("vector-generation-manifest.json")
        .canonicalize()
        .context("canonicalize fixture-verification source generation manifest sibling")?;
    ensure!(
        source_generation_manifest == expected_manifest,
        "fixture-verification source generation manifest is not the source sibling"
    );

    let fixture_bytes = fs::read(&fixture_path)?;
    let fixture: Fixture =
        serde_json::from_slice(&fixture_bytes).context("parse frozen fixture")?;
    let catalog_bytes = fs::read(&catalog_path)?;
    let catalog: Catalog =
        serde_json::from_slice(&catalog_bytes).context("parse reviewed source-truth catalog")?;
    let catalog_sha256 = sha256_bytes(&catalog_bytes);
    let fixture_sha256 = sha256_bytes(&fixture_bytes);
    verify_fixture_contract(&fixture, &catalog, &catalog_sha256, &source)?;

    let attestation = attest_source(&source)?;
    ensure!(
        attestation == fixture.source,
        "fixture-verification source does not match fixture attestation"
    );
    ensure!(
        attestation.generation_manifest_sha256 == sha256_file(&source_generation_manifest)?,
        "fixture-verification generation manifest does not match source attestation"
    );
    verify_source_matches_pinned_publication(&source, &attestation, &fixture.publication)?;
    let corpus = verify_catalog_corpus_identity(&project_root, &catalog.corpus_commit)?;
    ensure!(
        corpus == fixture.verified_corpus,
        "fixture-verification checkout identity does not match the prepared fixture"
    );

    let runtime = SidecarRuntimeConfig::for_project_auto(&project_root);
    let session = PinnedQuerySession::begin(&project_root, &storage, &runtime)
        .context("re-admit the prepared product publication for measurement")?;
    ensure!(
        session.manifest() == &fixture.publication.retrieval_manifest
            && session.publication_identity() == &fixture.publication.publication,
        "fixture-verification product publication identity changed"
    );
    let live_source = source_for_pinned_publication(&runtime, &fixture.publication)?;
    ensure!(
        attest_source(&live_source)? == attestation,
        "fixture-verification live source publication differs from the frozen source"
    );
    let embedder = ProductEmbeddingClient::new(&runtime);
    let recomputed = fixture
        .queries
        .iter()
        .map(|query| embedder.embed_query(&query.text))
        .collect::<Result<Vec<_>>>()?;
    verify_query_vectors(&fixture.queries, &recomputed)?;
    let query_embedder = current_query_embedder_identity(&runtime)?;
    ensure!(
        query_embedder == fixture.query_embedder,
        "fixture-verification live query embedder identity changed"
    );
    ensure!(
        verify_catalog_corpus_identity(&project_root, &catalog.corpus_commit)? == corpus,
        "fixture-verification source checkout changed during query reproduction"
    );
    ensure!(
        attest_source(&source)? == attestation,
        "fixture-verification source publication changed during query reproduction"
    );
    ensure!(
        attest_source(&live_source)? == attestation,
        "fixture-verification live source publication changed during query reproduction"
    );
    session
        .revalidate()
        .context("fixture-verification product publication changed during query reproduction")?;

    Ok(FixtureVerification {
        schema_version: 1,
        source_database_sha256: attestation.database_sha256,
        source_generation_manifest_sha256: sha256_file(&source_generation_manifest)?,
        fixture_sha256,
        catalog_sha256,
        binary_sha256: sha256_file(&std::env::current_exe()?)?,
        corpus,
        publication: fixture.publication.clone(),
        query_embedder,
        selection_seed: fixture.selection_seed.clone(),
        query_vector_digest: query_vector_digest(&fixture.queries),
        expected_document_digest: expected_document_digest(&fixture.queries),
    })
}

fn run_oracle(inputs: &Path, expected_input_sha256: &str, count: usize) -> Result<Oracle> {
    let verified = verified_inputs(inputs, expected_input_sha256)?;
    let selected = selected_ordinals(&verified.fixture, count)?;
    let top_k_ordinals = exact_scan(&verified.paths.source, &selected, &verified.fixture.queries)?;
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
    publish_generation(workdir, "generation-1", None, backend, binding)?;
    let build_ms = elapsed_ms(started);
    let old_hash = directory_hash(&generation_one)?;
    let disk_bytes = directory_size(&generation_one)?;
    let started = Instant::now();
    let pinned_old = open_generation(backend, workdir, "generation-1", binding)?;
    let load_ms = elapsed_ms(started);
    let first_query_started = Instant::now();
    let cold = pinned_old.search(&verified.fixture.queries[0].vector)?;
    let first_query_after_load_ms = elapsed_ms(first_query_started);
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
    let incremental_selected = selected_ordinals_after_incremental(&verified.fixture, count)?;
    let post_publish_expected = exact_scan(
        &verified.paths.source,
        &incremental_selected,
        std::slice::from_ref(&verified.fixture.queries[0]),
    )?
    .into_iter()
    .next()
    .context("incremental source-truth scan did not return the declared query")?;
    let reader_publish_barrier = open_readers_before_publish(
        backend,
        workdir,
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
    publish_generation(
        workdir,
        "generation-2",
        Some("generation-1"),
        backend,
        binding,
    )?;
    let incremental_reuse_ms = elapsed_ms(started);
    let current = open_current_generation(backend, workdir, binding)?;
    let reader_publish_barrier_post_publish_reader_matches_truth = current.count()?
        == count + INCREMENTAL_COUNT
        && current.search(&verified.fixture.queries[0].vector)? == post_publish_expected;
    let reader_publish_barrier_old_readers_pinned =
        reader_publish_barrier.release_after_publish()?;
    let pinned_old_reader_after_publication =
        pinned_old.search(&verified.fixture.queries[0].vector)? == cold;
    let old_generation_unchanged = directory_hash(&generation_one)? == old_hash;
    let pointer_before_corrupt = fs::read(workdir.join("publication.json"))?;

    let corrupt = workdir.join("generations").join("generation-corrupt");
    fs::create_dir_all(&corrupt)?;
    fs::copy(
        generation_two.join(backend.index_name()),
        corrupt.join(backend.index_name()),
    )?;
    fs::copy(
        generation_two.join("manifest.json"),
        corrupt.join("manifest.json"),
    )?;
    tamper_file(&corrupt.join(backend.index_name()))?;
    rebind_manifest_index_digest(&corrupt, backend)?;
    let corrupt_candidate_publish_rejected = publish_generation(
        workdir,
        "generation-corrupt",
        Some("generation-2"),
        backend,
        binding,
    )
    .is_err();

    let incomplete = workdir.join("generations").join("generation-incomplete");
    fs::create_dir_all(&incomplete)?;
    fs::copy(
        generation_two.join(backend.index_name()),
        incomplete.join(backend.index_name()),
    )?;
    let incomplete_candidate_publish_rejected = publish_generation(
        workdir,
        "generation-incomplete",
        Some("generation-2"),
        backend,
        binding,
    )
    .is_err();
    let failed_candidate_preserved_current_pointer =
        fs::read(workdir.join("publication.json"))? == pointer_before_corrupt;

    let cancellation_processed = AtomicUsize::new(0);
    let cancelled = workdir.join("generations").join("generation-cancelled");
    let cancellation_error = build_generation_with_control(
        backend,
        &verified.paths.source,
        &selected,
        &cancelled,
        binding,
        BuildControl {
            processed: &cancellation_processed,
            cancel_after: Some(8),
        },
    )
    .expect_err("fault probe must cancel after backend work begins");
    let cancellation_observed_after_vectors = cancellation_processed.load(Ordering::Acquire);
    ensure!(
        cancellation_observed_after_vectors == 8
            && cancellation_error
                .to_string()
                .contains("candidate_build_cancelled_after_vectors:8"),
        "candidate cancellation did not preserve its attributable progress signal"
    );
    let cancelled_candidate_publish_rejected = publish_generation(
        workdir,
        "generation-cancelled",
        Some("generation-2"),
        backend,
        binding,
    )
    .is_err();
    let cancelled_candidate_preserved_current_pointer =
        fs::read(workdir.join("publication.json"))? == pointer_before_corrupt;
    ensure!(
        corrupt_candidate_publish_rejected
            && incomplete_candidate_publish_rejected
            && failed_candidate_preserved_current_pointer
            && cancelled_candidate_publish_rejected
            && cancelled_candidate_preserved_current_pointer,
        "candidate fault probe did not reject publication while preserving the live pointer"
    );

    publish_generation(
        workdir,
        "generation-1",
        Some("generation-2"),
        backend,
        binding,
    )?;
    let rollback_pointer_readable =
        open_current_generation(backend, workdir, binding)?.count()? == count;
    tamper_file(&generation_one.join(backend.index_name()))?;
    let referenced_generation_tamper_rejected =
        open_current_generation(backend, workdir, binding).is_err();
    let pinned_reader_after_referenced_tamper =
        pinned_old.search(&verified.fixture.queries[0].vector)? == cold;
    verify_frozen_input_paths(&verified.paths)?;
    Ok(CandidateResult {
        schema_version: 3,
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
        first_query_after_load_ms,
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
        reader_publish_barrier_old_readers_pinned,
        reader_publish_barrier_post_publish_reader_matches_truth,
        pinned_old_reader_after_publication,
        old_generation_unchanged,
        corrupt_candidate_publish_rejected,
        incomplete_candidate_publish_rejected,
        failed_candidate_preserved_current_pointer,
        cancellation_signal: "candidate_build_cancelled_after_vectors",
        cancellation_observed_after_vectors,
        cancelled_candidate_publish_rejected,
        cancelled_candidate_preserved_current_pointer,
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
    let verified = verified_inputs(inputs, expected_input_sha256)?;
    let paths = &verified.paths;
    let fixture = &verified.fixture;
    let binding = FixtureBinding {
        input_manifest_sha256: &paths.input_manifest_sha256,
        source_database_sha256: &fixture.source.database_sha256,
        source_generation_manifest_sha256: &fixture.source.generation_manifest_sha256,
        fixture_sha256: &verified.fixture_sha256,
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
    verify_frozen_input_paths(paths)?;
    Ok(ResidentRss {
        schema_version: 1,
        input_manifest_sha256: paths.input_manifest_sha256.clone(),
        backend: backend.label().into(),
        generation: generation.into(),
        source_database_sha256: fixture.source.database_sha256.clone(),
        source_generation_manifest_sha256: fixture.source.generation_manifest_sha256.clone(),
        fixture_sha256: verified.fixture_sha256,
        query_embedder: fixture.query_embedder.clone(),
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
    let (catalog, catalog_sha256) = frozen_catalog(&paths)?;
    verify_fixture_contract(&fixture, &catalog, &catalog_sha256, &paths.source)?;
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
    validate_fixture_verification(&paths, &fixture, &catalog_sha256)?;
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

fn frozen_catalog(paths: &FrozenInputPaths) -> Result<(Catalog, String)> {
    let bytes = fs::read(&paths.catalog)
        .with_context(|| format!("read frozen catalog {}", paths.catalog.display()))?;
    let sha256 = sha256_bytes(&bytes);
    let catalog: Catalog = serde_json::from_slice(&bytes).context("parse frozen catalog")?;
    Ok((catalog, sha256))
}

fn verify_fixture_contract(
    fixture: &Fixture,
    catalog: &Catalog,
    catalog_sha256: &str,
    source: &Path,
) -> Result<()> {
    ensure!(fixture.schema_version == 4, "unsupported fixture schema");
    ensure!(
        catalog.schema_version == 1 && catalog.queries.len() == 30,
        "reviewed catalog does not meet the declared profile"
    );
    ensure!(
        fixture.source.embedding_dim == DIMENSIONS && fixture.queries.len() == 30,
        "fixture does not meet the declared profile"
    );
    ensure!(
        fixture.catalog_sha256 == catalog_sha256
            && fixture.selection_seed == SELECTION_SEED
            && fixture.corpus_commit == catalog.corpus_commit,
        "fixture is not bound to the reviewed catalog and selection contract"
    );
    ensure!(
        fixture.selected_node_ids.len() == 100_000
            && fixture.incremental_node_ids.len() == INCREMENTAL_COUNT,
        "fixture does not contain the declared nested real-anchor input"
    );
    let selected = fixture
        .selected_node_ids
        .iter()
        .chain(fixture.incremental_node_ids.iter())
        .collect::<HashSet<_>>();
    ensure!(
        selected.len() == 100_000 + INCREMENTAL_COUNT,
        "fixture selection contains duplicate base or incremental node IDs"
    );
    ensure!(
        fixture.query_embedder.runtime_id == fixture.source.embedding_backend
            && fixture.query_embedder.embedding_dim == fixture.source.embedding_dim
            && fixture.query_embedder.query_prefix == CODERANK_QUERY_PREFIX_DEFAULT
            && !fixture.query_embedder.model_digest.is_empty()
            && !fixture.query_embedder.ggml_build_identity.is_empty()
            && !fixture.query_embedder.backend.is_empty()
            && !fixture.query_embedder.policy.is_empty()
            && !fixture.query_embedder.execution_device_names.is_empty()
            && !fixture.query_embedder.execution_backend_names.is_empty()
            && fixture.query_embedder.accelerator_execution_verified,
        "fixture query embedder identity is incomplete or incompatible with the source publication"
    );
    ensure!(
        fixture.corpus_commit == fixture.verified_corpus.commit
            && fixture.verified_corpus.commit.len() == 40
            && fixture
                .verified_corpus
                .commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            && fixture.verified_corpus.tree.len() == 40
            && fixture
                .verified_corpus
                .tree
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            && fixture.verified_corpus.worktree_clean,
        "fixture catalog source identity is incomplete or was not frozen from a clean checkout"
    );
    verify_reviewed_query_bindings(&fixture.queries, catalog, source)?;
    for fixture_query in &fixture.queries {
        ensure!(
            fixture
                .selected_node_ids
                .contains(&fixture_query.expected_node_id),
            "fixture selection omits reviewed catalog target {}",
            fixture_query.id
        );
        validate_vector(&fixture_query.id, &fixture_query.vector)?;
    }
    Ok(())
}

fn verify_reviewed_query_bindings(
    fixture_queries: &[FrozenQuery],
    catalog: &Catalog,
    source: &Path,
) -> Result<()> {
    ensure!(
        fixture_queries.len() == catalog.queries.len(),
        "fixture and reviewed catalog query counts differ"
    );
    let unique_catalog_ids = catalog
        .queries
        .iter()
        .map(|query| query.id.as_str())
        .collect::<HashSet<_>>();
    ensure!(
        unique_catalog_ids.len() == catalog.queries.len(),
        "reviewed catalog contains duplicate query IDs"
    );
    let resolved = resolve_catalog(source, catalog)?;
    for ((catalog_query, resolved_query), fixture_query) in catalog
        .queries
        .iter()
        .zip(resolved.iter())
        .zip(fixture_queries.iter())
    {
        ensure!(
            fixture_query.id == catalog_query.id
                && fixture_query.kind == catalog_query.kind
                && fixture_query.text == catalog_query.text
                && fixture_query.expected_node_id == resolved_query.expected_node_id
                && fixture_query.expected_document_hash == resolved_query.expected_document_hash
                && !fixture_query.expected_document_hash.is_empty(),
            "fixture query {} does not match reviewed catalog/source truth",
            catalog_query.id
        );
    }
    Ok(())
}

fn verify_query_vectors(queries: &[FrozenQuery], recomputed: &[Vec<f32>]) -> Result<()> {
    ensure!(
        queries.len() == recomputed.len(),
        "fixture query-vector verification count mismatch"
    );
    for (query, observed) in queries.iter().zip(recomputed) {
        validate_vector(&query.id, observed)?;
        ensure!(
            query.vector.len() == observed.len()
                && query
                    .vector
                    .iter()
                    .zip(observed)
                    .all(|(expected, actual)| expected.to_bits() == actual.to_bits()),
            "fixture query vector does not match live product embedding for {}",
            query.id
        );
    }
    Ok(())
}

fn query_vector_digest(queries: &[FrozenQuery]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"codestory-vector-spike-query-vectors-v1\0");
    for query in queries {
        hash_len(&mut digest, query.id.as_bytes());
        for value in &query.vector {
            digest.update(value.to_bits().to_le_bytes());
        }
    }
    hex::encode(digest.finalize())
}

fn expected_document_digest(queries: &[FrozenQuery]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"codestory-vector-spike-source-truth-v1\0");
    for query in queries {
        hash_len(&mut digest, query.id.as_bytes());
        hash_len(&mut digest, query.expected_node_id.as_bytes());
        hash_len(&mut digest, query.expected_document_hash.as_bytes());
    }
    hex::encode(digest.finalize())
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
        input.schema_version == 2,
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
    let catalog = resolve_frozen_artifact(&input_path, &input.catalog, "catalog")?;
    let fixture_verification = resolve_frozen_artifact(
        &input_path,
        &input.fixture_verification,
        "fixture verification",
    )?;
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
        catalog,
        fixture_verification,
        host_evidence,
        binary_sha256: input.binary_sha256,
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

fn frozen_regular_path(path: &Path, label: &str) -> Result<PathBuf> {
    reject_symlinked_path(path, label)?;
    require_regular_file(path, label)?;
    path.canonicalize()
        .with_context(|| format!("canonicalize {label} {}", path.display()))
}

fn validate_fixture_verification(
    paths: &FrozenInputPaths,
    fixture: &Fixture,
    catalog_sha256: &str,
) -> Result<()> {
    let verification: FixtureVerification =
        serde_json::from_slice(&fs::read(&paths.fixture_verification).with_context(|| {
            format!(
                "read frozen fixture verification {}",
                paths.fixture_verification.display()
            )
        })?)
        .context("parse frozen fixture verification")?;
    ensure!(
        verification.schema_version == 1
            && verification.source_database_sha256 == fixture.source.database_sha256
            && verification.source_generation_manifest_sha256
                == fixture.source.generation_manifest_sha256
            && verification.fixture_sha256 == sha256_file(&paths.fixture)?
            && verification.catalog_sha256 == catalog_sha256
            && verification.binary_sha256 == paths.binary_sha256
            && verification.corpus == fixture.verified_corpus
            && verification.publication == fixture.publication
            && verification.query_embedder == fixture.query_embedder
            && verification.selection_seed == fixture.selection_seed
            && verification.query_vector_digest == query_vector_digest(&fixture.queries)
            && verification.expected_document_digest == expected_document_digest(&fixture.queries),
        "fixture verification does not bind the frozen reviewed preparation"
    );
    Ok(())
}

fn verify_frozen_input_paths(paths: &FrozenInputPaths) -> Result<()> {
    let reloaded = load_frozen_input_paths(&paths.input_path, &paths.input_manifest_sha256)?;
    ensure!(
        reloaded.source == paths.source
            && reloaded.source_generation_manifest == paths.source_generation_manifest
            && reloaded.fixture == paths.fixture
            && reloaded.catalog == paths.catalog
            && reloaded.fixture_verification == paths.fixture_verification
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
    let identity = PerUserEmbeddingClient::for_runtime(runtime)
        .context("connect to the live query embedder")?
        .ensure_resident()
        .context("observe live query embedder identity")?;
    ensure!(
        identity.worker_alive && identity.load_error.is_none(),
        "query embedder is not live while freezing the fixture"
    );
    Ok(QueryEmbedderIdentity {
        runtime_id: embedding_runtime_id_for_runtime(runtime),
        embedding_dim: semantic_vector_dim(),
        query_prefix: CODERANK_QUERY_PREFIX_DEFAULT.into(),
        model_digest: identity.model_digest,
        ggml_build_identity: identity.ggml_build_identity,
        backend: identity.backend,
        policy: identity.policy,
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

fn require_isolated_embedding_authority() -> Result<()> {
    let directory = std::env::var_os(EMBEDDING_AUTHORITY_DIR_ENV)
        .filter(|value| !value.is_empty())
        .context("vector evidence embedding authority directory is required")?;
    let nonce = std::env::var(EMBEDDING_AUTHORITY_NONCE_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .context("vector evidence embedding authority nonce is required")?;
    ensure!(
        Path::new(&directory).is_absolute(),
        "vector evidence embedding authority directory must be absolute"
    );
    ensure!(
        nonce.len() <= 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "vector evidence embedding authority nonce is invalid"
    );
    Ok(())
}

fn verify_catalog_corpus_identity(
    project_root: &Path,
    catalog_commit: &str,
) -> Result<VerifiedCorpusIdentity> {
    ensure!(
        catalog_commit.len() == 40 && catalog_commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "catalog corpus_commit must be a full Git object id"
    );
    let repository_root = git_output(project_root, ["rev-parse", "--show-toplevel"])?;
    let repository_root = PathBuf::from(repository_root)
        .canonicalize()
        .context("canonicalize Git repository root")?;
    ensure!(
        repository_root == project_root,
        "project root must be the Git checkout root for catalog source identity"
    );
    let commit = git_output(project_root, ["rev-parse", "HEAD"])?;
    ensure!(
        commit == catalog_commit,
        "catalog corpus_commit {catalog_commit} does not match project HEAD {commit}"
    );
    let status = git_output(
        project_root,
        [
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    ensure!(
        status.is_empty(),
        "catalog source checkout must be clean before collection"
    );
    Ok(VerifiedCorpusIdentity {
        commit,
        tree: git_output(project_root, ["rev-parse", "HEAD^{tree}"])?,
        worktree_clean: true,
    })
}

fn git_output<const N: usize>(project_root: &Path, args: [&str; N]) -> Result<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .with_context(|| format!("run git in {}", project_root.display()))?;
    ensure!(
        output.status.success(),
        "git {} failed in {}: {}",
        args.join(" "),
        project_root.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout)
        .context("Git output was not UTF-8")
        .map(|value| value.trim().to_owned())
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

fn selected_ordinals_after_incremental(
    fixture: &Fixture,
    count: usize,
) -> Result<HashMap<String, u64>> {
    let mut selected = selected_ordinals(fixture, count)?;
    for (offset, node_id) in fixture.incremental_node_ids.iter().enumerate() {
        ensure!(
            selected
                .insert(node_id.clone(), count as u64 + offset as u64 + 1)
                .is_none(),
            "incremental fixture node {node_id} was already present in the base selection"
        );
    }
    Ok(selected)
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
    let processed = AtomicUsize::new(0);
    build_generation_with_control(
        backend,
        source,
        selected,
        dir,
        binding,
        BuildControl {
            processed: &processed,
            cancel_after: None,
        },
    )
}

fn build_generation_with_control(
    backend: Backend,
    source: &Path,
    selected: &HashMap<String, u64>,
    dir: &Path,
    binding: FixtureBinding<'_>,
    control: BuildControl<'_>,
) -> Result<()> {
    fs::create_dir_all(dir)?;
    let index = dir.join(backend.index_name());
    match backend {
        Backend::SqliteVec => build_sqlite_vec(&index, source, selected, control)?,
        Backend::Usearch => build_usearch(&index, source, selected, control)?,
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
            index_handle.reserve(base_count + tail.len())?;
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

fn build_sqlite_vec(
    index: &Path,
    source: &Path,
    selected: &HashMap<String, u64>,
    control: BuildControl<'_>,
) -> Result<()> {
    register_sqlite_vec()?;
    let mut conn = Connection::open(index)?;
    conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; CREATE VIRTUAL TABLE vectors USING vec0(embedding float[768] distance_metric=cosine);")?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut insert = tx.prepare("INSERT INTO vectors(rowid, embedding) VALUES (?1, ?2)")?;
    let mut added = 0usize;
    stream_vectors_with_control(source, selected, control, |ordinal, vector| {
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

fn build_usearch(
    index: &Path,
    source: &Path,
    selected: &HashMap<String, u64>,
    control: BuildControl<'_>,
) -> Result<()> {
    let options = IndexOptions {
        dimensions: DIMENSIONS,
        metric: MetricKind::Cos,
        quantization: ScalarKind::F32,
        ..Default::default()
    };
    let index_handle = Index::new(&options)?;
    index_handle.reserve(selected.len())?;
    let mut added = 0usize;
    stream_vectors_with_control(source, selected, control, |ordinal, vector| {
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
    visit: impl FnMut(u64, Vec<f32>) -> Result<()>,
) -> Result<()> {
    let processed = AtomicUsize::new(0);
    stream_vectors_with_control(
        source,
        selection,
        BuildControl {
            processed: &processed,
            cancel_after: None,
        },
        visit,
    )
}

fn stream_vectors_with_control(
    source: &Path,
    selection: &HashMap<String, u64>,
    control: BuildControl<'_>,
    mut visit: impl FnMut(u64, Vec<f32>) -> Result<()>,
) -> Result<()> {
    let conn = open_read_only(source)?;
    let mut statement = conn.prepare("SELECT node_id, vector FROM vectors ORDER BY node_id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        if let Some(ordinal) = selection.get(&id).copied() {
            control.before_next()?;
            visit(ordinal, decode_vector(&row.get::<_, Vec<u8>>(1)?)?)?;
            control.record_processed();
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
    fn count(&self) -> Result<usize> {
        match self {
            Self::Sqlite(conn) => {
                let count = conn.query_row("SELECT count(*) FROM vectors", [], |row| {
                    row.get::<_, i64>(0)
                })?;
                Ok(count.max(0) as usize)
            }
            Self::Usearch(index) => Ok(index.size()),
        }
    }
}

fn open_candidate_index(backend: Backend, index: &Path) -> Result<Handle> {
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

fn open_generation(
    backend: Backend,
    root: &Path,
    generation: &str,
    binding: FixtureBinding<'_>,
) -> Result<Handle> {
    validate_generation_name(generation)?;
    let dir = root.join("generations").join(generation);
    validate_generation(&dir, backend, binding)?;
    let index = dir.join(backend.index_name());
    open_candidate_index(backend, &index)
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
            && manifest.backend_version == backend.version()
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
    ensure!(
        open_candidate_index(backend, &dir.join(backend.index_name()))?.count()? == manifest.count,
        "generation index count does not match its manifest"
    );
    Ok(())
}

fn rebind_manifest_index_digest(dir: &Path, backend: Backend) -> Result<()> {
    let manifest_path = dir.join("manifest.json");
    let mut manifest: GenerationManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    manifest.index_sha256 = sha256_file(&dir.join(backend.index_name()))?;
    atomic_write_json(&manifest_path, &manifest)
}

fn publish_generation(
    root: &Path,
    current: &str,
    rollback: Option<&str>,
    backend: Backend,
    binding: FixtureBinding<'_>,
) -> Result<()> {
    validate_generation_name(current)?;
    validate_generation(&root.join("generations").join(current), backend, binding)?;
    if let Some(rollback) = rollback {
        validate_generation_name(rollback)?;
        validate_generation(&root.join("generations").join(rollback), backend, binding)?;
    }
    atomic_write_json(
        &root.join("publication.json"),
        &Pointer {
            schema_version: 1,
            current: current.into(),
            rollback: rollback.map(str::to_owned),
        },
    )
}

fn validate_generation_name(generation: &str) -> Result<()> {
    let mut components = Path::new(generation).components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none(),
        "candidate generation name must be one plain path component"
    );
    Ok(())
}

/// A two-phase reader/publish barrier.
///
/// Each worker reads the current pointer and opens its handle before it reports
/// ready. The publisher only proceeds once every worker is holding the old
/// generation, and the workers do not issue their query until the publisher
/// releases them after atomically replacing the pointer.
struct ReaderPublishBarrier {
    releases: Vec<mpsc::SyncSender<()>>,
    results: Vec<mpsc::Receiver<std::result::Result<bool, String>>>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl ReaderPublishBarrier {
    fn release_after_publish(self) -> Result<bool> {
        for release in &self.releases {
            release.send(()).map_err(|_| {
                anyhow::anyhow!("reader exited before the publish barrier released")
            })?;
        }
        let mut all_pinned = true;
        for result in self.results {
            let result = result.recv().map_err(|_| {
                anyhow::anyhow!("reader exited without reporting its pinned result")
            })?;
            all_pinned &= result.map_err(anyhow::Error::msg)?;
        }
        for worker in self.workers {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("reader panicked during publish overlap"))?;
        }
        Ok(all_pinned)
    }
}

fn open_readers_before_publish(
    backend: Backend,
    root: &Path,
    binding: FixtureBinding<'_>,
    query: &[f32],
    expected: &[u64],
) -> Result<ReaderPublishBarrier> {
    const READER_COUNT: usize = 2;
    let mut opened = Vec::with_capacity(READER_COUNT);
    let mut releases = Vec::with_capacity(READER_COUNT);
    let mut results = Vec::with_capacity(READER_COUNT);
    let mut workers = Vec::new();
    for _ in 0..READER_COUNT {
        let root = root.to_path_buf();
        let query = query.to_vec();
        let expected = expected.to_vec();
        let input_manifest_sha256 = binding.input_manifest_sha256.to_owned();
        let source_database_sha256 = binding.source_database_sha256.to_owned();
        let source_generation_manifest_sha256 =
            binding.source_generation_manifest_sha256.to_owned();
        let fixture_sha256 = binding.fixture_sha256.to_owned();
        let query_embedder = binding.query_embedder.clone();
        let (opened_tx, opened_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        workers.push(std::thread::spawn(move || {
            let handle = match open_current_generation(
                backend,
                &root,
                FixtureBinding {
                    input_manifest_sha256: &input_manifest_sha256,
                    source_database_sha256: &source_database_sha256,
                    source_generation_manifest_sha256: &source_generation_manifest_sha256,
                    fixture_sha256: &fixture_sha256,
                    query_embedder: &query_embedder,
                },
            ) {
                Ok(handle) => handle,
                Err(error) => {
                    let _ = opened_tx.send(Err(error.to_string()));
                    return;
                }
            };
            if opened_tx.send(Ok(())).is_err() {
                return;
            }
            if release_rx.recv().is_err() {
                return;
            }
            let result = handle
                .search(&query)
                .map(|hits| hits == expected)
                .map_err(|error| error.to_string());
            let _ = result_tx.send(result);
        }));
        opened.push(opened_rx);
        releases.push(release_tx);
        results.push(result_rx);
    }
    for reader in opened {
        match reader
            .recv()
            .map_err(|_| anyhow::anyhow!("reader exited before opening the current generation"))?
        {
            Ok(()) => {}
            Err(error) => anyhow::bail!("reader could not pin the old generation: {error}"),
        }
    }
    Ok(ReaderPublishBarrier {
        releases,
        results,
        workers,
    })
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
    fn embedding_server_entrypoint_is_hidden_but_parsable() {
        let parsed = Cli::try_parse_from(["vector_backend_spike", "internal-embedding-server"])
            .expect("internal embedding server entrypoint should parse");
        assert!(matches!(parsed.command, Command::InternalEmbeddingServer));
        let help = <Cli as clap::CommandFactory>::command()
            .render_long_help()
            .to_string();
        assert!(!help.contains("internal-embedding-server"));
        assert!(!help.contains("verify-fixture"));
    }

    #[test]
    fn reviewed_catalog_binding_rejects_fabricated_source_truth() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("vectors.sqlite3");
        let conn = Connection::open(&source)?;
        conn.execute_batch(
            "CREATE TABLE vectors (
                node_id TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                display_name TEXT NOT NULL,
                document_hash TEXT NOT NULL
            );",
        )?;
        conn.execute(
            "INSERT INTO vectors VALUES ('node-real', 'kernel/example.c', 'real_symbol', 'doc-real')",
            [],
        )?;
        drop(conn);
        let catalog = Catalog {
            schema_version: 1,
            corpus_commit: "a".repeat(40),
            queries: vec![CatalogQuery {
                id: "query-real".into(),
                kind: "symbol".into(),
                text: "Where is the real symbol?".into(),
                file_path: "kernel/example.c".into(),
                symbol: "real_symbol".into(),
            }],
        };
        let mut queries = vec![FrozenQuery {
            id: "query-real".into(),
            kind: "symbol".into(),
            text: "Where is the real symbol?".into(),
            expected_node_id: "node-real".into(),
            expected_document_hash: "doc-real".into(),
            vector: vec![0.0; DIMENSIONS],
        }];
        verify_reviewed_query_bindings(&queries, &catalog, &source)?;
        queries[0].expected_document_hash = "caller-fabricated".into();
        assert!(verify_reviewed_query_bindings(&queries, &catalog, &source).is_err());
        queries[0].expected_document_hash = "doc-real".into();
        queries[0].text = "caller changed reviewed text".into();
        assert!(verify_reviewed_query_bindings(&queries, &catalog, &source).is_err());
        Ok(())
    }

    #[test]
    fn query_vector_verification_rejects_caller_fabrication() -> Result<()> {
        let mut expected = vec![0.0; DIMENSIONS];
        expected[0] = 1.0;
        let queries = vec![FrozenQuery {
            id: "query".into(),
            kind: "symbol".into(),
            text: "query text".into(),
            expected_node_id: "node".into(),
            expected_document_hash: "document".into(),
            vector: expected.clone(),
        }];
        verify_query_vectors(&queries, std::slice::from_ref(&expected))?;
        let mut fabricated = expected;
        fabricated[0] = 0.5;
        fabricated[1] = (0.75f32).sqrt();
        assert!(verify_query_vectors(&queries, &[fabricated]).is_err());
        Ok(())
    }

    #[test]
    fn catalog_corpus_identity_requires_the_clean_named_checkout() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        git_output(&root, ["init"])?;
        git_output(
            &root,
            ["config", "user.email", "vector-spike@example.invalid"],
        )?;
        git_output(&root, ["config", "user.name", "Vector Spike"])?;
        fs::write(root.join("tracked.txt"), b"source truth")?;
        git_output(&root, ["add", "tracked.txt"])?;
        git_output(&root, ["commit", "-m", "fixture source"])?;
        let head = git_output(&root, ["rev-parse", "HEAD"])?;

        let identity = verify_catalog_corpus_identity(&root, &head)?;
        assert_eq!(identity.commit, head);
        assert_eq!(identity.tree.len(), 40);
        assert!(identity.worktree_clean);
        let wrong_commit = "0".repeat(40);
        assert!(verify_catalog_corpus_identity(&root, &wrong_commit).is_err());

        fs::write(root.join("tracked.txt"), b"changed source")?;
        assert!(verify_catalog_corpus_identity(&root, &head).is_err());
        Ok(())
    }

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
            vector[0] = if ordinal <= 24 { 1.0 } else { -1.0 };
            conn.execute(
                "INSERT INTO vectors(node_id, vector) VALUES (?1, ?2)",
                params![format!("node-{ordinal:04}"), vector_bytes(&vector)],
            )?;
        }
        drop(conn);

        let base_selection = (1..=24u64)
            .map(|ordinal| (format!("node-{ordinal:04}"), ordinal))
            .collect::<HashMap<_, _>>();
        let incremental = (25..=32u64)
            .map(|ordinal| format!("node-{ordinal:04}"))
            .collect::<Vec<_>>();
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
            let generation_one = root.join("generations").join("generation-1");
            build_generation(backend, &source, &base_selection, &generation_one, binding)?;
            publish_generation(&root, "generation-1", None, backend, binding)?;

            let handle = open_current_generation(backend, &root, binding)?;
            assert_eq!(handle.count()?, base_selection.len());
            let expected = handle.search(&query)?;
            assert_eq!(expected.len(), TOP_K);

            let readers = open_readers_before_publish(backend, &root, binding, &query, &expected)?;
            let generation_two = root.join("generations").join("generation-2");
            build_incremental_generation(
                backend,
                &generation_one,
                &generation_two,
                base_selection.len(),
                &source,
                &incremental,
                binding,
            )?;
            publish_generation(
                &root,
                "generation-2",
                Some("generation-1"),
                backend,
                binding,
            )?;
            let post_publish = open_current_generation(backend, &root, binding)?;
            assert_eq!(post_publish.count()?, 32);
            assert_eq!(post_publish.search(&query)?, expected);
            assert!(readers.release_after_publish()?);
            assert_eq!(handle.search(&query)?, expected);
            drop(post_publish);

            let pointer_before_fault = fs::read(root.join("publication.json"))?;
            let corrupt = root.join("generations/generation-corrupt");
            fs::create_dir_all(&corrupt)?;
            fs::copy(
                generation_two.join(backend.index_name()),
                corrupt.join(backend.index_name()),
            )?;
            fs::copy(
                generation_two.join("manifest.json"),
                corrupt.join("manifest.json"),
            )?;
            tamper_file(&corrupt.join(backend.index_name()))?;
            rebind_manifest_index_digest(&corrupt, backend)?;
            assert!(
                publish_generation(
                    &root,
                    "generation-corrupt",
                    Some("generation-2"),
                    backend,
                    binding,
                )
                .is_err()
            );

            let incomplete = root.join("generations/generation-incomplete");
            fs::create_dir_all(&incomplete)?;
            fs::copy(
                generation_two.join(backend.index_name()),
                incomplete.join(backend.index_name()),
            )?;
            assert!(
                publish_generation(
                    &root,
                    "generation-incomplete",
                    Some("generation-2"),
                    backend,
                    binding,
                )
                .is_err()
            );

            let processed = AtomicUsize::new(0);
            let cancelled = root.join("generations/generation-cancelled");
            let cancellation = build_generation_with_control(
                backend,
                &source,
                &base_selection,
                &cancelled,
                binding,
                BuildControl {
                    processed: &processed,
                    cancel_after: Some(3),
                },
            )
            .expect_err("candidate build should cancel after work begins");
            assert_eq!(processed.load(Ordering::Acquire), 3);
            assert!(
                cancellation
                    .to_string()
                    .contains("candidate_build_cancelled_after_vectors:3")
            );
            assert!(
                publish_generation(
                    &root,
                    "generation-cancelled",
                    Some("generation-2"),
                    backend,
                    binding,
                )
                .is_err()
            );
            assert_eq!(
                fs::read(root.join("publication.json"))?,
                pointer_before_fault
            );

            tamper_file(&generation_two.join(backend.index_name()))?;
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
        let catalog = temp_root.join("catalog.json");
        let fixture_verification = temp_root.join("fixture-verification.json");
        let host_evidence = temp_root.join("host-evidence.json");
        fs::write(&source, b"source")?;
        fs::write(&source_manifest, b"manifest")?;
        fs::write(&fixture, b"fixture")?;
        fs::write(&catalog, b"catalog")?;
        fs::write(&fixture_verification, b"verification")?;
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
            "schema_version": 2,
            "source": { "path": "source.sqlite3", "sha256": sha256_file(&source)? },
            "source_generation_manifest": {
                "path": "vector-generation-manifest.json",
                "sha256": sha256_file(&source_manifest)?,
            },
            "fixture": { "path": "fixture.json", "sha256": sha256_file(&fixture)? },
            "catalog": { "path": "catalog.json", "sha256": sha256_file(&catalog)? },
            "fixture_verification": {
                "path": "fixture-verification.json",
                "sha256": sha256_file(&fixture_verification)?,
            },
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
