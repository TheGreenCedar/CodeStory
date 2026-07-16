use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Once};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

const CRITERIA: &str = include_str!("../../../benchmarks/vector-backend-spike/criteria.json");
const TOP_K: usize = 20;
const DECISION_DIMENSIONS: usize = 768;
const DECISION_QUERIES: usize = 30;
const DECISION_WARMUPS: usize = 5;
const DECISION_COUNTS: [usize; 4] = [1_000, 10_000, 25_000, 100_000];
const VECTOR_NORM_TOLERANCE: f64 = 1.0e-3;
const VECTOR_DIGEST_DOMAIN: &[u8] = b"codestory-vector-digest-v1\0";
const PRODUCTION_VECTOR_SCHEMA_VERSION: i64 = 2;
const REPETITIONS: usize = 2;

static REGISTER_SQLITE_VEC: Once = Once::new();

#[derive(Debug)]
struct Config {
    profile: String,
    vector_count: usize,
    dimensions: usize,
    query_count: usize,
    warmups: usize,
    source_sqlite: Option<PathBuf>,
    fixture_json: Option<PathBuf>,
    output: PathBuf,
}

impl Config {
    fn from_env() -> Result<Self> {
        let profile =
            std::env::var("CODESTORY_VECTOR_SPIKE_PROFILE").unwrap_or_else(|_| "smoke".to_owned());
        ensure!(
            profile == "smoke" || profile == "decision",
            "unknown profile {profile}"
        );
        let source_sqlite =
            std::env::var_os("CODESTORY_VECTOR_SPIKE_SOURCE_SQLITE").map(PathBuf::from);
        let fixture_json =
            std::env::var_os("CODESTORY_VECTOR_SPIKE_FIXTURE_JSON").map(PathBuf::from);
        let vector_count = parse_env_usize(
            "CODESTORY_VECTOR_SPIKE_VECTOR_COUNT",
            if profile == "decision" { 100_000 } else { 512 },
        )?;
        let dimensions = if profile == "decision" {
            DECISION_DIMENSIONS
        } else {
            parse_env_usize("CODESTORY_VECTOR_SPIKE_DIMENSIONS", DECISION_DIMENSIONS)?
        };
        let query_count = if profile == "decision" {
            DECISION_QUERIES
        } else {
            parse_env_usize("CODESTORY_VECTOR_SPIKE_QUERY_COUNT", 8)?
        };
        let warmups = if profile == "decision" {
            DECISION_WARMUPS
        } else {
            parse_env_usize("CODESTORY_VECTOR_SPIKE_WARMUPS", 2)?
        };
        ensure!(vector_count > 0, "vector count must be positive");
        ensure!(dimensions > 0, "dimensions must be positive");
        ensure!(query_count > 1, "query count must be at least two");

        if profile == "decision" {
            ensure!(
                DECISION_COUNTS.contains(&vector_count),
                "decision vector count must be one of {DECISION_COUNTS:?}"
            );
            ensure!(
                source_sqlite.is_some(),
                "decision profile requires a complete CodeStory vector publication"
            );
            ensure!(
                fixture_json.is_some(),
                "decision profile requires an evidence-bound frozen query and incremental fixture"
            );
        }

        let default_output = workspace_root()
            .join("target")
            .join("vector-backend-spike")
            .join(format!(
                "{}-{}-{profile}-{vector_count}.json",
                std::env::consts::OS,
                std::env::consts::ARCH
            ));
        let output = std::env::var_os("CODESTORY_VECTOR_SPIKE_OUTPUT")
            .map(PathBuf::from)
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    workspace_root().join(path)
                }
            })
            .unwrap_or(default_output);

        Ok(Self {
            profile,
            vector_count,
            dimensions,
            query_count,
            warmups,
            source_sqlite,
            fixture_json,
            output,
        })
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("benchmark crate must be beneath the workspace root")
        .to_owned()
}

fn parse_env_usize(name: &str, default: usize) -> Result<usize> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .with_context(|| format!("parse {name}={value}")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error).with_context(|| format!("read {name}")),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
struct Identity {
    node_id: String,
    document_hash: String,
}

