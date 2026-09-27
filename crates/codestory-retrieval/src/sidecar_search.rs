use crate::candidate::CandidateHit;
use crate::config::{SidecarLayout, SidecarRuntimeConfig};
use crate::embedded_vector::EmbeddedVectorIndex;
use crate::embeddings::EmbeddingDeviceReadiness;
use crate::lexical_client::LexicalClient;
use crate::scip_client::ScipClient;
use anyhow::Result;
use codestory_store::{RetrievalIndexManifest, Store};
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetrievalStopReason {
    RequestCancelled,
    StageCancelled,
    Deadline,
}

impl RetrievalStopReason {
    fn label(self) -> &'static str {
        match self {
            Self::RequestCancelled => "request_cancelled",
            Self::StageCancelled => "stage_cancelled",
            Self::Deadline => "deadline",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetrievalStopBoundary {
    Unknown,
    Stage(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RetrievalStopError {
    reason: RetrievalStopReason,
    boundary: RetrievalStopBoundary,
}

impl RetrievalStopError {
    fn message(self, unknown_boundary: Option<&str>) -> String {
        match self.boundary {
            RetrievalStopBoundary::Unknown => unknown_boundary.map_or_else(
                || {
                    format!(
                        "retrieval stopped: reason={} boundary=unknown",
                        self.reason.label()
                    )
                },
                |phase| {
                    format!(
                        "retrieval stopped: reason={} phase={phase}",
                        self.reason.label()
                    )
                },
            ),
            RetrievalStopBoundary::Stage(stage) => {
                format!(
                    "retrieval stopped: reason={} stage={stage}",
                    self.reason.label()
                )
            }
        }
    }
}

impl std::fmt::Display for RetrievalStopError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message(None))
    }
}

impl std::error::Error for RetrievalStopError {}

fn retrieval_stop_message_with_unknown_phase(
    error: &anyhow::Error,
    unknown_phase: Option<&'static str>,
) -> Option<String> {
    error
        .downcast_ref::<RetrievalStopError>()
        .map(|stop| stop.message(unknown_phase))
}

/// Preserve a typed retrieval stop through anyhow context while leaving unrelated errors alone.
pub fn retrieval_stop_message(error: &anyhow::Error) -> Option<String> {
    retrieval_stop_message_with_unknown_phase(error, None)
}

/// Name deferred full-readiness only when its runtime call boundary was entered.
pub fn deferred_full_readiness_stop_message(error: &anyhow::Error) -> Option<String> {
    retrieval_stop_message_with_unknown_phase(error, Some("deferred_full_readiness"))
}

/// Request-scoped deadline and cancellation state shared by retrieval stages and sidecar I/O.
#[derive(Debug, Clone)]
pub struct SearchExecutionContext {
    deadline: Instant,
    request_cancelled: Arc<AtomicBool>,
    stage_cancelled: Arc<AtomicBool>,
    boundary: RetrievalStopBoundary,
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
            boundary: RetrievalStopBoundary::Unknown,
        }
    }

    pub(crate) fn with_stage_boundary(mut self, stage: &'static str) -> Self {
        self.boundary = RetrievalStopBoundary::Stage(stage);
        self
    }

    fn stop_reason(&self) -> Option<RetrievalStopReason> {
        if self.request_cancelled.load(Ordering::Acquire) {
            Some(RetrievalStopReason::RequestCancelled)
        } else if self.stage_cancelled.load(Ordering::Acquire) {
            Some(RetrievalStopReason::StageCancelled)
        } else if Instant::now() >= self.deadline {
            Some(RetrievalStopReason::Deadline)
        } else {
            None
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.stop_reason().is_some()
    }

    pub fn check_cancelled(&self) -> Result<()> {
        if let Some(reason) = self.stop_reason() {
            return Err(RetrievalStopError {
                reason,
                boundary: self.boundary,
            }
            .into());
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

    /// Attach core structural identity before candidates enter cross-lane fusion.
    ///
    /// Live retrieval uses this to keep node kind, qualified name, and test
    /// ownership alongside each lane's independent score. Test sidecars and
    /// callers without a core publication retain the no-op default.
    fn enrich_candidates(&self, _candidates: &mut [CandidateHit]) -> Result<()> {
        Ok(())
    }

    fn lexical_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>>;
    fn lexical_search_batch(
        &self,
        _queries: &[(String, usize)],
        _context: &SearchExecutionContext,
    ) -> Result<Option<Vec<Vec<CandidateHit>>>> {
        Ok(None)
    }
    fn semantic_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>>;
    fn semantic_search_batch(
        &self,
        _queries: &[String],
        _limit: usize,
        _context: &SearchExecutionContext,
    ) -> Result<Option<Vec<Vec<CandidateHit>>>> {
        Ok(None)
    }
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

    fn lexical_descriptor_search_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        let mut hits = self.lexical_search_with_context(query, limit, context)?;
        for hit in &mut hits {
            hit.source_excerpt = None;
            hit.target = None;
        }
        Ok(hits)
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
    core_context: Option<CoreCandidateContext>,
}

#[derive(Clone)]
struct CoreCandidateContext {
    project_root: std::path::PathBuf,
    storage: Arc<Mutex<Store>>,
}

impl std::fmt::Debug for CoreCandidateContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CoreCandidateContext")
            .field("project_root", &self.project_root)
            .finish_non_exhaustive()
    }
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
            .map(|manifest| manifest.semantic_generation.clone())
            .unwrap_or_else(|| format!("codestory_{project_id}_missing_manifest"));
        let semantic = EmbeddedVectorIndex::open(
            &layout,
            &vector_generation,
            &sidecar_generation,
            &sidecar_input_hash,
            crate::embeddings::ProductEmbeddingClient::new(runtime),
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
            core_context: None,
        })
    }

    pub(crate) fn with_core_candidate_context(
        mut self,
        project_root: &Path,
        storage: Arc<Mutex<Store>>,
    ) -> Self {
        self.core_context = Some(CoreCandidateContext {
            project_root: project_root.to_path_buf(),
            storage,
        });
        self
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

    fn enrich_candidates(&self, candidates: &mut [CandidateHit]) -> Result<()> {
        let Some(context) = self.core_context.as_ref() else {
            return Ok(());
        };
        // Candidate classification must use the query's already pinned read
        // transaction, including for mutable legacy WAL cores. Sidecar I/O and
        // ranking run outside this short metadata-read guard.
        let storage = context.storage.lock();
        crate::query::enrich_candidates_from_core(&storage, &context.project_root, candidates)
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

    fn lexical_descriptor_search_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        let context = context.clone();
        self.lexical.search_descriptors_with_cancel(
            &self.layout,
            &self.sidecar_generation,
            &self.sidecar_input_hash,
            query,
            limit,
            move || context.is_cancelled(),
        )
    }

    fn lexical_search_batch(
        &self,
        queries: &[(String, usize)],
        context: &SearchExecutionContext,
    ) -> Result<Option<Vec<Vec<CandidateHit>>>> {
        let context = context.clone();
        Ok(Some(self.lexical.search_batch_with_cancel(
            &self.layout,
            &self.sidecar_generation,
            &self.sidecar_input_hash,
            queries,
            Arc::new(move || context.is_cancelled()),
        )?))
    }

    fn semantic_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        self.semantic.search(query, limit)
    }

    fn semantic_search_batch(
        &self,
        queries: &[String],
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Option<Vec<Vec<CandidateHit>>>> {
        Ok(Some(self.semantic.search_batch(queries, limit, context)?))
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
        ScipClient::expand_reference_adjacency(
            &self.layout,
            &self.sidecar_generation,
            anchors,
            limit,
        )
    }

    fn scip_expand_with_context(
        &self,
        anchors: &[CandidateHit],
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        ScipClient::expand_reference_adjacency_with_cancel(
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
            lexical_data_dir: root.join("lexical"),
            semantic_data_dir: root.join("semantic"),
            scip_artifacts_root: root.join("scip"),
            state_file: root.join("retrieval-sidecars.json"),
        }
    }

    fn accelerated_device() -> EmbeddingDeviceReadiness {
        EmbeddingDeviceReadiness {
            requested_policy: "accelerator_required",
            observed_state: "accelerated",
            observation_source: "per_user_server",
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
    fn search_context_preserves_the_observed_stop_predicate() {
        let future = Instant::now() + Duration::from_secs(1);
        let request = Arc::new(AtomicBool::new(true));
        let stage = Arc::new(AtomicBool::new(true));
        let error = SearchExecutionContext::new(future, request, stage)
            .check_cancelled()
            .expect_err("request cancellation must stop retrieval");
        assert_eq!(
            error.to_string(),
            "retrieval stopped: reason=request_cancelled boundary=unknown"
        );

        let request = Arc::new(AtomicBool::new(false));
        let stage = Arc::new(AtomicBool::new(true));
        let error = SearchExecutionContext::new(future, request, stage)
            .check_cancelled()
            .expect_err("stage cancellation must stop retrieval");
        assert_eq!(
            error.to_string(),
            "retrieval stopped: reason=stage_cancelled boundary=unknown"
        );

        let request = Arc::new(AtomicBool::new(false));
        let stage = Arc::new(AtomicBool::new(false));
        let error = SearchExecutionContext::new(Instant::now(), request, stage)
            .check_cancelled()
            .expect_err("expired deadline must stop retrieval");
        assert_eq!(
            error.to_string(),
            "retrieval stopped: reason=deadline boundary=unknown"
        );
    }

    #[test]
    fn zero_timeout_allowance_does_not_invent_an_expired_deadline() {
        let context = SearchExecutionContext::new(
            Instant::now() + Duration::from_secs(1),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let error = context
            .timeout(Duration::ZERO)
            .expect_err("a zero caller allowance cannot produce a usable timeout");

        assert_eq!(error.to_string(), "retrieval stage deadline exceeded");
        assert!(
            retrieval_stop_message(&error).is_none(),
            "zero caller allowance is not an observed stop predicate"
        );
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
