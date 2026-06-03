use codestory_store::RetrievalIndexManifest;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Cache key ties results to a published retrieval manifest generation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RetrievalCacheKey {
    pub project_id: String,
    pub zoekt_version: String,
    pub qdrant_collection: String,
    pub scip_revision: Option<String>,
    pub sidecar_generation: Option<String>,
    pub sidecar_input_hash: Option<String>,
    pub sidecar_schema_version: Option<i32>,
    pub projection_count: Option<i64>,
    pub query_fingerprint: String,
}

impl RetrievalCacheKey {
    pub fn from_manifest(
        manifest: &RetrievalIndexManifest,
        query_fingerprint: impl Into<String>,
    ) -> Self {
        Self {
            project_id: manifest.project_id.clone(),
            zoekt_version: manifest.zoekt_version.clone(),
            qdrant_collection: manifest.qdrant_collection.clone(),
            scip_revision: manifest.scip_revision.clone(),
            sidecar_generation: manifest.sidecar_generation.clone(),
            sidecar_input_hash: manifest.sidecar_input_hash.clone(),
            sidecar_schema_version: manifest.sidecar_schema_version,
            projection_count: manifest.projection_count,
            query_fingerprint: query_fingerprint.into(),
        }
    }
}

const DEFAULT_RETRIEVAL_CACHE_CAPACITY: usize = 128;

/// In-memory version-keyed query result cache.
#[derive(Debug)]
pub struct RetrievalCache {
    entries: HashMap<RetrievalCacheKey, Vec<super::CandidateHit>>,
    order: VecDeque<RetrievalCacheKey>,
    capacity: usize,
}

impl Default for RetrievalCache {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_RETRIEVAL_CACHE_CAPACITY)
    }
}

impl RetrievalCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    pub fn get(&self, key: &RetrievalCacheKey) -> Option<&[super::CandidateHit]> {
        self.entries.get(key).map(Vec::as_slice)
    }

    pub fn insert(&mut self, key: RetrievalCacheKey, hits: Vec<super::CandidateHit>) {
        if self.entries.contains_key(&key) {
            self.order.retain(|existing| existing != &key);
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, hits);
        while self.entries.len() > self.capacity {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&evicted);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trip() {
        let mut cache = RetrievalCache::new();
        let key = RetrievalCacheKey {
            project_id: "abc".into(),
            zoekt_version: "v1".into(),
            qdrant_collection: "codestory_abc".into(),
            scip_revision: None,
            sidecar_generation: Some("abc-hash".into()),
            sidecar_input_hash: Some("hash".into()),
            sidecar_schema_version: Some(1),
            projection_count: Some(1),
            query_fingerprint: "fp".into(),
        };
        cache.insert(
            key.clone(),
            vec![super::super::CandidateHit::lexical_stub("src/lib.rs", 1.0)],
        );
        assert_eq!(cache.get(&key).expect("hit").len(), 1);
    }

    #[test]
    fn cache_evicts_oldest_entry_when_capacity_is_reached() {
        let mut cache = RetrievalCache::with_capacity(1);
        let first = RetrievalCacheKey {
            project_id: "abc".into(),
            zoekt_version: "v1".into(),
            qdrant_collection: "codestory_abc".into(),
            scip_revision: None,
            sidecar_generation: Some("abc-hash".into()),
            sidecar_input_hash: Some("hash".into()),
            sidecar_schema_version: Some(1),
            projection_count: Some(1),
            query_fingerprint: "first".into(),
        };
        let second = RetrievalCacheKey {
            query_fingerprint: "second".into(),
            ..first.clone()
        };
        cache.insert(
            first.clone(),
            vec![super::super::CandidateHit::lexical_stub(
                "src/first.rs",
                1.0,
            )],
        );
        cache.insert(
            second.clone(),
            vec![super::super::CandidateHit::lexical_stub(
                "src/second.rs",
                1.0,
            )],
        );

        assert!(cache.get(&first).is_none());
        assert_eq!(
            cache.get(&second).expect("second entry should remain")[0].file_path,
            "src/second.rs"
        );
    }

    #[test]
    fn cache_key_tracks_sidecar_generation_contract() {
        let base = RetrievalIndexManifest {
            project_id: "abc".into(),
            zoekt_version: "v1".into(),
            qdrant_collection: "codestory_abc_hash_a".into(),
            scip_revision: Some("scip-a".into()),
            built_at_epoch_ms: 0,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: Some("llamacpp:bge-base".into()),
            embedding_dim: Some(768),
            sidecar_schema_version: Some(1),
            sidecar_input_hash: Some("hash-a".into()),
            sidecar_generation: Some("abc-hash-a".into()),
            projection_count: Some(10),
        };
        let mut changed = base.clone();
        changed.qdrant_collection = "codestory_abc_hash_b".into();
        changed.sidecar_input_hash = Some("hash-b".into());
        changed.sidecar_generation = Some("abc-hash-b".into());

        assert_ne!(
            RetrievalCacheKey::from_manifest(&base, "query"),
            RetrievalCacheKey::from_manifest(&changed, "query")
        );
    }
}
