use crate::candidate::CandidateHit;
use crate::config::{SidecarLayout, SidecarRuntimeConfig};
use crate::embedded_vector::EmbeddedVectorIndex;
use crate::embeddings::EmbeddingDeviceReadiness;
use crate::lexical_client::LexicalClient;
use crate::scip_client::ScipClient;
use anyhow::Result;
use codestory_store::RetrievalIndexManifest;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Request-scoped deadline and cancellation state shared by retrieval stages and sidecar I/O.
#[derive(Debug, Clone)]
pub struct SearchExecutionContext {
    deadline: Instant,
    request_cancelled: Arc<AtomicBool>,
    stage_cancelled: Arc<AtomicBool>,
}

impl SearchExecutionContext {
    pub(crate) fn new(
        deadline: Instant,
        request_cancelled: Arc<AtomicBool>,
        stage_cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            deadline,
            request_cancelled,
            stage_cancelled,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.request_cancelled.load(Ordering::Acquire)
            || self.stage_cancelled.load(Ordering::Acquire)
            || Instant::now() >= self.deadline
    }

    pub fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            anyhow::bail!("retrieval stage cancelled or deadline exceeded");
        }
        Ok(())
    }

    pub fn timeout(&self, maximum: Duration) -> Result<Duration> {
        self.check_cancelled()?;
        let timeout = self
            .deadline
            .saturating_duration_since(Instant::now())
            .min(maximum);
        if timeout.is_zero() {
            anyhow::bail!("retrieval stage deadline exceeded");
        }
        Ok(timeout)
    }

    fn run<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        self.check_cancelled()?;
        let value = operation()?;
        self.check_cancelled()?;
        Ok(value)
    }
}

/// Sidecar search surface used by the executor (mockable in unit tests).
pub trait SidecarSearch: Send + Sync {
    fn layout(&self) -> Option<&SidecarLayout> {
        None
    }

    fn embedding_device_readiness(&self) -> Option<&EmbeddingDeviceReadiness> {
        None
    }

    fn runtime_config(&self) -> Option<&SidecarRuntimeConfig> {
        None
    }

    fn lexical_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>>;
    fn semantic_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>>;
    fn scip_anchor(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>>;
    fn scip_expand(&self, anchors: &[CandidateHit], limit: usize) -> Result<Vec<CandidateHit>>;

    fn lexical_search_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        context.run(|| self.lexical_search(query, limit))
    }

    fn semantic_search_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        context.run(|| self.semantic_search(query, limit))
    }

    fn scip_anchor_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        context.run(|| self.scip_anchor(query, limit))
    }

    fn scip_expand_with_context(
        &self,
        anchors: &[CandidateHit],
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        context.run(|| self.scip_expand(anchors, limit))
    }
}

#[derive(Debug, Clone)]
pub struct LiveSidecarSearch {
    runtime: SidecarRuntimeConfig,
    layout: SidecarLayout,
    project_id: String,
    sidecar_generation: String,
    sidecar_input_hash: String,
    embedding_device: Option<EmbeddingDeviceReadiness>,
    lexical: LexicalClient,
    semantic: EmbeddedVectorIndex,
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
        let runtime = crate::config::SidecarRuntimeConfig::for_project_profile(
            None,
            crate::config::SidecarProfile::Local,
        );
        Self::new_for_runtime_with_embedding_device(
            &runtime,
            layout,
            project_id,
            manifest,
            embedding_device,
        )
        .expect("default embedding runtime configuration must be valid")
    }

    pub fn new_for_runtime_with_embedding_device(
        runtime: &crate::config::SidecarRuntimeConfig,
        layout: SidecarLayout,
        project_id: String,
        manifest: Option<&RetrievalIndexManifest>,
        embedding_device: Option<EmbeddingDeviceReadiness>,
    ) -> Result<Self> {
        let lexical = LexicalClient::new(&layout);
        let sidecar_generation = manifest
            .and_then(|manifest| manifest.sidecar_generation.clone())
            .unwrap_or_else(|| format!("{project_id}-missing-manifest"));
        let sidecar_input_hash = manifest
            .and_then(|manifest| manifest.sidecar_input_hash.clone())
            .unwrap_or_else(|| "missing-manifest".to_string());
        let vector_generation = manifest
            .map(|manifest| manifest.qdrant_collection.clone())
            .unwrap_or_else(|| format!("codestory_{project_id}_missing_manifest"));
        let semantic = EmbeddedVectorIndex::open(
            &layout,
            &vector_generation,
            &sidecar_generation,
            &sidecar_input_hash,
            crate::embeddings::LlamaCppEmbeddingClient::new(&runtime.embedding)?,
        );
        Ok(Self {
            runtime: runtime.clone(),
            layout,
            project_id,
            sidecar_generation,
            sidecar_input_hash,
            embedding_device,
            lexical,
            semantic,
        })
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

    fn runtime_config(&self) -> Option<&SidecarRuntimeConfig> {
        Some(&self.runtime)
    }

    fn lexical_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        self.lexical.search(
            &self.layout,
            &self.sidecar_generation,
            &self.sidecar_input_hash,
            query,
            limit,
        )
    }

    fn lexical_search_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        let context = context.clone();
        self.lexical.search_with_cancel(
            &self.layout,
            &self.sidecar_generation,
            &self.sidecar_input_hash,
            query,
            limit,
            move || context.is_cancelled(),
        )
    }

    fn semantic_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        self.semantic.search(query, limit)
    }

    fn semantic_search_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        self.semantic.search_with_context(query, limit, context)
    }

    fn scip_anchor(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        ScipClient::anchor_search(&self.layout, &self.sidecar_generation, query, limit)
    }

    fn scip_anchor_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        ScipClient::anchor_search_with_cancel(
            &self.layout,
            &self.sidecar_generation,
            query,
            limit,
            &|| context.is_cancelled(),
        )
    }

    fn scip_expand(&self, anchors: &[CandidateHit], limit: usize) -> Result<Vec<CandidateHit>> {
        ScipClient::expand_graph(&self.layout, &self.sidecar_generation, anchors, limit)
    }

    fn scip_expand_with_context(
        &self,
        anchors: &[CandidateHit],
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        ScipClient::expand_graph_with_cancel(
            &self.layout,
            &self.sidecar_generation,
            anchors,
            limit,
            &|| context.is_cancelled(),
        )
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    pub struct MockSidecarSearch {
        pub lexical: Mutex<HashMap<String, Vec<CandidateHit>>>,
        pub semantic: Mutex<HashMap<String, Vec<CandidateHit>>>,
        pub scip_anchor: Mutex<HashMap<String, Vec<CandidateHit>>>,
        pub scip_expand: Mutex<Vec<CandidateHit>>,
    }

    impl MockSidecarSearch {
        #[allow(dead_code)]
        pub fn with_lexical(query: &str, hits: Vec<CandidateHit>) -> Self {
            let mut lexical = HashMap::new();
            lexical.insert(query.to_string(), hits);
            Self {
                lexical: Mutex::new(lexical),
                ..Default::default()
            }
        }
    }

    impl SidecarSearch for MockSidecarSearch {
        fn lexical_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(self
                .lexical
                .lock()
                .expect("lexical lock")
                .get(query)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .take(limit)
                .collect())
        }

        fn semantic_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(self
                .semantic
                .lock()
                .expect("semantic lock")
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
            qdrant_http_port: 32102,
            qdrant_grpc_port: 32103,
            lexical_data_dir: root.join("lexical"),
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