#[derive(Clone, Debug)]
struct VectorRecord {
    identity: Identity,
    vector: Vec<f32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PublicationIdentity {
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ProductionSourceAttestation {
    #[serde(flatten)]
    publication: PublicationIdentity,
    database_sha256: String,
}

impl ProductionSourceAttestation {
    fn validate(&self) -> Result<()> {
        self.publication.validate()?;
        ensure!(
            self.publication.schema_version == PRODUCTION_VECTOR_SCHEMA_VERSION,
            "source attestation schema version must match the production vector contract"
        );
        validate_sha256(
            "source attestation vector_digest",
            &self.publication.vector_digest,
        )?;
        validate_sha256("source attestation database_sha256", &self.database_sha256)
    }
}

impl PublicationIdentity {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version > 0,
            "publication schema version must be positive"
        );
        ensure!(
            self.embedding_dim > 0,
            "publication embedding dimension must be positive"
        );
        ensure!(
            self.point_count > 0,
            "publication point count must be positive"
        );
        for (name, value) in [
            ("generation", &self.generation),
            ("input_hash", &self.input_hash),
            ("embedding_backend", &self.embedding_backend),
            ("producer_identity", &self.producer_identity),
            (
                "evidence_contract_identity",
                &self.evidence_contract_identity,
            ),
            ("vector_digest", &self.vector_digest),
        ] {
            ensure!(
                !value.trim().is_empty(),
                "publication {name} must be non-empty"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum QueryKind {
    Representative,
    Symbol,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FrozenQuery {
    query_id: String,
    kind: QueryKind,
    vector: Vec<f32>,
    expected: Vec<Identity>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FrozenVectorRecord {
    node_id: String,
    document_hash: String,
    vector: Vec<f32>,
}

#[derive(Deserialize)]
struct FrozenFixture {
    schema_version: u32,
    source_attestation: ProductionSourceAttestation,
    incremental_set_id: String,
    queries: Vec<FrozenQuery>,
    incremental_records: Vec<FrozenVectorRecord>,
}

struct Dataset {
    records: Vec<VectorRecord>,
    queries: Vec<FrozenQuery>,
    incremental_records: Vec<VectorRecord>,
    publication: PublicationIdentity,
    source_attestation: Option<ProductionSourceAttestation>,
    source_label: String,
    source_artifact_sha256: String,
    fixture_label: String,
    fixture_sha256: String,
    incremental_set_id: String,
}

impl Dataset {
    fn load(config: &Config) -> Result<Self> {
        let loaded_fixture = match (&config.source_sqlite, &config.fixture_json) {
            (Some(_), Some(path)) => Some(read_frozen_fixture(path)?),
            (None, None) => None,
            (Some(_), None) => {
                anyhow::bail!("a production source requires a predeclared source attestation")
            }
            (None, Some(_)) => anyhow::bail!("a frozen fixture requires its attested source"),
        };
        let (records, publication, source_attestation, source_label, source_artifact_sha256) =
            match (&config.source_sqlite, &loaded_fixture) {
                (Some(path), Some(loaded)) => {
                    let (records, publication, source_label, source_artifact_sha256) =
                        load_published_vectors(
                            path,
                            config.vector_count,
                            &loaded.fixture.source_attestation,
                        )?;
                    (
                        records,
                        publication,
                        Some(loaded.fixture.source_attestation.clone()),
                        source_label,
                        source_artifact_sha256,
                    )
                }
                (None, None) => {
                    let (records, publication, source_label, source_artifact_sha256) =
                        synthetic_publication(config.vector_count, config.dimensions);
                    (
                        records,
                        publication,
                        None,
                        source_label,
                        source_artifact_sha256,
                    )
                }
                _ => unreachable!("source and fixture presence was validated"),
            };
        publication.validate()?;
        ensure!(
            publication.embedding_dim == config.dimensions,
            "expected {} dimensions, found {}",
            config.dimensions,
            publication.embedding_dim
        );
        ensure!(
            records.len() == config.vector_count,
            "expected {} selected vectors, found {}",
            config.vector_count,
            records.len()
        );
        validate_records(&records, config.dimensions)?;

        let (queries, incremental_records, incremental_set_id, fixture_label, fixture_sha256) =
            match loaded_fixture {
                Some(loaded) => select_frozen_fixture(
                    loaded,
                    &publication,
                    &records,
                    config.dimensions,
                    config.query_count,
                )?,
                None => synthetic_fixture(&publication, &records, config.query_count),
            };
        validate_queries(&queries, &records, config.dimensions)?;
        validate_records(&incremental_records, config.dimensions)?;
        let base_identities = records
            .iter()
            .map(|record| record.identity.clone())
            .collect::<HashSet<_>>();
        ensure!(
            incremental_records
                .iter()
                .all(|record| !base_identities.contains(&record.identity)),
            "incremental identities must not collide with the base publication"
        );

        if config.profile == "decision" {
            ensure!(
                queries.len() == DECISION_QUERIES,
                "decision fixture must contain exactly {DECISION_QUERIES} selected queries"
            );
            ensure!(
                !incremental_records.is_empty(),
                "decision fixture must contain frozen incremental records"
            );
            ensure!(
                !incremental_set_id.trim().is_empty(),
                "decision incremental set identity must be non-empty"
            );
        }

        Ok(Self {
            records,
            queries,
            incremental_records,
            publication,
            source_attestation,
            source_label,
            source_artifact_sha256,
            fixture_label,
            fixture_sha256,
            incremental_set_id,
        })
    }
}

fn load_published_vectors(
    path: &Path,
    limit: usize,
    expected_attestation: &ProductionSourceAttestation,
) -> Result<(Vec<VectorRecord>, PublicationIdentity, String, String)> {
    load_published_vectors_with_hook(path, limit, expected_attestation, || Ok(()))
}

fn load_published_vectors_with_hook(
    path: &Path,
    limit: usize,
    expected_attestation: &ProductionSourceAttestation,
    before_sample: impl FnOnce() -> Result<()>,
) -> Result<(Vec<VectorRecord>, PublicationIdentity, String, String)> {
    expected_attestation.validate()?;
    ensure_no_sqlite_sidecars(path)?;
    let database_sha256 = sha256_file(path)
        .with_context(|| format!("hash attested vector database {}", path.display()))?;
    ensure!(
        database_sha256 == expected_attestation.database_sha256,
        "vector database SHA-256 does not match the predeclared source attestation"
    );
    let connection = open_sqlite_read_only(path)?;
    connection.execute_batch("BEGIN DEFERRED TRANSACTION")?;
    let quick_check: String =
        connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    ensure!(
        quick_check == "ok",
        "source SQLite quick_check failed: {quick_check}"
    );
    let metadata_rows: i64 =
        connection.query_row("SELECT count(*) FROM metadata", [], |row| row.get(0))?;
    ensure!(
        metadata_rows == 1,
        "source metadata must contain exactly one row"
    );
    let metadata = connection
        .query_row(
            "SELECT schema_version, generation, input_hash, embedding_backend,
                    embedding_dim, point_count, producer_identity,
                    evidence_contract_identity, vector_digest
             FROM metadata WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .with_context(|| format!("read publication identity from {}", path.display()))?;
    let publication = PublicationIdentity {
        schema_version: metadata.0,
        generation: metadata.1,
        input_hash: metadata.2,
        embedding_backend: metadata.3,
        embedding_dim: usize::try_from(metadata.4)
            .context("publication embedding dimension must fit usize")?,
        point_count: usize::try_from(metadata.5)
            .context("publication point count must fit usize")?,
        producer_identity: metadata.6,
        evidence_contract_identity: metadata.7,
        vector_digest: metadata.8,
    };
    publication.validate()?;
    ensure!(
        publication == expected_attestation.publication,
        "source metadata does not match the predeclared publication identity"
    );
    let actual_count: i64 =
        connection.query_row("SELECT count(*) FROM vectors", [], |row| row.get(0))?;
    ensure!(
        actual_count == publication.point_count as i64,
        "publication metadata point count {} does not match complete table count {actual_count}",
        publication.point_count
    );
    ensure!(
        publication.point_count >= limit,
        "publication has {} points but the run requires {limit}",
        publication.point_count
    );

    let (canonical_digest, canonical_count) =
        canonical_source_vector_digest(&connection, publication.embedding_dim)?;
    ensure!(
        canonical_count == publication.point_count,
        "canonical source coverage expected {} rows, found {canonical_count}",
        publication.point_count
    );
    ensure!(
        canonical_digest == expected_attestation.publication.vector_digest,
        "canonical source vector digest does not match the predeclared attestation"
    );
    before_sample()?;

    let mut statement = connection.prepare(
        "SELECT node_id, document_hash, vector FROM vectors ORDER BY node_id ASC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    let mut records = Vec::with_capacity(limit);
    for row in rows {
        let (node_id, document_hash, bytes) = row?;
        records.push(VectorRecord {
            identity: Identity {
                node_id: node_id.clone(),
                document_hash,
            },
            vector: decode_vector(&node_id, &bytes, publication.embedding_dim)?,
        });
    }
    drop(statement);
    let database_sha256_after_sampling = sha256_file(path)
        .with_context(|| format!("rehash attested vector database {}", path.display()))?;
    ensure!(
        database_sha256_after_sampling == expected_attestation.database_sha256,
        "vector database changed while its pinned source snapshot was sampled"
    );
    ensure_no_sqlite_sidecars(path)?;
    connection.execute_batch("ROLLBACK")?;
    drop(connection);
    Ok((
        records,
        publication,
        format!("CodeStory publication {}", path.display()),
        expected_attestation.database_sha256.clone(),
    ))
}

fn canonical_source_vector_digest(
    connection: &Connection,
    embedding_dim: usize,
) -> Result<(String, usize)> {
    let mut statement = connection
        .prepare("SELECT node_id, document_hash, vector FROM vectors ORDER BY node_id ASC")?;
    let mut rows = statement.query([])?;
    let mut digest = Sha256::new();
    digest.update(VECTOR_DIGEST_DOMAIN);
    let mut seen = HashSet::new();
    let mut count = 0_usize;
    while let Some(row) = rows.next()? {
        let node_id: String = row.get(0)?;
        let document_hash: String = row.get(1)?;
        let vector: Vec<u8> = row.get(2)?;
        ensure!(
            seen.insert(node_id.clone()),
            "duplicate source vector row {node_id}"
        );
        ensure!(
            !node_id.trim().is_empty() && !document_hash.trim().is_empty(),
            "source vector row identities must be non-empty"
        );
        decode_vector(&node_id, &vector, embedding_dim)?;
        hash_len_prefixed(&mut digest, node_id.as_bytes());
        hash_len_prefixed(&mut digest, document_hash.as_bytes());
        hash_len_prefixed(&mut digest, &vector);
        count += 1;
    }
    Ok((format!("{:x}", digest.finalize()), count))
}

type FixtureParts = (Vec<FrozenQuery>, Vec<VectorRecord>, String, String, String);

struct LoadedFrozenFixture {
    fixture: FrozenFixture,
    label: String,
    sha256: String,
}

fn read_frozen_fixture(path: &Path) -> Result<LoadedFrozenFixture> {
    let bytes = fs::read(path).with_context(|| format!("read fixture {}", path.display()))?;
    let fixture: FrozenFixture = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse fixture {}", path.display()))?;
    ensure!(
        fixture.schema_version == 2,
        "fixture schema version must be 2"
    );
    fixture.source_attestation.validate()?;
    Ok(LoadedFrozenFixture {
        fixture,
        label: format!("frozen fixture {}", path.display()),
        sha256: sha256_bytes(&bytes),
    })
}

fn select_frozen_fixture(
    loaded: LoadedFrozenFixture,
    publication: &PublicationIdentity,
    records: &[VectorRecord],
    dimensions: usize,
    query_count: usize,
) -> Result<FixtureParts> {
    let fixture = loaded.fixture;
    ensure!(
        fixture.source_attestation.publication == *publication,
        "fixture attestation does not exactly match the complete source publication"
    );
    ensure!(
        fixture.queries.len() >= query_count,
        "fixture needs at least {query_count} queries"
    );
    let queries = fixture
        .queries
        .into_iter()
        .take(query_count)
        .map(|mut query| {
            query.vector = checked_vector(&query.query_id, query.vector, dimensions)?;
            Ok(query)
        })
        .collect::<Result<Vec<_>>>()?;
    let incremental = fixture
        .incremental_records
        .into_iter()
        .map(|record| frozen_record(record, dimensions))
        .collect::<Result<Vec<_>>>()?;
    validate_queries(&queries, records, dimensions)?;
    Ok((
        queries,
        incremental,
        fixture.incremental_set_id,
        loaded.label,
        loaded.sha256,
    ))
}

fn synthetic_publication(
    count: usize,
    dimensions: usize,
) -> (Vec<VectorRecord>, PublicationIdentity, String, String) {
    let records = (0..count)
        .map(|index| VectorRecord {
            identity: Identity {
                node_id: format!("synthetic-node-{index:06}"),
                document_hash: sha256_bytes(format!("synthetic-document-{index}").as_bytes()),
            },
            vector: synthetic_vector(index as u64, dimensions),
        })
        .collect::<Vec<_>>();
    let digest = records_sha256(&records);
    let publication = PublicationIdentity {
        schema_version: 1,
        generation: "synthetic-smoke-generation".to_owned(),
        input_hash: sha256_bytes(b"synthetic-smoke-input"),
        embedding_backend: "synthetic-smoke".to_owned(),
        embedding_dim: dimensions,
        point_count: count,
        producer_identity: "vector-backend-spike-smoke".to_owned(),
        evidence_contract_identity: "synthetic-not-decision-evidence".to_owned(),
        vector_digest: digest.clone(),
    };
    (
        records,
        publication,
        "deterministic synthetic smoke publication".to_owned(),
        digest,
    )
}

fn synthetic_fixture(
    publication: &PublicationIdentity,
    records: &[VectorRecord],
    count: usize,
) -> FixtureParts {
    let queries = (0..count)
        .map(|index| {
            let record = &records[index * records.len() / count];
            FrozenQuery {
                query_id: format!("synthetic-query-{index:03}"),
                kind: if index % 2 == 0 {
                    QueryKind::Representative
                } else {
                    QueryKind::Symbol
                },
                vector: record.vector.clone(),
                expected: vec![record.identity.clone()],
            }
        })
        .collect::<Vec<_>>();
    let incremental_count = (records.len() / 100).clamp(1, 100);
    let incremental = (0..incremental_count)
        .map(|offset| VectorRecord {
            identity: Identity {
                node_id: format!("synthetic-increment-{offset:06}"),
                document_hash: sha256_bytes(format!("synthetic-increment-{offset}").as_bytes()),
            },
            vector: synthetic_vector((records.len() + offset) as u64, publication.embedding_dim),
        })
        .collect::<Vec<_>>();
    let fixture_hash = sha256_bytes(
        serde_json::to_string(&(publication, &queries))
            .expect("serialize synthetic fixture identity")
            .as_bytes(),
    );
    (
        queries,
        incremental,
        "synthetic-incremental-smoke".to_owned(),
        "deterministic synthetic smoke fixture".to_owned(),
        fixture_hash,
    )
}

fn frozen_record(record: FrozenVectorRecord, dimensions: usize) -> Result<VectorRecord> {
    Ok(VectorRecord {
        identity: Identity {
            node_id: record.node_id.clone(),
            document_hash: record.document_hash,
        },
        vector: checked_vector(&record.node_id, record.vector, dimensions)?,
    })
}

fn decode_vector(node_id: &str, bytes: &[u8], dimensions: usize) -> Result<Vec<f32>> {
    ensure!(
        bytes.len() == dimensions * 4,
        "vector byte length for {node_id} does not match {dimensions} dimensions"
    );
    checked_vector(
        node_id,
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("four-byte chunk")))
            .collect(),
        dimensions,
    )
}

fn checked_vector(label: &str, vector: Vec<f32>, dimensions: usize) -> Result<Vec<f32>> {
    ensure!(
        vector.len() == dimensions,
        "{label} expected {dimensions} values, found {}",
        vector.len()
    );
    let mut norm_squared = 0.0_f64;
    for value in &vector {
        ensure!(
            value.is_finite(),
            "{label} contains a non-finite vector value"
        );
        norm_squared += f64::from(*value) * f64::from(*value);
    }
    ensure!(
        norm_squared.is_finite() && norm_squared > f64::EPSILON,
        "{label} vector norm must be finite and non-zero"
    );
    let norm = norm_squared.sqrt();
    ensure!(
        (norm - 1.0).abs() <= VECTOR_NORM_TOLERANCE,
        "{label} vector must be L2-normalized; norm={norm:.8}"
    );
    Ok(vector)
}

fn validate_records(records: &[VectorRecord], dimensions: usize) -> Result<()> {
    ensure!(!records.is_empty(), "vector records must not be empty");
    let mut identities = HashSet::with_capacity(records.len());
    let mut node_ids = HashSet::with_capacity(records.len());
    for record in records {
        ensure!(
            !record.identity.node_id.trim().is_empty(),
            "node identity must be non-empty"
        );
        ensure!(
            !record.identity.document_hash.trim().is_empty(),
            "document hash must be non-empty"
        );
        ensure!(
            identities.insert(record.identity.clone()),
            "duplicate vector identity {} / {}",
            record.identity.node_id,
            record.identity.document_hash
        );
        ensure!(
            node_ids.insert(record.identity.node_id.clone()),
            "duplicate vector node identity {}",
            record.identity.node_id
        );
        checked_vector(&record.identity.node_id, record.vector.clone(), dimensions)?;
    }
    Ok(())
}

fn validate_queries(
    queries: &[FrozenQuery],
    records: &[VectorRecord],
    dimensions: usize,
) -> Result<()> {
    ensure!(!queries.is_empty(), "query set must not be empty");
    let identities = records
        .iter()
        .map(|record| record.identity.clone())
        .collect::<HashSet<_>>();
    let mut query_ids = HashSet::new();
    let mut representative = 0;
    let mut symbol = 0;
    for query in queries {
        ensure!(
            !query.query_id.trim().is_empty(),
            "query identity must be non-empty"
        );
        ensure!(
            query_ids.insert(&query.query_id),
            "duplicate query identity {}",
            query.query_id
        );
        checked_vector(&query.query_id, query.vector.clone(), dimensions)?;
        ensure!(
            !query.expected.is_empty(),
            "query {} needs expected identities",
            query.query_id
        );
        ensure!(
            query
                .expected
                .iter()
                .all(|expected| identities.contains(expected)),
            "query {} expects an identity outside the selected publication",
            query.query_id
        );
        match query.kind {
            QueryKind::Representative => representative += 1,
            QueryKind::Symbol => symbol += 1,
        }
    }
    ensure!(representative > 0, "query set needs representative queries");
    ensure!(symbol > 0, "query set needs representative symbol queries");
    Ok(())
}

fn synthetic_vector(seed: u64, dimensions: usize) -> Vec<f32> {
    let mut vector = vec![0.0; dimensions];
    let mut state = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    for _ in 0..24 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let slot = (state as usize) % dimensions;
        let value = (((state >> 32) % 2001) as f32 - 1000.0) / 1000.0;
        vector[slot] += value;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    for value in &mut vector {
        *value /= norm;
    }
    vector
}

#[derive(Clone, Copy)]
enum Candidate {
    SqliteVec,
    Usearch,
}

impl Candidate {
    fn from_name(name: &str) -> Result<Self> {
        match name {
            "sqlite-vec" => Ok(Self::SqliteVec),
            "usearch" => Ok(Self::Usearch),
            _ => anyhow::bail!("unknown vector backend in generation pointer: {name}"),
        }
    }

    fn name(self) -> &'static str {
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

    fn index_file(self) -> &'static str {
        match self {
            Self::SqliteVec => "index.sqlite3",
            Self::Usearch => "index.usearch",
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
struct GenerationPointer {
    schema_version: u32,
    backend: String,
    generation: String,
    manifest_sha256: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
struct PublicationPointers {
    schema_version: u32,
    current: GenerationPointer,
    rollback: Option<GenerationPointer>,
}

#[derive(Clone, Deserialize, Serialize)]
struct GenerationManifest {
    schema_version: u32,
    backend: String,
    backend_version: String,
    generation: String,
    metric: String,
    dimensions: usize,
    point_count: usize,
    index_sha256: String,
    directory_contents_sha256: String,
    records_sha256: String,
    identities_sha256: String,
    source_publication: PublicationIdentity,
    source_attestation: Option<ProductionSourceAttestation>,
    incremental_set_id: Option<String>,
}

#[derive(Serialize)]
struct Artifact {
    schema_version: u32,
    issue: u32,
    generated_at_unix_seconds: u64,
    criteria_sha256: String,
    profile: String,
    decision_profile: bool,
    decision_scope: &'static str,
    blocking_platform: &'static str,
    timing_comparable: bool,
    timing_protocol: &'static str,
    build: BuildIdentity,
    host: HostIdentity,
    source: SourceIdentity,
    fixture: FixtureIdentity,
    workload: Workload,
    runs: Vec<BackendResult>,
    limitations: Vec<&'static str>,
}

#[derive(Serialize)]
struct BuildIdentity {
    git_head: String,
    git_tree: String,
    git_dirty: bool,
    worktree_sha256: String,
    rustc: String,
    cargo: String,
    build_profile: &'static str,
    target: String,
}

#[derive(Serialize)]
struct HostIdentity {
    os: &'static str,
    architecture: &'static str,
    cpu_model: Option<String>,
    logical_cpus: Option<usize>,
    total_memory_bytes: Option<u64>,
    isa: Vec<&'static str>,
}

#[derive(Serialize)]
struct SourceIdentity {
    label: String,
    artifact_sha256: String,
    publication: PublicationIdentity,
    production_attestation: Option<ProductionSourceAttestation>,
    selected_records_sha256: String,
}

#[derive(Serialize)]
struct FixtureIdentity {
    label: String,
    artifact_sha256: String,
    incremental_set_id: String,
    incremental_records_sha256: String,
}

#[derive(Serialize)]
struct Workload {
    dimensions: usize,
    vector_count: usize,
    query_count: usize,
    top_k: usize,
    warmups: usize,
    repetitions: usize,
    metric: &'static str,
    records_sha256: String,
    queries_sha256: String,
}

#[derive(Serialize)]
struct BackendResult {
    repetition: usize,
    order_position: usize,
    backend: &'static str,
    version: &'static str,
    build_ms: f64,
    load_ms: f64,
    first_query_after_open_ms: f64,
    warm_query_p50_ms: f64,
    warm_query_p95_ms: f64,
    disk_bytes: u64,
    memory_bytes: Option<u64>,
    memory_method: &'static str,
    recall_at_20: f64,
    expected_identity_hit_at_20: f64,
    representative_query_hit_at_20: f64,
    symbol_query_hit_at_20: f64,
    incremental_reuse_ms: f64,
    concurrent_readers: usize,
    concurrent_reader_consistency: bool,
    pinned_old_reader_after_publication: bool,
    new_current_reader_observed_incremental: bool,
    old_generation_unchanged: bool,
    atomic_publication_pointer_pair: bool,
    referenced_generation_tamper_rejected: bool,
    pinned_reader_after_referenced_tamper: bool,
    corrupt_candidate_rejected: bool,
    failed_candidate_preserved_current_pointer: bool,
    rollback_pointer_readable: bool,
    pinned_incremental_reader_after_rollback: bool,
}

#[derive(Default)]
struct QueryMeasurements {
    first_after_open_ms: f64,
    warm_p50_ms: f64,
    warm_p95_ms: f64,
    recall: f64,
    expected_hit: f64,
    representative_hit: f64,
    symbol_hit: f64,
}

#[test]
fn decision_criteria_are_predeclared() {
    let criteria: serde_json::Value = serde_json::from_str(CRITERIA).expect("criteria JSON");
    assert_eq!(criteria["issue"], 1202);
    assert_eq!(criteria["schema_version"], 4);
    assert_eq!(
        criteria["decision_status"],
        "blocked_pending_required_evidence"
    );
    assert_eq!(
        criteria["shared_workload"]["dimensions"],
        DECISION_DIMENSIONS
    );
    assert_eq!(criteria["shared_workload"]["top_k"], TOP_K);
    assert_eq!(criteria["shared_workload"]["warmups"], DECISION_WARMUPS);
    assert_eq!(criteria["shared_workload"]["queries"], DECISION_QUERIES);
    assert_eq!(
        criteria["shared_workload"]["vector_counts"],
        serde_json::json!(DECISION_COUNTS)
    );
    assert_eq!(
        criteria["required_platforms"],
        serde_json::json!(["windows-x86_64"])
    );
    let adoption_follow_up = criteria["non_blocking_adoption_follow_up"]
        .as_array()
        .expect("non-blocking adoption follow-up list");
    assert!(adoption_follow_up.iter().any(|value| {
        value
            .as_str()
            .is_some_and(|value| value.contains("Linux x64"))
    }));
    assert!(adoption_follow_up.iter().any(|value| {
        value
            .as_str()
            .is_some_and(|value| value.contains("macOS arm64"))
    }));
    let decision_requires = criteria["decision_requires"]
        .as_array()
        .expect("blocking decision requirement list");
    for required in [
        "Windows x64 offline build",
        "license and native dependency review",
        "reversible fallback",
        "packaged archive size",
    ] {
        assert!(
            decision_requires
                .iter()
                .any(|value| { value.as_str().is_some_and(|value| value.contains(required)) })
        );
        assert!(
            !adoption_follow_up
                .iter()
                .any(|value| { value.as_str().is_some_and(|value| value.contains(required)) })
        );
    }
    let required = criteria["required_measurements"]
        .as_array()
        .expect("required measurement list");
    for measurement in [
        "cold_query_ms",
        "memory_bytes",
        "pinned_old_reader_after_publication",
        "atomic_publication_pointer_pair",
        "referenced_generation_tamper_rejected",
        "pinned_reader_after_referenced_tamper",
        "corrupt_candidate_rejected",
        "rollback_pointer_readable",
    ] {
        assert!(
            required.iter().any(|value| value == measurement),
            "missing required measurement {measurement}"
        );
    }
    let manifest: toml::Value =
        toml::from_str(include_str!("../Cargo.toml")).expect("bench manifest");
    let indexing = manifest["bench"]
        .as_array()
        .expect("bench targets")
        .iter()
        .find(|bench| bench["name"].as_str() == Some("indexing"))
        .expect("indexing bench target");
    assert_eq!(
        indexing["required-features"]
            .as_array()
            .expect("indexing required features")[0]
            .as_str(),
        Some("runtime-bench")
    );
    assert_eq!(criteria["synthetic_smoke_is_decision_evidence"], false);
}

#[test]
fn vector_contract_rejects_invalid_norms_and_values() {
    assert!(checked_vector("zero", vec![0.0, 0.0], 2).is_err());
    assert!(checked_vector("not-normalized", vec![2.0, 0.0], 2).is_err());
    assert!(checked_vector("non-finite", vec![f32::NAN, 0.0], 2).is_err());
    assert!(checked_vector("normalized", vec![1.0, 0.0], 2).is_ok());
}

#[test]
fn production_source_requires_and_revalidates_predeclared_attestation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("vectors.sqlite3");
    let attestation = write_test_source_database(&path)?;

    let (records, publication, _, database_sha256) =
        load_published_vectors(&path, 2, &attestation)?;
    assert_eq!(records.len(), 2);
    assert_eq!(publication, attestation.publication);
    assert_eq!(database_sha256, attestation.database_sha256);

    let mut wrong_database = attestation.clone();
    wrong_database.database_sha256 = "0".repeat(64);
    let error = load_published_vectors(&path, 2, &wrong_database)
        .expect_err("mismatched predeclared database hash must fail");
    assert!(format!("{error:#}").contains("predeclared source attestation"));

    let wrong_digest = "1".repeat(64);
    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE metadata SET vector_digest = ?1 WHERE singleton = 1",
        [&wrong_digest],
    )?;
    drop(connection);
    let mut copied_metadata = attestation.clone();
    copied_metadata.publication.vector_digest = wrong_digest;
    copied_metadata.database_sha256 = sha256_file(&path)?;
    let error = load_published_vectors(&path, 2, &copied_metadata)
        .expect_err("copied metadata with a fresh database hash must not replace canonical proof");
    assert!(format!("{error:#}").contains("canonical source vector digest"));

    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE metadata SET vector_digest = ?1, point_count = 1 WHERE singleton = 1",
        [&attestation.publication.vector_digest],
    )?;
    drop(connection);
    let mut incomplete_coverage = attestation.clone();
    incomplete_coverage.publication.point_count = 1;
    incomplete_coverage.database_sha256 = sha256_file(&path)?;
    let error = load_published_vectors(&path, 1, &incomplete_coverage)
        .expect_err("inexact complete-row coverage must fail");
    assert!(format!("{error:#}").contains("complete table count"));

    let missing_attestation = serde_json::json!({
        "schema_version": 2,
        "incremental_set_id": "missing-attestation",
        "queries": [],
        "incremental_records": []
    });
    assert!(serde_json::from_value::<FrozenFixture>(missing_attestation).is_err());
    Ok(())
}

#[test]
fn pinned_source_snapshot_rejects_concurrent_wal_drift_before_sampling() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("vectors.sqlite3");
    let mut attestation = write_test_source_database(&path)?;
    let connection = Connection::open(&path)?;
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    ensure!(
        journal_mode == "wal",
        "test database did not enter WAL mode"
    );
    drop(connection);
    ensure_no_sqlite_sidecars(&path)?;
    attestation.database_sha256 = sha256_file(&path)?;

    let writer_path = path.clone();
    let error = load_published_vectors_with_hook(&path, 2, &attestation, move || {
        let writer = Connection::open(&writer_path)?;
        writer.execute(
            "UPDATE vectors SET document_hash = ?1 WHERE node_id = 'node-a'",
            [sha256_bytes(b"concurrent-drift")],
        )?;
        drop(writer);
        Ok(())
    })
    .expect_err("WAL drift between attestation and sampling must fail closed");
    assert!(
        format!("{error:#}").contains("unbound sidecar"),
        "unexpected drift rejection: {error:#}"
    );
    Ok(())
}

fn write_test_source_database(path: &Path) -> Result<ProductionSourceAttestation> {
    let connection = Connection::open(path)?;
    connection.execute_batch(
        "CREATE TABLE metadata (
             singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
             schema_version INTEGER NOT NULL,
             generation TEXT NOT NULL,
             input_hash TEXT NOT NULL,
             embedding_backend TEXT NOT NULL,
             embedding_dim INTEGER NOT NULL,
             point_count INTEGER NOT NULL,
             producer_identity TEXT NOT NULL,
             evidence_contract_identity TEXT NOT NULL,
             vector_digest TEXT NOT NULL
         );
         CREATE TABLE vectors (
             node_id TEXT PRIMARY KEY NOT NULL,
             document_hash TEXT NOT NULL,
             vector BLOB NOT NULL
         );",
    )?;
    let rows = [
        ("node-a", sha256_bytes(b"document-a"), vec![1.0_f32, 0.0]),
        ("node-b", sha256_bytes(b"document-b"), vec![0.0_f32, 1.0]),
    ];
    for (node_id, document_hash, vector) in &rows {
        connection.execute(
            "INSERT INTO vectors (node_id, document_hash, vector) VALUES (?1, ?2, ?3)",
            params![node_id, document_hash, vector_blob(vector)],
        )?;
    }
    let (vector_digest, point_count) = canonical_source_vector_digest(&connection, 2)?;
    let publication = PublicationIdentity {
        schema_version: 2,
        generation: "test-generation".to_owned(),
        input_hash: sha256_bytes(b"test-input"),
        embedding_backend: "test-backend".to_owned(),
        embedding_dim: 2,
        point_count,
        producer_identity: "test-producer".to_owned(),
        evidence_contract_identity: sha256_bytes(b"test-evidence-contract"),
        vector_digest,
    };
    connection.execute(
        "INSERT INTO metadata VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            publication.schema_version,
            publication.generation,
            publication.input_hash,
            publication.embedding_backend,
            publication.embedding_dim as i64,
            publication.point_count as i64,
            publication.producer_identity,
            publication.evidence_contract_identity,
            publication.vector_digest,
        ],
    )?;
    drop(connection);
    Ok(ProductionSourceAttestation {
        publication,
        database_sha256: sha256_file(path)?,
    })
}

#[test]
#[ignore = "measurement lane; run explicitly and retain the JSON artifact"]
fn compare_vector_backends() -> Result<()> {
    let config = Config::from_env()?;
    let dataset = Dataset::load(&config)?;
    let build = build_identity()?;
    let host = host_identity();
    validate_decision_identity(&config, &dataset, &build, &host)?;
    let expected = dataset
        .queries
        .iter()
        .map(|query| exact_top_k(&dataset.records, &query.vector, TOP_K))
        .collect::<Vec<_>>();
    let temp = tempfile::tempdir().context("create comparison directory")?;
    let orders = [
        [Candidate::SqliteVec, Candidate::Usearch],
        [Candidate::Usearch, Candidate::SqliteVec],
    ];
    let mut runs = Vec::with_capacity(REPETITIONS * 2);
    for (repetition, order) in orders.into_iter().enumerate() {
        for (position, candidate) in order.into_iter().enumerate() {
            let root = temp
                .path()
                .join(format!("run-{}-{}", repetition + 1, candidate.name()));
            let result = match candidate {
                Candidate::SqliteVec => run_sqlite_vec(
                    &root,
                    &dataset,
                    &expected,
                    config.warmups,
                    repetition + 1,
                    position + 1,
                ),
                Candidate::Usearch => run_usearch(
                    &root,
                    &dataset,
                    &expected,
                    config.warmups,
                    repetition + 1,
                    position + 1,
                ),
            }
            .with_context(|| format!("run {} repetition {}", candidate.name(), repetition + 1))?;
            runs.push(result);
        }
    }

    let artifact = Artifact {
        schema_version: 4,
        issue: 1202,
        generated_at_unix_seconds: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        criteria_sha256: sha256_bytes(CRITERIA.as_bytes()),
        profile: config.profile.clone(),
        decision_profile: config.profile == "decision",
        decision_scope: "approved Windows x64 production comparison only; no backend adoption",
        blocking_platform: "windows-x86_64",
        timing_comparable: false,
        timing_protocol: "two same-process repetitions with reversed candidate order; timings remain diagnostic until isolated clean-host runs exist",
        build,
        host,
        source: SourceIdentity {
            label: dataset.source_label,
            artifact_sha256: dataset.source_artifact_sha256,
            publication: dataset.publication,
            production_attestation: dataset.source_attestation,
            selected_records_sha256: records_sha256(&dataset.records),
        },
        fixture: FixtureIdentity {
            label: dataset.fixture_label,
            artifact_sha256: dataset.fixture_sha256,
            incremental_set_id: dataset.incremental_set_id,
            incremental_records_sha256: records_sha256(&dataset.incremental_records),
        },
        workload: Workload {
            dimensions: config.dimensions,
            vector_count: dataset.records.len(),
            query_count: dataset.queries.len(),
            top_k: TOP_K,
            warmups: config.warmups,
            repetitions: REPETITIONS,
            metric: "cosine",
            records_sha256: records_sha256(&dataset.records),
            queries_sha256: queries_sha256(&dataset.queries),
        },
        runs,
        limitations: vec![
            "same-process smoke timings are diagnostic and not candidate-comparison evidence",
            "cold-cache latency, isolated RSS, cancellation, deep validation, current-scan regression, Windows offline build/archive size, license/native review, and reversible fallback remain required Windows x64 decision evidence",
            "Linux/macOS quality, publication, offline-build, and native-packaging proof are non-blocking adoption follow-up",
            "one artifact covers only its recorded host, source publication, fixture, and vector count",
        ],
    };
    let output = serde_json::to_vec_pretty(&artifact)?;
    if let Some(parent) = config.output.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    codestory_workspace::atomic_file::write_bytes_atomic(
        &config.output,
        "vector-backend-spike",
        &output,
    )?;
    println!("vector backend spike artifact: {}", config.output.display());
    println!("{}", String::from_utf8_lossy(&output));
    Ok(())
}

fn validate_decision_identity(
    config: &Config,
    dataset: &Dataset,
    build: &BuildIdentity,
    host: &HostIdentity,
) -> Result<()> {
    if config.profile != "decision" {
        return Ok(());
    }
    ensure!(
        !build.git_dirty,
        "decision profile requires a clean exact Git tree"
    );
    ensure!(
        build.build_profile == "release",
        "decision profile requires a release build"
    );
    ensure!(
        !build.git_head.is_empty() && !build.git_tree.is_empty(),
        "decision Git identity is incomplete"
    );
    ensure!(
        !build.target.is_empty(),
        "decision target triple is unavailable"
    );
    ensure!(
        host.os == "windows"
            && host.architecture == "x86_64"
            && build.target.starts_with("x86_64-pc-windows-"),
        "decision profile requires the approved Windows x64 production host"
    );
    ensure!(
        host.cpu_model.is_some(),
        "decision CPU model is unavailable"
    );
    ensure!(
        host.total_memory_bytes.is_some(),
        "decision total RAM is unavailable"
    );
    ensure!(!host.isa.is_empty(), "decision ISA envelope is unavailable");
    ensure!(
        dataset.source_artifact_sha256.len() == 64,
        "decision source artifact hash is missing"
    );
    let attestation = dataset
        .source_attestation
        .as_ref()
        .context("decision source attestation is missing")?;
    attestation.validate()?;
    ensure!(
        dataset.source_artifact_sha256 == attestation.database_sha256,
        "decision source artifact does not match its database attestation"
    );
    ensure!(
        dataset.fixture_sha256.len() == 64,
        "decision fixture hash is missing"
    );
    ensure!(
        dataset.publication.embedding_backend != "synthetic-smoke",
        "synthetic source cannot be decision evidence"
    );
    Ok(())
}

fn run_sqlite_vec(
    root: &Path,
    dataset: &Dataset,
    expected: &[Vec<u64>],
    warmups: usize,
    repetition: usize,
    order_position: usize,
) -> Result<BackendResult> {
    register_sqlite_vec();
    fs::create_dir_all(root)?;
    let started = Instant::now();
    let initial = create_sqlite_generation(
        root,
        &GenerationBuild {
            generation: "generation-0001",
            base: None,
            additions: &dataset.records,
            complete: &dataset.records,
            publication: &dataset.publication,
            source_attestation: dataset.source_attestation.as_ref(),
            incremental_set_id: None,
            corrupt: false,
        },
    )?;
    write_publication(
        root,
        &PublicationPointers {
            schema_version: 1,
            current: initial.clone(),
            rollback: None,
        },
    )?;
    let build_ms = elapsed_ms(started);
    let initial_dir = generation_dir(root, &initial)?;
    let disk_bytes = directory_size(&initial_dir)?;
    let old_generation_hash = directory_sha256(&initial_dir)?;

    let started = Instant::now();
    let pinned_old = open_sqlite_current(root)?;
    let load_ms = elapsed_ms(started);
    let measurements = measure_queries(
        &dataset.queries,
        expected,
        &pinned_old.identities,
        warmups,
        |query| sqlite_vec_search(&pinned_old.connection, query, TOP_K),
    )?;
    let concurrent_readers = 4;
    let expected_first =
        sqlite_vec_search(&pinned_old.connection, &dataset.queries[0].vector, TOP_K)?;
    let concurrent_reader_consistency = sqlite_concurrent_readers(
        root,
        &dataset.queries[0].vector,
        &expected_first,
        concurrent_readers,
    )?;

    let mut complete = dataset.records.clone();
    complete.extend(dataset.incremental_records.clone());
    let started = Instant::now();
    let incremental = create_sqlite_generation(
        root,
        &GenerationBuild {
            generation: "generation-0002",
            base: Some(&initial),
            additions: &dataset.incremental_records,
            complete: &complete,
            publication: &dataset.publication,
            source_attestation: dataset.source_attestation.as_ref(),
            incremental_set_id: Some(&dataset.incremental_set_id),
            corrupt: false,
        },
    )?;
    publish_incremental(root, &incremental)?;
    let incremental_reuse_ms = elapsed_ms(started);
    let published = read_publication(root)?;
    let atomic_publication_pointer_pair =
        published.current == incremental && published.rollback.as_ref() == Some(&initial);
    let pinned_old_reader_after_publication =
        sqlite_vec_search(&pinned_old.connection, &dataset.queries[0].vector, TOP_K)?
            == expected_first
            && pinned_old.pointer == initial;
    let pinned_incremental = open_sqlite_current(root)?;
    let new_current_reader_observed_incremental = pinned_incremental.pointer == incremental
        && pinned_incremental.identities.len() == complete.len();
    let old_generation_unchanged = directory_sha256(&initial_dir)? == old_generation_hash;

    let publication_before_failure = read_publication(root)?;
    let current_query_before = sqlite_vec_search(
        &pinned_incremental.connection,
        &dataset.queries[0].vector,
        TOP_K,
    )?;
    let corrupt_candidate_rejected = create_sqlite_generation(
        root,
        &GenerationBuild {
            generation: "generation-0003-corrupt",
            base: Some(&incremental),
            additions: &[],
            complete: &complete,
            publication: &dataset.publication,
            source_attestation: dataset.source_attestation.as_ref(),
            incremental_set_id: Some(&dataset.incremental_set_id),
            corrupt: true,
        },
    )
    .is_err();
    let publication_after_failure = read_publication(root)?;
    let reopened_after_failure = open_sqlite_current(root)?;
    let failed_candidate_preserved_current_pointer = publication_after_failure
        == publication_before_failure
        && sqlite_vec_search(
            &reopened_after_failure.connection,
            &dataset.queries[0].vector,
            TOP_K,
        )? == current_query_before;

    rollback_publication(root)?;
    let rollback_publication = read_publication(root)?;
    let rolled_back = open_sqlite_current(root)?;
    let rollback_pointer_readable = rollback_publication.current == initial
        && rollback_publication.rollback.as_ref() == Some(&incremental)
        && rolled_back.pointer == initial
        && rolled_back.identities.len() == dataset.records.len();
    let pinned_incremental_reader_after_rollback = pinned_incremental.pointer == incremental
        && sqlite_vec_search(
            &pinned_incremental.connection,
            &dataset.queries[0].vector,
            TOP_K,
        )? == current_query_before;
    let rolled_back_query_before_tamper =
        sqlite_vec_search(&rolled_back.connection, &dataset.queries[0].vector, TOP_K)?;
    tamper_published_index(&initial_dir.join(Candidate::SqliteVec.index_file()))?;
    let referenced_generation_tamper_rejected = open_sqlite_current(root).is_err();
    let pinned_reader_after_referenced_tamper =
        sqlite_vec_search(&rolled_back.connection, &dataset.queries[0].vector, TOP_K)?
            == rolled_back_query_before_tamper;

    Ok(BackendResult {
        repetition,
        order_position,
        backend: Candidate::SqliteVec.name(),
        version: Candidate::SqliteVec.version(),
        build_ms,
        load_ms,
        first_query_after_open_ms: measurements.first_after_open_ms,
        warm_query_p50_ms: measurements.warm_p50_ms,
        warm_query_p95_ms: measurements.warm_p95_ms,
        disk_bytes,
        memory_bytes: None,
        memory_method: "unmeasured; isolated RSS runner required",
        recall_at_20: measurements.recall,
        expected_identity_hit_at_20: measurements.expected_hit,
        representative_query_hit_at_20: measurements.representative_hit,
        symbol_query_hit_at_20: measurements.symbol_hit,
        incremental_reuse_ms,
        concurrent_readers,
        concurrent_reader_consistency,
        pinned_old_reader_after_publication,
        new_current_reader_observed_incremental,
        old_generation_unchanged,
        atomic_publication_pointer_pair,
        referenced_generation_tamper_rejected,
        pinned_reader_after_referenced_tamper,
        corrupt_candidate_rejected,
        failed_candidate_preserved_current_pointer,
        rollback_pointer_readable,
        pinned_incremental_reader_after_rollback,
    })
}

fn run_usearch(
    root: &Path,
    dataset: &Dataset,
    expected: &[Vec<u64>],
    warmups: usize,
    repetition: usize,
    order_position: usize,
) -> Result<BackendResult> {
    fs::create_dir_all(root)?;
    let started = Instant::now();
    let initial = create_usearch_generation(
        root,
        &GenerationBuild {
            generation: "generation-0001",
            base: None,
            additions: &dataset.records,
            complete: &dataset.records,
            publication: &dataset.publication,
            source_attestation: dataset.source_attestation.as_ref(),
            incremental_set_id: None,
            corrupt: false,
        },
    )?;
    write_publication(
        root,
        &PublicationPointers {
            schema_version: 1,
            current: initial.clone(),
            rollback: None,
        },
    )?;
    let build_ms = elapsed_ms(started);
    let initial_dir = generation_dir(root, &initial)?;
    let disk_bytes = directory_size(&initial_dir)?;
    let old_generation_hash = directory_sha256(&initial_dir)?;

    let started = Instant::now();
    let pinned_old = open_usearch_current(root)?;
    let load_ms = elapsed_ms(started);
    let memory_bytes = pinned_old.index.memory_usage() as u64;
    let measurements = measure_queries(
        &dataset.queries,
        expected,
        &pinned_old.identities,
        warmups,
        |query| usearch_search(&pinned_old.index, query, TOP_K),
    )?;
    let concurrent_readers = 4;
    let expected_first = usearch_search(&pinned_old.index, &dataset.queries[0].vector, TOP_K)?;
    let concurrent_reader_consistency = usearch_concurrent_readers(
        root,
        &dataset.queries[0].vector,
        &expected_first,
        concurrent_readers,
    )?;

    let mut complete = dataset.records.clone();
    complete.extend(dataset.incremental_records.clone());
    let started = Instant::now();
    let incremental = create_usearch_generation(
        root,
        &GenerationBuild {
            generation: "generation-0002",
            base: Some(&initial),
            additions: &dataset.incremental_records,
            complete: &complete,
            publication: &dataset.publication,
            source_attestation: dataset.source_attestation.as_ref(),
            incremental_set_id: Some(&dataset.incremental_set_id),
            corrupt: false,
        },
    )?;
    publish_incremental(root, &incremental)?;
    let incremental_reuse_ms = elapsed_ms(started);
    let published = read_publication(root)?;
    let atomic_publication_pointer_pair =
        published.current == incremental && published.rollback.as_ref() == Some(&initial);
    let pinned_old_reader_after_publication =
        usearch_search(&pinned_old.index, &dataset.queries[0].vector, TOP_K)? == expected_first
            && pinned_old.pointer == initial;
    let pinned_incremental = open_usearch_current(root)?;
    let new_current_reader_observed_incremental = pinned_incremental.pointer == incremental
        && pinned_incremental.identities.len() == complete.len();
    let old_generation_unchanged = directory_sha256(&initial_dir)? == old_generation_hash;

    let publication_before_failure = read_publication(root)?;
    let current_query_before =
        usearch_search(&pinned_incremental.index, &dataset.queries[0].vector, TOP_K)?;
    let corrupt_candidate_rejected = create_usearch_generation(
        root,
        &GenerationBuild {
            generation: "generation-0003-corrupt",
            base: Some(&incremental),
            additions: &[],
            complete: &complete,
            publication: &dataset.publication,
            source_attestation: dataset.source_attestation.as_ref(),
            incremental_set_id: Some(&dataset.incremental_set_id),
            corrupt: true,
        },
    )
    .is_err();
    let publication_after_failure = read_publication(root)?;
    let reopened_after_failure = open_usearch_current(root)?;
    let failed_candidate_preserved_current_pointer = publication_after_failure
        == publication_before_failure
        && usearch_search(
            &reopened_after_failure.index,
            &dataset.queries[0].vector,
            TOP_K,
        )? == current_query_before;

    rollback_publication(root)?;
    let rollback_publication = read_publication(root)?;
    let rolled_back = open_usearch_current(root)?;
    let rollback_pointer_readable = rollback_publication.current == initial
        && rollback_publication.rollback.as_ref() == Some(&incremental)
        && rolled_back.pointer == initial
        && rolled_back.identities.len() == dataset.records.len();
    let pinned_incremental_reader_after_rollback = pinned_incremental.pointer == incremental
        && usearch_search(&pinned_incremental.index, &dataset.queries[0].vector, TOP_K)?
            == current_query_before;
    let rolled_back_query_before_tamper =
        usearch_search(&rolled_back.index, &dataset.queries[0].vector, TOP_K)?;
    tamper_published_index(&initial_dir.join(Candidate::Usearch.index_file()))?;
    let referenced_generation_tamper_rejected = open_usearch_current(root).is_err();
    let pinned_reader_after_referenced_tamper =
        usearch_search(&rolled_back.index, &dataset.queries[0].vector, TOP_K)?
            == rolled_back_query_before_tamper;

    Ok(BackendResult {
        repetition,
        order_position,
        backend: Candidate::Usearch.name(),
        version: Candidate::Usearch.version(),
        build_ms,
        load_ms,
        first_query_after_open_ms: measurements.first_after_open_ms,
        warm_query_p50_ms: measurements.warm_p50_ms,
        warm_query_p95_ms: measurements.warm_p95_ms,
        disk_bytes,
        memory_bytes: Some(memory_bytes),
        memory_method: "USearch memory_usage lower-bound estimate",
        recall_at_20: measurements.recall,
        expected_identity_hit_at_20: measurements.expected_hit,
        representative_query_hit_at_20: measurements.representative_hit,
        symbol_query_hit_at_20: measurements.symbol_hit,
        incremental_reuse_ms,
        concurrent_readers,
        concurrent_reader_consistency,
        pinned_old_reader_after_publication,
        new_current_reader_observed_incremental,
        old_generation_unchanged,
        atomic_publication_pointer_pair,
        referenced_generation_tamper_rejected,
        pinned_reader_after_referenced_tamper,
        corrupt_candidate_rejected,
        failed_candidate_preserved_current_pointer,
        rollback_pointer_readable,
        pinned_incremental_reader_after_rollback,
    })
}

struct GenerationBuild<'a> {
    generation: &'a str,
    base: Option<&'a GenerationPointer>,
    additions: &'a [VectorRecord],
    complete: &'a [VectorRecord],
    publication: &'a PublicationIdentity,
    source_attestation: Option<&'a ProductionSourceAttestation>,
    incremental_set_id: Option<&'a str>,
    corrupt: bool,
}

fn create_sqlite_generation(root: &Path, build: &GenerationBuild<'_>) -> Result<GenerationPointer> {
    let stage = create_stage_directory(root, build.generation)?;
    let index_path = stage.join(Candidate::SqliteVec.index_file());
    match build.base {
        Some(pointer) => {
            let base_dir = generation_dir(root, pointer)?;
            fs::copy(
                base_dir.join(Candidate::SqliteVec.index_file()),
                &index_path,
            )?;
            append_sqlite_vec(
                &index_path,
                build.complete.len() - build.additions.len(),
                build.additions,
            )?;
        }
        None => write_sqlite_vec(&index_path, build.additions)?,
    }
    let manifest = write_generation_metadata(
        &stage,
        Candidate::SqliteVec,
        build.generation,
        build.complete,
        build.publication,
        build.source_attestation,
        build.incremental_set_id,
    )?;
    if build.corrupt {
        write_corrupt_file(&index_path)?;
    }
    validate_generation(&stage, Candidate::SqliteVec, Some(build.complete))?;
    publish_generation_directory(
        root,
        &stage,
        build.generation,
        Candidate::SqliteVec,
        &manifest,
    )
}

fn create_usearch_generation(
    root: &Path,
    build: &GenerationBuild<'_>,
) -> Result<GenerationPointer> {
    let stage = create_stage_directory(root, build.generation)?;
    let index_path = stage.join(Candidate::Usearch.index_file());
    let index = match build.base {
        Some(pointer) => {
            let base_dir = generation_dir(root, pointer)?;
            Index::restore(
                &base_dir
                    .join(Candidate::Usearch.index_file())
                    .to_string_lossy(),
            )?
        }
        None => new_usearch_index(build.publication.embedding_dim, build.complete.len())?,
    };
    index.reserve(build.complete.len())?;
    let first_key = build.complete.len() - build.additions.len();
    for (offset, record) in build.additions.iter().enumerate() {
        index.add((first_key + offset) as u64, &record.vector)?;
    }
    index.save(&index_path.to_string_lossy())?;
    sync_file(&index_path)?;
    drop(index);
    let manifest = write_generation_metadata(
        &stage,
        Candidate::Usearch,
        build.generation,
        build.complete,
        build.publication,
        build.source_attestation,
        build.incremental_set_id,
    )?;
    if build.corrupt {
        write_corrupt_file(&index_path)?;
    }
    validate_generation(&stage, Candidate::Usearch, Some(build.complete))?;
    publish_generation_directory(
        root,
        &stage,
        build.generation,
        Candidate::Usearch,
        &manifest,
    )
}

fn create_stage_directory(root: &Path, generation: &str) -> Result<PathBuf> {
    fs::create_dir_all(root.join("generations"))?;
    let stage = root.join(format!(".staging-{generation}"));
    ensure!(
        !stage.exists(),
        "staging generation already exists: {}",
        stage.display()
    );
    fs::create_dir(&stage)?;
    Ok(stage)
}

fn write_generation_metadata(
    stage: &Path,
    candidate: Candidate,
    generation: &str,
    records: &[VectorRecord],
    publication: &PublicationIdentity,
    source_attestation: Option<&ProductionSourceAttestation>,
    incremental_set_id: Option<&str>,
) -> Result<Vec<u8>> {
    let identities = records
        .iter()
        .map(|record| record.identity.clone())
        .collect::<Vec<_>>();
    let identity_bytes = serde_json::to_vec(&identities)?;
    write_synced(&stage.join("identities.json"), &identity_bytes)?;
    let index_sha256 = sha256_file(&stage.join(candidate.index_file()))?;
    let directory_contents_sha256 = canonical_directory_contents_sha256(stage)?;
    let manifest = GenerationManifest {
        schema_version: 2,
        backend: candidate.name().to_owned(),
        backend_version: candidate.version().to_owned(),
        generation: generation.to_owned(),
        metric: "cosine".to_owned(),
        dimensions: publication.embedding_dim,
        point_count: records.len(),
        index_sha256,
        directory_contents_sha256,
        records_sha256: records_sha256(records),
        identities_sha256: sha256_bytes(&identity_bytes),
        source_publication: publication.clone(),
        source_attestation: source_attestation.cloned(),
        incremental_set_id: incremental_set_id.map(str::to_owned),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    write_synced(&stage.join("manifest.json"), &manifest_bytes)?;
    Ok(manifest_bytes)
}

fn publish_generation_directory(
    root: &Path,
    stage: &Path,
    generation: &str,
    candidate: Candidate,
    manifest_bytes: &[u8],
) -> Result<GenerationPointer> {
    let destination = root.join("generations").join(generation);
    ensure!(
        !destination.exists(),
        "immutable generation already exists: {generation}"
    );
    fs::rename(stage, &destination).with_context(|| {
        format!(
            "publish immutable generation {} to {}",
            stage.display(),
            destination.display()
        )
    })?;
    Ok(GenerationPointer {
        schema_version: 1,
        backend: candidate.name().to_owned(),
        generation: generation.to_owned(),
        manifest_sha256: sha256_bytes(manifest_bytes),
    })
}

fn generation_dir(root: &Path, pointer: &GenerationPointer) -> Result<PathBuf> {
    let path = root.join("generations").join(&pointer.generation);
    ensure!(
        path.is_dir(),
        "generation directory is missing: {}",
        path.display()
    );
    let manifest_bytes = fs::read(path.join("manifest.json"))?;
    ensure!(
        sha256_bytes(&manifest_bytes) == pointer.manifest_sha256,
        "generation manifest hash does not match pointer"
    );
    let manifest: GenerationManifest = serde_json::from_slice(&manifest_bytes)?;
    ensure!(
        manifest.backend == pointer.backend,
        "pointer backend mismatch"
    );
    ensure!(
        manifest.generation == pointer.generation,
        "pointer generation mismatch"
    );
    Ok(path)
}

fn write_publication(root: &Path, publication: &PublicationPointers) -> Result<()> {
    validate_pointed_generation(root, &publication.current)?;
    if let Some(rollback) = &publication.rollback {
        validate_pointed_generation(root, rollback)?;
        ensure!(
            rollback != &publication.current,
            "current and rollback generations must differ"
        );
    }
    let bytes = serde_json::to_vec_pretty(publication)?;
    codestory_workspace::atomic_file::write_bytes_atomic(
        &root.join("publication.json"),
        "publication",
        &bytes,
    )
}

fn read_publication(root: &Path) -> Result<PublicationPointers> {
    let path = root.join("publication.json");
    let publication: PublicationPointers = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    ensure!(
        publication.schema_version == 1,
        "publication pointer schema version mismatch"
    );
    validate_pointed_generation(root, &publication.current)?;
    if let Some(rollback) = &publication.rollback {
        validate_pointed_generation(root, rollback)?;
        ensure!(
            rollback != &publication.current,
            "current and rollback generations must differ"
        );
    }
    Ok(publication)
}

fn validate_pointed_generation(root: &Path, pointer: &GenerationPointer) -> Result<()> {
    let directory = generation_dir(root, pointer)?;
    let candidate = Candidate::from_name(&pointer.backend)?;
    validate_generation(&directory, candidate, None)?;
    Ok(())
}

fn publish_incremental(root: &Path, next: &GenerationPointer) -> Result<()> {
    let publication = read_publication(root)?;
    generation_dir(root, next)?;
    write_publication(
        root,
        &PublicationPointers {
            schema_version: 1,
            current: next.clone(),
            rollback: Some(publication.current),
        },
    )
}

fn rollback_publication(root: &Path) -> Result<()> {
    let publication = read_publication(root)?;
    let rollback = publication
        .rollback
        .context("publication has no rollback generation")?;
    write_publication(
        root,
        &PublicationPointers {
            schema_version: 1,
            current: rollback,
            rollback: Some(publication.current),
        },
    )
}

fn validate_generation(
    directory: &Path,
    candidate: Candidate,
    expected_records: Option<&[VectorRecord]>,
) -> Result<(GenerationManifest, Vec<Identity>)> {
    let manifest_bytes = fs::read(directory.join("manifest.json"))?;
    let manifest: GenerationManifest = serde_json::from_slice(&manifest_bytes)?;
    ensure!(
        manifest.schema_version == 2,
        "generation schema version mismatch"
    );
    ensure!(
        manifest.backend == candidate.name(),
        "generation backend mismatch"
    );
    ensure!(
        manifest.backend_version == candidate.version(),
        "generation version mismatch"
    );
    ensure!(
        manifest.metric == "cosine",
        "generation metric must be cosine"
    );
    manifest.source_publication.validate()?;
    if let Some(attestation) = &manifest.source_attestation {
        attestation.validate()?;
        ensure!(
            attestation.publication == manifest.source_publication,
            "generation source attestation does not match its publication identity"
        );
    } else {
        ensure!(
            manifest.source_publication.embedding_backend == "synthetic-smoke",
            "non-synthetic generation is missing its production source attestation"
        );
    }
    let index_path = directory.join(candidate.index_file());
    ensure!(
        sha256_file(&index_path)? == manifest.index_sha256,
        "generation index SHA-256 mismatch"
    );
    ensure!(
        canonical_directory_contents_sha256(directory)? == manifest.directory_contents_sha256,
        "generation directory contents SHA-256 mismatch"
    );
    let identity_bytes = fs::read(directory.join("identities.json"))?;
    ensure!(
        sha256_bytes(&identity_bytes) == manifest.identities_sha256,
        "identity sidecar hash mismatch"
    );
    let identities: Vec<Identity> = serde_json::from_slice(&identity_bytes)?;
    ensure!(
        identities.len() == manifest.point_count,
        "identity count mismatch"
    );
    if let Some(records) = expected_records {
        ensure!(
            records.len() == manifest.point_count,
            "manifest point count mismatch"
        );
        ensure!(
            records_sha256(records) == manifest.records_sha256,
            "record digest mismatch"
        );
        ensure!(
            identities
                == records
                    .iter()
                    .map(|record| record.identity.clone())
                    .collect::<Vec<_>>(),
            "identity sidecar does not match records"
        );
    }
    match candidate {
        Candidate::SqliteVec => {
            validate_sqlite_index(directory, &manifest, &identities, expected_records)?
        }
        Candidate::Usearch => validate_usearch_index(directory, &manifest, expected_records)?,
    }
    Ok((manifest, identities))
}

fn validate_sqlite_index(
    directory: &Path,
    manifest: &GenerationManifest,
    identities: &[Identity],
    expected_records: Option<&[VectorRecord]>,
) -> Result<()> {
    let connection = open_sqlite_read_only(&directory.join(Candidate::SqliteVec.index_file()))?;
    let check: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    ensure!(check == "ok", "SQLite quick_check failed: {check}");
    let vector_count: i64 =
        connection.query_row("SELECT count(*) FROM vec_items", [], |row| row.get(0))?;
    let identity_count: i64 =
        connection.query_row("SELECT count(*) FROM identities", [], |row| row.get(0))?;
    ensure!(
        vector_count == manifest.point_count as i64,
        "sqlite-vec count mismatch"
    );
    ensure!(
        identity_count == manifest.point_count as i64,
        "SQLite identity count mismatch"
    );
    let schema: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'vec_items'",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        schema
            .to_ascii_lowercase()
            .contains("distance_metric=cosine"),
        "sqlite-vec index is not configured for cosine distance"
    );
    let mut statement = connection
        .prepare("SELECT vector_key, node_id, document_hash FROM identities ORDER BY vector_key")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            Identity {
                node_id: row.get(1)?,
                document_hash: row.get(2)?,
            },
        ))
    })?;
    for (expected_key, row) in rows.enumerate() {
        let (actual_key, identity) = row?;
        ensure!(actual_key == expected_key as i64, "SQLite identity key gap");
        ensure!(
            identity == identities[expected_key],
            "SQLite identity row mismatch"
        );
    }
    if let Some(records) = expected_records {
        let mut statement =
            connection.prepare("SELECT rowid, embedding FROM vec_items ORDER BY rowid")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        for (expected_key, row) in rows.enumerate() {
            let (actual_rowid, vector) = row?;
            ensure!(
                actual_rowid == (expected_key + 1) as i64,
                "sqlite-vec rowid gap"
            );
            ensure!(
                vector == vector_blob(&records[expected_key].vector),
                "sqlite-vec vector differs at key {expected_key}"
            );
        }
    }
    Ok(())
}

