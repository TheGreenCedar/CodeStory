//! Mandatory sidecar retrieval integration for packet and agent ask paths.

use crate::agent::nucleo_policy::with_sidecar_primary_retrieval;
use crate::agent::packet_evidence::decorate_search_hit_evidence;
use crate::{AppController, HybridSearchScoredHit};
use anyhow::Error as AnyhowError;
use codestory_contracts::api::{
    ApiError, PacketSidecarQueryDiagnosticDto, RetrievalCandidateResolutionCountDto,
    RetrievalCandidateSummaryDto, RetrievalScoreBreakdownDto, RetrievalShadowDto,
    RetrievalStageTimingDto, SearchHit,
};
use codestory_contracts::graph::{NodeId as CoreNodeId, NodeKind};
use codestory_retrieval::{
    CandidateHit, CandidateSource, QueryBatchItem, QueryBatchRequest, QueryRequest, QueryResult,
    QueryTrace, execute_retrieval_query_with_cache,
    execute_strict_retrieval_query_batch_with_cache, is_phantom_sidecar_hit,
    sidecar_project_id_for_root, strict_sidecar_status,
};
use codestory_store::Store;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

const DEFAULT_SIDECAR_BUDGET_MS: u64 = 1_000;
const DEFAULT_PACKET_BATCH_BUDGET_MS: u64 = 18_000;
const MAX_PACKET_BATCH_BUDGET_MS: u64 = 120_000;
const MAX_SHADOW_CANDIDATES: usize = 20;
const MAX_SHADOW_WOULD_RANK: usize = 10;
pub(crate) const RETRIEVAL_VERSION_SIDECAR: &str = "sidecar";

const RETRIEVAL_ENV: &str = "CODESTORY_RETRIEVAL";
const RETRIEVAL_SHADOW_ENV: &str = "CODESTORY_RETRIEVAL_SHADOW";

fn env_flag_enabled(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn env_flag_disabled(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

fn env_bool_override(key: &str) -> Option<bool> {
    std::env::var(key).ok().map(|value| {
        if env_flag_disabled(&value) {
            false
        } else {
            env_flag_enabled(&value)
        }
    })
}

fn retrieval_env_override() -> Option<bool> {
    env_bool_override(RETRIEVAL_ENV)
}

fn shadow_env_enabled() -> Option<bool> {
    if let Ok(value) = std::env::var(RETRIEVAL_SHADOW_ENV) {
        return Some(!env_flag_disabled(&value));
    }
    None
}

/// Whether sidecar retrieval should serve packet/agent results.
///
/// - `CODESTORY_RETRIEVAL=1` forces a sidecar-primary attempt when the agent sidecar is `full`.
/// - `CODESTORY_RETRIEVAL=0` is unsupported; packet paths fail closed.
/// - Unset: sidecar primary when the manifest exists and the agent sidecar is healthy.
pub(crate) fn sidecar_retrieval_primary_enabled(controller: &AppController) -> bool {
    match retrieval_env_override() {
        Some(false) => {
            tracing::error!("CODESTORY_RETRIEVAL=0 is unsupported; sidecar retrieval is mandatory");
            false
        }
        Some(true) => {
            sidecar_retrieval_eligible(controller) && sidecar_mode_is_required_full(controller)
        }
        None => {
            // Default product path: sidecar primary only from full agent-scoped retrieval.
            let auto_on =
                sidecar_retrieval_eligible(controller) && sidecar_mode_is_required_full(controller);
            if auto_on {
                tracing::info!(
                    "retrieval primary auto-on (unset CODESTORY_RETRIEVAL; agent sidecar full)"
                );
            }
            auto_on
        }
    }
}

pub(crate) fn sidecar_retrieval_unavailable_reason(controller: &AppController) -> Option<String> {
    if retrieval_env_override() == Some(false) {
        return Some("CODESTORY_RETRIEVAL=0 is unsupported; sidecar retrieval is mandatory".into());
    }
    if sidecar_retrieval_primary_enabled(controller) {
        return None;
    }
    let Ok(project_root) = controller.require_project_root() else {
        return Some("sidecar retrieval primary requires an open project".into());
    };
    let Ok(storage_path) = controller.require_storage_path() else {
        return Some("sidecar retrieval primary requires an index storage path".into());
    };
    let status = sidecar_mode_status_for_project(&project_root, &storage_path);
    let reason = status
        .degraded_reason
        .map(|reason| format!("; reason={reason}"))
        .unwrap_or_default();
    let profile = status.profile.as_deref().unwrap_or("unknown");
    Some(format!(
        "sidecar retrieval primary is unavailable or degraded (profile={profile} mode={}); expected profile=agent mode=full{reason}",
        status.mode
    ))
}

pub(crate) fn sidecar_retrieval_unavailable_error(
    controller: &AppController,
    reason: impl Into<String>,
) -> ApiError {
    let project = controller
        .require_project_root()
        .ok()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "<project>".to_string());
    ApiError::retrieval_unavailable(
        reason,
        project.clone(),
        sidecar_retrieval_recovery_commands(&project),
    )
}

fn sidecar_retrieval_recovery_commands(project: &str) -> Vec<String> {
    let project = quote_cli_arg(project);
    vec![
        format!("codestory-cli ready --goal agent --repair --project {project} --format json"),
        format!("codestory-cli retrieval status --project {project} --format json"),
        format!("codestory-cli doctor --project {project} --format markdown"),
    ]
}

fn quote_cli_arg(value: &str) -> String {
    let normalized = clean_cli_path(value);
    if normalized
        .chars()
        .any(|ch| matches!(ch, '$' | '`' | '\'' | '"'))
    {
        quote_shell_single_quoted_arg(&normalized)
    } else {
        format!("\"{}\"", normalized.replace('"', "\\\""))
    }
}

#[cfg(windows)]
fn quote_shell_single_quoted_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(not(windows))]
fn quote_shell_single_quoted_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn clean_cli_path(value: &str) -> String {
    let mut path = value.replace('\\', "/");
    if let Some(stripped) = path.strip_prefix("//?/UNC/") {
        path = format!("//{stripped}");
    } else if path.starts_with("//?/") {
        path = path[4..].to_string();
    }
    path
}

pub(crate) fn shadow_retrieval_enabled() -> bool {
    if retrieval_env_override() == Some(true) {
        return false;
    }
    shadow_env_enabled().unwrap_or(true)
}

pub(crate) fn sidecar_retrieval_eligible(controller: &AppController) -> bool {
    let Ok(project_root) = controller.require_project_root() else {
        return false;
    };
    let Ok(storage_path) = controller.require_storage_path() else {
        return false;
    };
    retrieval_manifest_exists(&storage_path, &project_root)
}

pub(crate) fn sidecar_retrieval_blocks_nucleo_supplement(
    controller: &AppController,
    served_hit_count: usize,
) -> bool {
    served_hit_count > 0 && sidecar_retrieval_primary_enabled(controller)
}

fn retrieval_manifest_exists(storage_path: &Path, project_root: &Path) -> bool {
    if !storage_path.exists() {
        return false;
    }
    let Ok(storage) = Store::open(storage_path) else {
        return false;
    };
    let project_id = sidecar_project_id_for_root(project_root);
    storage
        .get_retrieval_index_manifest(&project_id)
        .ok()
        .flatten()
        .is_some()
}

fn sidecar_mode_is_required_full(controller: &AppController) -> bool {
    let Ok(project_root) = controller.require_project_root() else {
        return false;
    };
    let Ok(storage_path) = controller.require_storage_path() else {
        return false;
    };
    sidecar_status_can_serve_primary(&sidecar_mode_status_for_project(
        &project_root,
        &storage_path,
    ))
}

fn sidecar_mode_can_serve_primary(mode: &str) -> bool {
    mode == "full"
}

