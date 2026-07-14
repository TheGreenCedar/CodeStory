use codestory_store::RetrievalIndexManifest;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Cache key ties results to a published retrieval manifest generation.
///
/// This is the boundary that prevents lexical/vector evidence from one sidecar build from being
/// reused after the manifest identity changes. The query fingerprint is not enough on its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RetrievalCacheKey {
    pub core_generation_id: Option<String>,
    pub core_run_id: Option<String>,
    pub project_id: String,
    pub lexical_version: String,
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
            core_generation_id: None,
            core_run_id: None,
            project_id: manifest.project_id.clone(),
            lexical_version: manifest.lexical_version.clone(),
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
///
/// Entries are safe only for the manifest identity captured by [`RetrievalCacheKey`]. Cache
/// rehydration across worktrees must invalidate copied retrieval manifests before this cache is
/// trusted again.
#[derive(Debug, Clone)]
pub struct RetrievalCache {
    entries: HashMap<RetrievalCacheKey, Vec<super::CandidateHit>>,
    order: VecDeque<RetrievalCacheKey>,
    capacity: usize,
    publication_identity: Option<(String, String)>,
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
            publication_identity: None,
        }
    }

    pub fn scope_to_publication(&mut self, identity: &super::RetrievalPublicationIdentity) {
        let publication = (
            identity.core_generation_id.clone(),
            identity.core_run_id.clone(),
        );
        if self.publication_identity.as_ref() != Some(&publication) {
            self.clear();
            self.publication_identity = Some(publication);
        }
    }

    pub fn key_for_manifest(
        &self,
        manifest: &RetrievalIndexManifest,
        query_fingerprint: impl Into<String>,
    ) -> RetrievalCacheKey {
        let mut key = RetrievalCacheKey::from_manifest(manifest, query_fingerprint);
        if let Some((generation_id, run_id)) = &self.publication_identity {
            key.core_generation_id = Some(generation_id.clone());
            key.core_run_id = Some(run_id.clone());
        }
        key
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

    pub fn remove(&mut self, key: &RetrievalCacheKey) {
        self.entries.remove(key);
        self.order.retain(|existing| existing != key);
    }

    pub fn merge_delta_from(&mut self, baseline: &RetrievalCache, other: RetrievalCache) {
        let RetrievalCache {
            entries,
            order,
            publication_identity,
            ..
        } = other;
        if self.publication_identity != baseline.publication_identity
            && self.publication_identity != publication_identity
        {
            return;
        }
        if self.publication_identity == baseline.publication_identity
            && publication_identity != baseline.publication_identity
        {
            self.clear();
            self.publication_identity = publication_identity.clone();
        }
        if self.publication_identity != publication_identity {
            return;
        }
        for key in order {
            let Some(hits) = entries.get(&key) else {
                continue;
            };
            if baseline.entries.get(&key) == Some(hits) {
                continue;
            }
            self.insert(key, hits.clone());
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.publication_identity = None;
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
            core_generation_id: None,
            core_run_id: None,
            project_id: "abc".into(),
            lexical_version: "v1".into(),
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
            core_generation_id: None,
            core_run_id: None,
            project_id: "abc".into(),
            lexical_version: "v1".into(),
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
    fn merge_delta_from_does_not_replay_snapshot_entries() {
        let first = RetrievalCacheKey {
            core_generation_id: None,
            core_run_id: None,
            project_id: "abc".into(),
            lexical_version: "v1".into(),
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

        let mut target = RetrievalCache::with_capacity(1);
        target.insert(
            first.clone(),
            vec![super::super::CandidateHit::lexical_stub(
                "src/first.rs",
                1.0,
            )],
        );
        let baseline = target.clone();

        let mut newer_worker = baseline.clone();
        newer_worker.insert(
            second.clone(),
            vec![super::super::CandidateHit::lexical_stub(
                "src/second.rs",
                1.0,
            )],
        );
        target.merge_delta_from(&baseline, newer_worker);
        assert_eq!(
            target.get(&second).expect("newer entry should remain")[0].file_path,
            "src/second.rs"
        );

        target.merge_delta_from(&baseline, baseline.clone());

        assert!(target.get(&first).is_none());
        assert_eq!(
            target.get(&second).expect("stale snapshot must not replay")[0].file_path,
            "src/second.rs"
        );
    }

    #[test]
    fn cache_key_tracks_core_and_sidecar_publications() {
        let base = RetrievalIndexManifest {
            project_id: "abc".into(),
            lexical_version: "v1".into(),
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
            symbol_doc_count: Some(10),
            dense_projection_count: Some(10),
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            graph_artifact_hash: Some("graph-a".into()),
            dense_reason_counts_json: Some("{\"public_api\":10}".into()),
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        };
        let mut changed = base.clone();
        changed.qdrant_collection = "codestory_abc_hash_b".into();
        changed.sidecar_input_hash = Some("hash-b".into());
        changed.sidecar_generation = Some("abc-hash-b".into());

        assert_ne!(
            RetrievalCacheKey::from_manifest(&base, "query"),
            RetrievalCacheKey::from_manifest(&changed, "query")
        );

        let first = crate::executor::RetrievalPublicationIdentity {
            core_generation_id: "core-a".into(),
            core_run_id: "run-a".into(),
            sidecar_generation: "abc-hash-a".into(),
            sidecar_input_hash: "hash-a".into(),
            qdrant_collection: base.qdrant_collection.clone(),
        };
        let second = crate::executor::RetrievalPublicationIdentity {
            core_generation_id: "core-b".into(),
            core_run_id: "run-b".into(),
            ..first.clone()
        };
        let mut shared = RetrievalCache::new();
        shared.scope_to_publication(&first);
        let first_key = shared.key_for_manifest(&base, "query");
        shared.insert(
            first_key.clone(),
            vec![super::super::CandidateHit::lexical_stub("src/old.rs", 1.0)],
        );
        let baseline = shared.clone();
        let mut worker = baseline.clone();
        worker.scope_to_publication(&second);
        let second_key = worker.key_for_manifest(&base, "query");
        worker.insert(
            second_key.clone(),
            vec![super::super::CandidateHit::lexical_stub("src/new.rs", 1.0)],
        );
        shared.merge_delta_from(&baseline, worker);

        assert_ne!(first_key, second_key);
        assert!(shared.get(&first_key).is_none());
        assert_eq!(shared.key_for_manifest(&base, "query"), second_key);
        assert!(shared.get(&second_key).is_some());
    }
}