fn validate_usearch_index(
    directory: &Path,
    manifest: &GenerationManifest,
    expected_records: Option<&[VectorRecord]>,
) -> Result<()> {
    let index = Index::restore(
        &directory
            .join(Candidate::Usearch.index_file())
            .to_string_lossy(),
    )?;
    ensure!(
        index.size() == manifest.point_count,
        "USearch point count mismatch"
    );
    ensure!(
        index.dimensions() == manifest.dimensions,
        "USearch dimension mismatch"
    );
    ensure!(
        index.metric_kind() == MetricKind::Cos,
        "USearch metric must be cosine"
    );
    ensure!(
        index.scalar_kind() == ScalarKind::F32,
        "USearch scalar kind must be F32"
    );
    if let Some(records) = expected_records {
        let mut actual = vec![0.0_f32; manifest.dimensions];
        for (key, record) in records.iter().enumerate() {
            ensure!(
                index.get(key as u64, &mut actual)? == 1,
                "USearch key {key} is missing"
            );
            ensure!(
                actual
                    .iter()
                    .zip(&record.vector)
                    .all(|(left, right)| left.to_bits() == right.to_bits()),
                "USearch vector differs at key {key}"
            );
        }
    }
    Ok(())
}

struct OpenSqliteGeneration {
    connection: Connection,
    identities: Vec<Identity>,
    pointer: GenerationPointer,
}

