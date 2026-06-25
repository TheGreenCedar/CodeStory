use crate::cache::{RetrievalCache, RetrievalCacheKey};
use crate::candidate::CandidateHit;
use crate::health::probe_sidecar_health;
use crate::index::query_fingerprint;
use crate::mode::{RetrievalDegradedMode, derive_degraded_mode};
use crate::planner::{PlannedStage, RetrievalStageKind};
use crate::query_features::{QueryFeatures, classify_query};
use crate::ranker::rank_candidates;
use crate::sidecar_search::SidecarSearch;
use anyhow::{Result, bail};
use codestory_store::RetrievalIndexManifest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Trace for one retrieval stage.
///
/// `degraded` and `stub_reason` are diagnostic fields. A stage trace does not make partial
/// sidecar output eligible for packet/search primary results.
pub struct StageTrace {
    pub stage: RetrievalStageKind,
    pub budget_ms: u64,
    pub elapsed_ms: u64,
    pub candidates_added: usize,
    pub marginal_gain: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_reason: Option<String>,
    pub cache_hit: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub degraded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stub_reason: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Trace for a complete sidecar query.
///
/// `retrieval_mode="full"` is the only product-ready mode. `degraded_reason`, cancellation, and
/// cache-hit fields explain why a query could not provide fresh full-mode evidence.
pub struct QueryTrace {
    pub retrieval_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    pub total_budget_ms: u64,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_reason: Option<String>,
    pub cache_hit: bool,
    pub stages: Vec<StageTrace>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Result of executing one retrieval query against the sidecar stack.
///
/// Hits may include lexical, graph, or dense-anchor candidates. Runtime packet code must still
/// resolve candidates to indexed symbols before treating them as answer support.
pub struct QueryResult {
    pub query: String,
    pub features: QueryFeatures,
    pub hits: Vec<CandidateHit>,
    pub trace: QueryTrace,
}

/// Executes sidecar retrieval stages with manifest-scoped caching.
///
/// The executor is fail-closed: live degraded modes return an error instead of serving partial
/// results. Tests may use `mode_override`, but product callers should rely on live health probes.
pub struct QueryExecutor<'a> {
    pub sidecars: Arc<dyn SidecarSearch>,
    pub cache: &'a mut RetrievalCache,
    pub manifest: Option<RetrievalIndexManifest>,
    pub file_roles: HashMap<String, codestory_store::FileRole>,
    pub cancelled: Arc<AtomicBool>,
    /// When set (tests), skips live health probing.
    pub mode_override: Option<RetrievalDegradedMode>,
}

impl<'a> QueryExecutor<'a> {
    /// Run one query within the provided total budget.
    ///
    /// `total_budget_ms` caps retrieval work only; it does not include runtime candidate
    /// resolution, packet sufficiency checks, or answer composition.
    pub fn execute(&mut self, query: &str, total_budget_ms: Option<u64>) -> Result<QueryResult> {
        let features = classify_query(query);
        let fingerprint = query_fingerprint(&features.raw_query);

        let (mode, degraded_reason) = self.resolve_mode();
        if mode != RetrievalDegradedMode::Full {
            bail!(
                "retrieval sidecar is mandatory; project is not in full mode (mode={}, reason={})",
                mode.as_str(),
                degraded_reason.as_deref().unwrap_or("unknown")
            );
        }

        if let Some(manifest) = self.manifest.as_ref() {
            let key = RetrievalCacheKey::from_manifest(manifest, fingerprint.clone());
            if let Some(cached) = self.cache.get(&key) {
                return Ok(QueryResult {
                    query: features.raw_query.clone(),
                    features,
                    hits: cached.to_vec(),
                    trace: QueryTrace {
                        retrieval_mode: mode.as_str().into(),
                        degraded_reason: None,
                        total_budget_ms: 0,
                        elapsed_ms: 0,
                        cancel_reason: None,
                        cache_hit: true,
                        stages: Vec::new(),
                    },
                });
            }
        }

        let mut plan = crate::planner::plan_query(&features, mode);
        let planned_budget_ms = plan.stages.iter().map(|stage| stage.budget_ms).sum::<u64>();
        if let Some(budget) = total_budget_ms {
            if is_broad_query(features.shape) && budget < planned_budget_ms {
                scale_stage_budgets(&mut plan.stages, budget);
            }
            plan.total_budget_ms = budget;
        }

        let started = Instant::now();
        let mut candidates = Vec::new();
        let mut stage_traces = Vec::new();

        let cancel_reason = self.run_stage_sequence(
            &features,
            &plan.stages,
            &mut candidates,
            &mut stage_traces,
            StageSequenceOptions {
                stop_marginal_gain_threshold: Some(plan.stop_marginal_gain_threshold),
                stop_after_low_gain_streak: plan.stop_after_low_gain_streak,
            },
        )?;

        enrich_candidates_with_file_roles(&mut candidates, &self.file_roles);
        let ranked = rank_candidates(&features, candidates);
        let hits = ranked;

        if cancel_reason.is_none()
            && let Some(manifest) = self.manifest.as_ref()
        {
            let key = RetrievalCacheKey::from_manifest(manifest, fingerprint);
            self.cache.insert(key, hits.clone());
        }

        Ok(QueryResult {
            query: features.raw_query.clone(),
            features,
            hits,
            trace: QueryTrace {
                retrieval_mode: mode.as_str().into(),
                degraded_reason,
                total_budget_ms: plan.total_budget_ms,
                elapsed_ms: started.elapsed().as_millis() as u64,
                cancel_reason,
                cache_hit: false,
                stages: stage_traces,
            },
        })
    }

