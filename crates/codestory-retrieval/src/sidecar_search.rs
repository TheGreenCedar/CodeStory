use crate::candidate::CandidateHit;
use crate::config::SidecarLayout;
use crate::embeddings::EmbeddingDeviceReadiness;
use crate::qdrant_client::QdrantClient;
use crate::scip_client::ScipClient;
use crate::zoekt_client::ZoektClient;
use anyhow::Result;
use codestory_store::RetrievalIndexManifest;
/// Sidecar search surface used by the executor (mockable in unit tests).
pub trait SidecarSearch: Send + Sync {
    fn layout(&self) -> Option<&SidecarLayout> {
        None
    }

    fn embedding_device_readiness(&self) -> Option<&EmbeddingDeviceReadiness> {
        None
    }

    fn zoekt_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>>;
    fn qdrant_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>>;
    fn scip_anchor(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>>;
    fn scip_expand(&self, anchors: &[CandidateHit], limit: usize) -> Result<Vec<CandidateHit>>;
}

#[derive(Debug, Clone)]
pub struct LiveSidecarSearch {
    layout: SidecarLayout,
    project_id: String,
    sidecar_generation: String,
    sidecar_input_hash: String,
    qdrant_collection: String,
    embedding_device: Option<EmbeddingDeviceReadiness>,
    zoekt: ZoektClient,
    qdrant: QdrantClient,
}

impl LiveSidecarSearch {
    pub fn new(
        layout: SidecarLayout,
        project_id: String,
        manifest: Option<&RetrievalIndexManifest>,
    ) -> Self {
        Self::new_with_embedding_device(layout, project_id, manifest, None)
    }

    pub fn new_with_embedding_device(
        layout: SidecarLayout,
        project_id: String,
        manifest: Option<&RetrievalIndexManifest>,
        embedding_device: Option<EmbeddingDeviceReadiness>,
    ) -> Self {
        let zoekt = ZoektClient::new(&layout);
        let qdrant = QdrantClient::new(&layout);
        let sidecar_generation = manifest
            .and_then(|manifest| manifest.sidecar_generation.clone())
            .unwrap_or_else(|| format!("{project_id}-missing-manifest"));
        let sidecar_input_hash = manifest
            .and_then(|manifest| manifest.sidecar_input_hash.clone())
            .unwrap_or_else(|| "missing-manifest".to_string());
        let qdrant_collection = manifest
            .map(|manifest| manifest.qdrant_collection.clone())
            .unwrap_or_else(|| format!("codestory_{project_id}_missing_manifest"));
        Self {
            layout,
            project_id,
            sidecar_generation,
            sidecar_input_hash,
            qdrant_collection,
            embedding_device,
            zoekt,
            qdrant,
        }
    }

    pub fn layout(&self) -> &SidecarLayout {
        &self.layout
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn sidecar_generation(&self) -> &str {
        &self.sidecar_generation
    }
}

impl SidecarSearch for LiveSidecarSearch {
    fn layout(&self) -> Option<&SidecarLayout> {
        Some(&self.layout)
    }

    fn embedding_device_readiness(&self) -> Option<&EmbeddingDeviceReadiness> {
        self.embedding_device.as_ref()
    }

    fn zoekt_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        self.zoekt.search(
            &self.layout,
            &self.sidecar_generation,
            &self.sidecar_input_hash,
            query,
            limit,
        )
    }

    fn qdrant_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        self.qdrant.search(&self.qdrant_collection, query, limit)
    }

    fn scip_anchor(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        ScipClient::anchor_search(&self.layout, &self.sidecar_generation, query, limit)
    }

    fn scip_expand(&self, anchors: &[CandidateHit], limit: usize) -> Result<Vec<CandidateHit>> {
        ScipClient::expand_graph(&self.layout, &self.sidecar_generation, anchors, limit)
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    pub struct MockSidecarSearch {
        pub zoekt: Mutex<HashMap<String, Vec<CandidateHit>>>,
        pub qdrant: Mutex<HashMap<String, Vec<CandidateHit>>>,
        pub scip_anchor: Mutex<HashMap<String, Vec<CandidateHit>>>,
        pub scip_expand: Mutex<Vec<CandidateHit>>,
    }

    impl MockSidecarSearch {
        #[allow(dead_code)]
        pub fn with_zoekt(query: &str, hits: Vec<CandidateHit>) -> Self {
            let mut zoekt = HashMap::new();
            zoekt.insert(query.to_string(), hits);
            Self {
                zoekt: Mutex::new(zoekt),
                ..Default::default()
            }
        }
    }

    impl SidecarSearch for MockSidecarSearch {
        fn zoekt_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(self
                .zoekt
                .lock()
                .expect("zoekt lock")
                .get(query)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .take(limit)
                .collect())
        }

        fn qdrant_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(self
                .qdrant
                .lock()
                .expect("qdrant lock")
                .get(query)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .take(limit)
                .collect())
        }

        fn scip_anchor(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(self
                .scip_anchor
                .lock()
                .expect("scip anchor lock")
                .get(query)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .take(limit)
                .collect())
        }

        fn scip_expand(
            &self,
            _anchors: &[CandidateHit],
            limit: usize,
        ) -> Result<Vec<CandidateHit>> {
            Ok(self
                .scip_expand
                .lock()
                .expect("scip expand lock")
                .clone()
                .into_iter()
                .take(limit)
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_layout() -> SidecarLayout {
        let root = std::env::temp_dir().join("codestory-sidecar-search-test");
        SidecarLayout {
            zoekt_http_port: 32101,
            qdrant_http_port: 32102,
            qdrant_grpc_port: 32103,
            zoekt_data_dir: root.join("zoekt"),
            qdrant_data_dir: root.join("qdrant"),
            scip_artifacts_root: root.join("scip"),
            state_file: root.join("retrieval-sidecars.json"),
        }
    }

    fn accelerated_device() -> EmbeddingDeviceReadiness {
        EmbeddingDeviceReadiness {
            requested_policy: "accelerator_required",
            observed_state: "accelerated",
            observation_source: "native_log",
            detected_provider: None,
            detected_gpu: None,
            accelerator_requested: true,
            accelerator_request_provider: Some("metal".into()),
            accelerator_request_device: None,
            cpu_allowed: false,
            full_retrieval_allowed: true,
            degraded_reason: None,
        }
    }

    #[test]
    fn live_sidecar_search_carries_runtime_embedding_device_truth() {
        let device = accelerated_device();
        let live = LiveSidecarSearch::new_with_embedding_device(
            test_layout(),
            "project".into(),
            None,
            Some(device.clone()),
        );

        assert_eq!(live.embedding_device_readiness(), Some(&device));

        let generic = LiveSidecarSearch::new(test_layout(), "project".into(), None);
        assert!(generic.embedding_device_readiness().is_none());
    }
}