fn open_sqlite_current(root: &Path) -> Result<OpenSqliteGeneration> {
    let pointer = read_publication(root)?.current;
    let directory = generation_dir(root, &pointer)?;
    let (_, identities) = validate_generation(&directory, Candidate::SqliteVec, None)?;
    let connection = open_sqlite_read_only(&directory.join(Candidate::SqliteVec.index_file()))?;
    Ok(OpenSqliteGeneration {
        connection,
        identities,
        pointer,
    })
}

struct OpenUsearchGeneration {
    index: Index,
    identities: Vec<Identity>,
    pointer: GenerationPointer,
}

fn open_usearch_current(root: &Path) -> Result<OpenUsearchGeneration> {
    let pointer = read_publication(root)?.current;
    let directory = generation_dir(root, &pointer)?;
    let (_, identities) = validate_generation(&directory, Candidate::Usearch, None)?;
    let index = Index::restore(
        &directory
            .join(Candidate::Usearch.index_file())
            .to_string_lossy(),
    )?;
    Ok(OpenUsearchGeneration {
        index,
        identities,
        pointer,
    })
}

fn register_sqlite_vec() {
    REGISTER_SQLITE_VEC.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut i8,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> i32,
        >(
            sqlite_vec::sqlite3_vec_init as *const ()
        )));
    });
}

