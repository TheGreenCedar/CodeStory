use crate::cache::{RetrievalCache, RetrievalCacheKey};
use crate::candidate::{CandidateHit, CandidateSource};
use crate::health::probe_sidecar_health;
use crate::index::query_fingerprint;
use crate::mode::{RetrievalDegradedMode, derive_degraded_mode};
use crate::planner::{PlannedStage, RetrievalStageKind};
use crate::query_features::{QueryFeatures, QueryShape, classify_query};
use crate::ranker::rank_candidates;
use crate::sidecar_search::{SemanticSearchScope, SidecarSearch};
use anyhow::{Result, bail};
use codestory_store::RetrievalIndexManifest;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_scope: Option<SemanticScopeTrace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticScopeTrace {
    pub mode: String,
    pub allowlist_size: usize,
    #[serde(default, skip_serializing_if = "is_false")]
    pub full_fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

const SEMANTIC_SCOPE_MAX_ALLOWLIST_PATHS: usize = 96;
const SEMANTIC_SCOPE_MIN_SUCCESS_HITS: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct QueryResult {
    pub query: String,
    pub features: QueryFeatures,
    pub hits: Vec<CandidateHit>,
    pub trace: QueryTrace,
}

pub struct QueryExecutor<'a> {
    pub sidecars: &'a dyn SidecarSearch,
    pub cache: &'a mut RetrievalCache,
    pub manifest: Option<RetrievalIndexManifest>,
    pub file_roles: HashMap<String, codestory_store::FileRole>,
    pub cancelled: Arc<AtomicBool>,
    /// When set (tests), skips live health probing.
    pub mode_override: Option<RetrievalDegradedMode>,
}

impl<'a> QueryExecutor<'a> {
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
        if let Some(budget) = total_budget_ms {
            plan.total_budget_ms = budget;
        }

        let started = Instant::now();
        let deadline = started + Duration::from_millis(plan.total_budget_ms);
        let mut candidates = Vec::new();
        let mut stage_traces = Vec::new();

        let cancel_reason = self.run_stage_sequence(
            &features,
            &plan.stages,
            &mut candidates,
            &mut stage_traces,
            deadline,
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
            let layout = crate::config::SidecarLayout::from_env();
            let report =
                probe_sidecar_health(&layout, &manifest.project_id, Some(manifest.clone()));
            return derive_degraded_mode(&report.zoekt, &report.qdrant, &report.scip);
        }
        (
            RetrievalDegradedMode::LexicalOnly,
            Some("manifest_missing".into()),
        )
    }

    fn run_stage(
        &self,
        stage: &PlannedStage,
        features: &QueryFeatures,
        anchors: &[CandidateHit],
    ) -> Result<StageRunOutput> {
        let query = &features.raw_query;
        let hits = match stage.kind {
            RetrievalStageKind::Stage0ScipAnchor => {
                self.sidecars.scip_anchor(query, stage.top_k)?
            }
            RetrievalStageKind::Stage1ZoektLexical => {
                self.sidecars.zoekt_search(query, stage.top_k)?
            }
            RetrievalStageKind::Stage1bQdrantSemantic => {
                return self.run_qdrant_semantic_stage(stage, features, anchors);
            }
            RetrievalStageKind::Stage2ScipExpand => {
                self.sidecars.scip_expand(anchors, stage.top_k)?
            }
            RetrievalStageKind::Stage3RepoTextFallback => {
                bail!("repo-text diagnostic stage is unsupported in mandatory sidecar retrieval")
            }
        };
        Ok(StageRunOutput {
            hits,
            semantic_scope: None,
        })
    }

    fn run_qdrant_semantic_stage(
        &self,
        stage: &PlannedStage,
        features: &QueryFeatures,
        anchors: &[CandidateHit],
    ) -> Result<StageRunOutput> {
        let Some(scope) = semantic_candidate_scope(features, anchors) else {
            return Ok(StageRunOutput {
                hits: self
                    .sidecars
                    .qdrant_search(&features.raw_query, stage.top_k)?,
                semantic_scope: Some(SemanticScopeTrace {
                    mode: "full".into(),
                    allowlist_size: 0,
                    full_fallback: false,
                    fallback_reason: None,
                }),
            });
        };

        let allowlist_size = scope.allow_paths.len();
        match self
            .sidecars
            .qdrant_search_scoped(&features.raw_query, stage.top_k, &scope)
        {
            Ok(scoped_hits)
                if scoped_hits.len()
                    >= scoped_semantic_success_floor(stage.top_k, allowlist_size) =>
            {
                Ok(StageRunOutput {
                    hits: scoped_hits,
                    semantic_scope: Some(SemanticScopeTrace {
                        mode: "candidate_allowlist".into(),
                        allowlist_size,
                        full_fallback: false,
                        fallback_reason: None,
                    }),
                })
            }
            Ok(scoped_hits) => {
                let mut hits = scoped_hits;
                let fallback = self
                    .sidecars
                    .qdrant_search(&features.raw_query, stage.top_k)?;
                merge_candidates(&mut hits, fallback);
                Ok(StageRunOutput {
                    hits,
                    semantic_scope: Some(SemanticScopeTrace {
                        mode: "candidate_allowlist".into(),
                        allowlist_size,
                        full_fallback: true,
                        fallback_reason: Some("allowlist_underfilled".into()),
                    }),
                })
            }
            Err(error) => {
                tracing::warn!(
                    "scoped qdrant semantic search failed; falling back to full semantic search: {error}"
                );
                Ok(StageRunOutput {
                    hits: self
                        .sidecars
                        .qdrant_search(&features.raw_query, stage.top_k)?,
                    semantic_scope: Some(SemanticScopeTrace {
                        mode: "candidate_allowlist".into(),
                        allowlist_size,
                        full_fallback: true,
                        fallback_reason: Some("allowlist_query_failed".into()),
                    }),
                })
            }
        }
    }

    fn run_stage_sequence(
        &self,
        features: &QueryFeatures,
        stages: &[PlannedStage],
        candidates: &mut Vec<CandidateHit>,
        stage_traces: &mut Vec<StageTrace>,
        deadline: Instant,
        options: StageSequenceOptions,
    ) -> Result<Option<String>> {
        let mut low_gain_streak = 0u32;
        for stage in stages {
            if self.cancelled.load(Ordering::Relaxed) {
                return Ok(Some("cancelled".into()));
            }
            if Instant::now() >= deadline {
                return Ok(Some("deadline".into()));
            }

            if should_skip_after_exact_symbol_anchor(stage, features, candidates) {
                stage_traces.push(stage_trace(
                    stage,
                    0,
                    0,
                    0.0,
                    StageTraceMeta {
                        cancel_reason: Some("exact_symbol_anchor".into()),
                        ..Default::default()
                    },
                ));
                continue;
            }

            let stage_started = Instant::now();
            let before_score = candidate_mass(candidates);
            let stage_output = self.run_stage(stage, features, candidates)?;
            let (stub_reason, stage_degraded) = stage_stub_metadata(&stage_output.hits);
            let added = merge_candidates(candidates, stage_output.hits);
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
                StageTraceMeta {
                    degraded: stage_degraded,
                    stub_reason,
                    semantic_scope: stage_output.semantic_scope,
                    ..Default::default()
                },
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
        Ok(None)
    }
}