    fn resolve_mode(&self) -> (RetrievalDegradedMode, Option<String>) {
        if let Some(mode) = self.mode_override {
            return (mode, None);
        }
        if let Some(manifest) = self.manifest.as_ref() {
            let Some(layout) = self.sidecars.layout() else {
                return (
                    RetrievalDegradedMode::Unavailable,
                    Some("sidecar_layout_missing".into()),
                );
            };
            let report = probe_sidecar_health(layout, &manifest.project_id, Some(manifest.clone()));
            return derive_degraded_mode(&report.zoekt, &report.qdrant, &report.scip);
        }
        (
            RetrievalDegradedMode::LexicalOnly,
            Some("manifest_missing".into()),
        )
    }

    fn run_stage(
        sidecars: &dyn SidecarSearch,
        stage: &PlannedStage,
        features: &QueryFeatures,
        anchors: &[CandidateHit],
    ) -> Result<Vec<CandidateHit>> {
        let query = &features.raw_query;
        match stage.kind {
            RetrievalStageKind::Stage0ScipAnchor => sidecars.scip_anchor(query, stage.top_k),
            RetrievalStageKind::Stage1ZoektLexical => sidecars.zoekt_search(query, stage.top_k),
            RetrievalStageKind::Stage1bQdrantSemantic => sidecars.qdrant_search(query, stage.top_k),
            RetrievalStageKind::Stage2ScipExpand => sidecars.scip_expand(anchors, stage.top_k),
            RetrievalStageKind::Stage3RepoTextFallback => {
                bail!("repo-text diagnostic stage is unsupported in mandatory sidecar retrieval")
            }
        }
    }

    fn run_stage_bounded(
        &self,
        stage: &PlannedStage,
        features: &QueryFeatures,
        anchors: &[CandidateHit],
    ) -> Result<StageRun> {
        if !is_broad_query(features.shape) {
            return Self::run_stage(self.sidecars.as_ref(), stage, features, anchors)
                .map(StageRun::Completed);
        }

        let stage = stage.clone();
        let timeout_ms = stage.budget_ms.max(1);
        let features = features.clone();
        let anchors = anchors.to_vec();
        let sidecars = Arc::clone(&self.sidecars);
        let (sender, receiver) = mpsc::channel();
        // ponytail: timed-out sidecar work is left to finish; use a shared worker pool if timeout volume matters.
        std::thread::spawn(move || {
            let result = Self::run_stage(sidecars.as_ref(), &stage, &features, &anchors);
            let _ = sender.send(result);
        });

        match receiver.recv_timeout(Duration::from_millis(timeout_ms)) {
            Ok(result) => result.map(StageRun::Completed),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(StageRun::TimedOut),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("sidecar stage worker disconnected")
            }
        }
    }