fn write_sqlite_vec(path: &Path, records: &[VectorRecord]) -> Result<()> {
    ensure!(
        !records.is_empty(),
        "initial sqlite-vec records must not be empty"
    );
    let mut connection = Connection::open(path)?;
    connection.execute_batch(&format!(
        "PRAGMA journal_mode=DELETE;
         PRAGMA synchronous=FULL;
         CREATE VIRTUAL TABLE vec_items USING vec0(
             embedding float[{}] distance_metric=cosine
         );
         CREATE TABLE identities (
             vector_key INTEGER PRIMARY KEY NOT NULL,
             node_id TEXT NOT NULL,
             document_hash TEXT NOT NULL,
             UNIQUE(node_id, document_hash)
         );",
        records[0].vector.len()
    ))?;
    append_sqlite_records(&mut connection, 0, records)?;
    drop(connection);
    sync_file(path)
}

fn append_sqlite_vec(path: &Path, first_key: usize, records: &[VectorRecord]) -> Result<()> {
    if records.is_empty() {
        sync_file(path)?;
        return Ok(());
    }
    let mut connection = Connection::open(path)?;
    append_sqlite_records(&mut connection, first_key, records)?;
    drop(connection);
    sync_file(path)
}

fn append_sqlite_records(
    connection: &mut Connection,
    first_key: usize,
    records: &[VectorRecord],
) -> Result<()> {
    let transaction = connection.transaction()?;
    {
        let mut insert_vector =
            transaction.prepare("INSERT INTO vec_items(rowid, embedding) VALUES (?1, ?2)")?;
        let mut insert_identity = transaction.prepare(
            "INSERT INTO identities(vector_key, node_id, document_hash) VALUES (?1, ?2, ?3)",
        )?;
        for (offset, record) in records.iter().enumerate() {
            let key = first_key + offset;
            insert_vector.execute(params![(key + 1) as i64, vector_blob(&record.vector)])?;
            insert_identity.execute(params![
                key as i64,
                record.identity.node_id,
                record.identity.document_hash
            ])?;
        }
    }
    transaction.commit()?;
    Ok(())
}

