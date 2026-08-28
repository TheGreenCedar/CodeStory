//! Exact reuse for complete ordered document-embedding requests.
//!
//! Native batched embeddings are numerically sensitive to batch composition.
//! A per-document cache or duplicate-text coalescer can therefore change f32
//! vector bytes, cosine scores, and tie ordering even when the text matches.
//! The cache unit stays the complete ordered outer request and closes over its
//! anchor identities, prepared text, and full producer contract.

use crate::config::{SidecarRuntimeConfig, private_cache_directory};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

const CACHE_SCHEMA_VERSION: i64 = 3;
const CACHE_DIRECTORY: &str = "content-addressed-vectors-v3";
const CACHE_KEY_DOMAIN: &[u8] = b"codestory-content-addressed-vector-batch-v3\0";
const CACHE_CORPUS_DOMAIN: &[u8] = b"codestory-content-addressed-vector-corpus-v2\0";
const CACHE_CONTRACT_DOMAIN: &[u8] = b"codestory-content-addressed-vector-contract-v3\0";
const CACHE_PACKING_CONTRACT: &str = "ordered-outer-request-native-token-pack-v1";
const CACHE_TRUNCATION_CONTRACT: &str = "native-tokenize-truncate-max-input-v1";
const CACHE_NORMALIZATION_CONTRACT: &str = "server-f32-l2-then-index-f64-l2-v1";

pub(crate) struct VectorCacheBatchInput<'a> {
    pub anchor_identity: &'a str,
    pub document_hash: &'a str,
    pub text: &'a str,
}

pub(crate) struct ContentAddressedVectorCache {
    connection: Connection,
    artifact_scope_id: String,
    contract_sha256: String,
    embedding_dim: usize,
    hits: u64,
    misses: u64,
}