    fn run_stage_sequence(
        &self,
        features: &QueryFeatures,
        stages: &[PlannedStage],
        candidates: &mut Vec<CandidateHit>,
        stage_traces: &mut Vec<StageTrace>,
        options: StageSequenceOptions,
    ) -> Result<Option<String>> {
        let mut low_gain_streak = 0u32;
        let mut cancel_reason = None;
        for stage in stages {
            if self.cancelled.load(Ordering::Relaxed) {
                return Ok(Some("cancelled".into()));
            }

            if should_skip_after_exact_symbol_anchor(stage, features, candidates) {
                stage_traces.push(stage_trace(
                    stage,
                    0,
                    0,
                    0.0,
                    Some("exact_symbol_anchor".into()),
                    false,
                    None,
                ));
                continue;
            }
            if should_skip_zero_dense_stage(stage, self.manifest.as_ref()) {
                stage_traces.push(stage_trace(
                    stage,
                    0,
                    0,
                    0.0,
                    Some("zero_dense_anchors".into()),
                    false,
                    None,
                ));
                continue;
            }

            let stage_started = Instant::now();
            let before_score = candidate_mass(candidates);
            let mut stage_hits = match self.run_stage_bounded(stage, features, candidates)? {
                StageRun::Completed(hits) => hits,
                StageRun::TimedOut => {
                    stage_traces.push(stage_trace(
                        stage,
                        stage.budget_ms,
                        0,
                        0.0,
                        Some("stage_deadline".into()),
                        false,
                        None,
                    ));
                    cancel_reason.get_or_insert_with(|| "stage_deadline".into());
                    continue;
                }
            };
            annotate_stage_provenance(stage, &mut stage_hits);
            let (stub_reason, stage_degraded) = stage_stub_metadata(&stage_hits);
            let added = merge_candidates(candidates, stage_hits);
            let after_score = candidate_mass(candidates);
            let marginal_gain = if before_score <= 0.0 {
                after_score
            } else {
                ((after_score - before_score) / before_score).max(0.0)
            };

            stage_traces.push(stage_trace(
                stage,
                stage_started.elapsed().as_millis() as u64,
                added,
                marginal_gain,
                None,
                stage_degraded,
                stub_reason,
            ));

            if let Some(threshold) = options.stop_marginal_gain_threshold {
                if marginal_gain < threshold && !candidates.is_empty() {
                    low_gain_streak += 1;
                    if low_gain_streak >= options.stop_after_low_gain_streak {
                        return Ok(Some("marginal_gain".into()));
                    }
                } else {
                    low_gain_streak = 0;
                }
            }
        }
        Ok(cancel_reason)
    }
}

enum StageRun {
    Completed(Vec<CandidateHit>),
    TimedOut,
}

#[derive(Debug, Clone, Copy)]
struct StageSequenceOptions {
    stop_marginal_gain_threshold: Option<f32>,
    stop_after_low_gain_streak: u32,
}

fn is_broad_query(shape: crate::query_features::QueryShape) -> bool {
    matches!(
        shape,
        crate::query_features::QueryShape::NaturalLanguage
            | crate::query_features::QueryShape::Mixed
    )
}

fn scale_stage_budgets(stages: &mut [PlannedStage], total_budget_ms: u64) {
    let planned_total = stages.iter().map(|stage| stage.budget_ms).sum::<u64>();
    if stages.is_empty() || planned_total == 0 {
        return;
    }

    let mut remaining = total_budget_ms.max(stages.len() as u64);
    let last = stages.len() - 1;
    for (index, stage) in stages.iter_mut().enumerate() {
        if index == last {
            stage.budget_ms = remaining.max(1);
            break;
        }
        let stages_left = (last - index) as u64;
        let scaled = stage.budget_ms.saturating_mul(total_budget_ms) / planned_total;
        let budget = scaled.max(1).min(remaining.saturating_sub(stages_left));
        stage.budget_ms = budget;
        remaining = remaining.saturating_sub(budget);
    }
}

fn stage_trace(
    stage: &PlannedStage,
    elapsed_ms: u64,
    candidates_added: usize,
    marginal_gain: f32,
    cancel_reason: Option<String>,
    degraded: bool,
    stub_reason: Option<String>,
) -> StageTrace {
    StageTrace {
        stage: stage.kind,
        budget_ms: stage.budget_ms,
        elapsed_ms,
        candidates_added,
        marginal_gain,
        cancel_reason,
        cache_hit: false,
        degraded,
        stub_reason,
    }
}