fn open_sqlite_read_only(path: &Path) -> Result<Connection> {
    register_sqlite_vec();
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open {} read-only", path.display()))
}

fn sqlite_vec_search(connection: &Connection, query: &[f32], count: usize) -> Result<Vec<u64>> {
    let mut statement = connection.prepare(
        "SELECT rowid FROM vec_items
         WHERE embedding MATCH ?1 AND k = ?2
         ORDER BY distance",
    )?;
    let rows = statement.query_map(params![vector_blob(query), count as i64], |row| {
        row.get::<_, i64>(0)
    })?;
    rows.map(|row| {
        let key = row?;
        ensure!(key > 0, "sqlite-vec returned a non-positive rowid");
        Ok((key - 1) as u64)
    })
    .collect()
}

fn sqlite_concurrent_readers(
    root: &Path,
    query: &[f32],
    expected: &[u64],
    readers: usize,
) -> Result<bool> {
    let root = root.to_owned();
    let query = query.to_owned();
    let expected = expected.to_owned();
    let handles = (0..readers)
        .map(|_| {
            let root = root.clone();
            let query = query.clone();
            let expected = expected.clone();
            std::thread::spawn(move || -> Result<bool> {
                let opened = open_sqlite_current(&root)?;
                Ok(sqlite_vec_search(&opened.connection, &query, TOP_K)? == expected)
            })
        })
        .collect::<Vec<_>>();
    handles
        .into_iter()
        .map(|handle| {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("sqlite reader panicked"))?
        })
        .try_fold(true, |all, result| {
            result.map(|consistent| all && consistent)
        })
}