struct StageRunOutput {
    hits: Vec<CandidateHit>,
    semantic_scope: Option<SemanticScopeTrace>,
}

#[derive(Debug, Clone, Copy)]
struct StageSequenceOptions {
    stop_marginal_gain_threshold: Option<f32>,
    stop_after_low_gain_streak: u32,
}

fn stage_trace(
    stage: &PlannedStage,
    elapsed_ms: u64,
    candidates_added: usize,
    marginal_gain: f32,
    meta: StageTraceMeta,
) -> StageTrace {
    StageTrace {
        stage: stage.kind,
        budget_ms: stage.budget_ms,
        elapsed_ms,
        candidates_added,
        marginal_gain,
        cancel_reason: meta.cancel_reason,
        cache_hit: false,
        degraded: meta.degraded,
        stub_reason: meta.stub_reason,
        semantic_scope: meta.semantic_scope,
    }
}

#[derive(Default)]
struct StageTraceMeta {
    cancel_reason: Option<String>,
    degraded: bool,
    stub_reason: Option<String>,
    semantic_scope: Option<SemanticScopeTrace>,
}

fn semantic_candidate_scope(
    features: &QueryFeatures,
    candidates: &[CandidateHit],
) -> Option<SemanticSearchScope> {
    if !matches!(features.shape, QueryShape::Mixed | QueryShape::SymbolLike) {
        return None;
    }

    let mut seen = HashSet::new();
    let mut allow_paths = Vec::new();
    for candidate in candidates {
        if matches!(
            candidate.source,
            CandidateSource::Qdrant | CandidateSource::Legacy
        ) || crate::candidate::is_phantom_sidecar_hit(candidate)
        {
            continue;
        }
        let Some(path) = normalize_allowlist_path(&candidate.file_path) else {
            continue;
        };
        if seen.insert(path.clone()) {
            allow_paths.push(path);
            if allow_paths.len() > SEMANTIC_SCOPE_MAX_ALLOWLIST_PATHS {
                return None;
            }
        }
    }

    (!allow_paths.is_empty()).then_some(SemanticSearchScope { allow_paths })
}

fn normalize_allowlist_path(path: &str) -> Option<String> {
    let normalized = path.trim().replace('\\', "/");
    if normalized.is_empty() || normalized.contains(':') {
        return None;
    }
    Some(normalized.trim_start_matches("./").to_string())
}