impl ContentAddressedVectorCache {
    pub(crate) fn open(
        runtime: &SidecarRuntimeConfig,
        artifact_scope_id: &str,
        producer_compatibility_identity: &str,
        embedding_dim: usize,
    ) -> Result<Self> {
        if artifact_scope_id.trim().is_empty()
            || producer_compatibility_identity.trim().is_empty()
            || embedding_dim == 0
        {
            bail!("content-addressed vector cache identity is incomplete");
        }
        let contract_sha256 =
            cache_contract_sha256(runtime, producer_compatibility_identity, embedding_dim)?;
        let scope_directory = cache_scope_directory(runtime, artifact_scope_id)?;
        let path = scope_directory.join(format!("{contract_sha256}.sqlite3"));
        if let Ok(metadata) = std::fs::symlink_metadata(&path)
            && (metadata.file_type().is_symlink() || !metadata.file_type().is_file())
        {
            bail!("content-addressed vector cache is not a regular file");
        }
        let connection = Connection::open(&path)
            .with_context(|| format!("open content-addressed vector cache {}", path.display()))?;
        connection.busy_timeout(Duration::from_secs(30))?;
        connection.execute_batch(
            "PRAGMA journal_mode=DELETE;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             PRAGMA trusted_schema=OFF;
             CREATE TABLE IF NOT EXISTS cache_metadata (
                 singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                 schema_version INTEGER NOT NULL,
                 artifact_scope_id TEXT NOT NULL,
                 contract_sha256 TEXT NOT NULL,
                 embedding_dim INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS vector_batch_cache (
                 cache_key TEXT PRIMARY KEY,
                 vector_count INTEGER NOT NULL CHECK(vector_count > 0),
                 embedding_dim INTEGER NOT NULL CHECK(embedding_dim > 0),
                 vectors BLOB NOT NULL,
                 vectors_sha256 TEXT NOT NULL CHECK(length(vectors_sha256) = 64)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS vector_corpus_plan (
                 corpus_key TEXT PRIMARY KEY,
                 anchor_count INTEGER NOT NULL CHECK(anchor_count > 0),
                 ordered_anchor_identities BLOB NOT NULL,
                 plan_sha256 TEXT NOT NULL CHECK(length(plan_sha256) = 64)
             ) WITHOUT ROWID;",
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO cache_metadata
             (singleton, schema_version, artifact_scope_id, contract_sha256, embedding_dim)
             VALUES (1, ?1, ?2, ?3, ?4)",
            params![
                CACHE_SCHEMA_VERSION,
                artifact_scope_id,
                contract_sha256,
                i64::try_from(embedding_dim).context("embedding dimension overflow")?,
            ],
        )?;
        let metadata = connection.query_row(
            "SELECT schema_version, artifact_scope_id, contract_sha256, embedding_dim
             FROM cache_metadata WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?;
        if metadata
            != (
                CACHE_SCHEMA_VERSION,
                artifact_scope_id.to_string(),
                contract_sha256.clone(),
                i64::try_from(embedding_dim).context("embedding dimension overflow")?,
            )
        {
            bail!("content-addressed vector cache metadata mismatch");
        }
        Ok(Self {
            connection,
            artifact_scope_id: artifact_scope_id.to_string(),
            contract_sha256,
            embedding_dim,
            hits: 0,
            misses: 0,
        })
    }

    /// Return current indices in the exact batch order selected by the first
    /// complete build of this corpus.
    ///
    /// Core node ids are store-local and therefore reorder otherwise identical
    /// clean builds. Persisting the first complete logical-anchor order keeps
    /// the initial build's native batch geometry unchanged while letting later
    /// publications reproduce it exactly.
    pub(crate) fn canonical_order(
        &mut self,
        inputs: &[VectorCacheBatchInput<'_>],
    ) -> Result<Vec<usize>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        validate_unique_anchor_identities(inputs)?;
        let corpus_key = self.corpus_key(inputs)?;
        let current_plan = inputs
            .iter()
            .map(|input| input.anchor_identity)
            .collect::<Vec<_>>();
        let encoded_current = encode_identity_plan(&current_plan);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT OR IGNORE INTO vector_corpus_plan
             (corpus_key, anchor_count, ordered_anchor_identities, plan_sha256)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                corpus_key,
                i64::try_from(inputs.len()).context("corpus anchor count overflow")?,
                encoded_current.bytes,
                encoded_current.sha256,
            ],
        )?;
        let stored = read_corpus_plan(&transaction, &corpus_key)?
            .context("content-addressed vector cache dropped a corpus plan")?;
        let canonical = match decode_identity_plan(stored, inputs.len()) {
            Ok(plan) => plan,
            Err(_) => {
                transaction.execute(
                    "DELETE FROM vector_corpus_plan WHERE corpus_key = ?1",
                    params![corpus_key],
                )?;
                let replacement = encode_identity_plan(&current_plan);
                transaction.execute(
                    "INSERT INTO vector_corpus_plan
                     (corpus_key, anchor_count, ordered_anchor_identities, plan_sha256)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        corpus_key,
                        i64::try_from(inputs.len()).context("corpus anchor count overflow")?,
                        replacement.bytes,
                        replacement.sha256,
                    ],
                )?;
                current_plan
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect()
            }
        };
        let index_by_identity = inputs
            .iter()
            .enumerate()
            .map(|(index, input)| (input.anchor_identity, index))
            .collect::<HashMap<_, _>>();
        let indices = canonical
            .iter()
            .map(|identity| {
                index_by_identity
                    .get(identity.as_str())
                    .copied()
                    .with_context(|| {
                        format!("cached corpus plan contains foreign anchor identity {identity}")
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        if indices.len() != inputs.len()
            || indices.iter().copied().collect::<HashSet<_>>().len() != inputs.len()
        {
            bail!("cached corpus plan does not cover each anchor exactly once");
        }
        transaction.commit()?;
        Ok(indices)
    }

    pub(crate) fn load_batch(
        &mut self,
        batch: &[VectorCacheBatchInput<'_>],
    ) -> Result<Option<Vec<Vec<f32>>>> {
        let cache_key = self.cache_key(batch)?;
        let row = read_cached_batch(&self.connection, &cache_key)?;
        let Some(row) = row else {
            self.misses = self.misses.saturating_add(1);
            return Ok(None);
        };
        match decode_cached_vectors(&cache_key, row, batch.len(), self.embedding_dim) {
            Ok(vectors) => {
                self.hits = self.hits.saturating_add(1);
                Ok(Some(vectors))
            }
            Err(_) => {
                self.connection.execute(
                    "DELETE FROM vector_batch_cache WHERE cache_key = ?1",
                    params![cache_key],
                )?;
                self.misses = self.misses.saturating_add(1);
                Ok(None)
            }
        }
    }

    pub(crate) fn publish_batch(
        &mut self,
        batch: &[VectorCacheBatchInput<'_>],
        vectors: &[Vec<f32>],
    ) -> Result<Vec<Vec<f32>>> {
        if batch.len() != vectors.len() || batch.is_empty() {
            bail!("content-addressed vector batch coverage mismatch");
        }
        let cache_key = self.cache_key(batch)?;
        let encoded = encode_vectors(&cache_key, vectors, self.embedding_dim)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT OR IGNORE INTO vector_batch_cache
             (cache_key, vector_count, embedding_dim, vectors, vectors_sha256)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                cache_key,
                i64::try_from(vectors.len()).context("vector count overflow")?,
                i64::try_from(self.embedding_dim).context("embedding dimension overflow")?,
                encoded.bytes,
                encoded.sha256,
            ],
        )?;
        let canonical = match read_cached_batch_from_transaction(&transaction, &cache_key)? {
            Some(row) => {
                match decode_cached_vectors(&cache_key, row, batch.len(), self.embedding_dim) {
                    Ok(vectors) => vectors,
                    Err(_) => {
                        transaction.execute(
                            "DELETE FROM vector_batch_cache WHERE cache_key = ?1",
                            params![cache_key],
                        )?;
                        let replacement = encode_vectors(&cache_key, vectors, self.embedding_dim)?;
                        transaction.execute(
                            "INSERT INTO vector_batch_cache
                         (cache_key, vector_count, embedding_dim, vectors, vectors_sha256)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![
                                cache_key,
                                i64::try_from(vectors.len()).context("vector count overflow")?,
                                i64::try_from(self.embedding_dim)
                                    .context("embedding dimension overflow")?,
                                replacement.bytes,
                                replacement.sha256,
                            ],
                        )?;
                        vectors.to_vec()
                    }
                }
            }
            None => bail!("content-addressed vector cache dropped a published batch"),
        };
        transaction.commit()?;
        Ok(canonical)
    }

    pub(crate) fn activity(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    fn cache_key(&self, batch: &[VectorCacheBatchInput<'_>]) -> Result<String> {
        if batch.is_empty() {
            bail!("content-addressed vector cache refuses an empty batch");
        }
        let mut digest = Sha256::new();
        digest.update(CACHE_KEY_DOMAIN);
        hash_part(&mut digest, self.artifact_scope_id.as_bytes());
        hash_part(&mut digest, self.contract_sha256.as_bytes());
        hash_part(&mut digest, &(batch.len() as u64).to_le_bytes());
        for (position, input) in batch.iter().enumerate() {
            if input.anchor_identity.trim().is_empty()
                || input.document_hash.trim().is_empty()
                || input.text.trim().is_empty()
            {
                bail!("content-addressed vector cache input is incomplete");
            }
            hash_part(&mut digest, &(position as u64).to_le_bytes());
            hash_part(&mut digest, input.anchor_identity.as_bytes());
            hash_part(&mut digest, input.document_hash.as_bytes());
            let prefix = crate::embeddings::CODERANK_DOCUMENT_PREFIX_DEFAULT.as_bytes();
            digest.update((prefix.len().saturating_add(input.text.len()) as u64).to_le_bytes());
            digest.update(prefix);
            digest.update(input.text.as_bytes());
        }
        Ok(hex_digest(digest.finalize()))
    }

    fn corpus_key(&self, inputs: &[VectorCacheBatchInput<'_>]) -> Result<String> {
        let mut identities = inputs
            .iter()
            .map(|input| input.anchor_identity)
            .collect::<Vec<_>>();
        identities.sort_unstable();
        let mut digest = Sha256::new();
        digest.update(CACHE_CORPUS_DOMAIN);
        hash_part(&mut digest, self.artifact_scope_id.as_bytes());
        hash_part(&mut digest, self.contract_sha256.as_bytes());
        hash_part(&mut digest, &(identities.len() as u64).to_le_bytes());
        for identity in identities {
            hash_part(&mut digest, identity.as_bytes());
        }
        Ok(hex_digest(digest.finalize()))
    }
}

struct EncodedVectors {
    bytes: Vec<u8>,
    sha256: String,
}

struct CachedVectorRow {
    vector_count: i64,
    embedding_dim: i64,
    bytes: Vec<u8>,
    sha256: String,
}

struct EncodedIdentityPlan {
    bytes: Vec<u8>,
    sha256: String,
}

struct CachedIdentityPlan {
    anchor_count: i64,
    bytes: Vec<u8>,
    sha256: String,
}

fn validate_unique_anchor_identities(inputs: &[VectorCacheBatchInput<'_>]) -> Result<()> {
    let mut seen = HashSet::with_capacity(inputs.len());
    for input in inputs {
        if input.anchor_identity.trim().is_empty() || !seen.insert(input.anchor_identity) {
            bail!("content-addressed vector corpus requires unique logical anchor identities");
        }
    }
    Ok(())
}

fn encode_identity_plan(plan: &[&str]) -> EncodedIdentityPlan {
    let mut bytes = Vec::new();
    for identity in plan {
        bytes.extend((identity.len() as u64).to_le_bytes());
        bytes.extend(identity.as_bytes());
    }
    EncodedIdentityPlan {
        sha256: hex_digest(Sha256::digest(&bytes)),
        bytes,
    }
}

fn decode_identity_plan(plan: CachedIdentityPlan, expected_count: usize) -> Result<Vec<String>> {
    if plan.anchor_count != i64::try_from(expected_count).context("anchor count overflow")?
        || plan.sha256 != hex_digest(Sha256::digest(&plan.bytes))
    {
        bail!("content-addressed vector corpus plan integrity mismatch");
    }
    let mut cursor = plan.bytes.as_slice();
    let mut identities = Vec::with_capacity(expected_count);
    while !cursor.is_empty() {
        let length_bytes = cursor
            .get(..8)
            .context("content-addressed vector corpus plan truncated length")?;
        let length = usize::try_from(u64::from_le_bytes(
            length_bytes.try_into().expect("eight-byte plan length"),
        ))
        .context("content-addressed vector corpus identity length overflow")?;
        cursor = &cursor[8..];
        let identity = cursor
            .get(..length)
            .context("content-addressed vector corpus plan truncated identity")?;
        identities.push(
            std::str::from_utf8(identity)
                .context("content-addressed vector corpus identity is not UTF-8")?
                .to_string(),
        );
        cursor = &cursor[length..];
    }
    if identities.len() != expected_count
        || identities.iter().collect::<HashSet<_>>().len() != expected_count
    {
        bail!("content-addressed vector corpus plan coverage mismatch");
    }
    Ok(identities)
}

fn cache_scope_directory(
    runtime: &SidecarRuntimeConfig,
    artifact_scope_id: &str,
) -> Result<PathBuf> {
    let cache_root = runtime.cache_root.join(CACHE_DIRECTORY);
    private_cache_directory(&cache_root).context("create vector cache root")?;
    let scope_sha256 = hex_digest(Sha256::digest(artifact_scope_id.as_bytes()));
    let scope = cache_root.join(scope_sha256);
    private_cache_directory(&scope).context("create vector cache scope")?;
    Ok(scope)
}

fn cache_contract_sha256(
    runtime: &SidecarRuntimeConfig,
    producer_compatibility_identity: &str,
    embedding_dim: usize,
) -> Result<String> {
    let native = crate::embedding_contract::native_engine_config(runtime.embedding.allow_cpu)?;
    let mut digest = Sha256::new();
    digest.update(CACHE_CONTRACT_DOMAIN);
    hash_part(&mut digest, &CACHE_SCHEMA_VERSION.to_le_bytes());
    for value in [
        producer_compatibility_identity,
        crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
        crate::embedding_contract::EMBEDDING_MODEL_ID,
        crate::embedding_contract::EMBEDDING_MODEL_SHA256,
        codestory_llama_sys::MODEL_TOKENIZER_SHA256,
        codestory_llama_sys::MODEL_CONFIG_SHA256,
        crate::embeddings::CODERANK_QUERY_PREFIX_DEFAULT,
        crate::embeddings::CODERANK_DOCUMENT_PREFIX_DEFAULT,
        crate::embedding_contract::EMBEDDING_POOLING,
        crate::embedding_contract::EMBEDDING_NORMALIZATION,
        crate::embedding_contract::EMBEDDING_ELEMENT_TYPE,
        CACHE_PACKING_CONTRACT,
        CACHE_TRUNCATION_CONTRACT,
        CACHE_NORMALIZATION_CONTRACT,
        native.backend.backend.as_str(),
        native.embedding.model_id.as_str(),
        native.embedding.model_sha256.as_str(),
        pooling_name(native.embedding.pooling),
    ] {
        hash_part(&mut digest, value.as_bytes());
    }
    for value in [
        embedding_dim as u64,
        crate::embedding_contract::EMBEDDING_VECTOR_SCHEMA_VERSION as u64,
        runtime.retrieval.llm_doc_embed_batch_size as u64,
        u64::from(runtime.embedding.allow_cpu),
        native.embedding.dimension as u64,
        u64::from(native.embedding.context_tokens),
        native.embedding.max_input_tokens as u64,
        u64::from(native.embedding.batch_tokens),
        u64::from(native.embedding.max_batch_sequences),
    ] {
        hash_part(&mut digest, &value.to_le_bytes());
    }
    Ok(hex_digest(digest.finalize()))
}

fn pooling_name(pooling: codestory_llama_sys::NativeEmbeddingPooling) -> &'static str {
    match pooling {
        codestory_llama_sys::NativeEmbeddingPooling::Mean => "mean",
        codestory_llama_sys::NativeEmbeddingPooling::Cls => "cls",
        codestory_llama_sys::NativeEmbeddingPooling::Last => "last",
        codestory_llama_sys::NativeEmbeddingPooling::Rank => "rank",
    }
}

fn encode_vectors(
    cache_key: &str,
    vectors: &[Vec<f32>],
    embedding_dim: usize,
) -> Result<EncodedVectors> {
    let mut bytes = Vec::with_capacity(
        vectors
            .len()
            .saturating_mul(embedding_dim)
            .saturating_mul(4),
    );
    for (index, vector) in vectors.iter().enumerate() {
        crate::embedded_vector::validate_vector(
            &format!("content-cache:{cache_key}:{index}"),
            vector,
            embedding_dim,
        )?;
        bytes.extend(
            vector
                .iter()
                .flat_map(|value| value.to_bits().to_le_bytes()),
        );
    }
    Ok(EncodedVectors {
        sha256: hex_digest(Sha256::digest(&bytes)),
        bytes,
    })
}

fn decode_cached_vectors(
    cache_key: &str,
    row: CachedVectorRow,
    expected_count: usize,
    embedding_dim: usize,
) -> Result<Vec<Vec<f32>>> {
    if row.vector_count != i64::try_from(expected_count).context("vector count overflow")?
        || row.embedding_dim != i64::try_from(embedding_dim).context("dimension overflow")?
        || row.sha256 != hex_digest(Sha256::digest(&row.bytes))
        || row.bytes.len()
            != expected_count
                .checked_mul(embedding_dim)
                .and_then(|count| count.checked_mul(4))
                .context("cached vector byte length overflow")?
    {
        bail!("content-addressed vector cache row integrity mismatch");
    }
    let mut vectors = Vec::with_capacity(expected_count);
    for (index, vector_bytes) in row.bytes.chunks_exact(embedding_dim * 4).enumerate() {
        let vector = vector_bytes
            .chunks_exact(4)
            .map(|chunk| {
                f32::from_bits(u32::from_le_bytes(
                    chunk.try_into().expect("four-byte vector component"),
                ))
            })
            .collect::<Vec<_>>();
        crate::embedded_vector::validate_vector(
            &format!("content-cache:{cache_key}:{index}"),
            &vector,
            embedding_dim,
        )?;
        vectors.push(vector);
    }
    Ok(vectors)
}

fn read_cached_batch(connection: &Connection, cache_key: &str) -> Result<Option<CachedVectorRow>> {
    connection
        .query_row(
            "SELECT vector_count, embedding_dim, vectors, vectors_sha256
             FROM vector_batch_cache WHERE cache_key = ?1",
            params![cache_key],
            cached_vector_row,
        )
        .optional()
        .map_err(Into::into)
}

fn read_cached_batch_from_transaction(
    transaction: &Transaction<'_>,
    cache_key: &str,
) -> Result<Option<CachedVectorRow>> {
    transaction
        .query_row(
            "SELECT vector_count, embedding_dim, vectors, vectors_sha256
             FROM vector_batch_cache WHERE cache_key = ?1",
            params![cache_key],
            cached_vector_row,
        )
        .optional()
        .map_err(Into::into)
}

fn read_corpus_plan(
    transaction: &Transaction<'_>,
    corpus_key: &str,
) -> Result<Option<CachedIdentityPlan>> {
    transaction
        .query_row(
            "SELECT anchor_count, ordered_anchor_identities, plan_sha256
             FROM vector_corpus_plan WHERE corpus_key = ?1",
            params![corpus_key],
            |row| {
                Ok(CachedIdentityPlan {
                    anchor_count: row.get(0)?,
                    bytes: row.get(1)?,
                    sha256: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn cached_vector_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CachedVectorRow> {
    Ok(CachedVectorRow {
        vector_count: row.get(0)?,
        embedding_dim: row.get(1)?,
        bytes: row.get(2)?,
        sha256: row.get(3)?,
    })
}

fn hash_part(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        SidecarProcessDefaults, SidecarProfile, SidecarRuntimeDefaults, SidecarRuntimeOverrides,
    };
    use tempfile::TempDir;

    fn runtime(cache: &TempDir, outer_batch_size: usize) -> SidecarRuntimeConfig {
        let private_root = cache.path().join("private");
        private_cache_directory(&private_root).expect("private cache root");
        runtime_with_root(private_root, outer_batch_size)
    }

    fn runtime_with_root(cache_root: PathBuf, outer_batch_size: usize) -> SidecarRuntimeConfig {
        let defaults = SidecarProcessDefaults::new(cache_root, SidecarRuntimeDefaults::default());
        let mut runtime = SidecarRuntimeConfig::for_project_profile_with_process_defaults(
            None,
            SidecarProfile::Local,
            None,
            &defaults,
            &SidecarRuntimeOverrides::default(),
        );
        runtime.retrieval.llm_doc_embed_batch_size = outer_batch_size;
        runtime
    }

    fn inputs<'a>(rows: &'a [(&'a str, &'a str, &'a str)]) -> Vec<VectorCacheBatchInput<'a>> {
        rows.iter()
            .map(
                |(anchor_identity, document_hash, text)| VectorCacheBatchInput {
                    anchor_identity,
                    document_hash,
                    text,
                },
            )
            .collect()
    }

    #[test]
    fn complete_batch_identity_reuses_exact_vector_bits() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let rows = [
            ("node-1", "doc-1", "same text"),
            ("node-2", "doc-2", "next"),
        ];
        let batch = inputs(&rows);
        let expected = vec![vec![1.0, -0.0], vec![0.0, 1.0]];

        let mut writer =
            ContentAddressedVectorCache::open(&selected_runtime, "scope-a", "producer-a", 2)
                .expect("open writer");
        assert_eq!(
            writer.publish_batch(&batch, &expected).expect("publish"),
            expected
        );
        drop(writer);

        let mut reader =
            ContentAddressedVectorCache::open(&selected_runtime, "scope-a", "producer-a", 2)
                .expect("open reader");
        let observed = reader.load_batch(&batch).expect("load").expect("cache hit");
        assert_eq!(observed, expected);
        assert_eq!(observed[0][0].to_bits(), expected[0][0].to_bits());
        assert_eq!(observed[0][1].to_bits(), expected[0][1].to_bits());
    }

    #[test]
    fn first_corpus_order_is_reproduced_when_store_local_order_changes() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let first_rows = [
            ("stable-a", "doc-a", "first"),
            ("stable-b", "doc-b", "second"),
        ];
        let reversed_rows = [
            ("stable-b", "changed-doc-b", "changed second"),
            ("stable-a", "changed-doc-a", "changed first"),
        ];
        let first = inputs(&first_rows);
        let reversed = inputs(&reversed_rows);

        let mut owner =
            ContentAddressedVectorCache::open(&selected_runtime, "scope-a", "producer-a", 2)
                .expect("open cache");
        assert_eq!(owner.canonical_order(&first).expect("first plan"), [0, 1]);
        assert_eq!(
            owner.canonical_order(&reversed).expect("reproduced plan"),
            [1, 0]
        );
    }

    #[test]
    fn ambiguous_logical_anchor_identity_disables_corpus_reuse() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let rows = [
            ("stable-a", "doc-a", "first"),
            ("stable-a", "doc-a", "first"),
        ];
        let repeated = inputs(&rows);
        let mut owner =
            ContentAddressedVectorCache::open(&selected_runtime, "scope-a", "producer-a", 2)
                .expect("open cache");

        assert!(
            owner
                .canonical_order(&repeated)
                .expect_err("ambiguous identity")
                .to_string()
                .contains("unique logical anchor identities")
        );
    }

    #[cfg(unix)]
    #[test]
    fn cache_claims_a_private_child_under_a_readable_user_cache_root() {
        use std::os::unix::fs::PermissionsExt;

        let cache = TempDir::new().expect("cache root");
        let readable_root = cache.path().join("readable-user-cache");
        std::fs::create_dir(&readable_root).expect("readable cache root");
        std::fs::set_permissions(&readable_root, std::fs::Permissions::from_mode(0o755))
            .expect("readable cache permissions");
        let selected_runtime = runtime_with_root(readable_root, 128);

        ContentAddressedVectorCache::open(&selected_runtime, "scope-a", "producer-a", 2)
            .expect("private cache child");
        let cache_root = selected_runtime.cache_root.join(CACHE_DIRECTORY);
        assert_eq!(
            std::fs::metadata(&cache_root)
                .expect("cache metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn cache_key_closes_text_order_anchor_scope_and_contract_classes() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let rows = [
            ("node-1", "doc-1", "same text"),
            ("node-2", "doc-2", "next"),
        ];
        let batch = inputs(&rows);
        let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let mut owner =
            ContentAddressedVectorCache::open(&selected_runtime, "scope-a", "producer-a", 2)
                .expect("open cache");
        owner.publish_batch(&batch, &vectors).expect("publish");

        for changed in [
            vec![("node-1", "doc-1", "changed"), ("node-2", "doc-2", "next")],
            vec![
                ("node-2", "doc-2", "next"),
                ("node-1", "doc-1", "same text"),
            ],
            vec![
                ("other-node", "doc-1", "same text"),
                ("node-2", "doc-2", "next"),
            ],
            vec![
                ("node-1", "other-doc", "same text"),
                ("node-2", "doc-2", "next"),
            ],
        ] {
            assert!(
                owner
                    .load_batch(&inputs(&changed))
                    .expect("lookup")
                    .is_none()
            );
        }
        drop(owner);

        let mut other_scope =
            ContentAddressedVectorCache::open(&selected_runtime, "scope-b", "producer-a", 2)
                .expect("other scope");
        assert!(other_scope.load_batch(&batch).expect("lookup").is_none());
        let mut other_contract =
            ContentAddressedVectorCache::open(&selected_runtime, "scope-a", "producer-b", 2)
                .expect("other contract");
        assert!(other_contract.load_batch(&batch).expect("lookup").is_none());
        let changed_runtime = runtime(&cache, 64);
        let mut other_packing =
            ContentAddressedVectorCache::open(&changed_runtime, "scope-a", "producer-a", 2)
                .expect("other packing");
        assert!(other_packing.load_batch(&batch).expect("lookup").is_none());
    }

    #[test]
    fn identical_text_for_distinct_anchors_is_not_coalesced() {
        let cache = TempDir::new().expect("cache root");
        let runtime = runtime(&cache, 128);
        let first_rows = [("node-1", "doc-1", "duplicate")];
        let second_rows = [("node-2", "doc-2", "duplicate")];
        let first = inputs(&first_rows);
        let second = inputs(&second_rows);
        let mut owner = ContentAddressedVectorCache::open(&runtime, "scope-a", "producer-a", 2)
            .expect("open cache");
        owner
            .publish_batch(&first, &[vec![1.0, 0.0]])
            .expect("publish");
        assert!(owner.load_batch(&second).expect("lookup").is_none());
    }

    #[test]
    fn first_complete_publication_is_canonical_for_a_batch_key() {
        let cache = TempDir::new().expect("cache root");
        let runtime = runtime(&cache, 128);
        let rows = [("node-1", "doc-1", "same text")];
        let batch = inputs(&rows);
        let mut owner = ContentAddressedVectorCache::open(&runtime, "scope-a", "producer-a", 2)
            .expect("open cache");
        let first = vec![vec![1.0, 0.0]];
        let competing = vec![vec![0.0, 1.0]];
        owner.publish_batch(&batch, &first).expect("first");
        assert_eq!(
            owner.publish_batch(&batch, &competing).expect("competing"),
            first
        );
    }

    #[test]
    fn corrupted_rows_are_evicted_and_never_authorize_reuse() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let rows = [("node-1", "doc-1", "same text")];
        let batch = inputs(&rows);
        let mut owner =
            ContentAddressedVectorCache::open(&selected_runtime, "scope-a", "producer-a", 2)
                .expect("open cache");
        owner
            .publish_batch(&batch, &[vec![1.0, 0.0]])
            .expect("publish");
        let key = owner.cache_key(&batch).expect("cache key");
        owner
            .connection
            .execute(
                "UPDATE vector_batch_cache SET vectors_sha256 = ?2 WHERE cache_key = ?1",
                params![key, "0".repeat(64)],
            )
            .expect("corrupt row");

        assert!(owner.load_batch(&batch).expect("lookup").is_none());
        let retained = owner
            .connection
            .query_row(
                "SELECT COUNT(*) FROM vector_batch_cache WHERE cache_key = ?1",
                params![key],
                |row| row.get::<_, i64>(0),
            )
            .expect("count rows");
        assert_eq!(retained, 0);
    }

    #[test]
    fn invalid_vectors_never_enter_the_cache() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let rows = [("node-1", "doc-1", "same text")];
        let batch = inputs(&rows);
        let mut owner =
            ContentAddressedVectorCache::open(&selected_runtime, "scope-a", "producer-a", 2)
                .expect("open cache");
        assert!(
            owner
                .publish_batch(&batch, &[vec![2.0, 0.0]])
                .expect_err("non-normalized vector must fail")
                .to_string()
                .contains("not L2-normalized")
        );
        assert!(owner.load_batch(&batch).expect("lookup").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_cache_scope_is_refused() {
        use std::os::unix::fs::symlink;

        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let cache_root = selected_runtime.cache_root.join(CACHE_DIRECTORY);
        private_cache_directory(&cache_root).expect("cache directory");
        let scope_sha256 = hex_digest(Sha256::digest(b"scope-a"));
        let outside = TempDir::new().expect("outside");
        symlink(outside.path(), cache_root.join(scope_sha256)).expect("symlink scope");

        let error =
            ContentAddressedVectorCache::open(&selected_runtime, "scope-a", "producer-a", 2)
                .err()
                .expect("symlink scope must fail");
        assert!(error.to_string().contains("create vector cache scope"));
    }
}