fn new_usearch_index(dimensions: usize, capacity: usize) -> Result<Index> {
    let options = IndexOptions {
        dimensions,
        metric: MetricKind::Cos,
        quantization: ScalarKind::F32,
        ..IndexOptions::default()
    };
    let index = Index::new(&options)?;
    index.reserve(capacity)?;
    Ok(index)
}

fn usearch_search(index: &Index, query: &[f32], count: usize) -> Result<Vec<u64>> {
    Ok(index.search(query, count)?.keys)
}

fn usearch_concurrent_readers(
    root: &Path,
    query: &[f32],
    expected: &[u64],
    readers: usize,
) -> Result<bool> {
    let opened = open_usearch_current(root)?;
    let index = Arc::new(opened.index);
    let handles = (0..readers)
        .map(|_| {
            let index = Arc::clone(&index);
            let query = query.to_owned();
            let expected = expected.to_owned();
            std::thread::spawn(move || -> Result<bool> {
                Ok(usearch_search(&index, &query, TOP_K)? == expected)
            })
        })
        .collect::<Vec<_>>();
    handles
        .into_iter()
        .map(|handle| {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("USearch reader panicked"))?
        })
        .try_fold(true, |all, result| {
            result.map(|consistent| all && consistent)
        })
}

fn measure_queries(
    queries: &[FrozenQuery],
    expected: &[Vec<u64>],
    identities: &[Identity],
    warmups: usize,
    mut search: impl FnMut(&[f32]) -> Result<Vec<u64>>,
) -> Result<QueryMeasurements> {
    let started = Instant::now();
    search(&queries[0].vector)?;
    let first_after_open_ms = elapsed_ms(started);
    for _ in 0..warmups {
        search(&queries[0].vector)?;
    }
    let mut latencies = Vec::with_capacity(queries.len());
    let mut actual = Vec::with_capacity(queries.len());
    for query in queries {
        let started = Instant::now();
        actual.push(search(&query.vector)?);
        latencies.push(elapsed_ms(started));
    }
    let recall = actual
        .iter()
        .zip(expected)
        .map(|(actual, expected)| recall_at_k(actual, expected))
        .sum::<f64>()
        / queries.len() as f64;
    let hits = queries
        .iter()
        .zip(&actual)
        .map(|(query, keys)| -> Result<(QueryKind, f64)> {
            let returned = keys
                .iter()
                .map(|key| {
                    identities
                        .get(*key as usize)
                        .cloned()
                        .with_context(|| format!("query returned out-of-range key {key}"))
                })
                .collect::<Result<HashSet<_>>>()?;
            Ok((
                query.kind,
                if query
                    .expected
                    .iter()
                    .any(|expected| returned.contains(expected))
                {
                    1.0
                } else {
                    0.0
                },
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let expected_hit = hits.iter().map(|(_, hit)| hit).sum::<f64>() / hits.len() as f64;
    let representative_hit = hit_for_kind(&hits, QueryKind::Representative);
    let symbol_hit = hit_for_kind(&hits, QueryKind::Symbol);
    Ok(QueryMeasurements {
        first_after_open_ms,
        warm_p50_ms: percentile(&latencies, 0.50),
        warm_p95_ms: percentile(&latencies, 0.95),
        recall,
        expected_hit,
        representative_hit,
        symbol_hit,
    })
}

fn hit_for_kind(hits: &[(QueryKind, f64)], kind: QueryKind) -> f64 {
    let selected = hits
        .iter()
        .filter(|(actual, _)| *actual == kind)
        .map(|(_, hit)| *hit)
        .collect::<Vec<_>>();
    selected.iter().sum::<f64>() / selected.len() as f64
}

fn exact_top_k(records: &[VectorRecord], query: &[f32], count: usize) -> Vec<u64> {
    let mut scored = records
        .iter()
        .enumerate()
        .map(|(key, record)| (key as u64, cosine_similarity(&record.vector, query)))
        .collect::<Vec<_>>();
    scored.sort_unstable_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    scored.truncate(count.min(scored.len()));
    scored.into_iter().map(|(key, _)| key).collect()
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    let dot = left.iter().zip(right).map(|(a, b)| a * b).sum::<f32>();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    dot / (left_norm * right_norm)
}

fn recall_at_k(actual: &[u64], expected: &[u64]) -> f64 {
    let actual = actual.iter().copied().collect::<HashSet<_>>();
    expected.iter().filter(|key| actual.contains(key)).count() as f64 / expected.len() as f64
}

fn build_identity() -> Result<BuildIdentity> {
    let root = workspace_root();
    let git_head = command_text(
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "HEAD"]),
    )?;
    let git_tree = command_text(
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "HEAD^{tree}"]),
    )?;
    let status = command_bytes(Command::new("git").arg("-C").arg(&root).args([
        "status",
        "--porcelain=v1",
        "--untracked-files=all",
    ]))?;
    let diff = command_bytes(
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["diff", "--binary", "HEAD"]),
    )?;
    let untracked = command_bytes(Command::new("git").arg("-C").arg(&root).args([
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
    ]))?;
    let mut fingerprint = Sha256::new();
    fingerprint.update(b"codestory-vector-spike-worktree-v1\0");
    fingerprint.update(&status);
    fingerprint.update(&diff);
    for path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        fingerprint.update(path);
        let relative = String::from_utf8(path.to_vec()).context("untracked path is not UTF-8")?;
        fingerprint.update(fs::read(root.join(relative))?);
    }
    let rustc = command_text(Command::new("rustc").arg("-Vv"))?;
    let cargo = command_text(Command::new("cargo").arg("-V"))?;
    let target = rustc
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .unwrap_or_default()
        .to_owned();
    Ok(BuildIdentity {
        git_head,
        git_tree,
        git_dirty: !status.is_empty(),
        worktree_sha256: format!("{:x}", fingerprint.finalize()),
        rustc,
        cargo,
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        target,
    })
}