fn sidecar_status_can_serve_primary(status: &SidecarModeStatus) -> bool {
    status.profile.as_deref() == Some("agent") && sidecar_mode_can_serve_primary(&status.mode)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarModeStatus {
    profile: Option<String>,
    mode: String,
    degraded_reason: Option<String>,
}

fn sidecar_mode_status_for_project(project_root: &Path, storage_path: &Path) -> SidecarModeStatus {
    match strict_sidecar_status(project_root, Some(storage_path)) {
        Ok(report) => SidecarModeStatus {
            profile: report
                .ownership
                .as_ref()
                .map(|ownership| ownership.profile.clone()),
            mode: report.retrieval_mode,
            degraded_reason: report.degraded_reason,
        },
        Err(error) => SidecarModeStatus {
            profile: None,
            mode: "unavailable".into(),
            degraded_reason: Some(format!("sidecar_status_error: {error}")),
        },
    }
}

pub(crate) fn sidecar_result_rejection_reason(
    query_result: &QueryResult,
    resolved_hits: &[SearchHit],
) -> Option<String> {
    if !sidecar_mode_can_serve_primary(&query_result.trace.retrieval_mode) {
        return Some(format!(
            "sidecar retrieval mode `{}` is not eligible for primary results",
            query_result.trace.retrieval_mode
        ));
    }
    if !query_result.hits.is_empty() && resolved_hits.is_empty() {
        return Some("sidecar retrieval candidates did not resolve to indexed symbols".into());
    }
    None
}

pub(crate) fn sidecar_budget_ms(latency_budget_ms: Option<u32>) -> u64 {
    latency_budget_ms
        .map(|ms| u64::from(ms).min(DEFAULT_SIDECAR_BUDGET_MS))
        .unwrap_or(DEFAULT_SIDECAR_BUDGET_MS)
        .max(100)
}

fn sidecar_packet_batch_budget_ms(latency_budget_ms: Option<u32>) -> u64 {
    latency_budget_ms
        .map(u64::from)
        .unwrap_or(DEFAULT_PACKET_BATCH_BUDGET_MS)
        .clamp(100, MAX_PACKET_BATCH_BUDGET_MS)
}

pub(crate) fn run_sidecar_query(
    controller: &AppController,
    query: &str,
    latency_budget_ms: Option<u32>,
) -> Result<QueryResult, AnyhowError> {
    let project_root = controller
        .require_project_root()
        .map_err(|error| anyhow::anyhow!("project root required: {}", error.message))?;
    let storage_path = controller
        .require_storage_path()
        .map_err(|error| anyhow::anyhow!("storage path required: {}", error.message))?;
    let mut cache = controller.sidecar_query_cache.lock();
    execute_retrieval_query_with_cache(
        QueryRequest {
            project_root: &project_root,
            storage_path: &storage_path,
            query,
            budget_ms: Some(sidecar_budget_ms(latency_budget_ms)),
            cancelled: None,
        },
        &mut cache,
    )
}

pub(crate) fn run_sidecar_query_batch(
    controller: &AppController,
    queries: &[(String, u64)],
) -> Result<Vec<QueryResult>, AnyhowError> {
    let project_root = controller
        .require_project_root()
        .map_err(|error| anyhow::anyhow!("project root required: {}", error.message))?;
    let storage_path = controller
        .require_storage_path()
        .map_err(|error| anyhow::anyhow!("storage path required: {}", error.message))?;
    let batch_items = queries
        .iter()
        .map(|(query, budget_ms)| QueryBatchItem {
            query,
            budget_ms: Some(*budget_ms),
        })
        .collect::<Vec<_>>();
    let mut cache = controller.sidecar_query_cache.lock();
    execute_strict_retrieval_query_batch_with_cache(
        QueryBatchRequest {
            project_root: &project_root,
            storage_path: &storage_path,
            queries: &batch_items,
            cancelled: None,
        },
        &mut cache,
    )
}

pub(crate) fn maybe_run_retrieval_shadow(
    controller: &AppController,
    question: &str,
    latency_budget_ms: Option<u32>,
) -> Option<RetrievalShadowDto> {
    if !shadow_retrieval_enabled() || sidecar_retrieval_primary_enabled(controller) {
        return None;
    }
    if !sidecar_retrieval_eligible(controller) {
        return None;
    }

    match run_sidecar_query(controller, question, latency_budget_ms) {
        Ok(query_result) => Some(shadow_from_query_result(query_result)),
        Err(error) => Some(RetrievalShadowDto {
            retrieval_mode: "error".into(),
            degraded_reason: Some("shadow_invoke_failed".into()),
            retrieval_total_ms: 0,
            total_budget_ms: Some(sidecar_budget_ms(latency_budget_ms).min(u32::MAX as u64) as u32),
            cancel_reason: None,
            cache_hit: false,
            stage_timings: Vec::new(),
            candidates: Vec::new(),
            would_rank: Vec::new(),
            error: Some(error.to_string()),
            candidate_count: 0,
            resolved_hit_count: 0,
            unresolved_candidate_count: 0,
            diagnostic_only: false,
            candidate_resolution_counts: Vec::new(),
        }),
    }
}

pub(crate) enum SidecarPrimarySearchOutcome {
    Rejected {
        shadow: RetrievalShadowDto,
        reason: String,
    },
    Unavailable {
        reason: String,
    },
    Served {
        hits: Vec<SearchHit>,
        scored_hits: Vec<HybridSearchScoredHit>,
        shadow: RetrievalShadowDto,
    },
}

pub(crate) fn try_sidecar_primary_search(
    controller: &AppController,
    prompt: &str,
    max_results: usize,
    latency_budget_ms: Option<u32>,
) -> Option<SidecarPrimarySearchOutcome> {
    try_sidecar_primary_search_inner_with_query(
        controller,
        prompt,
        max_results,
        latency_budget_ms,
        run_sidecar_query,
    )
}

fn try_sidecar_primary_search_inner_with_query(
    controller: &AppController,
    prompt: &str,
    max_results: usize,
    latency_budget_ms: Option<u32>,
    mut run_query: impl FnMut(&AppController, &str, Option<u32>) -> Result<QueryResult, AnyhowError>,
) -> Option<SidecarPrimarySearchOutcome> {
    if !sidecar_retrieval_primary_enabled(controller) {
        return sidecar_retrieval_unavailable_reason(controller)
            .map(|reason| SidecarPrimarySearchOutcome::Unavailable { reason });
    }

    let query_result = match run_query(controller, prompt, latency_budget_ms) {
        Ok(result) => result,
        Err(error) => {
            return Some(SidecarPrimarySearchOutcome::Unavailable {
                reason: format!("sidecar retrieval primary unavailable: {error}"),
            });
        }
    };

    Some(sidecar_primary_search_outcome_from_query_result(
        controller,
        query_result,
        max_results,
    ))
}

fn sidecar_primary_search_outcome_from_query_result(
    controller: &AppController,
    query_result: QueryResult,
    max_results: usize,
) -> SidecarPrimarySearchOutcome {
    let candidate_count = query_result.hits.len();
    let resolved_hits = match resolve_sidecar_candidates_to_search_hits(
        controller,
        &query_result.hits,
        max_results,
    ) {
        Ok(hits) => hits,
        Err(error) => {
            return SidecarPrimarySearchOutcome::Unavailable {
                reason: format!(
                    "sidecar retrieval primary unavailable: candidate resolution failed: {}",
                    error.message
                ),
            };
        }
    };
    let shadow = shadow_from_query_result_with_candidate_admission_diagnostics(
        controller,
        query_result.clone(),
        candidate_count,
        resolved_hits.len(),
        &resolved_hits,
        &resolved_hits,
    );

    if let Some(reason) = sidecar_result_rejection_reason(&query_result, &resolved_hits) {
        let diagnostic = sidecar_rejection_diagnostic(controller, &query_result, &resolved_hits, 5);
        let reason = format!("{reason}; {diagnostic}");
        return SidecarPrimarySearchOutcome::Rejected { shadow, reason };
    }

    let hits = resolved_hits;

    let scored_hits = hits
        .iter()
        .map(|hit| HybridSearchScoredHit {
            hit: hit.clone(),
            lexical_score: hit.score,
            semantic_score: 0.0,
            graph_score: 0.0,
            total_score: hit.score,
        })
        .collect();

    SidecarPrimarySearchOutcome::Served {
        hits,
        scored_hits,
        shadow,
    }
}

pub(crate) fn search_sidecar_packet_batch(
    controller: &AppController,
    queries: &[(String, usize)],
    latency_budget_ms: Option<u32>,
) -> Result<SidecarPacketBatchOutcome, ApiError> {
    with_sidecar_primary_retrieval(|| {
        search_sidecar_packet_batch_inner(controller, queries, latency_budget_ms)
    })
}

pub(crate) struct SidecarPacketBatchOutcome {
    pub results: Vec<(String, Vec<SearchHit>)>,
    pub diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
}

struct SidecarCandidateResolutionOutcome {
    resolved_hits: Vec<SearchHit>,
    attempted_candidate_count: usize,
    unresolved_candidate_count: usize,
}

fn packet_sidecar_query_diagnostic(
    query_result: &QueryResult,
    resolution: &SidecarCandidateResolutionOutcome,
    sidecar_query_ms: u32,
    candidate_resolution_ms: u32,
    batch_query_wall_ms: u32,
) -> PacketSidecarQueryDiagnosticDto {
    let total_elapsed_ms = sidecar_query_ms.saturating_add(candidate_resolution_ms);
    let stage_timings = retrieval_stage_timings(&query_result.trace);
    let sidecar_stage_total_ms = stage_timings
        .iter()
        .map(|stage| stage.elapsed_ms)
        .fold(0_u32, u32::saturating_add);
    PacketSidecarQueryDiagnosticDto {
        query: query_result.query.clone(),
        retrieval_mode: query_result.trace.retrieval_mode.clone(),
        sidecar_query_ms: Some(sidecar_query_ms),
        candidate_resolution_ms: Some(candidate_resolution_ms),
        total_elapsed_ms: Some(total_elapsed_ms),
        sidecar_stage_count: u32::try_from(stage_timings.len()).unwrap_or(u32::MAX),
        sidecar_stage_total_ms: Some(sidecar_stage_total_ms),
        batch_query_wall_ms: Some(batch_query_wall_ms),
        candidate_count: u32::try_from(resolution.attempted_candidate_count).unwrap_or(u32::MAX),
        resolved_hit_count: u32::try_from(resolution.resolved_hits.len()).unwrap_or(u32::MAX),
        unresolved_candidate_count: u32::try_from(resolution.unresolved_candidate_count)
            .unwrap_or(u32::MAX),
        diagnostic: (resolution.unresolved_candidate_count > 0)
            .then(|| "sidecar candidates did not all resolve to indexed symbols".to_string()),
    }
}

fn search_sidecar_packet_batch_inner(
    controller: &AppController,
    queries: &[(String, usize)],
    latency_budget_ms: Option<u32>,
) -> Result<SidecarPacketBatchOutcome, ApiError> {
    search_sidecar_packet_batch_inner_with_query_batch(
        controller,
        queries,
        latency_budget_ms,
        run_sidecar_query_batch,
    )
}

fn search_sidecar_packet_batch_inner_with_query_batch(
    controller: &AppController,
    queries: &[(String, usize)],
    latency_budget_ms: Option<u32>,
    mut run_query_batch: impl FnMut(
        &AppController,
        &[(String, u64)],
    ) -> Result<Vec<QueryResult>, AnyhowError>,
) -> Result<SidecarPacketBatchOutcome, ApiError> {
    let per_query_budget = sidecar_packet_batch_budget_ms(latency_budget_ms)
        .checked_div(queries.len().max(1) as u64)
        .unwrap_or(100)
        .max(100);
    let batch_queries = queries
        .iter()
        .map(|(query, _)| (query.clone(), per_query_budget))
        .collect::<Vec<_>>();
    let batch_started_at = Instant::now();
    let query_results = run_query_batch(controller, &batch_queries).map_err(|error| {
        sidecar_retrieval_unavailable_error(
            controller,
            format!("sidecar retrieval batch query failed: {error}"),
        )
    })?;
    let batch_query_wall_ms = clamp_elapsed_ms(batch_started_at);
    if query_results.len() != queries.len() {
        return Err(sidecar_retrieval_unavailable_error(
            controller,
            format!(
                "sidecar retrieval batch returned {} results for {} queries",
                query_results.len(),
                queries.len()
            ),
        ));
    }
    let mut results = Vec::with_capacity(queries.len());
    let mut diagnostics = Vec::with_capacity(queries.len());
    for ((query, max_results), query_result) in queries.iter().zip(query_results) {
        if query_result.query != *query {
            return Err(sidecar_retrieval_unavailable_error(
                controller,
                format!(
                    "sidecar retrieval batch query mismatch expected `{}` got `{}`",
                    query, query_result.query
                ),
            ));
        }
        let sidecar_query_ms = u32::try_from(query_result.trace.elapsed_ms).unwrap_or(u32::MAX);
        let max_results = (*max_results).clamp(1, 50);
        let resolution_started_at = Instant::now();
        let resolution =
            resolve_sidecar_candidates_with_stats(controller, &query_result.hits, max_results)
                .map_err(|error| {
                    sidecar_retrieval_unavailable_error(
                        controller,
                        format!(
                            "sidecar retrieval rejected packet batch query `{query}`: candidate resolution failed: {}",
                            error.message
                        ),
                    )
                })?;
        let candidate_resolution_ms = clamp_elapsed_ms(resolution_started_at);
        diagnostics.push(packet_sidecar_query_diagnostic(
            &query_result,
            &resolution,
            sidecar_query_ms,
            candidate_resolution_ms,
            batch_query_wall_ms,
        ));
        let resolved_hits = resolution.resolved_hits;
        if let Some(reason) = sidecar_packet_batch_rejection_reason(&query_result, &resolved_hits) {
            let diagnostic =
                sidecar_rejection_diagnostic(controller, &query_result, &resolved_hits, 5);
            return Err(sidecar_retrieval_unavailable_error(
                controller,
                format!(
                    "sidecar retrieval rejected packet batch query `{query}`: {reason}; {diagnostic}"
                ),
            ));
        }
        results.push((query.clone(), resolved_hits));
    }
    Ok(SidecarPacketBatchOutcome {
        results,
        diagnostics,
    })
}

fn clamp_elapsed_ms(started_at: Instant) -> u32 {
    started_at.elapsed().as_millis().min(u32::MAX as u128) as u32
}

fn sidecar_packet_batch_rejection_reason(
    query_result: &QueryResult,
    _resolved_hits: &[SearchHit],
) -> Option<String> {
    if !sidecar_mode_can_serve_primary(&query_result.trace.retrieval_mode) {
        return Some(format!(
            "sidecar retrieval mode `{}` is not eligible for packet batch results",
            query_result.trace.retrieval_mode
        ));
    }
    None
}

pub(crate) fn packet_batch_should_use_sidecar(controller: &AppController) -> bool {
    sidecar_retrieval_primary_enabled(controller)
}

pub(crate) fn shadow_from_query_result(result: QueryResult) -> RetrievalShadowDto {
    shadow_from_query_result_with_counts(result, 0, 0)
}

pub(crate) fn shadow_from_query_result_with_candidate_admission_diagnostics(
    controller: &AppController,
    result: QueryResult,
    candidate_count: usize,
    resolved_hit_count: usize,
    search_hits: &[SearchHit],
    final_hits: &[SearchHit],
) -> RetrievalShadowDto {
    let resolution_labels = sidecar_candidate_resolution_labels(controller, &result.hits);
    let admission_labels = sidecar_candidate_admission_labels(
        controller,
        &result.hits,
        &resolution_labels,
        search_hits,
        final_hits,
    );
    shadow_from_query_result_with_counts_and_resolution_labels(
        result,
        candidate_count,
        resolved_hit_count,
        &resolution_labels,
        &admission_labels,
    )
}

pub(crate) fn sidecar_rejection_diagnostic(
    controller: &AppController,
    query_result: &QueryResult,
    resolved_hits: &[SearchHit],
    max_candidates: usize,
) -> String {
    let project_root = controller.require_project_root().ok();
    let storage = controller.open_storage().ok();
    let node_names = controller.state.lock().node_names.clone();
    let candidate_summaries: Vec<String> = query_result
        .hits
        .iter()
        .take(max_candidates)
        .enumerate()
        .map(|(index, candidate)| {
            let resolution = candidate_resolution_label(
                project_root.as_deref(),
                storage.as_ref(),
                &node_names,
                candidate,
            );
            let symbol = candidate
                .symbol_name
                .as_deref()
                .filter(|symbol| !symbol.trim().is_empty())
                .unwrap_or("-");
            let line = candidate
                .start_line
                .map(|line| format!(":{line}"))
                .unwrap_or_default();
            format!(
                "#{rank} {source} {path}{line} symbol={symbol} score={score:.3} resolution={resolution}",
                rank = index + 1,
                source = candidate_source_label(candidate.source),
                path = candidate.file_path,
                score = candidate.score,
            )
        })
        .collect();
    let stage_summaries: Vec<String> = query_result
        .trace
        .stages
        .iter()
        .map(|stage| {
            let cancel = stage
                .cancel_reason
                .as_deref()
                .map(|reason| format!(" cancel={reason}"))
                .unwrap_or_default();
            format!(
                "{} added={} elapsed_ms={}{}",
                stage.stage.label(),
                stage.candidates_added,
                stage.elapsed_ms,
                cancel,
            )
        })
        .collect();
    format!(
        "sidecar_trace mode={} elapsed_ms={} candidates={} resolved_hits={} stages=[{}] top_candidates=[{}]",
        query_result.trace.retrieval_mode,
        query_result.trace.elapsed_ms,
        query_result.hits.len(),
        resolved_hits.len(),
        stage_summaries.join("; "),
        candidate_summaries.join("; "),
    )
}

fn sidecar_candidate_resolution_labels(
    controller: &AppController,
    candidates: &[CandidateHit],
) -> Vec<String> {
    let project_root = controller.require_project_root().ok();
    let storage = controller.open_storage().ok();
    let node_names = controller.state.lock().node_names.clone();
    candidates
        .iter()
        .map(|candidate| {
            candidate_resolution_label(
                project_root.as_deref(),
                storage.as_ref(),
                &node_names,
                candidate,
            )
            .to_string()
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarCandidateAdmissionLabel {
    admission_status: String,
    loss_reason: Option<String>,
    resolved_node_id: Option<String>,
    search_hit_rank: Option<u32>,
    final_rank: Option<u32>,
}

fn sidecar_candidate_admission_labels(
    controller: &AppController,
    candidates: &[CandidateHit],
    resolution_labels: &[String],
    search_hits: &[SearchHit],
    final_hits: &[SearchHit],
) -> Vec<SidecarCandidateAdmissionLabel> {
    let project_root = controller.require_project_root().ok();
    let storage = controller.open_storage().ok();
    let node_names = controller.state.lock().node_names.clone();
    let search_nodes = ranked_hit_nodes(search_hits);
    let search_paths = project_root
        .as_deref()
        .map(|root| ranked_hit_paths(root, search_hits))
        .unwrap_or_default();
    let final_nodes = ranked_hit_nodes(final_hits);
    let final_paths = project_root
        .as_deref()
        .map(|root| ranked_hit_paths(root, final_hits))
        .unwrap_or_default();

    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let resolution = resolution_labels
                .get(index)
                .map(String::as_str)
                .unwrap_or("unlabeled");
            if resolution != "resolved" {
                return SidecarCandidateAdmissionLabel {
                    admission_status: "unresolved".to_string(),
                    loss_reason: Some(resolution.to_string()),
                    resolved_node_id: None,
                    search_hit_rank: None,
                    final_rank: None,
                };
            }
            let Some(project_root) = project_root.as_deref() else {
                return SidecarCandidateAdmissionLabel {
                    admission_status: "rejected".to_string(),
                    loss_reason: Some("project_unavailable".to_string()),
                    resolved_node_id: None,
                    search_hit_rank: None,
                    final_rank: None,
                };
            };
            let rel_path = normalize_repo_relative_path(project_root, &candidate.file_path);
            let resolved_node_id = storage.as_ref().and_then(|storage| {
                resolve_candidate_node_id(storage, &node_names, project_root, &rel_path, candidate)
            });
            let resolved_node_id_text = resolved_node_id.map(|node_id| node_id.0.to_string());
            let search_hit_rank = resolved_node_id_text
                .as_deref()
                .and_then(|node_id| search_nodes.get(node_id).copied())
                .or_else(|| search_paths.get(&rel_path).copied());
            let final_rank = resolved_node_id_text
                .as_deref()
                .and_then(|node_id| final_nodes.get(node_id).copied())
                .or_else(|| final_paths.get(&rel_path).copied());
            if let Some(final_rank) = final_rank {
                SidecarCandidateAdmissionLabel {
                    admission_status: "admitted".to_string(),
                    loss_reason: None,
                    resolved_node_id: resolved_node_id_text,
                    search_hit_rank,
                    final_rank: Some(final_rank),
                }
            } else {
                SidecarCandidateAdmissionLabel {
                    admission_status: "rejected".to_string(),
                    loss_reason: Some(
                        if search_hit_rank.is_some() {
                            "post_final_truncation"
                        } else {
                            "not_in_resolved_search_window"
                        }
                        .to_string(),
                    ),
                    resolved_node_id: resolved_node_id_text,
                    search_hit_rank,
                    final_rank: None,
                }
            }
        })
        .collect()
}

fn ranked_hit_nodes(hits: &[SearchHit]) -> HashMap<String, u32> {
    hits.iter()
        .enumerate()
        .map(|(rank, hit)| {
            (
                hit.node_id.0.clone(),
                u32::try_from(rank + 1).unwrap_or(u32::MAX),
            )
        })
        .collect()
}

fn ranked_hit_paths(project_root: &Path, hits: &[SearchHit]) -> HashMap<String, u32> {
    hits.iter()
        .enumerate()
        .filter_map(|(rank, hit)| {
            hit.file_path.as_deref().map(|path| {
                (
                    normalize_repo_relative_path(project_root, path),
                    u32::try_from(rank + 1).unwrap_or(u32::MAX),
                )
            })
        })
        .collect()
}

fn candidate_resolution_label(
    project_root: Option<&Path>,
    storage: Option<&Store>,
    node_names: &HashMap<CoreNodeId, String>,
    candidate: &CandidateHit,
) -> &'static str {
    if is_phantom_sidecar_hit(candidate) {
        return "phantom_hit";
    }
    let Some(project_root) = project_root else {
        return "project_unavailable";
    };
    let rel_path = normalize_repo_relative_path(project_root, &candidate.file_path);
    if !candidate_path_resolvable(project_root, &candidate.file_path) {
        return "path_unresolvable";
    }
    let Some(storage) = storage else {
        return "storage_unavailable";
    };
    let Some(node_id) =
        resolve_candidate_node_id(storage, node_names, project_root, &rel_path, candidate)
    else {
        return "node_unresolved";
    };
    match storage.get_node(node_id) {
        Ok(Some(node)) if node.kind != NodeKind::UNKNOWN => "resolved",
        Ok(Some(_)) => "unknown_node",
        Ok(None) => "node_missing",
        Err(_) => "node_load_error",
    }
}

pub(crate) fn shadow_from_query_result_with_counts(
    result: QueryResult,
    candidate_count: usize,
    resolved_hit_count: usize,
) -> RetrievalShadowDto {
    shadow_from_query_result_with_counts_and_resolution_labels(
        result,
        candidate_count,
        resolved_hit_count,
        &[],
        &[],
    )
}

fn build_candidate_resolution_counts(
    resolution_labels: &[String],
) -> Vec<RetrievalCandidateResolutionCountDto> {
    let mut counts = BTreeMap::new();
    for label in resolution_labels {
        *counts.entry(label.clone()).or_insert(0_u32) += 1;
    }
    counts
        .into_iter()
        .map(|(resolution, count)| RetrievalCandidateResolutionCountDto { resolution, count })
        .collect()
}

fn shadow_from_query_result_with_counts_and_resolution_labels(
    result: QueryResult,
    candidate_count: usize,
    resolved_hit_count: usize,
    resolution_labels: &[String],
    admission_labels: &[SidecarCandidateAdmissionLabel],
) -> RetrievalShadowDto {
    let trace = &result.trace;
    let stage_timings = retrieval_stage_timings(trace);

    let candidates = result
        .hits
        .iter()
        .take(MAX_SHADOW_CANDIDATES)
        .enumerate()
        .map(|(index, hit)| RetrievalCandidateSummaryDto {
            rank: u32::try_from(index + 1).unwrap_or(u32::MAX),
            file_path: hit.file_path.clone(),
            line: hit.start_line,
            symbol_name: hit.symbol_name.clone(),
            score: hit.score,
            source: candidate_source_label(hit.source),
            resolution: resolution_labels.get(index).cloned(),
            admission_status: admission_labels
                .get(index)
                .map(|label| label.admission_status.clone()),
            loss_reason: admission_labels
                .get(index)
                .and_then(|label| label.loss_reason.clone()),
            resolved_node_id: admission_labels
                .get(index)
                .and_then(|label| label.resolved_node_id.clone()),
            search_hit_rank: admission_labels
                .get(index)
                .and_then(|label| label.search_hit_rank),
            final_rank: admission_labels
                .get(index)
                .and_then(|label| label.final_rank),
        })
        .collect();

    let would_rank = result
        .hits
        .iter()
        .take(MAX_SHADOW_WOULD_RANK)
        .map(|hit| hit.file_path.clone())
        .collect();

    let candidate_resolution_counts = build_candidate_resolution_counts(resolution_labels);
    let effective_candidate_count = candidate_count.max(result.hits.len());
    let unresolved_candidate_count = if resolution_labels.is_empty() {
        effective_candidate_count.saturating_sub(resolved_hit_count)
    } else {
        resolution_labels
            .iter()
            .filter(|label| label.as_str() != "resolved")
            .count()
    };
    let diagnostic_only = unresolved_candidates_are_diagnostic_only(
        &result.hits,
        resolution_labels,
        unresolved_candidate_count,
    );

    RetrievalShadowDto {
        retrieval_mode: trace.retrieval_mode.clone(),
        degraded_reason: trace.degraded_reason.clone(),
        retrieval_total_ms: u32::try_from(trace.elapsed_ms).unwrap_or(u32::MAX),
        total_budget_ms: u32::try_from(trace.total_budget_ms).ok(),
        cancel_reason: trace.cancel_reason.clone(),
        cache_hit: trace.cache_hit,
        stage_timings,
        candidates,
        would_rank,
        error: None,
        candidate_count: u32::try_from(effective_candidate_count).unwrap_or(u32::MAX),
        resolved_hit_count: u32::try_from(resolved_hit_count).unwrap_or(u32::MAX),
        unresolved_candidate_count: u32::try_from(unresolved_candidate_count).unwrap_or(u32::MAX),
        diagnostic_only,
        candidate_resolution_counts,
    }
}

fn unresolved_candidates_are_diagnostic_only(
    candidates: &[CandidateHit],
    resolution_labels: &[String],
    unresolved_candidate_count: usize,
) -> bool {
    let has_resolved_hit = resolution_labels
        .iter()
        .any(|label| label.as_str() == "resolved");
    unresolved_candidate_count > 0
        && !resolution_labels.is_empty()
        && candidates
            .iter()
            .zip(resolution_labels)
            .filter(|(_, label)| label.as_str() != "resolved")
            .all(|(candidate, label)| {
                bare_dense_anchor_unresolved(candidate, label)
                    || (has_resolved_hit
                        && non_source_markdown_candidate_unresolved(candidate, label))
            })
}

fn bare_dense_anchor_unresolved(candidate: &CandidateHit, resolution_label: &str) -> bool {
    resolution_label == "path_unresolvable"
        && candidate.source == CandidateSource::Qdrant
        && bare_dense_anchor_path(candidate)
}

fn bare_dense_anchor_path(candidate: &CandidateHit) -> bool {
    let file_path = candidate.file_path.trim();
    !file_path.is_empty()
        && !candidate_path_text_is_path_like(file_path)
        && candidate
            .symbol_name
            .as_deref()
            .is_some_and(|symbol| symbol.trim().eq_ignore_ascii_case(file_path))
}

fn non_source_markdown_candidate_unresolved(
    candidate: &CandidateHit,
    resolution_label: &str,
) -> bool {
    resolution_label == "node_unresolved"
        && candidate.source == CandidateSource::Zoekt
        && candidate.symbol_name.is_none()
        && candidate.file_path.to_ascii_lowercase().ends_with(".md")
}

fn retrieval_stage_timings(trace: &QueryTrace) -> Vec<RetrievalStageTimingDto> {
    trace
        .stages
        .iter()
        .map(|stage| RetrievalStageTimingDto {
            stage: stage.stage.label().to_string(),
            deadline_ms: u32::try_from(stage.budget_ms).ok(),
            elapsed_ms: u32::try_from(stage.elapsed_ms).unwrap_or(u32::MAX),
            candidates_added: u32::try_from(stage.candidates_added).unwrap_or(u32::MAX),
            marginal_gain: stage.marginal_gain,
            cancel_reason: stage.cancel_reason.clone(),
            cache_hit: stage.cache_hit,
            sidecar_latency_ms: stage.stage.sidecar_latency_ms(stage.elapsed_ms),
            degraded: stage.degraded,
            stub_reason: stage.stub_reason.clone(),
        })
        .collect()
}

fn candidate_source_label(source: CandidateSource) -> String {
    serde_json::to_value(source)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{source:?}"))
}

fn candidate_path_resolvable(project_root: &Path, file_path: &str) -> bool {
    let rel = normalize_repo_relative_path(project_root, file_path);
    let trimmed = rel.trim();
    !trimmed.is_empty()
        && candidate_path_text_is_path_like(trimmed)
        && candidate_lookup_paths(project_root, &rel)
            .into_iter()
            .any(|path| path.exists())
}

fn candidate_path_text_is_path_like(path: &str) -> bool {
    let trimmed = path.trim();
    !trimmed.is_empty()
        && !trimmed.contains(':')
        && (trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains('.'))
}

fn normalize_repo_relative_path(project_root: &Path, file_path: &str) -> String {
    if let Some(rel) = strip_project_prefix_from_normalized_path(project_root, file_path) {
        return rel;
    }
    let path = PathBuf::from(file_path);
    if path.is_absolute() {
        path.strip_prefix(project_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| file_path.replace('\\', "/"))
    } else {
        file_path.replace('\\', "/")
    }
}

fn strip_project_prefix_from_normalized_path(
    project_root: &Path,
    file_path: &str,
) -> Option<String> {
    let candidate = normalize_storage_path_text(file_path);
    let roots = [
        normalize_storage_path_text(&project_root.to_string_lossy()),
        std::fs::canonicalize(project_root)
            .ok()
            .map(|path| normalize_storage_path_text(&path.to_string_lossy()))
            .unwrap_or_default(),
    ];
    roots
        .into_iter()
        .filter(|root| !root.is_empty())
        .find_map(|root| {
            if candidate.eq_ignore_ascii_case(&root) {
                return Some(String::new());
            }
            let prefix = format!("{root}/");
            candidate
                .strip_prefix(&prefix)
                .or_else(|| {
                    let candidate_lower = candidate.to_ascii_lowercase();
                    let prefix_lower = prefix.to_ascii_lowercase();
                    candidate_lower
                        .strip_prefix(&prefix_lower)
                        .map(|_| &candidate[prefix.len()..])
                })
                .map(str::to_string)
        })
}

fn normalize_storage_path_text(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if let Some(rest) = normalized.strip_prefix("//?/UNC/") {
        normalized = format!("//{rest}");
    } else if let Some(rest) = normalized.strip_prefix("//?/") {
        normalized = rest.to_string();
    }
    while normalized.contains("//") && !normalized.starts_with("//") {
        normalized = normalized.replace("//", "/");
    }
    normalized.trim_end_matches('/').to_string()
}

fn candidate_lookup_paths(project_root: &Path, rel_path: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    push_candidate_lookup_path(&mut paths, project_root, rel_path);
    if let Some(source_rooted) = source_root_candidate_path(rel_path) {
        push_candidate_lookup_path(&mut paths, project_root, &source_rooted);
    }
    paths
}

fn push_candidate_lookup_path(paths: &mut Vec<PathBuf>, project_root: &Path, rel_path: &str) {
    push_unique_path(paths, PathBuf::from(rel_path));
    let joined = project_root.join(rel_path);
    push_unique_path(paths, joined.clone());
    if let Ok(canonical) = std::fs::canonicalize(&joined) {
        push_unique_path(paths, canonical);
    }
}

fn source_root_candidate_path(rel_path: &str) -> Option<String> {
    let rel = rel_path.trim_start_matches("./").trim_start_matches('/');
    ["main/java/", "test/java/"]
        .iter()
        .any(|prefix| rel.starts_with(prefix))
        .then(|| format!("src/{rel}"))
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let key = path.to_string_lossy().to_string();
    if !paths
        .iter()
        .any(|existing| existing.to_string_lossy() == key)
    {
        paths.push(path);
    }
}

fn symbol_name_matches(needle: &str, serialized_name: &str, display_name: Option<&String>) -> bool {
    let needle = needle.trim();
    if needle.is_empty() {
        return false;
    }
    if serialized_name.eq_ignore_ascii_case(needle) {
        return true;
    }
    if let Some(display) = display_name
        && display.eq_ignore_ascii_case(needle)
    {
        return true;
    }
    serialized_name
        .rsplit("::")
        .next()
        .is_some_and(|tail| tail.eq_ignore_ascii_case(needle))
        || serialized_name
            .rsplit('.')
            .next()
            .is_some_and(|tail| tail.eq_ignore_ascii_case(needle))
}

fn resolve_candidate_node_id(
    storage: &Store,
    node_names: &HashMap<CoreNodeId, String>,
    project_root: &Path,
    rel_path: &str,
    candidate: &CandidateHit,
) -> Option<CoreNodeId> {
    if let Some(node_id) = candidate
        .node_id
        .as_deref()
        .and_then(|raw| raw.parse::<i64>().ok())
        .map(CoreNodeId)
        && storage.get_node(node_id).ok().flatten().is_some()
    {
        return Some(node_id);
    }

    if let Some(line) = candidate.start_line {
        let mut first_nodes = Vec::new();
        for lookup_path in candidate_lookup_paths(project_root, rel_path) {
            let lookup = lookup_path.to_string_lossy();
            let Ok(nodes) = storage.get_nodes_for_file_line(&lookup, line) else {
                continue;
            };
            if nodes.is_empty() {
                continue;
            }
            if let Some(symbol) = candidate.symbol_name.as_deref() {
                for node in &nodes {
                    if matches!(node.kind, NodeKind::FILE | NodeKind::UNKNOWN) {
                        continue;
                    }
                    if symbol_name_matches(symbol, &node.serialized_name, node_names.get(&node.id))
                    {
                        return Some(node.id);
                    }
                }
            }
            if first_nodes.is_empty() {
                first_nodes = nodes;
            }
        }
        if !first_nodes.is_empty() && candidate.symbol_name.is_none() {
            return first_nodes.first().map(|node| node.id);
        }
    }

    let file = candidate_lookup_paths(project_root, rel_path)
        .into_iter()
        .find_map(|path| storage.get_file_by_path(&path).ok().flatten())?;
    let file_node_id = CoreNodeId(file.id);
    let nodes = storage
        .get_node_kinds_for_files(&[file.id])
        .ok()
        .unwrap_or_default();
    if let Some(symbol) = candidate.symbol_name.as_deref() {
        for (node_id, kind) in &nodes {
            if matches!(kind, NodeKind::FILE | NodeKind::UNKNOWN) {
                continue;
            }
            let Ok(Some(node)) = storage.get_node(*node_id) else {
                continue;
            };
            if symbol_name_matches(symbol, &node.serialized_name, node_names.get(node_id)) {
                return Some(*node_id);
            }
        }
        return None;
    }
    nodes
        .into_iter()
        .find(|(_, kind)| !matches!(kind, NodeKind::FILE | NodeKind::UNKNOWN))
        .map(|(id, _)| id)
        .or(Some(file_node_id))
}

pub(crate) fn resolve_sidecar_candidates_to_search_hits(
    controller: &AppController,
    candidates: &[CandidateHit],
    max_results: usize,
) -> Result<Vec<SearchHit>, ApiError> {
    resolve_sidecar_candidates_with_stats(controller, candidates, max_results)
        .map(|outcome| outcome.resolved_hits)
}

fn resolve_sidecar_candidates_with_stats(
    controller: &AppController,
    candidates: &[CandidateHit],
    max_results: usize,
) -> Result<SidecarCandidateResolutionOutcome, ApiError> {
    controller.ensure_search_state()?;
    let storage = controller.open_storage()?;
    let project_root = controller.require_project_root()?;
    let node_names = controller.state.lock().node_names.clone();
    let mut hits = Vec::new();
    let mut attempted_candidate_count = 0;
    let mut unresolved_candidate_count = 0;
    let mut seen = HashMap::<String, ()>::new();
    let mut ordered: Vec<&CandidateHit> = candidates
        .iter()
        .filter(|candidate| !is_phantom_sidecar_hit(candidate))
        .collect();
    ordered.sort_by(|left, right| {
        let left_resolvable = candidate_path_resolvable(&project_root, &left.file_path);
        let right_resolvable = candidate_path_resolvable(&project_root, &right.file_path);
        right_resolvable.cmp(&left_resolvable).then(
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });

    for candidate in ordered {
        if hits.len() >= max_results {
            break;
        }
        attempted_candidate_count += 1;
        let rel_path = normalize_repo_relative_path(&project_root, &candidate.file_path);
        let Some(node_id) =
            resolve_candidate_node_id(&storage, &node_names, &project_root, &rel_path, candidate)
        else {
            unresolved_candidate_count += 1;
            continue;
        };
        let dedupe_key = node_id.0.to_string();
        if seen.contains_key(&dedupe_key) {
            continue;
        }
        seen.insert(dedupe_key, ());
        let Some(mut hit) =
            AppController::build_search_hit(&storage, &node_names, node_id, candidate.score)
        else {
            unresolved_candidate_count += 1;
            continue;
        };
        hit.score_breakdown = Some(score_breakdown_for_candidate(candidate));
        decorate_search_hit_evidence(&mut hit);
        hits.push(hit);
    }

    Ok(SidecarCandidateResolutionOutcome {
        resolved_hits: hits,
        attempted_candidate_count,
        unresolved_candidate_count,
    })
}

fn score_breakdown_for_candidate(candidate: &CandidateHit) -> RetrievalScoreBreakdownDto {
    let provenance = candidate_provenance_labels(candidate);
    let (lexical, semantic, graph) = match candidate.source {
        CandidateSource::Zoekt => (candidate.score, 0.0, 0.0),
        CandidateSource::Qdrant => (0.0, candidate.score, 0.0),
        CandidateSource::Scip => (0.0, 0.0, candidate.score),
        CandidateSource::Legacy => (candidate.score, 0.0, 0.0),
    };
    RetrievalScoreBreakdownDto {
        lexical,
        semantic,
        graph,
        total: candidate.score,
        tier_cap: None,
        boosts: Vec::new(),
        dampening: Vec::new(),
        final_rank_reason: None,
        provenance,
    }
}

fn candidate_provenance_labels(candidate: &CandidateHit) -> Vec<String> {
    if !candidate.provenance.is_empty() {
        return candidate.provenance.clone();
    }
    let label = match candidate.source {
        CandidateSource::Zoekt => "lexical_source",
        CandidateSource::Qdrant => "dense_anchor",
        CandidateSource::Scip => "graph_neighbor",
        CandidateSource::Legacy => "legacy",
    };
    vec![label.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_retrieval::{
        CandidateHit, QueryTrace, RetrievalStageKind, StageTrace, classify_query,
        project_id_for_root, test_support::retrieval_manifest_fixture,
    };

    fn env_test_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::process_env_test_lock()
    }

    #[test]
    fn env_flag_parsing_for_retrieval_rollout() {
        assert!(env_flag_enabled("1"));
        assert!(env_flag_enabled("TRUE"));
        assert!(!env_flag_enabled("0"));
        assert!(env_flag_disabled("off"));
        assert!(!env_flag_disabled("yes"));
    }

    #[test]
    fn candidate_lookup_paths_include_canonical_storage_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source_dir = temp.path().join("src");
        std::fs::create_dir_all(&source_dir).expect("mkdir");
        let source_file = source_dir.join("lib.rs");
        std::fs::write(&source_file, "fn main() {}\n").expect("write");

        let paths = candidate_lookup_paths(temp.path(), "src/lib.rs");
        let canonical = std::fs::canonicalize(&source_file).expect("canonical");
        assert!(
            paths
                .iter()
                .any(|path| path.to_string_lossy() == canonical.to_string_lossy()),
            "lookup paths should include canonical storage path: {paths:?}"
        );
    }

    #[test]
    fn normalize_repo_relative_path_strips_forward_slash_verbatim_prefix() {
        let project = Path::new("C:/workspaces/example");
        let file = "//?/C:/workspaces/example/workspace/app/src/lib.rs";

        assert_eq!(
            normalize_repo_relative_path(project, file),
            "workspace/app/src/lib.rs"
        );
    }

    #[test]
    fn candidate_lookup_resolves_java_main_source_root_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source_file = temp
            .path()
            .join("src/main/java/org/apache/commons/lang3/StringUtils.java");
        std::fs::create_dir_all(source_file.parent().expect("source parent"))
            .expect("mkdir source parent");
        std::fs::write(&source_file, "class StringUtils {}\n").expect("write source");

        assert!(
            candidate_path_resolvable(
                temp.path(),
                "main/java/org/apache/commons/lang3/StringUtils.java"
            ),
            "source-root path should resolve through src/main/java"
        );
    }

    #[test]
    fn candidate_lookup_resolves_java_test_source_root_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source_file = temp
            .path()
            .join("src/test/java/org/apache/commons/lang3/StringUtilsTest.java");
        std::fs::create_dir_all(source_file.parent().expect("source parent"))
            .expect("mkdir source parent");
        std::fs::write(&source_file, "class StringUtilsTest {}\n").expect("write source");

        assert!(
            candidate_path_resolvable(
                temp.path(),
                "test/java/org/apache/commons/lang3/StringUtilsTest.java"
            ),
            "source-root path should resolve through src/test/java"
        );
    }

    #[test]
    fn symbol_candidate_skips_unknown_callsite_and_resolves_definition() {
        use codestory_contracts::graph::{Occurrence, OccurrenceKind, SourceLocation};
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("src/lib.rs"),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 64,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        storage
            .insert_nodes_batch(&[
                codestory_contracts::graph::Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::FILE,
                    serialized_name: "src/lib.rs".to_string(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(1),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(2),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "run_exec_session".to_string(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(20),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(3),
                    kind: NodeKind::UNKNOWN,
                    serialized_name: "run_exec_session".to_string(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(10),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_occurrences_batch(&[Occurrence {
                element_id: 3,
                kind: OccurrenceKind::REFERENCE,
                location: SourceLocation {
                    file_node_id: CoreNodeId(1),
                    start_line: 10,
                    start_col: 5,
                    end_line: 10,
                    end_col: 21,
                },
            }])
            .expect("insert occurrence");
        let mut candidate = CandidateHit::with_source(
            "src/lib.rs",
            Some("run_exec_session".to_string()),
            1.0,
            codestory_retrieval::CandidateSource::Scip,
        );
        candidate.start_line = Some(10);

        let node_id = resolve_candidate_node_id(
            &storage,
            &HashMap::new(),
            Path::new("."),
            "src/lib.rs",
            &candidate,
        );

        assert_eq!(node_id, Some(CoreNodeId(2)));
    }

    #[test]
    fn unresolved_sidecar_candidates_are_diagnostic_only() {
        let result = QueryResult {
            query: "application use".into(),
            features: classify_query("application use"),
            hits: vec![CandidateHit::with_source(
                "lib/application.js",
                Some("use".to_string()),
                0.7,
                CandidateSource::Scip,
            )],
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 100,
                elapsed_ms: 1,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        let resolution = SidecarCandidateResolutionOutcome {
            resolved_hits: Vec::new(),
            attempted_candidate_count: 1,
            unresolved_candidate_count: 1,
        };

        let diagnostic = packet_sidecar_query_diagnostic(&result, &resolution, 2, 1, 3);

        assert_eq!(diagnostic.candidate_count, 1);
        assert_eq!(diagnostic.resolved_hit_count, 0);
        assert_eq!(diagnostic.unresolved_candidate_count, 1);
        assert_eq!(diagnostic.total_elapsed_ms, Some(3));
        assert!(diagnostic.diagnostic.is_some());
    }

    #[test]
    fn shadow_maps_unavailable_trace() {
        let shadow = shadow_from_query_result(QueryResult {
            query: "extension".into(),
            features: classify_query("extension"),
            hits: Vec::new(),
            trace: QueryTrace {
                retrieval_mode: "unavailable".into(),
                degraded_reason: Some("mandatory_sidecar_unavailable".into()),
                total_budget_ms: 0,
                elapsed_ms: 0,
                cancel_reason: Some("mandatory_sidecar_unavailable".into()),
                cache_hit: false,
                stages: Vec::new(),
            },
        });
        assert_eq!(shadow.retrieval_mode, "unavailable");
        assert_eq!(
            shadow.degraded_reason.as_deref(),
            Some("mandatory_sidecar_unavailable")
        );
        assert!(shadow.would_rank.is_empty());
    }

    #[test]
    fn shadow_maps_stage_timings_and_would_rank() {
        let shadow = shadow_from_query_result(QueryResult {
            query: "extension".into(),
            features: classify_query("ExtensionService"),
            hits: vec![
                CandidateHit::lexical_stub("src/a.rs", 0.9),
                CandidateHit::lexical_stub("src/b.rs", 0.5),
            ],
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 1_000,
                elapsed_ms: 25,
                cancel_reason: None,
                cache_hit: false,
                stages: vec![StageTrace {
                    stage: RetrievalStageKind::Stage1ZoektLexical,
                    budget_ms: 120,
                    elapsed_ms: 20,
                    candidates_added: 2,
                    marginal_gain: 0.4,
                    cancel_reason: None,
                    cache_hit: false,
                    degraded: false,
                    stub_reason: None,
                }],
            },
        });
        assert_eq!(shadow.retrieval_mode, "full");
        assert_eq!(shadow.stage_timings.len(), 1);
        assert_eq!(shadow.stage_timings[0].stage, "stage1_zoekt_lexical");
        assert_eq!(shadow.candidates.len(), 2);
        assert_eq!(shadow.would_rank, vec!["src/a.rs", "src/b.rs"]);
    }

    #[test]
    fn shadow_candidate_summaries_include_loss_point_resolution() {
        let mut candidate = CandidateHit::with_source(
            "semantic:handler",
            Some("handler".into()),
            0.5,
            CandidateSource::Qdrant,
        );
        candidate.start_line = Some(42);
        let shadow = shadow_from_query_result_with_counts_and_resolution_labels(
            QueryResult {
                query: "handler".into(),
                features: classify_query("handler"),
                hits: vec![candidate],
                trace: QueryTrace {
                    retrieval_mode: "full".into(),
                    degraded_reason: None,
                    total_budget_ms: 500,
                    elapsed_ms: 1,
                    cancel_reason: None,
                    cache_hit: false,
                    stages: Vec::new(),
                },
            },
            1,
            0,
            &["path_unresolvable".to_string()],
            &[SidecarCandidateAdmissionLabel {
                admission_status: "unresolved".to_string(),
                loss_reason: Some("path_unresolvable".to_string()),
                resolved_node_id: None,
                search_hit_rank: None,
                final_rank: None,
            }],
        );

        assert_eq!(shadow.candidate_count, 1);
        assert_eq!(shadow.resolved_hit_count, 0);
        assert_eq!(shadow.candidates[0].line, Some(42));
        assert_eq!(
            shadow.candidates[0].resolution.as_deref(),
            Some("path_unresolvable")
        );
        assert_eq!(
            shadow.candidates[0].admission_status.as_deref(),
            Some("unresolved")
        );
        assert_eq!(
            shadow.candidates[0].loss_reason.as_deref(),
            Some("path_unresolvable")
        );
        assert_eq!(shadow.unresolved_candidate_count, 1);
        assert_eq!(shadow.candidate_resolution_counts.len(), 1);
        assert_eq!(
            shadow.candidate_resolution_counts[0].resolution,
            "path_unresolvable"
        );
        assert_eq!(shadow.candidate_resolution_counts[0].count, 1);
    }

    #[test]
    fn shadow_marks_only_bare_dense_anchors_as_diagnostic_only() {
        let dense_anchor = QueryResult {
            query: "apii".into(),
            features: classify_query("apii"),
            hits: vec![CandidateHit::with_source(
                "apii",
                Some("apii".to_string()),
                0.5,
                CandidateSource::Qdrant,
            )],
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 1,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        let shadow = shadow_from_query_result_with_counts_and_resolution_labels(
            dense_anchor,
            1,
            0,
            &["path_unresolvable".to_string()],
            &[],
        );
        let value = serde_json::to_value(&shadow).expect("serialize shadow");
        assert_eq!(shadow.unresolved_candidate_count, 1);
        assert_eq!(value["diagnostic_only"], true);

        let missing_path = QueryResult {
            query: "StringUtils".into(),
            features: classify_query("StringUtils"),
            hits: vec![CandidateHit::with_source(
                "main/java/org/apache/commons/lang3/Missing.java",
                Some("StringUtils".to_string()),
                0.5,
                CandidateSource::Qdrant,
            )],
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 1,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        let shadow = shadow_from_query_result_with_counts_and_resolution_labels(
            missing_path,
            1,
            0,
            &["path_unresolvable".to_string()],
            &[],
        );
        let value = serde_json::to_value(&shadow).expect("serialize shadow");
        assert_eq!(shadow.unresolved_candidate_count, 1);
        assert_eq!(value.get("diagnostic_only"), None);
    }

    #[test]
    fn shadow_marks_non_source_markdown_candidates_diagnostic_only_with_source_hits() {
        let shadow = shadow_from_query_result_with_counts_and_resolution_labels(
            QueryResult {
                query: "form validation".into(),
                features: classify_query("form validation"),
                hits: vec![
                    CandidateHit::with_source(
                        "docs/tasks/form-validation/marking.md",
                        None,
                        0.9,
                        CandidateSource::Zoekt,
                    ),
                    CandidateHit::with_source(
                        "src/forms/validation.html",
                        Some("email".into()),
                        0.8,
                        CandidateSource::Qdrant,
                    ),
                ],
                trace: QueryTrace {
                    retrieval_mode: "full".into(),
                    degraded_reason: None,
                    total_budget_ms: 500,
                    elapsed_ms: 1,
                    cancel_reason: None,
                    cache_hit: false,
                    stages: Vec::new(),
                },
            },
            2,
            1,
            &["node_unresolved".to_string(), "resolved".to_string()],
            &[],
        );
        let value = serde_json::to_value(&shadow).expect("serialize shadow");
        assert_eq!(shadow.unresolved_candidate_count, 1);
        assert_eq!(value["diagnostic_only"], true);
    }

    #[test]
    fn shadow_candidate_summaries_include_admission_diagnostics() {
        let shadow = shadow_from_query_result_with_counts_and_resolution_labels(
            QueryResult {
                query: "exec json flow".into(),
                features: classify_query("exec json flow"),
                hits: vec![
                    CandidateHit::with_source(
                        "src/exec.rs",
                        Some("run_exec_session".into()),
                        0.9,
                        CandidateSource::Scip,
                    ),
                    CandidateHit::with_source(
                        "src/noise.rs",
                        Some("CommandExec".into()),
                        0.8,
                        CandidateSource::Zoekt,
                    ),
                ],
                trace: QueryTrace {
                    retrieval_mode: "full".into(),
                    degraded_reason: None,
                    total_budget_ms: 500,
                    elapsed_ms: 1,
                    cancel_reason: None,
                    cache_hit: false,
                    stages: Vec::new(),
                },
            },
            2,
            2,
            &["resolved".to_string(), "resolved".to_string()],
            &[
                SidecarCandidateAdmissionLabel {
                    admission_status: "admitted".to_string(),
                    loss_reason: None,
                    resolved_node_id: Some("2".to_string()),
                    search_hit_rank: Some(1),
                    final_rank: Some(1),
                },
                SidecarCandidateAdmissionLabel {
                    admission_status: "rejected".to_string(),
                    loss_reason: Some("not_in_final_result_window".to_string()),
                    resolved_node_id: Some("3".to_string()),
                    search_hit_rank: Some(2),
                    final_rank: None,
                },
            ],
        );

        assert_eq!(
            shadow.candidates[0].admission_status.as_deref(),
            Some("admitted")
        );
        assert_eq!(shadow.candidates[0].loss_reason.as_deref(), None);
        assert_eq!(shadow.candidates[0].resolved_node_id.as_deref(), Some("2"));
        assert_eq!(shadow.candidates[0].search_hit_rank, Some(1));
        assert_eq!(shadow.candidates[0].final_rank, Some(1));
        assert_eq!(
            shadow.candidates[1].admission_status.as_deref(),
            Some("rejected")
        );
        assert_eq!(
            shadow.candidates[1].loss_reason.as_deref(),
            Some("not_in_final_result_window")
        );
        assert_eq!(shadow.candidates[1].resolved_node_id.as_deref(), Some("3"));
        assert_eq!(shadow.candidates[1].search_hit_rank, Some(2));
        assert_eq!(shadow.candidates[1].final_rank, None);
        assert_eq!(
            shadow.unresolved_candidate_count, 0,
            "resolved candidates rejected by the final result window are not unresolved sidecar candidates"
        );
    }

    #[test]
    fn sidecar_budget_respects_latency_cap() {
        assert_eq!(sidecar_budget_ms(Some(400)), 400);
        assert_eq!(sidecar_budget_ms(Some(5_000)), DEFAULT_SIDECAR_BUDGET_MS);
        assert_eq!(sidecar_budget_ms(None), DEFAULT_SIDECAR_BUDGET_MS);
    }

    #[test]
    fn packet_batch_budget_uses_packet_latency_budget() {
        assert_eq!(
            sidecar_packet_batch_budget_ms(None),
            DEFAULT_PACKET_BATCH_BUDGET_MS
        );
        assert_eq!(sidecar_packet_batch_budget_ms(Some(18_000)), 18_000);
        assert_eq!(sidecar_packet_batch_budget_ms(Some(5_000)), 5_000);
        assert_eq!(sidecar_packet_batch_budget_ms(Some(5)), 100);
        assert_eq!(
            sidecar_packet_batch_budget_ms(Some(250_000)),
            MAX_PACKET_BATCH_BUDGET_MS
        );
    }

    #[test]
    fn recovery_commands_quote_shell_sensitive_project_paths() {
        let commands = sidecar_retrieval_recovery_commands(r"C:\tmp\cost$cache`tick's repo");

        #[cfg(windows)]
        let expected_project = r"'C:/tmp/cost$cache`tick''s repo'";
        #[cfg(not(windows))]
        let expected_project = r"'C:/tmp/cost$cache`tick'\''s repo'";

        assert!(
            commands
                .first()
                .is_some_and(|command| command.contains("ready --goal agent --repair")),
            "sidecar recovery should start with the canonical agent repair command: {commands:?}"
        );
        assert!(
            commands
                .iter()
                .all(|command| command.contains(&format!("--project {expected_project}"))),
            "all recovery commands should quote the project path literally: {commands:?}"
        );
    }

    #[test]
    fn sidecar_primary_modes_fail_closed_for_partial_sidecars() {
        assert!(sidecar_mode_can_serve_primary("full"));
        assert!(!sidecar_mode_can_serve_primary("no_scip"));
        assert!(!sidecar_mode_can_serve_primary("no_semantic"));
        assert!(!sidecar_mode_can_serve_primary("lexical_only"));
        assert!(!sidecar_mode_can_serve_primary("unavailable"));
    }

    #[test]
    fn sidecar_primary_requires_agent_profile_even_when_local_mode_is_full() {
        let local_full = SidecarModeStatus {
            profile: Some("local".into()),
            mode: "full".into(),
            degraded_reason: None,
        };
        let agent_full = SidecarModeStatus {
            profile: Some("agent".into()),
            mode: "full".into(),
            degraded_reason: None,
        };
        let missing_profile_full = SidecarModeStatus {
            profile: None,
            mode: "full".into(),
            degraded_reason: None,
        };

        assert!(
            !sidecar_status_can_serve_primary(&local_full),
            "local/default full sidecar must not serve packet/search/context primary retrieval"
        );
        assert!(sidecar_status_can_serve_primary(&agent_full));
        assert!(!sidecar_status_can_serve_primary(&missing_profile_full));
    }

    #[test]
    fn retrieval_manifest_exists_uses_canonical_sidecar_project_id_for_clean_repos() {
        let Some(project) = git_project() else {
            return;
        };
        let storage_dir = tempfile::tempdir().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let canonical_id = sidecar_project_id_for_root(project.path());
        let root_id = project_id_for_root(project.path());
        assert_ne!(canonical_id, root_id);
        upsert_manifest(&storage_path, &canonical_id);

        assert!(retrieval_manifest_exists(&storage_path, project.path()));

        std::fs::write(project.path().join("lib.rs"), "pub fn dirty() {}\n").expect("dirty source");
        assert!(!retrieval_manifest_exists(&storage_path, project.path()));
    }

    #[test]
    fn retrieval_manifest_exists_uses_root_id_for_unidentifiable_repos() {
        let project = tempfile::tempdir().expect("project");
        let storage_dir = tempfile::tempdir().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        upsert_manifest(&storage_path, "repo-v1-ffffffffffffffff");
        assert!(!retrieval_manifest_exists(&storage_path, project.path()));

        let root_id = project_id_for_root(project.path());
        upsert_manifest(&storage_path, &root_id);
        assert!(retrieval_manifest_exists(&storage_path, project.path()));
    }

    #[test]
    fn sidecar_mode_status_rejects_stale_manifest_before_health_probe() {
        let project = tempfile::tempdir().expect("project");
        let storage_dir = tempfile::tempdir().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = project_id_for_root(project.path());
        let hash = "deadbeefcafebabe";
        let mut storage = Store::open(&storage_path).expect("open storage");
        let mut manifest = retrieval_manifest_fixture(&project_id, hash);
        manifest.embedding_backend = Some("stale-backend".into());
        storage
            .upsert_retrieval_index_manifest(&manifest)
            .expect("manifest");

        let status = sidecar_mode_status_for_project(project.path(), &storage_path);

        assert_eq!(status.mode, "unavailable");
        let reason = status.degraded_reason.expect("stale reason");
        assert!(
            reason.contains("sidecar_manifest_stale"),
            "expected stale manifest reason, got: {reason}"
        );
        assert!(
            reason.contains("sidecar_embedding_backend_changed"),
            "expected embedding backend detail, got: {reason}"
        );
    }

    fn upsert_manifest(storage_path: &Path, project_id: &str) {
        let hash = "deadbeefcafebabe";
        let generation = format!("{project_id}-{hash}");
        let qdrant_collection = format!("codestory_{project_id}_{hash}");
        let built_at_epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_millis() as i64;
        let mut storage = Store::open(storage_path).expect("open storage");
        storage
            .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                project_id: project_id.into(),
                zoekt_version: "zoekt-real-v1".into(),
                qdrant_collection,
                scip_revision: Some("graph-test".into()),
                built_at_epoch_ms,
                disk_bytes: None,
                degraded_modes_json: "[]".into(),
                embedding_backend: Some(codestory_retrieval::embedding_runtime_id()),
                embedding_dim: Some(codestory_retrieval::RETRIEVAL_EMBEDDING_DIM as i32),
                sidecar_schema_version: Some(codestory_retrieval::SIDECAR_SCHEMA_VERSION),
                sidecar_input_hash: Some(hash.into()),
                sidecar_generation: Some(generation),
                projection_count: Some(0),
                symbol_doc_count: Some(0),
                dense_projection_count: Some(0),
                semantic_policy_version: Some("graph_first_v1".into()),
                graph_artifact_hash: Some("graph-test-hash".into()),
                dense_reason_counts_json: Some("{}".into()),
                precise_semantic_import_status: None,
                precise_semantic_import_reason: None,
                precise_semantic_import_revision: None,
                precise_semantic_import_producer: None,
            })
            .expect("manifest");
    }

    fn git_project() -> Option<tempfile::TempDir> {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return None;
        }
        let project = tempfile::tempdir().expect("project");
        git(project.path(), &["init"]);
        git(
            project.path(),
            &["config", "user.email", "codestory@example.invalid"],
        );
        git(project.path(), &["config", "user.name", "CodeStory Test"]);
        git(
            project.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/TheGreenCedar/CodeStory.git",
            ],
        );
        std::fs::write(project.path().join("lib.rs"), "pub fn run() {}\n").expect("write source");
        git(project.path(), &["add", "."]);
        git(project.path(), &["commit", "-m", "init"]);
        Some(project)
    }

    fn git(project: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(project)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn sidecar_result_allows_empty_full_mode_and_rejects_unresolved_candidates() {
        use codestory_retrieval::{CandidateSource, classify_query};

        let empty_full = QueryResult {
            query: "handler".into(),
            features: classify_query("handler"),
            hits: Vec::new(),
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 1,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        assert_eq!(
            sidecar_result_rejection_reason(&empty_full, &[]).as_deref(),
            None
        );

        let unresolved = QueryResult {
            query: "handler".into(),
            features: classify_query("handler"),
            hits: vec![CandidateHit::with_source(
                "semantic:handler",
                Some("handler".into()),
                0.5,
                CandidateSource::Qdrant,
            )],
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 1,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        assert_eq!(
            sidecar_result_rejection_reason(&unresolved, &[]).as_deref(),
            Some("sidecar retrieval candidates did not resolve to indexed symbols")
        );
    }

    #[test]
    fn sidecar_search_result_rejects_non_full_modes_even_without_candidates() {
        use codestory_retrieval::classify_query;

        for mode in ["no_semantic", "no_scip", "lexical_only", "unavailable"] {
            let result = QueryResult {
                query: "handler".into(),
                features: classify_query("handler"),
                hits: Vec::new(),
                trace: QueryTrace {
                    retrieval_mode: mode.into(),
                    degraded_reason: Some("fixture degraded".into()),
                    total_budget_ms: 500,
                    elapsed_ms: 1,
                    cancel_reason: None,
                    cache_hit: false,
                    stages: Vec::new(),
                },
            };
            let expected =
                format!("sidecar retrieval mode `{mode}` is not eligible for primary results");
            assert_eq!(
                sidecar_result_rejection_reason(&result, &[]).as_deref(),
                Some(expected.as_str()),
                "{mode} must fail closed before product search results are served"
            );
        }
    }

    #[test]
    fn packet_sidecar_query_diagnostic_distinguishes_empty_and_unresolved_candidates() {
        use codestory_retrieval::{CandidateSource, classify_query};

        let empty_full = QueryResult {
            query: "unlikely symbol".into(),
            features: classify_query("unlikely symbol"),
            hits: Vec::new(),
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 1,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        let empty_resolution = SidecarCandidateResolutionOutcome {
            resolved_hits: Vec::new(),
            attempted_candidate_count: 0,
            unresolved_candidate_count: 0,
        };
        let empty_diagnostic =
            packet_sidecar_query_diagnostic(&empty_full, &empty_resolution, 1, 0, 1);
        assert_eq!(empty_diagnostic.candidate_count, 0);
        assert_eq!(empty_diagnostic.resolved_hit_count, 0);
        assert_eq!(empty_diagnostic.unresolved_candidate_count, 0);
        assert!(empty_diagnostic.diagnostic.is_none());

        let unresolved = QueryResult {
            query: "handler".into(),
            features: classify_query("handler"),
            hits: vec![CandidateHit::with_source(
                "semantic:handler",
                Some("handler".into()),
                0.5,
                CandidateSource::Qdrant,
            )],
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 1,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        let unresolved_resolution = SidecarCandidateResolutionOutcome {
            resolved_hits: Vec::new(),
            attempted_candidate_count: 1,
            unresolved_candidate_count: 1,
        };
        let unresolved_diagnostic =
            packet_sidecar_query_diagnostic(&unresolved, &unresolved_resolution, 1, 0, 1);
        assert_eq!(unresolved_diagnostic.candidate_count, 1);
        assert_eq!(unresolved_diagnostic.resolved_hit_count, 0);
        assert_eq!(unresolved_diagnostic.unresolved_candidate_count, 1);
        assert!(
            unresolved_diagnostic
                .diagnostic
                .as_deref()
                .is_some_and(|value| value.contains("did not all resolve"))
        );
    }

    #[test]
    fn packet_sidecar_query_diagnostic_ignores_candidates_skipped_by_result_cap() {
        use codestory_retrieval::{CandidateSource, classify_query};
        use codestory_store::{FileInfo, FileRole};

        let temp = tempfile::tempdir().expect("tempdir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("storage parent"))
            .expect("create storage parent");
        let source_path = temp.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "fn alpha() {}\n").expect("write source");

        {
            let mut storage = Store::open(&storage_path).expect("open storage");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert file");
            storage
                .insert_nodes_batch(&[
                    codestory_contracts::graph::Node {
                        id: CoreNodeId(1),
                        kind: NodeKind::FILE,
                        serialized_name: source_path.to_string_lossy().to_string(),
                        file_node_id: Some(CoreNodeId(1)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                    codestory_contracts::graph::Node {
                        id: CoreNodeId(2),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "alpha".to_string(),
                        file_node_id: Some(CoreNodeId(1)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), storage_path)
            .expect("open project");
        let mut resolved_candidate = CandidateHit::with_source(
            "src/lib.rs",
            Some("alpha".to_string()),
            1.0,
            CandidateSource::Scip,
        );
        resolved_candidate.node_id = Some("2".to_string());
        let query_result = QueryResult {
            query: "alpha".into(),
            features: classify_query("alpha"),
            hits: vec![
                resolved_candidate,
                CandidateHit::with_source(
                    "src/missing.rs",
                    Some("missing".to_string()),
                    0.5,
                    CandidateSource::Scip,
                ),
            ],
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 1,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };

        let resolution = resolve_sidecar_candidates_with_stats(&controller, &query_result.hits, 1)
            .expect("resolve sidecar candidates");
        assert_eq!(resolution.attempted_candidate_count, 1);
        assert_eq!(resolution.resolved_hits.len(), 1);
        assert_eq!(resolution.unresolved_candidate_count, 0);

        let diagnostic = packet_sidecar_query_diagnostic(&query_result, &resolution, 1, 0, 1);
        assert_eq!(diagnostic.candidate_count, 1);
        assert_eq!(diagnostic.resolved_hit_count, 1);
        assert_eq!(diagnostic.unresolved_candidate_count, 0);
        assert!(
            diagnostic.diagnostic.is_none(),
            "capped-away candidates should not create unresolved diagnostics: {diagnostic:?}"
        );
    }

    #[test]
    fn packet_batch_rejects_unavailable_sidecar_mode() {
        use codestory_retrieval::{CandidateSource, classify_query};

        let unavailable = QueryResult {
            query: "handler".into(),
            features: classify_query("handler"),
            hits: Vec::new(),
            trace: QueryTrace {
                retrieval_mode: "no_semantic".into(),
                degraded_reason: Some("semantic store unavailable".into()),
                total_budget_ms: 500,
                elapsed_ms: 1,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        assert_eq!(
            sidecar_packet_batch_rejection_reason(&unavailable, &[]).as_deref(),
            Some("sidecar retrieval mode `no_semantic` is not eligible for packet batch results")
        );

        let unresolved = QueryResult {
            query: "handler".into(),
            features: classify_query("handler"),
            hits: vec![CandidateHit::with_source(
                "semantic:handler",
                Some("handler".into()),
                0.5,
                CandidateSource::Qdrant,
            )],
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 1,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        assert_eq!(
            sidecar_packet_batch_rejection_reason(&unresolved, &[]).as_deref(),
            None,
            "packet subqueries should report unresolved full-mode candidates as diagnostics instead of aborting the whole packet"
        );
    }

    #[test]
    fn packet_batch_reports_unresolved_full_mode_candidates_without_rejecting() {
        use codestory_retrieval::CandidateSource;

        let temp = tempfile::tempdir().expect("tempdir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("storage parent"))
            .expect("create storage parent");
        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), storage_path)
            .expect("open project");

        let queries = vec![("helpers".to_string(), 5)];
        let outcome = search_sidecar_packet_batch_inner_with_query_batch(
            &controller,
            &queries,
            Some(500),
            |_, batch| {
                assert_eq!(batch, &[("helpers".to_string(), 500)]);
                Ok(vec![QueryResult {
                    query: "helpers".into(),
                    features: classify_query("helpers"),
                    hits: vec![CandidateHit::with_source(
                        "docs/helpers.md",
                        Some("helpers".into()),
                        0.5,
                        CandidateSource::Scip,
                    )],
                    trace: QueryTrace {
                        retrieval_mode: "full".into(),
                        degraded_reason: None,
                        total_budget_ms: 500,
                        elapsed_ms: 1,
                        cancel_reason: None,
                        cache_hit: false,
                        stages: Vec::new(),
                    },
                }])
            },
        )
        .expect("full-mode unresolved candidates should not reject packet batch");

        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].0, "helpers");
        assert!(
            outcome.results[0].1.is_empty(),
            "unresolved packet query should contribute no resolved hits"
        );
        assert_eq!(outcome.diagnostics.len(), 1);
        let diagnostic = &outcome.diagnostics[0];
        assert_eq!(diagnostic.query, "helpers");
        assert_eq!(diagnostic.retrieval_mode, "full");
        assert_eq!(diagnostic.candidate_count, 1);
        assert_eq!(diagnostic.resolved_hit_count, 0);
        assert_eq!(diagnostic.unresolved_candidate_count, 1);
        assert!(
            diagnostic
                .diagnostic
                .as_deref()
                .is_some_and(|value| value.contains("did not all resolve")),
            "diagnostic should preserve unresolved sidecar detail: {diagnostic:?}"
        );
    }

    #[test]
    fn packet_batch_divides_request_budget_across_queries() {
        use codestory_retrieval::classify_query;

        let temp = tempfile::tempdir().expect("tempdir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("storage parent"))
            .expect("create storage parent");
        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), storage_path)
            .expect("open project");

        let queries = vec![
            ("entrypoint".to_string(), 5),
            ("file discovery".to_string(), 5),
            ("symbol extraction".to_string(), 5),
            ("search projection".to_string(), 5),
        ];
        let mut observed_budgets = Vec::new();
        let mut batch_call_count = 0;
        let outcome = search_sidecar_packet_batch_inner_with_query_batch(
            &controller,
            &queries,
            Some(18_000),
            |_, batch| {
                batch_call_count += 1;
                observed_budgets.extend(batch.iter().map(|(_, budget)| *budget));
                Ok(batch
                    .iter()
                    .map(|(query, budget)| QueryResult {
                        query: query.to_string(),
                        features: classify_query(query),
                        hits: Vec::new(),
                        trace: QueryTrace {
                            retrieval_mode: "full".into(),
                            degraded_reason: None,
                            total_budget_ms: *budget,
                            elapsed_ms: 1,
                            cancel_reason: None,
                            cache_hit: false,
                            stages: Vec::new(),
                        },
                    })
                    .collect())
            },
        )
        .expect("empty full-mode packet query results should not reject");

        assert_eq!(outcome.results.len(), queries.len());
        assert_eq!(batch_call_count, 1);
        assert_eq!(observed_budgets, vec![4_500, 4_500, 4_500, 4_500]);
    }

    #[test]
    fn packet_batch_rejects_candidate_resolution_errors() {
        use codestory_retrieval::CandidateSource;

        let temp = tempfile::tempdir().expect("tempdir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), storage_path.clone())
            .expect("open project");
        std::fs::remove_dir_all(storage_path.parent().expect("storage parent"))
            .expect("remove storage parent");

        let queries = vec![("handler".to_string(), 5)];
        let result = search_sidecar_packet_batch_inner_with_query_batch(
            &controller,
            &queries,
            Some(500),
            |_, batch| {
                assert_eq!(batch, &[("handler".to_string(), 500)]);
                Ok(vec![QueryResult {
                    query: "handler".into(),
                    features: classify_query("handler"),
                    hits: vec![CandidateHit::with_source(
                        "src/lib.rs",
                        Some("handler".into()),
                        0.5,
                        CandidateSource::Scip,
                    )],
                    trace: QueryTrace {
                        retrieval_mode: "full".into(),
                        degraded_reason: None,
                        total_budget_ms: 500,
                        elapsed_ms: 1,
                        cancel_reason: None,
                        cache_hit: false,
                        stages: Vec::new(),
                    },
                }])
            },
        );

        let error = match result {
            Ok(_) => panic!("packet batch must reject candidate resolution errors"),
            Err(error) => error,
        };
        assert_eq!(error.code, "retrieval_unavailable");
        assert!(
            error.message.contains("sidecar retrieval rejected")
                || error.message.contains("candidate resolution failed"),
            "error should preserve candidate resolution failure: {}",
            error.message
        );
    }

    #[test]
    fn sidecar_primary_search_reports_candidate_resolution_errors() {
        use codestory_retrieval::CandidateSource;

        let temp = tempfile::tempdir().expect("tempdir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), storage_path.clone())
            .expect("open project");
        std::fs::remove_dir_all(storage_path.parent().expect("storage parent"))
            .expect("remove storage parent");

        let query_result = QueryResult {
            query: "handler".into(),
            features: classify_query("handler"),
            hits: vec![CandidateHit::with_source(
                "src/lib.rs",
                Some("handler".into()),
                0.5,
                CandidateSource::Scip,
            )],
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 1,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };

        let outcome =
            sidecar_primary_search_outcome_from_query_result(&controller, query_result, 5);

        match outcome {
            SidecarPrimarySearchOutcome::Unavailable { reason } => assert!(
                reason.contains("candidate resolution failed"),
                "reason should preserve candidate resolution failure: {reason}"
            ),
            _ => panic!("candidate resolution errors must make primary search unavailable"),
        }
    }

    #[test]
    fn primary_env_override_rejects_zero() {
        let _lock = env_test_lock();
        // SAFETY: test-only env mutation; no concurrent tests rely on this variable.
        unsafe {
            std::env::set_var(RETRIEVAL_ENV, "0");
        }
        assert_eq!(retrieval_env_override(), Some(false));
        // SAFETY: test-only env cleanup.
        unsafe {
            std::env::remove_var(RETRIEVAL_ENV);
        }
        assert_eq!(retrieval_env_override(), None);
    }
}