fn should_skip_after_exact_symbol_anchor(
    stage: &PlannedStage,
    features: &QueryFeatures,
    candidates: &[CandidateHit],
) -> bool {
    if !matches!(
        features.shape,
        crate::query_features::QueryShape::SymbolLike
    ) {
        return false;
    }
    if !matches!(
        stage.kind,
        RetrievalStageKind::Stage1bQdrantSemantic | RetrievalStageKind::Stage2ScipExpand
    ) {
        return false;
    }
    candidates
        .iter()
        .any(|candidate| candidate_is_exact_symbol_anchor(&features.raw_query, candidate))
}

fn should_skip_zero_dense_stage(
    stage: &PlannedStage,
    manifest: Option<&RetrievalIndexManifest>,
) -> bool {
    if !matches!(stage.kind, RetrievalStageKind::Stage1bQdrantSemantic) {
        return false;
    }
    let dense_count = manifest
        .and_then(|manifest| {
            manifest
                .dense_projection_count
                .or(manifest.projection_count)
        })
        .unwrap_or(0);
    dense_count <= 0
}

fn annotate_stage_provenance(stage: &PlannedStage, hits: &mut [CandidateHit]) {
    if let Some(label) = stage.kind.provenance_label() {
        for hit in hits {
            hit.add_provenance(label);
        }
    }
}

fn candidate_is_exact_symbol_anchor(query: &str, candidate: &CandidateHit) -> bool {
    if matches!(
        candidate.source,
        crate::candidate::CandidateSource::Qdrant | crate::candidate::CandidateSource::Legacy
    ) {
        return false;
    }
    let Some(symbol) = candidate.symbol_name.as_deref() else {
        return false;
    };
    let query_lower = query.trim().to_ascii_lowercase();
    if query_lower.is_empty() {
        return false;
    }
    let symbol_lower = symbol.trim().to_ascii_lowercase();
    if symbol_lower == query_lower {
        return true;
    }
    let symbol_tail = symbol_lower
        .rsplit("::")
        .next()
        .unwrap_or(&symbol_lower)
        .rsplit('.')
        .next()
        .unwrap_or(&symbol_lower);
    query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|token| token.len() >= 2)
        .map(|token| token.to_ascii_lowercase())
        .any(|token| token == symbol_tail)
}

fn candidate_mass(candidates: &[CandidateHit]) -> f32 {
    candidates.iter().map(|hit| hit.score.max(0.01)).sum()
}

fn stage_stub_metadata(hits: &[CandidateHit]) -> (Option<String>, bool) {
    if hits.is_empty() {
        return (None, false);
    }
    if crate::candidate::phantom_sidecar_candidates_only(hits) {
        return (Some("phantom_stub_hits".into()), true);
    }
    (None, false)
}

fn merge_candidates(acc: &mut Vec<CandidateHit>, incoming: Vec<CandidateHit>) -> usize {
    let mut added = 0usize;
    for hit in incoming {
        let duplicate = acc.iter_mut().find(|existing| {
            existing.file_path == hit.file_path && existing.symbol_name == hit.symbol_name
        });
        if let Some(existing) = duplicate {
            existing.score = existing.score.max(hit.score);
            if existing.node_id.is_none() {
                existing.node_id = hit.node_id.clone();
            }
            if existing.start_line.is_none() {
                existing.start_line = hit.start_line;
            }
            for label in hit.provenance {
                existing.add_provenance(label);
            }
            continue;
        }
        acc.push(hit);
        added += 1;
    }
    added
}

pub fn cancellation_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn enrich_candidates_with_file_roles(
    candidates: &mut [CandidateHit],
    file_roles: &HashMap<String, codestory_store::FileRole>,
) {
    for candidate in candidates {
        if candidate.file_role.is_some() {
            continue;
        }
        candidate.file_role = Some(
            lookup_file_role(file_roles, &candidate.file_path).unwrap_or_else(|| {
                codestory_store::FileRole::classify_path(Path::new(&candidate.file_path))
            }),
        );
    }
}