fn scoped_semantic_success_floor(stage_top_k: usize, allowlist_size: usize) -> usize {
    stage_top_k
        .min(allowlist_size)
        .clamp(1, SEMANTIC_SCOPE_MIN_SUCCESS_HITS)
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

fn candidate_is_exact_symbol_anchor(query: &str, candidate: &CandidateHit) -> bool {
    if matches!(
        candidate.source,
        CandidateSource::Qdrant | CandidateSource::Legacy
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
        let duplicate = acc.iter().any(|existing| {
            existing.file_path == hit.file_path && existing.symbol_name == hit.symbol_name
        });
        if !duplicate {
            acc.push(hit);
            added += 1;
        }
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
    use crate::sidecar_search::mock::MockSidecarSearch;
    use codestory_store::RetrievalIndexManifest;
    use std::collections::HashMap;
    use std::sync::Mutex;

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
            projection_count: None,
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
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: &mock,
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
            sidecars: &mock,
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
        let mock = MockSidecarSearch {
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
        };
        let mut cache = RetrievalCache::new();
        let manifest = sample_manifest();
        {
            let mut executor = QueryExecutor {
                sidecars: &mock,
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
            sidecars: &mock,
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
            sidecars: &mock,
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
            sidecars: &mock,
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
    fn executor_rejects_cached_hits_when_mode_is_not_full() {
        let mock = MockSidecarSearch::default();
        let mut cache = RetrievalCache::new();
        let manifest = sample_manifest();
        let key = RetrievalCacheKey::from_manifest(&manifest, query_fingerprint("cached-query"));
        cache.insert(key, vec![CandidateHit::lexical_stub("cached.rs", 1.0)]);

        let mut executor = QueryExecutor {
            sidecars: &mock,
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
            sidecars: &mock,
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
    fn executor_scopes_semantic_search_to_lexical_candidates_for_mixed_queries() {
        let query = "Explain how StartupFlow handles requests";
        let mock = MockSidecarSearch {
            zoekt: Mutex::new(HashMap::from([(
                query.into(),
                vec![CandidateHit::with_source(
                    "src/startup.rs",
                    Some("StartupFlow".into()),
                    0.9,
                    CandidateSource::Zoekt,
                )],
            )])),
            qdrant_scoped: Mutex::new(HashMap::from([(
                query.into(),
                vec![CandidateHit::with_source(
                    "src/startup.rs",
                    Some("StartupFlow".into()),
                    0.88,
                    CandidateSource::Qdrant,
                )],
            )])),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: &mock,
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };

        let result = executor.execute(query, Some(800)).expect("query");

        assert_eq!(
            *mock.qdrant_full_calls.lock().expect("full call lock"),
            0,
            "scoped semantic hit should avoid full Qdrant scan"
        );
        let scoped_calls = mock.qdrant_scoped_calls.lock().expect("scoped call lock");
        assert_eq!(scoped_calls.len(), 1);
        assert_eq!(scoped_calls[0].allow_paths, vec!["src/startup.rs"]);
        let qdrant_trace = result
            .trace
            .stages
            .iter()
            .find(|stage| stage.stage == RetrievalStageKind::Stage1bQdrantSemantic)
            .expect("qdrant stage");
        assert_eq!(
            qdrant_trace
                .semantic_scope
                .as_ref()
                .map(|scope| scope.full_fallback),
            Some(false)
        );
    }

    #[test]
    fn executor_falls_back_to_full_semantic_when_allowlist_underfills() {
        let query = "Explain how StartupFlow handles requests";
        let mock = MockSidecarSearch {
            zoekt: Mutex::new(HashMap::from([(
                query.into(),
                vec![CandidateHit::with_source(
                    "src/startup.rs",
                    Some("StartupFlow".into()),
                    0.9,
                    CandidateSource::Zoekt,
                )],
            )])),
            qdrant_scoped: Mutex::new(HashMap::from([(query.into(), Vec::new())])),
            qdrant: Mutex::new(HashMap::from([(
                query.into(),
                vec![CandidateHit::with_source(
                    "src/request_dispatch.rs",
                    Some("RequestDispatch".into()),
                    0.93,
                    CandidateSource::Qdrant,
                )],
            )])),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: &mock,
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: HashMap::new(),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };

        let result = executor.execute(query, Some(800)).expect("query");

        assert_eq!(*mock.qdrant_full_calls.lock().expect("full call lock"), 1);
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.file_path == "src/request_dispatch.rs"),
            "full semantic fallback must preserve recall: {:?}",
            result.hits
        );
        let qdrant_trace = result
            .trace
            .stages
            .iter()
            .find(|stage| stage.stage == RetrievalStageKind::Stage1bQdrantSemantic)
            .expect("qdrant stage");
        let semantic_scope = qdrant_trace
            .semantic_scope
            .as_ref()
            .expect("semantic scope trace");
        assert!(semantic_scope.full_fallback);
        assert_eq!(
            semantic_scope.fallback_reason.as_deref(),
            Some("allowlist_underfilled")
        );
    }

    #[test]
    fn executor_respects_cancellation() {
        let mock = MockSidecarSearch::default();
        let mut cache = RetrievalCache::new();
        let cancelled = cancellation_flag();
        cancelled.store(true, Ordering::Relaxed);
        let mut executor = QueryExecutor {
            sidecars: &mock,
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
            sidecars: &mock,
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
            sidecars: &mock,
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
            sidecars: &mock,
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
