//! Exact reuse for complete ordered document-embedding requests.
//!
//! Native batched embeddings are numerically sensitive to batch composition.
//! A per-document cache or duplicate-text coalescer can therefore change f32
//! vector bytes, cosine scores, and tie ordering even when the text matches.
//! The cache unit stays the complete ordered outer request and closes over its
//! anchor identities, prepared text, and full producer contract.

use crate::config::{SidecarRuntimeConfig, private_cache_directory};
use anyhow::{Context, Result, bail};
use codestory_contracts::bounded_locks::{self, FileLockKind, LockDeadline};
use codestory_workspace::owned_deletion::OwnedDeletionRoot;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

const CACHE_SCHEMA_VERSION: i64 = 5;
const CACHE_DIRECTORY: &str = "content-addressed-vectors-v5";
const CACHE_KEY_DOMAIN: &[u8] = b"codestory-content-addressed-vector-batch-v5\0";
const CACHE_CORPUS_DOMAIN: &[u8] = b"codestory-content-addressed-vector-corpus-v4\0";
const CACHE_CONTRACT_DOMAIN: &[u8] = b"codestory-content-addressed-vector-contract-v5\0";
const CACHE_PACKING_CONTRACT: &str = "ordered-outer-request-native-token-pack-v1";
const CACHE_TRUNCATION_CONTRACT: &str = "native-tokenize-truncate-max-input-v1";
const CACHE_NORMALIZATION_CONTRACT: &str = "server-f32-l2-then-index-f64-l2-v1";
const CACHE_MAX_PAYLOAD_BYTES: u64 = 96 * 1024 * 1024;
const CACHE_MAX_DATABASE_BYTES: u64 = 128 * 1024 * 1024;
const CACHE_MAX_AGGREGATE_DATABASE_BYTES: u64 = 512 * 1024 * 1024;
const CACHE_ROW_ACCOUNTING_BYTES: u64 = 512;
const CACHE_RETENTION_DIRECTORY: &str = "content-addressed-vector-retention-v1";
const CACHE_RETENTION_REGISTRY: &str = "registry.sqlite3";
const CACHE_RETENTION_GLOBAL_LOCK: &str = "maintenance.lock";
const CACHE_RETENTION_SCOPE_LOCKS: &str = "scope-locks";
const CACHE_RETENTION_REGISTRY_SCHEMA_VERSION: i64 = 2;
const CACHE_RETENTION_REGISTRY_MAX_BYTES: u64 = 8 * 1024 * 1024;
const CACHE_SCOPE_OWNERSHIP_MARKER: &str = "ownership-v1";

struct VectorCacheScopeLease {
    file: File,
}

impl Drop for VectorCacheScopeLease {
    fn drop(&mut self) {
        let _ = bounded_locks::release(&self.file);
    }
}

struct VectorCacheMaintenanceLock {
    file: File,
}

impl Drop for VectorCacheMaintenanceLock {
    fn drop(&mut self) {
        let _ = bounded_locks::release(&self.file);
    }
}

pub(crate) struct VectorCacheBatchInput<'a> {
    pub anchor_identity: &'a str,
    pub document_hash: &'a str,
    pub text: &'a str,
}