fn lookup_file_role(
    file_roles: &HashMap<String, codestory_store::FileRole>,
    file_path: &str,
) -> Option<codestory_store::FileRole> {
    file_roles.get(file_path).copied().or_else(|| {
        let normalized = file_path.replace('\\', "/");
        file_roles.get(&normalized).copied()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::RetrievalCache;
    use crate::candidate::{CandidateHit, CandidateSource};
    use crate::config::SidecarLayout;
    use crate::sidecar_search::{SidecarSearch, mock::MockSidecarSearch};
    use crate::test_support::retrieval_manifest_fixture;
    use codestory_store::RetrievalIndexManifest;
    use std::collections::HashMap;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn sample_manifest() -> RetrievalIndexManifest {
        RetrievalIndexManifest {
            project_id: "testproj".into(),
            zoekt_version: "v1".into(),
            qdrant_collection: "codestory_testproj".into(),
            scip_revision: Some("rev1".into()),
            built_at_epoch_ms: 0,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: None,
            embedding_dim: None,
            sidecar_schema_version: None,
            sidecar_input_hash: None,
            sidecar_generation: None,
            projection_count: Some(10),
            symbol_doc_count: Some(20),
            dense_projection_count: Some(10),
            semantic_policy_version: None,
            graph_artifact_hash: None,
            dense_reason_counts_json: None,
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        }
    }

    #[test]
    fn executor_runs_stages_with_mock_sidecars() {
        let mock = MockSidecarSearch {
            zoekt: Mutex::new(HashMap::from([(
                "ExtensionService".into(),
                vec![CandidateHit::with_source(
                    "src/service.rs",
                    Some("ExtensionService".into()),
                    0.9,
                    CandidateSource::Zoekt,
                )],
            )])),
            qdrant: Mutex::new(HashMap::from([(
                "ExtensionService".into(),
                vec![CandidateHit::with_source(
                    "src/service_semantic.rs",
                    Some("ExtensionService".into()),
                    0.8,
                    CandidateSource::Qdrant,
                )],
            )])),
            scip_anchor: Mutex::new(HashMap::new()),
            scip_expand: Mutex::new(Vec::new()),
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute("ExtensionService", Some(800))
            .expect("query succeeds");
        assert!(!result.hits.is_empty());
        assert!(!result.trace.stages.is_empty());
        assert!(!result.trace.cache_hit);
    }

    #[test]
    fn executor_uses_cache_on_second_query() {
        let mock = MockSidecarSearch::default();
        let mut cache = RetrievalCache::new();
        let manifest = sample_manifest();
        let key = RetrievalCacheKey::from_manifest(&manifest, query_fingerprint("cached-query"));
        cache.insert(key, vec![CandidateHit::lexical_stub("cached.rs", 1.0)]);

        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(manifest),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("cached-query", None).expect("cache hit");
        assert!(result.trace.cache_hit);
        assert_eq!(result.hits[0].file_path, "cached.rs");
    }

    #[test]
    fn executor_caches_only_complete_query_results() {
        let mock = Arc::new(MockSidecarSearch {
            zoekt: Mutex::new(HashMap::from([(
                "startup".into(),
                vec![CandidateHit::with_source(
                    "src/startup.rs",
                    Some("startup".into()),
                    0.9,
                    CandidateSource::Zoekt,
                )],
            )])),
            ..Default::default()
        });
        let mut cache = RetrievalCache::new();
        let manifest = sample_manifest();
        {
            let mut executor = QueryExecutor {
                sidecars: mock.clone(),
                cache: &mut cache,
                manifest: Some(manifest.clone()),
                file_roles: HashMap::new(),
                cancelled: cancellation_flag(),
                mode_override: Some(RetrievalDegradedMode::Full),
            };
            let result = executor.execute("startup", Some(800)).expect("query");
            assert_eq!(result.trace.cancel_reason, None);
        }
        let key = RetrievalCacheKey::from_manifest(&manifest, query_fingerprint("startup"));
        assert!(cache.get(&key).is_some());

        let mut cancelled_cache = RetrievalCache::new();
        let cancelled = cancellation_flag();
        cancelled.store(true, Ordering::Relaxed);
        let mut executor = QueryExecutor {
            sidecars: mock.clone(),
            cache: &mut cancelled_cache,
            manifest: Some(manifest.clone()),
            file_roles: HashMap::new(),
            cancelled,
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("startup", Some(800)).expect("query");
        assert_eq!(result.trace.cancel_reason.as_deref(), Some("cancelled"));
        assert_eq!(cancelled_cache.len(), 0);
    }

    #[test]
    fn executor_skips_semantic_and_expand_after_exact_symbol_anchor() {
        let mock = MockSidecarSearch {
            scip_anchor: Mutex::new(HashMap::from([(
                "EventProcessor".into(),
                vec![CandidateHit::with_source(
                    "src/event_processor.rs",
                    Some("EventProcessor".into()),
                    0.95,
                    CandidateSource::Scip,
                )],
            )])),
            qdrant: Mutex::new(HashMap::from([(
                "EventProcessor".into(),
                vec![CandidateHit::with_source(
                    "docs/event-output.md",
                    Some("event output".into()),
                    0.99,
                    CandidateSource::Qdrant,
                )],
            )])),
            scip_expand: Mutex::new(vec![CandidateHit::with_source(
                "src/neighbor.rs",
                Some("Neighbor".into()),
                0.80,
                CandidateSource::Scip,
            )]),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute("EventProcessor", Some(800))
            .expect("query succeeds");
        assert_eq!(
            result.hits.first().map(|hit| hit.file_path.as_str()),
            Some("src/event_processor.rs")
        );
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.file_path != "docs/event-output.md")
        );
        let skipped: Vec<_> = result
            .trace
            .stages
            .iter()
            .filter(|stage| stage.cancel_reason.as_deref() == Some("exact_symbol_anchor"))
            .map(|stage| stage.stage)
            .collect();
        assert!(skipped.contains(&RetrievalStageKind::Stage1bQdrantSemantic));
        assert!(skipped.contains(&RetrievalStageKind::Stage2ScipExpand));
    }

    #[test]
    fn executor_rejects_non_full_modes() {
        let mock = MockSidecarSearch::default();
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::NoSemantic),
        };
        let error = executor
            .execute("ExtensionService", Some(800))
            .expect_err("non-full modes must fail closed");
        assert!(error.to_string().contains("retrieval sidecar is mandatory"));
    }

    #[test]
    fn executor_resolve_mode_probes_live_sidecar_layout_instead_of_env_default() {
        let mut layout = SidecarLayout::from_env();
        layout.zoekt_http_port = unused_local_port();
        let manifest = retrieval_manifest_fixture("testproj", "cafebabedeadbeef");
        let sidecars = Arc::new(TrackingSidecars {
            layout,
            layout_calls: AtomicUsize::new(0),
        });
        let mut cache = RetrievalCache::new();
        let executor = QueryExecutor {
            sidecars: sidecars.clone(),
            cache: &mut cache,
            manifest: Some(manifest),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: None,
        };

        let (mode, reason) = executor.resolve_mode();

        assert_eq!(sidecars.layout_calls.load(Ordering::Relaxed), 1);
        assert_eq!(mode, RetrievalDegradedMode::Unavailable);
        assert_eq!(reason.as_deref(), Some("zoekt_unreachable"));
    }

    #[test]
    fn executor_rejects_cached_hits_when_mode_is_not_full() {
        let mock = MockSidecarSearch::default();
        let mut cache = RetrievalCache::new();
        let manifest = sample_manifest();
        let key = RetrievalCacheKey::from_manifest(&manifest, query_fingerprint("cached-query"));
        cache.insert(key, vec![CandidateHit::lexical_stub("cached.rs", 1.0)]);

        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(manifest),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::NoSemantic),
        };
        let error = executor
            .execute("cached-query", None)
            .expect_err("cache must not bypass mandatory full sidecar mode");
        assert!(error.to_string().contains("retrieval sidecar is mandatory"));
    }

    #[test]
    fn executor_reaches_semantic_stage_after_empty_lexical_stages() {
        let mock = MockSidecarSearch {
            qdrant: Mutex::new(HashMap::from([(
                "how does startup sequence work".into(),
                vec![CandidateHit::with_source(
                    "src/semantic.rs",
                    Some("SemanticAnchor".into()),
                    0.8,
                    CandidateSource::Qdrant,
                )],
            )])),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute("how does startup sequence work", Some(800))
            .expect("query");
        assert!(
            result
                .trace
                .stages
                .iter()
                .any(|stage| stage.stage == RetrievalStageKind::Stage1bQdrantSemantic),
            "semantic stage should run after empty SCIP/Zoekt stages: {:?}",
            result.trace.stages
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.file_path == "src/semantic.rs"),
            "expected semantic hit after empty lexical stages: {:?}",
            result.hits
        );
    }

    #[test]
    fn broad_query_stage_deadline_preserves_later_sidecar_contribution() {
        struct SlowZoektSidecars;

        impl SidecarSearch for SlowZoektSidecars {
            fn zoekt_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                std::thread::sleep(Duration::from_millis(200));
                Ok(vec![CandidateHit::with_source(
                    "src/slow_lexical.rs",
                    Some("SlowLexical".into()),
                    0.99,
                    CandidateSource::Zoekt,
                )])
            }

            fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::with_source(
                    "src/semantic.rs",
                    Some("SemanticAnchor".into()),
                    0.8,
                    CandidateSource::Qdrant,
                )])
            }

            fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn scip_expand(
                &self,
                _anchors: &[CandidateHit],
                _limit: usize,
            ) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }
        }

        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(SlowZoektSidecars),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let started = Instant::now();
        let result = executor
            .execute(
                "LiveSidecarSearch qdrant_search retrieval_mode full sidecar unavailable",
                Some(120),
            )
            .expect("query");

        assert!(
            started.elapsed() < Duration::from_millis(190),
            "slow Zoekt must not consume the whole broad-query path: {:?}",
            result.trace.stages
        );
        assert_eq!(
            result.trace.cancel_reason.as_deref(),
            Some("stage_deadline")
        );
        assert!(
            result.trace.stages.iter().any(|stage| {
                stage.stage == RetrievalStageKind::Stage1ZoektLexical
                    && stage.cancel_reason.as_deref() == Some("stage_deadline")
            }),
            "Zoekt overrun should be explicit in stage provenance: {:?}",
            result.trace.stages
        );
        assert!(
            result.trace.stages.iter().any(|stage| {
                stage.stage == RetrievalStageKind::Stage1bQdrantSemantic
                    && stage.candidates_added > 0
            }),
            "Qdrant must still contribute after Zoekt overrun: {:?}",
            result.trace.stages
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.file_path == "src/semantic.rs"),
            "semantic fallback should be rankable after lexical overrun: {:?}",
            result.hits
        );
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.file_path != "src/slow_lexical.rs"),
            "timed-out Zoekt hits must not merge late into this query: {:?}",
            result.hits
        );
    }

    #[test]
    fn executor_skips_qdrant_when_policy_selects_zero_dense_anchors() {
        let mock = MockSidecarSearch {
            qdrant: Mutex::new(HashMap::from([(
                "how does startup sequence work".into(),
                vec![CandidateHit::with_source(
                    "src/semantic.rs",
                    Some("SemanticAnchor".into()),
                    0.8,
                    CandidateSource::Qdrant,
                )],
            )])),
            zoekt: Mutex::new(HashMap::from([(
                "how does startup sequence work".into(),
                vec![CandidateHit::with_source(
                    "src/lexical.rs",
                    Some("LexicalAnchor".into()),
                    0.7,
                    CandidateSource::Zoekt,
                )],
            )])),
            ..Default::default()
        };
        let mut manifest = sample_manifest();
        manifest.projection_count = Some(0);
        manifest.dense_projection_count = Some(0);
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(manifest),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute("how does startup sequence work", Some(800))
            .expect("query");
        assert!(
            result.trace.stages.iter().any(|stage| stage.stage
                == RetrievalStageKind::Stage1bQdrantSemantic
                && stage.cancel_reason.as_deref() == Some("zero_dense_anchors")),
            "zero dense policy should skip qdrant explicitly: {:?}",
            result.trace.stages
        );
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.file_path != "src/semantic.rs"),
            "qdrant hits must not be recalled when dense count is zero: {:?}",
            result.hits
        );
    }

    #[test]
    fn executor_merges_duplicate_candidate_provenance() {
        let query = "how extension service starts";
        let mock = MockSidecarSearch {
            zoekt: Mutex::new(HashMap::from([(
                query.into(),
                vec![CandidateHit::with_source(
                    "src/service.rs",
                    Some("ExtensionService".into()),
                    0.70,
                    CandidateSource::Zoekt,
                )],
            )])),
            qdrant: Mutex::new(HashMap::from([(
                query.into(),
                vec![CandidateHit::with_source(
                    "src/service.rs",
                    Some("ExtensionService".into()),
                    0.85,
                    CandidateSource::Qdrant,
                )],
            )])),
            scip_expand: Mutex::new(vec![CandidateHit::with_source(
                "src/service.rs",
                Some("ExtensionService".into()),
                0.75,
                CandidateSource::Scip,
            )]),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute(query, Some(800)).expect("query");
        let hit = result
            .hits
            .iter()
            .find(|hit| hit.file_path == "src/service.rs")
            .expect("merged candidate");
        assert!(
            hit.score > 0.70,
            "merged candidate should keep ranker-adjusted score above lexical-only input: {hit:?}"
        );
        assert!(hit.provenance.iter().any(|label| label == "graph_neighbor"));
        assert!(hit.provenance.iter().any(|label| label == "dense_anchor"));
    }

    #[test]
    fn executor_respects_cancellation() {
        let mock = MockSidecarSearch::default();
        let mut cache = RetrievalCache::new();
        let cancelled = cancellation_flag();
        cancelled.store(true, Ordering::Relaxed);
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: HashMap::new(),
            cancelled,
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("anything", Some(500)).expect("partial ok");
        assert_eq!(result.trace.cancel_reason.as_deref(), Some("cancelled"));
    }

    #[test]
    fn executor_enriches_file_role_before_ranking() {
        let mock = MockSidecarSearch {
            zoekt: Mutex::new(HashMap::from([(
                "startup".into(),
                vec![
                    CandidateHit::with_source("src/main.rs", None, 0.55, CandidateSource::Zoekt),
                    CandidateHit::with_source(
                        "src\\boot_test.rs",
                        None,
                        0.80,
                        CandidateSource::Zoekt,
                    ),
                ],
            )])),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut roles = HashMap::new();
        roles.insert(
            "src/main.rs".to_string(),
            codestory_store::FileRole::Entrypoint,
        );
        roles.insert(
            "src/boot_test.rs".to_string(),
            codestory_store::FileRole::Test,
        );
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: roles,
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("startup", Some(500)).expect("query");
        let role_by_path = result
            .hits
            .iter()
            .map(|hit| (hit.file_path.as_str(), hit.file_role))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            role_by_path.get("src/main.rs").copied().flatten(),
            Some(codestory_store::FileRole::Entrypoint)
        );
        assert_eq!(
            role_by_path.get("src\\boot_test.rs").copied().flatten(),
            Some(codestory_store::FileRole::Test)
        );
    }

    #[test]
    fn executor_infers_file_role_when_storage_lookup_misses() {
        let mock = MockSidecarSearch {
            zoekt: Mutex::new(HashMap::from([(
                "startup".into(),
                vec![CandidateHit::with_source(
                    "fixtures/generated/boot_test.rs",
                    None,
                    0.80,
                    CandidateSource::Zoekt,
                )],
            )])),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("startup", Some(500)).expect("query");
        assert_eq!(
            result.hits.first().and_then(|hit| hit.file_role),
            Some(codestory_store::FileRole::Generated)
        );
    }

    struct TrackingSidecars {
        layout: SidecarLayout,
        layout_calls: AtomicUsize,
    }

    impl SidecarSearch for TrackingSidecars {
        fn layout(&self) -> Option<&SidecarLayout> {
            self.layout_calls.fetch_add(1, Ordering::Relaxed);
            Some(&self.layout)
        }

        fn zoekt_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }

        fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }

        fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }

        fn scip_expand(
            &self,
            _anchors: &[CandidateHit],
            _limit: usize,
        ) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }
    }

    fn unused_local_port() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind free port");
        listener.local_addr().expect("local addr").port()
    }

    #[test]
    fn executor_does_not_use_repo_text_diagnostic_for_natural_language_queries() {
        let mock = MockSidecarSearch {
            zoekt: Mutex::new(HashMap::from([(
                "how does startup sequence work".into(),
                vec![CandidateHit::with_source(
                    "src/main.rs",
                    None,
                    0.7,
                    CandidateSource::Zoekt,
                )],
            )])),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute("how does startup sequence work", Some(500))
            .expect("query");
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.file_path != "docs/startup.md")
        );
        assert!(
            result
                .trace
                .stages
                .iter()
                .all(|stage| stage.stage != RetrievalStageKind::Stage3RepoTextFallback)
        );
    }
}