fn host_identity() -> HostIdentity {
    HostIdentity {
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        cpu_model: cpu_model(),
        logical_cpus: std::thread::available_parallelism().ok().map(usize::from),
        total_memory_bytes: total_memory_bytes(),
        isa: isa_features(),
    }
}

fn cpu_model() -> Option<String> {
    if let Ok(value) = std::env::var("PROCESSOR_IDENTIFIER")
        && !value.trim().is_empty()
    {
        return Some(value);
    }
    #[cfg(target_os = "linux")]
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        return cpuinfo.lines().find_map(|line| {
            line.strip_prefix("model name")
                .and_then(|value| value.split_once(':'))
                .map(|(_, value)| value.trim().to_owned())
        });
    }
    #[cfg(target_os = "macos")]
    if let Ok(output) = Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        && output.status.success()
    {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

#[cfg(windows)]
fn total_memory_bytes() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    }
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    let success = unsafe { GlobalMemoryStatusEx(&mut status) };
    (success != 0).then_some(status.total_phys)
}

#[cfg(target_os = "linux")]
fn total_memory_bytes() -> Option<u64> {
    fs::read_to_string("/proc/meminfo")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
        .map(|kib| kib * 1024)
}

#[cfg(target_os = "macos")]
fn total_memory_bytes() -> Option<u64> {
    let output = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn total_memory_bytes() -> Option<u64> {
    None
}

fn isa_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        for (enabled, name) in [
            (std::is_x86_feature_detected!("sse2"), "sse2"),
            (std::is_x86_feature_detected!("sse4.1"), "sse4.1"),
            (std::is_x86_feature_detected!("avx"), "avx"),
            (std::is_x86_feature_detected!("avx2"), "avx2"),
            (std::is_x86_feature_detected!("fma"), "fma"),
        ] {
            if enabled {
                features.push(name);
            }
        }
    }
    #[cfg(target_arch = "aarch64")]
    if std::arch::is_aarch64_feature_detected!("neon") {
        features.push("neon");
    }
    features
}

fn command_text(command: &mut Command) -> Result<String> {
    let output = command.output().context("run identity command")?;
    ensure!(
        output.status.success(),
        "identity command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn command_bytes(command: &mut Command) -> Result<Vec<u8>> {
    let output = command.output().context("run identity command")?;
    ensure!(
        output.status.success(),
        "identity command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn records_sha256(records: &[VectorRecord]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"codestory-vector-backend-records-v2\0");
    digest.update((records.len() as u64).to_le_bytes());
    for record in records {
        hash_len_prefixed(&mut digest, record.identity.node_id.as_bytes());
        hash_len_prefixed(&mut digest, record.identity.document_hash.as_bytes());
        for value in &record.vector {
            digest.update(value.to_le_bytes());
        }
    }
    format!("{:x}", digest.finalize())
}

fn queries_sha256(queries: &[FrozenQuery]) -> String {
    sha256_bytes(&serde_json::to_vec(queries).expect("serialize frozen queries"))
}

fn hash_len_prefixed(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn ensure_no_sqlite_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        ensure!(
            !sidecar.exists(),
            "attested SQLite source has an unbound sidecar: {}",
            sidecar.display()
        );
    }
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_sha256(label: &str, value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{label} must be a canonical lowercase SHA-256"
    );
    Ok(())
}

fn canonical_directory_contents_sha256(path: &Path) -> Result<String> {
    let mut files = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.sort();
    let mut digest = Sha256::new();
    digest.update(b"codestory-vector-backend-generation-contents-v1\0");
    let mut count = 0_u64;
    for file in files {
        ensure!(file.is_file(), "generation contains a non-file entry");
        if file.file_name().and_then(|name| name.to_str()) == Some("manifest.json") {
            continue;
        }
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .context("generation filename is not UTF-8")?;
        hash_len_prefixed(&mut digest, name.as_bytes());
        hash_len_prefixed(&mut digest, &fs::read(&file)?);
        count += 1;
    }
    ensure!(count > 0, "generation contents are empty");
    digest.update(count.to_le_bytes());
    Ok(format!("{:x}", digest.finalize()))
}

fn directory_sha256(path: &Path) -> Result<String> {
    let mut files = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.sort();
    let mut digest = Sha256::new();
    for file in files {
        ensure!(file.is_file(), "generation contains a non-file entry");
        hash_len_prefixed(
            &mut digest,
            file.file_name()
                .and_then(|name| name.to_str())
                .context("generation filename is not UTF-8")?
                .as_bytes(),
        );
        hash_len_prefixed(&mut digest, &fs::read(file)?);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn directory_size(path: &Path) -> Result<u64> {
    fs::read_dir(path)?.try_fold(0_u64, |total, entry| {
        let entry = entry?;
        Ok(total + entry.metadata()?.len())
    })
}

fn vector_blob(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_corrupt_file(path: &Path) -> Result<()> {
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(b"corrupt vector generation")?;
    file.sync_all()?;
    Ok(())
}

fn tamper_published_index(path: &Path) -> Result<()> {
    let mut file = OpenOptions::new().append(true).open(path)?;
    file.write_all(b"post-publication-tamper")?;
    file.sync_all()?;
    Ok(())
}

fn sync_file(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()?;
    Ok(())
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() as f64 * percentile).ceil() as usize).saturating_sub(1);
    sorted[index]
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}
