use crate::candidate::CandidateHit;
use crate::config::SidecarLayout;
use crate::qdrant_client::QdrantClient;
use crate::scip_client::ScipClient;
use crate::zoekt_client::ZoektClient;
use anyhow::Result;
use codestory_store::RetrievalIndexManifest;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SemanticSearchScope {
    pub allow_paths: Vec<String>,
}

/// Sidecar search surface used by the executor (mockable in unit tests).
pub trait SidecarSearch: Send + Sync {
    fn zoekt_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>>;
    fn qdrant_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>>;
    fn qdrant_search_scoped(
        &self,
        query: &str,
        limit: usize,
        scope: &SemanticSearchScope,
    ) -> Result<Vec<CandidateHit>> {
        let _ = scope;
        self.qdrant_search(query, limit)
    }
    fn scip_anchor(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>>;
    fn scip_expand(&self, anchors: &[CandidateHit], limit: usize) -> Result<Vec<CandidateHit>>;
}

#[derive(Debug, Clone)]
pub struct LiveSidecarSearch {
    layout: SidecarLayout,
    project_id: String,
    sidecar_generation: String,
    qdrant_collection: String,
    zoekt: ZoektClient,
    qdrant: QdrantClient,
}

impl LiveSidecarSearch {
    pub fn new(
        layout: SidecarLayout,
        project_id: String,
        manifest: Option<&RetrievalIndexManifest>,
    ) -> Self {
        let zoekt = ZoektClient::new(&layout);
        let qdrant = QdrantClient::new(&layout);
        let sidecar_generation = manifest
            .and_then(|manifest| manifest.sidecar_generation.clone())
            .unwrap_or_else(|| format!("{project_id}-missing-manifest"));
        let qdrant_collection = manifest
            .map(|manifest| manifest.qdrant_collection.clone())
            .unwrap_or_else(|| format!("codestory_{project_id}_missing_manifest"));
        Self {
            layout,
            project_id,
            sidecar_generation,
            qdrant_collection,
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
    fn zoekt_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        self.zoekt
            .search(&self.layout, &self.sidecar_generation, query, limit)
    }

    fn qdrant_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        self.qdrant.search(&self.qdrant_collection, query, limit)
    }

    fn qdrant_search_scoped(
        &self,
        query: &str,
        limit: usize,
        scope: &SemanticSearchScope,
    ) -> Result<Vec<CandidateHit>> {
        self.qdrant
            .search_scoped(&self.qdrant_collection, query, limit, &scope.allow_paths)
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
        pub qdrant_scoped: Mutex<HashMap<String, Vec<CandidateHit>>>,
        pub qdrant_full_calls: Mutex<usize>,
        pub qdrant_scoped_calls: Mutex<Vec<SemanticSearchScope>>,
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
            *self.qdrant_full_calls.lock().expect("full call lock") += 1;
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

        fn qdrant_search_scoped(
            &self,
            query: &str,
            limit: usize,
            scope: &SemanticSearchScope,
        ) -> Result<Vec<CandidateHit>> {
            self.qdrant_scoped_calls
                .lock()
                .expect("scoped call lock")
                .push(scope.clone());
            Ok(self
                .qdrant_scoped
                .lock()
                .expect("qdrant scoped lock")
                .get(query)
                .cloned()
                .or_else(|| self.qdrant.lock().expect("qdrant lock").get(query).cloned())
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