pub(crate) struct ContentAddressedVectorCache {
    connection: Connection,
    _scope_lease: VectorCacheScopeLease,
    #[cfg(test)]
    path: PathBuf,
    artifact_scope_id: String,
    contract_sha256: String,
    embedding_dim: usize,
    max_payload_bytes: u64,
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
        Self::open_with_limits(
            runtime,
            artifact_scope_id,
            producer_compatibility_identity,
            embedding_dim,
            CACHE_MAX_PAYLOAD_BYTES,
            CACHE_MAX_DATABASE_BYTES,
        )
    }

    fn open_with_limits(
        runtime: &SidecarRuntimeConfig,
        artifact_scope_id: &str,
        producer_compatibility_identity: &str,
        embedding_dim: usize,
        max_payload_bytes: u64,
        max_database_bytes: u64,
    ) -> Result<Self> {
        Self::open_with_retention_limits(
            runtime,
            artifact_scope_id,
            producer_compatibility_identity,
            embedding_dim,
            max_payload_bytes,
            max_database_bytes,
            CACHE_MAX_AGGREGATE_DATABASE_BYTES,
        )
    }

    fn open_with_retention_limits(
        runtime: &SidecarRuntimeConfig,
        artifact_scope_id: &str,
        producer_compatibility_identity: &str,
        embedding_dim: usize,
        max_payload_bytes: u64,
        max_database_bytes: u64,
        max_aggregate_database_bytes: u64,
    ) -> Result<Self> {
        if artifact_scope_id.trim().is_empty()
            || producer_compatibility_identity.trim().is_empty()
            || embedding_dim == 0
            || max_payload_bytes == 0
            || max_database_bytes <= max_payload_bytes
            || max_aggregate_database_bytes < max_database_bytes
        {
            bail!("content-addressed vector cache identity is incomplete");
        }
        let contract_sha256 =
            cache_contract_sha256(runtime, producer_compatibility_identity, embedding_dim)?;
        let scope_name = hex_digest(Sha256::digest(artifact_scope_id.as_bytes()));
        let retention = VectorCacheRetention::open(runtime)?;
        retention.claim_current_scope(
            CACHE_DIRECTORY,
            &scope_name,
            CACHE_SCHEMA_VERSION,
            max_database_bytes,
        )?;
        let scope_lease = retention.acquire_shared_scope(CACHE_DIRECTORY, &scope_name)?;
        retention.register_and_enforce(
            CACHE_DIRECTORY,
            &scope_name,
            CACHE_SCHEMA_VERSION,
            max_database_bytes,
            max_aggregate_database_bytes,
        )?;
        let scope_directory = cache_scope_directory(runtime, artifact_scope_id)?;
        let path = scope_directory.join("vectors.sqlite3");
        if let Ok(metadata) = std::fs::symlink_metadata(&path)
            && (metadata.file_type().is_symlink() || !metadata.file_type().is_file())
        {
            bail!("content-addressed vector cache is not a regular file");
        }
        let connection = Connection::open(&path)
            .with_context(|| format!("open content-addressed vector cache {}", path.display()))?;
        connection.busy_timeout(Duration::from_secs(30))?;
        let page_size = u64::try_from(
            connection.pragma_query_value(None, "page_size", |row| row.get::<_, i64>(0))?,
        )
        .context("content-addressed vector cache page size is invalid")?;
        let max_page_count = max_database_bytes
            .checked_div(page_size)
            .context("content-addressed vector cache database limit is below one page")?;
        connection.pragma_update(
            None,
            "max_page_count",
            i64::try_from(max_page_count).context("cache page count overflow")?,
        )?;
        let applied_max_page_count = u64::try_from(connection.pragma_query_value(
            None,
            "max_page_count",
            |row| row.get::<_, i64>(0),
        )?)
        .context("content-addressed vector cache maximum page count is invalid")?;
        if applied_max_page_count > max_page_count {
            bail!("content-addressed vector cache already exceeds its database limit");
        }
        connection.execute_batch(
            "PRAGMA journal_mode=DELETE;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             PRAGMA trusted_schema=OFF;
             CREATE TABLE IF NOT EXISTS cache_metadata (
                 singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                 schema_version INTEGER NOT NULL,
                 artifact_scope_id TEXT NOT NULL,
                 max_payload_bytes INTEGER NOT NULL CHECK(max_payload_bytes > 0),
                 max_database_bytes INTEGER NOT NULL CHECK(max_database_bytes > max_payload_bytes),
                 access_sequence INTEGER NOT NULL CHECK(access_sequence >= 0)
             );
             CREATE TABLE IF NOT EXISTS vector_batch_cache (
                 cache_key TEXT PRIMARY KEY,
                 contract_sha256 TEXT NOT NULL CHECK(length(contract_sha256) = 64),
                 vector_count INTEGER NOT NULL CHECK(vector_count > 0),
                 embedding_dim INTEGER NOT NULL CHECK(embedding_dim > 0),
                 vectors BLOB NOT NULL,
                 vectors_sha256 TEXT NOT NULL CHECK(length(vectors_sha256) = 64),
                 last_access_sequence INTEGER NOT NULL CHECK(last_access_sequence > 0)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS vector_corpus_plan (
                 corpus_key TEXT PRIMARY KEY,
                 contract_sha256 TEXT NOT NULL CHECK(length(contract_sha256) = 64),
                 anchor_count INTEGER NOT NULL CHECK(anchor_count > 0),
                 ordered_anchor_identities BLOB NOT NULL,
                 plan_sha256 TEXT NOT NULL CHECK(length(plan_sha256) = 64),
                 last_access_sequence INTEGER NOT NULL CHECK(last_access_sequence > 0)
             ) WITHOUT ROWID;",
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO cache_metadata
             (singleton, schema_version, artifact_scope_id, max_payload_bytes,
              max_database_bytes, access_sequence)
             VALUES (1, ?1, ?2, ?3, ?4, 0)",
            params![
                CACHE_SCHEMA_VERSION,
                artifact_scope_id,
                i64::try_from(max_payload_bytes).context("cache payload limit overflow")?,
                i64::try_from(max_database_bytes).context("cache database limit overflow")?,
            ],
        )?;
        let metadata = connection.query_row(
            "SELECT schema_version, artifact_scope_id, max_payload_bytes, max_database_bytes
             FROM cache_metadata WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?;
        if metadata
            != (
                CACHE_SCHEMA_VERSION,
                artifact_scope_id.to_string(),
                i64::try_from(max_payload_bytes).context("cache payload limit overflow")?,
                i64::try_from(max_database_bytes).context("cache database limit overflow")?,
            )
        {
            bail!("content-addressed vector cache metadata mismatch");
        }
        Ok(Self {
            connection,
            _scope_lease: scope_lease,
            #[cfg(test)]
            path,
            artifact_scope_id: artifact_scope_id.to_string(),
            contract_sha256,
            embedding_dim,
            max_payload_bytes,
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
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let canonical = match read_corpus_plan(&transaction, &corpus_key)? {
            Some(stored) => match decode_identity_plan(stored, inputs.len(), &self.contract_sha256)
            {
                Ok(plan) => {
                    touch_corpus_plan(&transaction, &corpus_key)?;
                    plan
                }
                Err(_) => {
                    transaction.execute(
                        "DELETE FROM vector_corpus_plan WHERE corpus_key = ?1",
                        params![corpus_key],
                    )?;
                    insert_corpus_plan(
                        &transaction,
                        &corpus_key,
                        &self.contract_sha256,
                        &current_plan,
                        self.max_payload_bytes,
                    )?;
                    current_plan
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect()
                }
            },
            None => {
                insert_corpus_plan(
                    &transaction,
                    &corpus_key,
                    &self.contract_sha256,
                    &current_plan,
                    self.max_payload_bytes,
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
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = read_cached_batch_from_transaction(&transaction, &cache_key)?;
        let Some(row) = row else {
            transaction.commit()?;
            self.misses = self.misses.saturating_add(1);
            return Ok(None);
        };
        match decode_cached_vectors(
            &cache_key,
            row,
            batch.len(),
            self.embedding_dim,
            &self.contract_sha256,
        ) {
            Ok(vectors) => {
                touch_vector_batch(&transaction, &cache_key)?;
                transaction.commit()?;
                self.hits = self.hits.saturating_add(1);
                Ok(Some(vectors))
            }
            Err(_) => {
                transaction.execute(
                    "DELETE FROM vector_batch_cache WHERE cache_key = ?1",
                    params![cache_key],
                )?;
                transaction.commit()?;
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
        let canonical = match read_cached_batch_from_transaction(&transaction, &cache_key)? {
            Some(row) => match decode_cached_vectors(
                &cache_key,
                row,
                batch.len(),
                self.embedding_dim,
                &self.contract_sha256,
            ) {
                Ok(canonical) => {
                    touch_vector_batch(&transaction, &cache_key)?;
                    canonical
                }
                Err(_) => {
                    transaction.execute(
                        "DELETE FROM vector_batch_cache WHERE cache_key = ?1",
                        params![cache_key],
                    )?;
                    insert_vector_batch(
                        &transaction,
                        &cache_key,
                        &self.contract_sha256,
                        vectors,
                        encoded,
                        self.embedding_dim,
                        self.max_payload_bytes,
                    )?;
                    vectors.to_vec()
                }
            },
            None => {
                insert_vector_batch(
                    &transaction,
                    &cache_key,
                    &self.contract_sha256,
                    vectors,
                    encoded,
                    self.embedding_dim,
                    self.max_payload_bytes,
                )?;
                vectors.to_vec()
            }
        };
        transaction.commit()?;
        Ok(canonical)
    }

    pub(crate) fn activity(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    #[cfg(test)]
    fn accounted_payload_bytes(&self) -> Result<u64> {
        accounted_payload_bytes(&self.connection)
    }

    #[cfg(test)]
    fn retained_batch_count(&self) -> Result<u64> {
        let count = self
            .connection
            .query_row("SELECT COUNT(*) FROM vector_batch_cache", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(anyhow::Error::from)?;
        u64::try_from(count).context("cache row count is negative")
    }

    #[cfg(test)]
    fn retained_plan_count(&self) -> Result<u64> {
        let count = self
            .connection
            .query_row("SELECT COUNT(*) FROM vector_corpus_plan", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(anyhow::Error::from)?;
        u64::try_from(count).context("cache plan count is negative")
    }

    #[cfg(test)]
    fn database_bytes(&self) -> Result<u64> {
        Ok(std::fs::metadata(&self.path)?.len())
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
    contract_sha256: String,
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
    contract_sha256: String,
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

fn decode_identity_plan(
    plan: CachedIdentityPlan,
    expected_count: usize,
    expected_contract_sha256: &str,
) -> Result<Vec<String>> {
    if plan.contract_sha256 != expected_contract_sha256
        || plan.anchor_count != i64::try_from(expected_count).context("anchor count overflow")?
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

struct VectorCacheRetention {
    cache_root: PathBuf,
    retention_root: PathBuf,
    registry: Connection,
    _maintenance: VectorCacheMaintenanceLock,
}

#[derive(Clone)]
struct RegisteredVectorCacheScope {
    root_name: String,
    scope_name: String,
    cache_schema_version: i64,
    last_access_sequence: i64,
    max_database_bytes: u64,
    ownership_token: String,
}

struct InactiveVectorCacheScope {
    registered: RegisteredVectorCacheScope,
    bytes: u64,
    lock: File,
}

impl VectorCacheRetention {
    fn open(runtime: &SidecarRuntimeConfig) -> Result<Self> {
        let retention_root = runtime.cache_root.join(CACHE_RETENTION_DIRECTORY);
        private_cache_directory(&retention_root)
            .context("create content-addressed vector retention root")?;
        let locks_root = retention_root.join(CACHE_RETENTION_SCOPE_LOCKS);
        private_cache_directory(&locks_root)
            .context("create content-addressed vector retention lock root")?;
        let maintenance_path = retention_root.join(CACHE_RETENTION_GLOBAL_LOCK);
        let maintenance_file = open_lock_file(&maintenance_path)?;
        bounded_locks::acquire_with_deadline(
            &maintenance_file,
            FileLockKind::Exclusive,
            LockDeadline::after(bounded_locks::DEFAULT_LOCK_WAIT),
            None,
        )
        .map_err(anyhow::Error::new)
        .context("acquire content-addressed vector retention maintenance lock")?;
        let maintenance = VectorCacheMaintenanceLock {
            file: maintenance_file,
        };

        let registry_path = retention_root.join(CACHE_RETENTION_REGISTRY);
        reject_non_regular_file(&registry_path, "vector retention registry")?;
        let registry = Connection::open(&registry_path).with_context(|| {
            format!(
                "open content-addressed vector retention registry {}",
                registry_path.display()
            )
        })?;
        registry.busy_timeout(Duration::from_secs(30))?;
        registry.execute_batch(
            "PRAGMA journal_mode=DELETE;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             PRAGMA trusted_schema=OFF;
             CREATE TABLE IF NOT EXISTS retention_metadata (
                 singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                 schema_version INTEGER NOT NULL,
                 access_sequence INTEGER NOT NULL CHECK(access_sequence >= 0)
             );
             CREATE TABLE IF NOT EXISTS owned_vector_cache_scope (
                 root_name TEXT NOT NULL,
                 scope_name TEXT NOT NULL,
                 cache_schema_version INTEGER NOT NULL CHECK(cache_schema_version > 0),
                 last_access_sequence INTEGER NOT NULL CHECK(last_access_sequence > 0),
                 max_database_bytes INTEGER NOT NULL CHECK(max_database_bytes > 0),
                 ownership_token TEXT NOT NULL CHECK(length(ownership_token) = 36),
                 PRIMARY KEY(root_name, scope_name)
             ) WITHOUT ROWID;",
        )?;
        registry.execute(
            "INSERT OR IGNORE INTO retention_metadata
             (singleton, schema_version, access_sequence) VALUES (1, ?1, 0)",
            params![CACHE_RETENTION_REGISTRY_SCHEMA_VERSION],
        )?;
        let schema_version = registry.query_row(
            "SELECT schema_version FROM retention_metadata WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if schema_version != CACHE_RETENTION_REGISTRY_SCHEMA_VERSION {
            bail!("content-addressed vector retention registry schema mismatch");
        }
        let page_size = u64::try_from(
            registry.pragma_query_value(None, "page_size", |row| row.get::<_, i64>(0))?,
        )
        .context("content-addressed vector retention page size is invalid")?;
        let max_page_count = CACHE_RETENTION_REGISTRY_MAX_BYTES
            .checked_div(page_size)
            .context("content-addressed vector retention registry page size exceeds its cap")?;
        registry.pragma_update(
            None,
            "max_page_count",
            i64::try_from(max_page_count).context("vector retention page count overflow")?,
        )?;
        let applied_max_page_count =
            u64::try_from(
                registry.pragma_query_value(None, "max_page_count", |row| row.get::<_, i64>(0))?,
            )
            .context("content-addressed vector retention maximum page count is invalid")?;
        if applied_max_page_count > max_page_count {
            bail!("content-addressed vector retention registry exceeds its database limit");
        }

        Ok(Self {
            cache_root: runtime.cache_root.clone(),
            retention_root,
            registry,
            _maintenance: maintenance,
        })
    }

    fn acquire_shared_scope(
        &self,
        root_name: &str,
        scope_name: &str,
    ) -> Result<VectorCacheScopeLease> {
        let (_, file) = self.open_scope_lock(root_name, scope_name)?;
        bounded_locks::acquire_with_deadline(
            &file,
            FileLockKind::Shared,
            LockDeadline::after(bounded_locks::DEFAULT_LOCK_WAIT),
            None,
        )
        .map_err(anyhow::Error::new)
        .context("acquire content-addressed vector cache scope lease")?;
        Ok(VectorCacheScopeLease { file })
    }

    fn claim_current_scope(
        &self,
        root_name: &str,
        scope_name: &str,
        cache_schema_version: i64,
        max_database_bytes: u64,
    ) -> Result<()> {
        validate_registered_scope(root_name, scope_name, cache_schema_version)?;
        let root = self.cache_root.join(root_name);
        private_cache_directory(&root).context("verify content-addressed vector cache root")?;
        let scope = root.join(scope_name);
        let scope_bytes = known_scope_bytes(&scope)?
            .context("content-addressed vector cache scope contains unknown or unsafe artifacts")?;
        let registered = self
            .registry
            .query_row(
                "SELECT cache_schema_version, max_database_bytes, ownership_token
                 FROM owned_vector_cache_scope
                 WHERE root_name = ?1 AND scope_name = ?2",
                params![root_name, scope_name],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((registered_schema, registered_max, ownership_token)) = registered {
            if registered_schema != cache_schema_version
                || registered_max
                    != i64::try_from(max_database_bytes)
                        .context("vector cache database reservation overflow")?
            {
                bail!("content-addressed vector cache ownership metadata mismatch");
            }
            match read_scope_ownership_token(&scope)? {
                Some(observed) if observed == ownership_token => return Ok(()),
                Some(_) => bail!("content-addressed vector cache ownership token mismatch"),
                None if scope_bytes == 0 => {
                    private_cache_directory(&scope)
                        .context("recover registered vector cache scope")?;
                    write_scope_ownership_token(&scope, &ownership_token)?;
                    return Ok(());
                }
                None => {
                    bail!(
                        "registered content-addressed vector cache is missing its ownership token"
                    )
                }
            }
        }

        if scope_bytes != 0 || read_scope_ownership_token(&scope)?.is_some() {
            bail!("refuse to claim unregistered content-addressed vector cache artifacts");
        }
        let ownership_token = Uuid::new_v4().to_string();
        self.registry.execute(
            "UPDATE retention_metadata
             SET access_sequence = CASE
                 WHEN access_sequence < 9223372036854775807 THEN access_sequence + 1
                 ELSE access_sequence
             END
             WHERE singleton = 1",
            [],
        )?;
        let access_sequence = self.registry.query_row(
            "SELECT access_sequence FROM retention_metadata WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        self.registry.execute(
            "INSERT INTO owned_vector_cache_scope
             (root_name, scope_name, cache_schema_version, last_access_sequence,
              max_database_bytes, ownership_token)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                root_name,
                scope_name,
                cache_schema_version,
                access_sequence,
                i64::try_from(max_database_bytes)
                    .context("vector cache database reservation overflow")?,
                ownership_token,
            ],
        )?;
        private_cache_directory(&scope).context("create registered vector cache scope")?;
        write_scope_ownership_token(&scope, &ownership_token)?;

        Ok(())
    }

    fn register_and_enforce(
        &self,
        root_name: &str,
        scope_name: &str,
        cache_schema_version: i64,
        max_database_bytes: u64,
        max_aggregate_database_bytes: u64,
    ) -> Result<()> {
        validate_registered_scope(root_name, scope_name, cache_schema_version)?;
        let current_path = self.cache_root.join(root_name).join(scope_name);
        known_scope_bytes(&current_path)?
            .context("content-addressed vector cache scope contains unknown or unsafe artifacts")?;

        let existing = self
            .registry
            .query_row(
                "SELECT cache_schema_version, max_database_bytes
                 FROM owned_vector_cache_scope
                 WHERE root_name = ?1 AND scope_name = ?2",
                params![root_name, scope_name],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        if let Some((registered_schema, registered_max)) = existing
            && (registered_schema != cache_schema_version
                || registered_max
                    != i64::try_from(max_database_bytes)
                        .context("vector cache database reservation overflow")?)
        {
            bail!("content-addressed vector cache ownership metadata mismatch");
        }

        self.registry.execute(
            "UPDATE retention_metadata
             SET access_sequence = CASE
                 WHEN access_sequence < 9223372036854775807 THEN access_sequence + 1
                 ELSE access_sequence
             END
             WHERE singleton = 1",
            [],
        )?;
        let access_sequence = self.registry.query_row(
            "SELECT access_sequence FROM retention_metadata WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        let touched = self.registry.execute(
            "UPDATE owned_vector_cache_scope
             SET last_access_sequence = ?1
             WHERE root_name = ?2 AND scope_name = ?3",
            params![access_sequence, root_name, scope_name],
        )?;
        if touched != 1 {
            bail!("content-addressed vector cache ownership registration is missing");
        }

        (|| {
            let mut inactive = Vec::new();
            let registered_scopes = self.registered_scopes()?;
            let registered_identities = registered_scopes
                .iter()
                .map(|registered| (registered.root_name.clone(), registered.scope_name.clone()))
                .collect::<HashSet<_>>();
            let mut reserved_or_stored_bytes =
                unregistered_vector_cache_bytes(&self.cache_root, &registered_identities)?;
            for registered in registered_scopes {
                validate_registered_scope(
                    &registered.root_name,
                    &registered.scope_name,
                    registered.cache_schema_version,
                )?;
                let path = self
                    .cache_root
                    .join(&registered.root_name)
                    .join(&registered.scope_name);
                let Some(bytes) = known_scope_bytes(&path)? else {
                    bail!(
                        "registered content-addressed vector cache contains unknown or unsafe artifacts"
                    );
                };
                let observed_token = read_scope_ownership_token(&path)?;
                if observed_token.as_deref() != Some(registered.ownership_token.as_str())
                    && !(observed_token.is_none() && bytes == 0)
                {
                    bail!("registered content-addressed vector cache ownership token mismatch");
                }
                if registered.root_name == root_name && registered.scope_name == scope_name {
                    reserved_or_stored_bytes = reserved_or_stored_bytes
                        .checked_add(registered.max_database_bytes)
                        .context("aggregate vector cache byte count overflow")?;
                    continue;
                }

                let (lock_path, lock) =
                    self.open_scope_lock(&registered.root_name, &registered.scope_name)?;
                if bounded_locks::try_acquire(&lock, FileLockKind::Exclusive)
                    .map_err(anyhow::Error::new)
                    .context("inspect content-addressed vector cache scope lease")?
                {
                    if bytes == 0 || !scope_has_database(&path)? {
                        self.remove_registered_scope(&registered, lock_path, lock)?;
                    } else {
                        reserved_or_stored_bytes = reserved_or_stored_bytes
                            .checked_add(bytes)
                            .context("aggregate vector cache byte count overflow")?;
                        inactive.push(InactiveVectorCacheScope {
                            registered,
                            bytes,
                            lock,
                        });
                    }
                } else {
                    reserved_or_stored_bytes = reserved_or_stored_bytes
                        .checked_add(registered.max_database_bytes)
                        .context("aggregate vector cache byte count overflow")?;
                }
            }

            inactive.sort_by(|left, right| {
                left.registered
                    .last_access_sequence
                    .cmp(&right.registered.last_access_sequence)
                    .then_with(|| left.registered.root_name.cmp(&right.registered.root_name))
                    .then_with(|| left.registered.scope_name.cmp(&right.registered.scope_name))
            });
            let removable_bytes = inactive.iter().try_fold(0_u64, |total, entry| {
                total
                    .checked_add(entry.bytes)
                    .context("aggregate vector cache removable byte count overflow")
            })?;
            if reserved_or_stored_bytes.saturating_sub(removable_bytes)
                > max_aggregate_database_bytes
            {
                bail!(
                    "aggregate vector cache limit cannot reserve the current cache while preserving active and unowned bytes"
                );
            }
            for entry in inactive {
                if reserved_or_stored_bytes <= max_aggregate_database_bytes {
                    break;
                }
                reserved_or_stored_bytes = reserved_or_stored_bytes.saturating_sub(entry.bytes);
                let lock_path = self
                    .scope_lock_path(&entry.registered.root_name, &entry.registered.scope_name)?;
                self.remove_registered_scope(&entry.registered, lock_path, entry.lock)?;
            }
            Ok(())
        })()
    }

    fn registered_scopes(&self) -> Result<Vec<RegisteredVectorCacheScope>> {
        let mut statement = self.registry.prepare(
            "SELECT root_name, scope_name, cache_schema_version,
                    last_access_sequence, max_database_bytes, ownership_token
             FROM owned_vector_cache_scope",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (
                root_name,
                scope_name,
                cache_schema_version,
                last_access_sequence,
                max_bytes,
                ownership_token,
            ) = row?;
            Ok(RegisteredVectorCacheScope {
                root_name,
                scope_name,
                cache_schema_version,
                last_access_sequence,
                max_database_bytes: u64::try_from(max_bytes)
                    .context("registered vector cache reservation is invalid")?,
                ownership_token,
            })
        })
        .collect()
    }

    fn remove_registered_scope(
        &self,
        registered: &RegisteredVectorCacheScope,
        lock_path: PathBuf,
        lock: File,
    ) -> Result<()> {
        remove_owned_scope(
            &self.cache_root,
            &registered.root_name,
            &registered.scope_name,
            &registered.ownership_token,
        )?;
        bounded_locks::release(&lock)
            .map_err(anyhow::Error::new)
            .context("release evicted vector cache scope lock")?;
        drop(lock);
        match std::fs::remove_file(&lock_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("remove evicted vector cache scope lock"),
        }
        self.registry.execute(
            "DELETE FROM owned_vector_cache_scope
             WHERE root_name = ?1 AND scope_name = ?2",
            params![registered.root_name, registered.scope_name],
        )?;
        Ok(())
    }

    fn open_scope_lock(&self, root_name: &str, scope_name: &str) -> Result<(PathBuf, File)> {
        let path = self.scope_lock_path(root_name, scope_name)?;
        Ok((path.clone(), open_lock_file(&path)?))
    }

    fn scope_lock_path(&self, root_name: &str, scope_name: &str) -> Result<PathBuf> {
        validate_scope_components(root_name, scope_name)?;
        let mut digest = Sha256::new();
        digest.update(b"codestory-vector-cache-scope-lock-v1\0");
        digest.update(root_name.as_bytes());
        digest.update([0]);
        digest.update(scope_name.as_bytes());
        Ok(self
            .retention_root
            .join(CACHE_RETENTION_SCOPE_LOCKS)
            .join(format!("{}.lock", hex_digest(digest.finalize()))))
    }
}

fn validate_registered_scope(
    root_name: &str,
    scope_name: &str,
    cache_schema_version: i64,
) -> Result<()> {
    validate_scope_components(root_name, scope_name)?;
    let Some(version) = root_name.strip_prefix("content-addressed-vectors-v") else {
        bail!("registered vector cache root is invalid");
    };
    if version.parse::<i64>().ok() != Some(cache_schema_version) || cache_schema_version <= 0 {
        bail!("registered vector cache scope is invalid");
    }
    Ok(())
}

fn validate_scope_components(root_name: &str, scope_name: &str) -> Result<()> {
    let Some(version) = root_name.strip_prefix("content-addressed-vectors-v") else {
        bail!("registered vector cache root is invalid");
    };
    if version.is_empty()
        || !version.bytes().all(|byte| byte.is_ascii_digit())
        || scope_name.len() != 64
        || !scope_name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("registered vector cache scope is invalid");
    }
    Ok(())
}

fn open_lock_file(path: &Path) -> Result<File> {
    reject_non_regular_file(path, "vector cache lock")?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("open vector cache lock {}", path.display()))
}

fn reject_non_regular_file(path: &Path, label: &str) -> Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink() || !metadata.file_type().is_file())
    {
        bail!("content-addressed {label} is not a regular file");
    }
    Ok(())
}

fn known_scope_bytes(path: &Path) -> Result<Option<u64>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Some(0)),
        Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(None);
    }
    let mut total = 0_u64;
    for entry in std::fs::read_dir(path)
        .with_context(|| format!("enumerate vector cache scope {}", path.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Ok(None);
        };
        if !matches!(
            name,
            "vectors.sqlite3"
                | "vectors.sqlite3-wal"
                | "vectors.sqlite3-shm"
                | "vectors.sqlite3-journal"
                | CACHE_SCOPE_OWNERSHIP_MARKER
        ) {
            return Ok(None);
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() || !file_type.is_file() {
            return Ok(None);
        }
        let metadata = entry.metadata()?;
        total = total
            .checked_add(metadata.len())
            .context("vector cache scope byte count overflow")?;
    }
    Ok(Some(total))
}

fn unregistered_vector_cache_bytes(
    cache_root: &Path,
    registered: &HashSet<(String, String)>,
) -> Result<u64> {
    let entries = match std::fs::read_dir(cache_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("enumerate vector cache root {}", cache_root.display()));
        }
    };
    let mut total = 0_u64;
    for root_entry in entries {
        let root_entry = root_entry?;
        let Some(root_name) = root_entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(version) = root_name.strip_prefix("content-addressed-vectors-v") else {
            continue;
        };
        if version.is_empty() || !version.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let root_type = root_entry.file_type()?;
        if root_type.is_symlink() || !root_type.is_dir() {
            bail!("unregistered vector cache root contains unknown or unsafe artifacts");
        }
        for scope_entry in std::fs::read_dir(root_entry.path()).with_context(|| {
            format!(
                "enumerate unregistered vector cache scopes {}",
                root_entry.path().display()
            )
        })? {
            let scope_entry = scope_entry?;
            let scope_name = scope_entry.file_name().to_string_lossy().into_owned();
            if registered.contains(&(root_name.clone(), scope_name)) {
                continue;
            }
            total = total
                .checked_add(unregistered_vector_cache_entry_bytes(&scope_entry.path())?)
                .context("aggregate unregistered vector cache byte count overflow")?;
        }
    }
    Ok(total)
}

fn unregistered_vector_cache_entry_bytes(path: &Path) -> Result<u64> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspect unregistered vector cache entry {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("unregistered vector cache scope contains unknown or unsafe artifacts");
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        bail!("unregistered vector cache scope contains unknown or unsafe artifacts");
    }
    let mut total = 0_u64;
    for entry in std::fs::read_dir(path).with_context(|| {
        format!(
            "enumerate unregistered vector cache scope {}",
            path.display()
        )
    })? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() || !file_type.is_file() {
            bail!("unregistered vector cache scope contains unknown or unsafe artifacts");
        }
        total = total
            .checked_add(entry.metadata()?.len())
            .context("unregistered vector cache scope byte count overflow")?;
    }
    Ok(total)
}

fn scope_has_database(scope: &Path) -> Result<bool> {
    let path = scope.join("vectors.sqlite3");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => Ok(!metadata.file_type().is_symlink() && metadata.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("inspect vector cache database identity"),
    }
}

fn read_scope_ownership_token(scope: &Path) -> Result<Option<String>> {
    let path = scope.join(CACHE_SCOPE_OWNERSHIP_MARKER);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect vector cache ownership token"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != 36 {
        bail!("content-addressed vector cache ownership token is invalid");
    }
    let token = std::fs::read_to_string(&path).context("read vector cache ownership token")?;
    Uuid::parse_str(&token).context("parse vector cache ownership token")?;
    Ok(Some(token))
}

fn write_scope_ownership_token(scope: &Path, ownership_token: &str) -> Result<()> {
    Uuid::parse_str(ownership_token).context("validate vector cache ownership token")?;
    let path = scope.join(CACHE_SCOPE_OWNERSHIP_MARKER);
    if path.exists() {
        bail!("content-addressed vector cache ownership token already exists");
    }
    codestory_workspace::atomic_file::publish_new_private_file_atomic(
        &path,
        "codestory-vector-cache-ownership",
        ownership_token.as_bytes(),
    )
    .map_err(anyhow::Error::new)
    .with_context(|| format!("publish vector cache ownership token {}", path.display()))
}

fn remove_owned_scope(
    cache_root: &Path,
    root_name: &str,
    scope_name: &str,
    ownership_token: &str,
) -> Result<()> {
    validate_scope_components(root_name, scope_name)?;
    let root = cache_root.join(root_name);
    let scope = root.join(scope_name);
    let Some(bytes) = known_scope_bytes(&scope)? else {
        bail!("refuse to remove vector cache scope containing unknown artifacts");
    };
    if !scope.exists() {
        return Ok(());
    }
    let observed_token = read_scope_ownership_token(&scope)?;
    if observed_token.as_deref() != Some(ownership_token)
        && !(observed_token.is_none() && bytes == 0)
    {
        bail!("refuse to remove vector cache scope with mismatched ownership token");
    }
    private_cache_directory(&root).context("verify owned vector cache root")?;
    let deletion = OwnedDeletionRoot::open(&root).context("pin owned vector cache root")?;
    for name in [
        "vectors.sqlite3-journal",
        "vectors.sqlite3-wal",
        "vectors.sqlite3-shm",
        "vectors.sqlite3",
    ] {
        deletion
            .remove(&PathBuf::from(scope_name).join(name))
            .with_context(|| format!("remove owned vector cache file {scope_name}/{name}"))?;
    }
    deletion
        .remove(&PathBuf::from(scope_name).join(CACHE_SCOPE_OWNERSHIP_MARKER))
        .context("remove owned vector cache ownership token")?;
    match deletion.remove_empty_directory(Path::new(scope_name)) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("remove empty owned vector cache scope"),
    }
}

#[cfg(test)]
fn reassign_owned_scope_root_for_test(
    runtime: &SidecarRuntimeConfig,
    artifact_scope_id: &str,
    destination_root_name: &str,
    destination_schema_version: i64,
) -> Result<PathBuf> {
    let scope_name = hex_digest(Sha256::digest(artifact_scope_id.as_bytes()));
    validate_registered_scope(
        destination_root_name,
        &scope_name,
        destination_schema_version,
    )?;
    let retention = VectorCacheRetention::open(runtime)?;
    let (source_lock_path, source_lock) =
        retention.open_scope_lock(CACHE_DIRECTORY, &scope_name)?;
    if !bounded_locks::try_acquire(&source_lock, FileLockKind::Exclusive)
        .map_err(anyhow::Error::new)?
    {
        bail!("test vector cache scope is still active");
    }
    let source = runtime.cache_root.join(CACHE_DIRECTORY).join(&scope_name);
    let destination_root = runtime.cache_root.join(destination_root_name);
    private_cache_directory(&destination_root)?;
    let destination = destination_root.join(&scope_name);
    std::fs::rename(&source, &destination).with_context(|| {
        format!(
            "move registered vector cache scope {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    let changed = retention.registry.execute(
        "UPDATE owned_vector_cache_scope
         SET root_name = ?1, cache_schema_version = ?2
         WHERE root_name = ?3 AND scope_name = ?4",
        params![
            destination_root_name,
            destination_schema_version,
            CACHE_DIRECTORY,
            scope_name,
        ],
    )?;
    if changed != 1 {
        bail!("test vector cache ownership row is missing");
    }
    bounded_locks::release(&source_lock).map_err(anyhow::Error::new)?;
    drop(source_lock);
    match std::fs::remove_file(source_lock_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(destination)
}

#[cfg(test)]
fn registered_owned_cache_bytes_for_test(runtime: &SidecarRuntimeConfig) -> Result<u64> {
    let retention = VectorCacheRetention::open(runtime)?;
    retention
        .registered_scopes()?
        .into_iter()
        .try_fold(0_u64, |total, registered| {
            let path = runtime
                .cache_root
                .join(&registered.root_name)
                .join(&registered.scope_name);
            let bytes = known_scope_bytes(&path)?
                .context("registered test vector cache contains unknown artifacts")?;
            let (_, lock) =
                retention.open_scope_lock(&registered.root_name, &registered.scope_name)?;
            let contribution = if bounded_locks::try_acquire(&lock, FileLockKind::Exclusive)
                .map_err(anyhow::Error::new)?
            {
                bytes
            } else {
                registered.max_database_bytes
            };
            total
                .checked_add(contribution)
                .context("registered test vector cache byte count overflow")
        })
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
    expected_contract_sha256: &str,
) -> Result<Vec<Vec<f32>>> {
    if row.contract_sha256 != expected_contract_sha256
        || row.vector_count != i64::try_from(expected_count).context("vector count overflow")?
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

fn insert_vector_batch(
    transaction: &Transaction<'_>,
    cache_key: &str,
    contract_sha256: &str,
    vectors: &[Vec<f32>],
    encoded: EncodedVectors,
    embedding_dim: usize,
    max_payload_bytes: u64,
) -> Result<()> {
    reserve_payload(
        transaction,
        payload_weight(encoded.bytes.len())?,
        max_payload_bytes,
    )?;
    let access_sequence = next_access_sequence(transaction)?;
    transaction.execute(
        "INSERT INTO vector_batch_cache
         (cache_key, contract_sha256, vector_count, embedding_dim, vectors,
          vectors_sha256, last_access_sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            cache_key,
            contract_sha256,
            i64::try_from(vectors.len()).context("vector count overflow")?,
            i64::try_from(embedding_dim).context("embedding dimension overflow")?,
            encoded.bytes,
            encoded.sha256,
            access_sequence,
        ],
    )?;
    Ok(())
}

fn insert_corpus_plan(
    transaction: &Transaction<'_>,
    corpus_key: &str,
    contract_sha256: &str,
    current_plan: &[&str],
    max_payload_bytes: u64,
) -> Result<()> {
    let encoded = encode_identity_plan(current_plan);
    reserve_payload(
        transaction,
        payload_weight(encoded.bytes.len())?,
        max_payload_bytes,
    )?;
    let access_sequence = next_access_sequence(transaction)?;
    transaction.execute(
        "INSERT INTO vector_corpus_plan
         (corpus_key, contract_sha256, anchor_count, ordered_anchor_identities,
          plan_sha256, last_access_sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            corpus_key,
            contract_sha256,
            i64::try_from(current_plan.len()).context("corpus anchor count overflow")?,
            encoded.bytes,
            encoded.sha256,
            access_sequence,
        ],
    )?;
    Ok(())
}

fn touch_vector_batch(transaction: &Transaction<'_>, cache_key: &str) -> Result<()> {
    let access_sequence = next_access_sequence(transaction)?;
    transaction.execute(
        "UPDATE vector_batch_cache SET last_access_sequence = ?2 WHERE cache_key = ?1",
        params![cache_key, access_sequence],
    )?;
    Ok(())
}

fn touch_corpus_plan(transaction: &Transaction<'_>, corpus_key: &str) -> Result<()> {
    let access_sequence = next_access_sequence(transaction)?;
    transaction.execute(
        "UPDATE vector_corpus_plan SET last_access_sequence = ?2 WHERE corpus_key = ?1",
        params![corpus_key, access_sequence],
    )?;
    Ok(())
}

fn next_access_sequence(transaction: &Transaction<'_>) -> Result<i64> {
    let current = transaction.query_row(
        "SELECT access_sequence FROM cache_metadata WHERE singleton = 1",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let next = current
        .checked_add(1)
        .context("content-addressed vector cache access sequence exhausted")?;
    transaction.execute(
        "UPDATE cache_metadata SET access_sequence = ?1 WHERE singleton = 1",
        params![next],
    )?;
    Ok(next)
}

fn reserve_payload(
    transaction: &Transaction<'_>,
    incoming_weight: u64,
    max_payload_bytes: u64,
) -> Result<()> {
    if incoming_weight > max_payload_bytes {
        bail!("content-addressed vector cache row exceeds the project payload limit");
    }
    let mut retained = accounted_payload_bytes(transaction)?;
    while retained.saturating_add(incoming_weight) > max_payload_bytes {
        let oldest = transaction
            .query_row(
                "SELECT entry_kind, entry_key, payload_bytes FROM (
                     SELECT 0 AS entry_kind, cache_key AS entry_key,
                            length(vectors) + ?1 AS payload_bytes,
                            last_access_sequence
                     FROM vector_batch_cache
                     UNION ALL
                     SELECT 1 AS entry_kind, corpus_key AS entry_key,
                            length(ordered_anchor_identities) + ?1 AS payload_bytes,
                            last_access_sequence
                     FROM vector_corpus_plan
                 )
                 ORDER BY last_access_sequence, entry_kind, entry_key
                 LIMIT 1",
                params![
                    i64::try_from(CACHE_ROW_ACCOUNTING_BYTES)
                        .context("cache row accounting overflow")?
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
            .context("content-addressed vector cache cannot free its accounted payload")?;
        match oldest.0 {
            0 => transaction.execute(
                "DELETE FROM vector_batch_cache WHERE cache_key = ?1",
                params![oldest.1],
            )?,
            1 => transaction.execute(
                "DELETE FROM vector_corpus_plan WHERE corpus_key = ?1",
                params![oldest.1],
            )?,
            _ => bail!("content-addressed vector cache selected an unknown retention row"),
        };
        retained = retained.saturating_sub(
            u64::try_from(oldest.2).context("cache retention row has a negative payload")?,
        );
    }
    Ok(())
}

fn payload_weight(blob_len: usize) -> Result<u64> {
    u64::try_from(blob_len)
        .context("content-addressed vector cache payload length overflow")?
        .checked_add(CACHE_ROW_ACCOUNTING_BYTES)
        .context("content-addressed vector cache payload weight overflow")
}

fn accounted_payload_bytes(connection: &Connection) -> Result<u64> {
    let payload = connection
        .query_row(
            "SELECT
                 COALESCE((SELECT SUM(length(vectors) + ?1) FROM vector_batch_cache), 0)
                 + COALESCE((SELECT SUM(length(ordered_anchor_identities) + ?1)
                             FROM vector_corpus_plan), 0)",
            params![
                i64::try_from(CACHE_ROW_ACCOUNTING_BYTES)
                    .context("cache row accounting overflow")?
            ],
            |row| row.get::<_, i64>(0),
        )
        .map_err(anyhow::Error::from)?;
    u64::try_from(payload).context("cache accounted payload is negative")
}

fn read_cached_batch_from_transaction(
    transaction: &Transaction<'_>,
    cache_key: &str,
) -> Result<Option<CachedVectorRow>> {
    transaction
        .query_row(
            "SELECT contract_sha256, vector_count, embedding_dim, vectors, vectors_sha256
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
            "SELECT contract_sha256, anchor_count, ordered_anchor_identities, plan_sha256
             FROM vector_corpus_plan WHERE corpus_key = ?1",
            params![corpus_key],
            |row| {
                Ok(CachedIdentityPlan {
                    contract_sha256: row.get(0)?,
                    anchor_count: row.get(1)?,
                    bytes: row.get(2)?,
                    sha256: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn cached_vector_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CachedVectorRow> {
    Ok(CachedVectorRow {
        contract_sha256: row.get(0)?,
        vector_count: row.get(1)?,
        embedding_dim: row.get(2)?,
        bytes: row.get(3)?,
        sha256: row.get(4)?,
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

    #[test]
    fn retention_keeps_recent_exact_batches_and_evicts_the_lru_batch() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let row_weight = CACHE_ROW_ACCOUNTING_BYTES + 8;
        let mut owner = ContentAddressedVectorCache::open_with_limits(
            &selected_runtime,
            "scope-a",
            "producer-a",
            2,
            row_weight * 2,
            1024 * 1024,
        )
        .expect("open bounded cache");
        let a_rows = [("a", "doc-a", "a")];
        let b_rows = [("b", "doc-b", "b")];
        let c_rows = [("c", "doc-c", "c")];
        let a = inputs(&a_rows);
        let b = inputs(&b_rows);
        let c = inputs(&c_rows);
        owner
            .publish_batch(&a, &[vec![1.0, 0.0]])
            .expect("publish a");
        owner
            .publish_batch(&b, &[vec![0.0, 1.0]])
            .expect("publish b");
        assert!(owner.load_batch(&a).expect("touch a").is_some());
        owner
            .publish_batch(&c, &[vec![-1.0, 0.0]])
            .expect("publish c");

        assert!(owner.load_batch(&a).expect("retained a").is_some());
        assert!(owner.load_batch(&b).expect("evicted b").is_none());
        assert!(owner.load_batch(&c).expect("retained c").is_some());
        assert!(owner.accounted_payload_bytes().expect("payload") <= row_weight * 2);
    }

    #[test]
    fn one_project_budget_covers_all_embedding_contracts() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let row_weight = CACHE_ROW_ACCOUNTING_BYTES + 8;
        let open = |producer: &str| {
            ContentAddressedVectorCache::open_with_limits(
                &selected_runtime,
                "scope-a",
                producer,
                2,
                row_weight * 2,
                1024 * 1024,
            )
            .expect("open bounded cache")
        };
        let a_rows = [("a", "doc-a", "a")];
        let b_rows = [("b", "doc-b", "b")];
        let c_rows = [("c", "doc-c", "c")];
        let mut first = open("producer-a");
        first
            .publish_batch(&inputs(&a_rows), &[vec![1.0, 0.0]])
            .expect("publish first contract");
        first
            .publish_batch(&inputs(&b_rows), &[vec![0.0, 1.0]])
            .expect("publish second first-contract row");
        drop(first);
        let mut second = open("producer-b");
        second
            .publish_batch(&inputs(&c_rows), &[vec![-1.0, 0.0]])
            .expect("publish second contract");

        assert!(second.accounted_payload_bytes().expect("payload") <= row_weight * 2);
        assert_eq!(second.retained_batch_count().expect("row count"), 2);
    }

    #[test]
    fn corpus_order_plans_share_the_same_project_payload_budget() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let one_plan_weight = CACHE_ROW_ACCOUNTING_BYTES + 8 + "stable-a".len() as u64;
        let mut owner = ContentAddressedVectorCache::open_with_limits(
            &selected_runtime,
            "scope-a",
            "producer-a",
            2,
            one_plan_weight,
            1024 * 1024,
        )
        .expect("open bounded cache");
        let first_rows = [("stable-a", "doc-a", "a")];
        let second_rows = [("stable-b", "doc-b", "b")];
        owner
            .canonical_order(&inputs(&first_rows))
            .expect("first plan");
        owner
            .canonical_order(&inputs(&second_rows))
            .expect("second plan");

        assert_eq!(owner.retained_plan_count().expect("plan count"), 1);
        assert!(owner.accounted_payload_bytes().expect("payload") <= one_plan_weight);
    }

    #[test]
    fn database_file_stays_hard_bounded_across_batch_churn() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let row_weight = CACHE_ROW_ACCOUNTING_BYTES + 8;
        let database_limit = 1024 * 1024;
        let mut owner = ContentAddressedVectorCache::open_with_limits(
            &selected_runtime,
            "scope-a",
            "producer-a",
            2,
            row_weight * 4,
            database_limit,
        )
        .expect("open bounded cache");
        for index in 0..200 {
            let identity = format!("node-{index}");
            let document_hash = format!("doc-{index}");
            let text = format!("text-{index}");
            let rows = [(identity.as_str(), document_hash.as_str(), text.as_str())];
            owner
                .publish_batch(&inputs(&rows), &[vec![1.0, 0.0]])
                .expect("publish churn row");
        }

        assert!(owner.accounted_payload_bytes().expect("payload") <= row_weight * 4);
        assert_eq!(owner.retained_batch_count().expect("row count"), 4);
        assert!(owner.database_bytes().expect("database bytes") <= database_limit);
    }

    #[test]
    fn aggregate_retention_evicts_owned_obsolete_and_project_scopes() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let database_limit = 1024 * 1024;
        let aggregate_limit = database_limit + 8 * 1024;

        let obsolete = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "obsolete-scope",
            "producer-a",
            2,
            64 * 1024,
            database_limit,
            aggregate_limit,
        )
        .expect("open obsolete scope");
        drop(obsolete);
        let obsolete_scope = reassign_owned_scope_root_for_test(
            &selected_runtime,
            "obsolete-scope",
            "content-addressed-vectors-v4",
            4,
        )
        .expect("move registered scope to obsolete root");

        let previous = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "previous-scope",
            "producer-a",
            2,
            64 * 1024,
            database_limit,
            aggregate_limit,
        )
        .expect("open previous scope");
        let previous_scope = previous.path.parent().expect("scope path").to_path_buf();
        drop(previous);

        let current = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "current-scope",
            "producer-a",
            2,
            64 * 1024,
            database_limit,
            aggregate_limit,
        )
        .expect("open current scope");
        assert!(current.path.exists());
        assert!(
            !obsolete_scope.exists(),
            "obsolete registered cache survived"
        );
        assert!(
            !previous_scope.exists(),
            "oldest inactive project cache survived"
        );
        assert!(
            registered_owned_cache_bytes_for_test(&selected_runtime).expect("aggregate bytes")
                <= aggregate_limit
        );
    }

    #[test]
    fn aggregate_retention_preserves_active_readers_and_refuses_overcommit() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let database_limit = 1024 * 1024;
        let aggregate_limit = database_limit * 2;
        let rows = [("a", "doc-a", "a")];
        let batch = inputs(&rows);

        let mut active = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "active-scope",
            "producer-a",
            2,
            64 * 1024,
            database_limit,
            aggregate_limit,
        )
        .expect("open active scope");
        active
            .publish_batch(&batch, &[vec![1.0, 0.0]])
            .expect("publish active row");

        let second = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "second-scope",
            "producer-a",
            2,
            64 * 1024,
            database_limit,
            aggregate_limit,
        )
        .expect("open second active scope");
        assert!(active.load_batch(&batch).expect("active read").is_some());

        let error = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "third-scope",
            "producer-a",
            2,
            64 * 1024,
            database_limit,
            aggregate_limit,
        )
        .err()
        .expect("third reservation must be refused");
        assert!(error.to_string().contains("aggregate vector cache limit"));
        assert!(
            active
                .load_batch(&batch)
                .expect("active survives")
                .is_some()
        );
        drop(second);
    }

    #[test]
    fn aggregate_retention_retries_partial_owned_removal_idempotently() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let database_limit = 1024 * 1024;
        let aggregate_limit = database_limit + 16 * 1024;
        let stale = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "stale-scope",
            "producer-a",
            2,
            64 * 1024,
            database_limit,
            aggregate_limit,
        )
        .expect("open stale scope");
        let stale_scope = stale.path.parent().expect("scope path").to_path_buf();
        drop(stale);
        std::fs::remove_file(stale_scope.join("vectors.sqlite3"))
            .expect("simulate interrupted owned removal");

        let current = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "current-scope",
            "producer-a",
            2,
            64 * 1024,
            database_limit,
            aggregate_limit,
        )
        .expect("retry retention");
        assert!(!stale_scope.exists());
        assert!(current.path.exists());
        drop(current);

        ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "current-scope",
            "producer-a",
            2,
            64 * 1024,
            database_limit,
            aggregate_limit,
        )
        .expect("idempotent retry");
    }

    #[test]
    fn aggregate_retention_refuses_oversized_unregistered_scope_without_deleting_it() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let cache_root = selected_runtime.cache_root.join(CACHE_DIRECTORY);
        private_cache_directory(&cache_root).expect("cache root");
        let unknown = cache_root.join("0".repeat(64));
        private_cache_directory(&unknown).expect("unknown scope");
        let unknown_bytes = vec![0x5a; 32 * 1024];
        std::fs::write(unknown.join("sentinel"), &unknown_bytes).expect("unknown sentinel");

        let error = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "current-scope",
            "producer-a",
            2,
            64 * 1024,
            1024 * 1024,
            1024 * 1024 + 16 * 1024,
        )
        .err()
        .expect("oversized unknown scope must refuse aggregate admission");
        assert!(error.to_string().contains("aggregate vector cache limit"));
        assert_eq!(
            std::fs::read(unknown.join("sentinel")).expect("unknown survives"),
            unknown_bytes
        );
    }

    #[test]
    fn aggregate_retention_accounts_for_bounded_unregistered_scope_without_deleting_it() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let cache_root = selected_runtime.cache_root.join(CACHE_DIRECTORY);
        private_cache_directory(&cache_root).expect("cache root");
        let unknown = cache_root.join("0".repeat(64));
        private_cache_directory(&unknown).expect("unknown scope");
        let unknown_bytes = vec![0x5a; 8 * 1024];
        std::fs::write(unknown.join("sentinel"), &unknown_bytes).expect("unknown sentinel");

        ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "current-scope",
            "producer-a",
            2,
            64 * 1024,
            1024 * 1024,
            1024 * 1024 + 16 * 1024,
        )
        .expect("bounded unknown scope fits aggregate admission");
        assert_eq!(
            std::fs::read(unknown.join("sentinel")).expect("unknown survives"),
            unknown_bytes
        );
    }

    #[test]
    fn aggregate_retention_refuses_to_claim_or_delete_unknown_registered_contents() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let owned = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "owned-scope",
            "producer-a",
            2,
            64 * 1024,
            1024 * 1024,
            1024 * 1024 + 16 * 1024,
        )
        .expect("open owned scope");
        let owned_scope = owned.path.parent().expect("owned scope").to_path_buf();
        drop(owned);
        std::fs::write(owned_scope.join("future-format"), b"unknown")
            .expect("inject unknown owned content");

        let error = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "current-scope",
            "producer-a",
            2,
            64 * 1024,
            1024 * 1024,
            1024 * 1024 + 16 * 1024,
        )
        .err()
        .expect("unknown registered contents must refuse retention");
        assert!(error.to_string().contains("unknown or unsafe artifacts"));
        assert_eq!(
            std::fs::read(owned_scope.join("future-format")).expect("unknown content survives"),
            b"unknown"
        );
    }

    #[test]
    fn failed_retention_setup_does_not_create_an_unregistered_cache_scope() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        std::fs::write(
            selected_runtime.cache_root.join(CACHE_RETENTION_DIRECTORY),
            b"not a retention directory",
        )
        .expect("block retention setup");
        let scope_name = hex_digest(Sha256::digest(b"never-created"));

        ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "never-created",
            "producer-a",
            2,
            64 * 1024,
            1024 * 1024,
            1024 * 1024 + 16 * 1024,
        )
        .err()
        .expect("retention setup must fail before cache mutation");
        assert!(
            !selected_runtime
                .cache_root
                .join(CACHE_DIRECTORY)
                .join(scope_name)
                .exists()
        );
    }

    #[test]
    fn requested_scope_collision_is_refused_before_database_creation() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let root = selected_runtime.cache_root.join(CACHE_DIRECTORY);
        private_cache_directory(&root).expect("cache root");
        let scope = root.join(hex_digest(Sha256::digest(b"colliding-scope")));
        private_cache_directory(&scope).expect("colliding scope");
        std::fs::write(scope.join("sentinel"), b"unowned").expect("collision sentinel");

        ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "colliding-scope",
            "producer-a",
            2,
            64 * 1024,
            1024 * 1024,
            1024 * 1024 + 16 * 1024,
        )
        .err()
        .expect("colliding unknown scope must be refused");
        assert!(!scope.join("vectors.sqlite3").exists());
        assert_eq!(
            std::fs::read(scope.join("sentinel")).expect("collision survives"),
            b"unowned"
        );
    }

    #[test]
    fn stale_registration_cannot_delete_replacement_database() {
        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let stale = ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "stale-scope",
            "producer-a",
            2,
            64 * 1024,
            1024 * 1024,
            1024 * 1024 + 16 * 1024,
        )
        .expect("open stale scope");
        let stale_scope = stale.path.parent().expect("stale scope").to_path_buf();
        drop(stale);
        std::fs::remove_dir_all(&stale_scope)
            .expect("simulate completed deletion before row commit");
        private_cache_directory(&stale_scope).expect("install replacement scope");
        std::fs::write(stale_scope.join("vectors.sqlite3"), b"replacement")
            .expect("install replacement database");

        ContentAddressedVectorCache::open_with_retention_limits(
            &selected_runtime,
            "current-scope",
            "producer-a",
            2,
            64 * 1024,
            1024 * 1024,
            1024 * 1024 + 16 * 1024,
        )
        .err()
        .expect("stale ownership token must not authorize replacement deletion");
        assert_eq!(
            std::fs::read(stale_scope.join("vectors.sqlite3"))
                .expect("replacement database survives"),
            b"replacement"
        );
    }

    #[test]
    fn concurrent_publishers_share_one_integrity_checked_canonical_row() {
        use std::sync::{Arc, Barrier};

        let cache = TempDir::new().expect("cache root");
        let selected_runtime = runtime(&cache, 128);
        let barrier = Arc::new(Barrier::new(2));
        let handles = [vec![1.0, 0.0], vec![0.0, 1.0]].map(|vector| {
            let runtime = selected_runtime.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let rows = [("same", "same-doc", "same text")];
                let batch = inputs(&rows);
                let mut owner =
                    ContentAddressedVectorCache::open(&runtime, "scope-a", "producer-a", 2)
                        .expect("open concurrent cache");
                barrier.wait();
                owner
                    .publish_batch(&batch, &[vector])
                    .expect("publish concurrent row")
            })
        });
        let [left, right] = handles.map(|handle| handle.join().expect("join publisher"));
        assert_eq!(left, right);
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
        assert!(error.to_string().contains("unknown or unsafe artifacts"));
    }
}
