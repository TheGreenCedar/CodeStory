//! Mandatory sidecar retrieval integration for packet and agent ask paths.

use crate::agent::nucleo_policy::with_sidecar_primary_retrieval;
use crate::agent::packet_candidate::{
    PacketCandidateTrailScan, PacketGraphDirection, PacketGraphEdgeProvenance, PacketSearchHit,
};
use crate::agent::packet_degradation::semantic_stage_degradation;
use crate::agent::packet_evidence::decorate_search_hit_evidence;
use crate::{
    AppController, HybridSearchScoredHit, app_graph_flags, graph_edge_dto, member_access_dto,
    node_display_name,
};
use anyhow::Error as AnyhowError;
use codestory_agent::packet_flow_requirements::{
    FlowRequirement, flow_requirement_call_boundary_is_discoverable,
    flow_requirement_call_receipt_is_valid,
};
use codestory_contracts::api::NodeKind as ApiNodeKind;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, AgentPacketDto, ApiError,
    EmbeddingVectorPublicationIdentityDto, GraphArtifactDto, GraphEdgeDto, GraphNodeDto,
    GraphResponse, PacketQueryCompletionDto, PacketSidecarQueryDiagnosticDto,
    RetrievalCandidateResolutionCountDto, RetrievalCandidateSummaryDto, RetrievalScoreBreakdownDto,
    RetrievalShadowDto, RetrievalStageTimingDto, SearchHit, SearchHitOrigin, SearchResultsDto,
};
use codestory_contracts::graph::{
    EdgeKind, NodeId as CoreNodeId, NodeKind, ResolutionCertainty, TrailCallerScope, TrailConfig,
    TrailDirection,
};
#[cfg(test)]
use codestory_retrieval::SidecarRuntimeConfig;
use codestory_retrieval::{
    CandidateGraphDirection, CandidateHit, CandidateSource, PinnedQuerySession, QueryBatchItem,
    QueryRequest, QueryResult, QueryTrace, SidecarProfile,
    execute_retrieval_query_with_cache_for_runtime, is_phantom_sidecar_hit,
    is_retrieval_publication_changed, sidecar_project_id_for_root,
    strict_sidecar_status_for_runtime,
};
use codestory_store::Store;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

const DEFAULT_SIDECAR_BUDGET_MS: u64 = 1_500;
const DEFAULT_PACKET_BATCH_BUDGET_MS: u64 = 18_000;
const MIN_PACKET_BATCH_BUDGET_MS: u64 = 1_000;
const MAX_PACKET_BATCH_BUDGET_MS: u64 = 120_000;
const MAX_SHADOW_CANDIDATES: usize = 20;
const MAX_SHADOW_WOULD_RANK: usize = 10;
const RETRIEVAL_PUBLICATION_ATTEMPTS: usize = 2;
pub(crate) const RETRIEVAL_VERSION_SIDECAR: &str = "sidecar";
/// Typed cancel reason for a query whose semantic stage timed out and resolved nothing.
pub(crate) const SEMANTIC_TIMEOUT_ZERO_HITS_CANCEL: &str = "semantic_stage_timeout_zero_hits";

const RETRIEVAL_ENV: &str = "CODESTORY_RETRIEVAL";
const RETRIEVAL_SHADOW_ENV: &str = "CODESTORY_RETRIEVAL_SHADOW";

struct PinnedRetrievalRead {
    session: PinnedQuerySession,
    project_root: PathBuf,
    node_names: Arc<HashMap<CoreNodeId, String>>,
}

thread_local! {
    /// The complete public operation owns one pin. Lower-level query adapters borrow it so packet
    /// subqueries cannot silently open a different retrieval generation during the same response.
    static ACTIVE_PINNED_RETRIEVAL_READ: RefCell<Option<(usize, Rc<PinnedRetrievalRead>)>> =
        const { RefCell::new(None) };
}

fn controller_identity(controller: &AppController) -> usize {
    controller.identity()
}

fn active_pinned_retrieval_read(controller: &AppController) -> Option<Rc<PinnedRetrievalRead>> {
    let controller_identity = controller_identity(controller);
    ACTIVE_PINNED_RETRIEVAL_READ.with(|active| {
        active
            .borrow()
            .as_ref()
            .filter(|(active_controller, _)| *active_controller == controller_identity)
            .map(|(_, pinned)| Rc::clone(pinned))
    })
}

struct ActivePinnedRetrievalReadGuard {
    previous: Option<(usize, Rc<PinnedRetrievalRead>)>,
}

impl Drop for ActivePinnedRetrievalReadGuard {
    fn drop(&mut self) {
        ACTIVE_PINNED_RETRIEVAL_READ.with(|active| {
            active.replace(self.previous.take());
        });
    }
}

fn with_active_pinned_retrieval_read<T>(
    controller: &AppController,
    pinned: Rc<PinnedRetrievalRead>,
    build: impl FnOnce() -> T,
) -> T {
    let previous = ACTIVE_PINNED_RETRIEVAL_READ
        .with(|active| active.replace(Some((controller_identity(controller), pinned))));
    let _guard = ActivePinnedRetrievalReadGuard { previous };
    build()
}

pub(crate) trait RetrievalPublicationResponse {
    fn attach_retrieval_publication(&mut self, publication: EmbeddingVectorPublicationIdentityDto);
}

impl RetrievalPublicationResponse for SearchResultsDto {
    fn attach_retrieval_publication(&mut self, publication: EmbeddingVectorPublicationIdentityDto) {
        self.retrieval_publication = Some(publication);
    }
}

impl RetrievalPublicationResponse for AgentAnswerDto {
    fn attach_retrieval_publication(&mut self, publication: EmbeddingVectorPublicationIdentityDto) {
        self.retrieval_trace.retrieval_publication = Some(publication);
    }
}

impl RetrievalPublicationResponse for AgentPacketDto {
    fn attach_retrieval_publication(&mut self, publication: EmbeddingVectorPublicationIdentityDto) {
        self.answer.retrieval_trace.retrieval_publication = Some(publication.clone());
        self.retrieval_trace_summary
            .retrieval_trace
            .retrieval_publication = Some(publication);
    }
}

fn publication_dto(pinned: &PinnedRetrievalRead) -> EmbeddingVectorPublicationIdentityDto {
    let publication = pinned.session.publication_identity();
    EmbeddingVectorPublicationIdentityDto {
        core_generation_id: publication.core_generation_id.clone(),
        core_run_id: publication.core_run_id.clone(),
        retrieval_generation: publication.sidecar_generation.clone(),
        retrieval_input_hash: publication.sidecar_input_hash.clone(),
        semantic_generation: publication.semantic_generation.clone(),
    }
}

fn ensure_pinned_core_publication(
    pinned: &PinnedRetrievalRead,
    expected_core_generation_id: &str,
    expected_core_run_id: &str,
) -> Result<(), ApiError> {
    let publication = pinned.session.publication_identity();
    if publication.core_generation_id == expected_core_generation_id
        && publication.core_run_id == expected_core_run_id
    {
        return Ok(());
    }
    Err(ApiError::new(
        "publication_changed",
        format!(
            "publication_changed: retrieval pin belongs to core generation {}/{} but the public operation pinned {}/{}; retry the complete operation",
            publication.core_generation_id,
            publication.core_run_id,
            expected_core_generation_id,
            expected_core_run_id,
        ),
    ))
}

pub(crate) fn active_pinned_retrieval_publication(
    controller: &AppController,
) -> Option<EmbeddingVectorPublicationIdentityDto> {
    active_pinned_retrieval_read(controller).map(|pinned| publication_dto(&pinned))
}

/// Canonical display names for exactly one published core generation.
///
/// The canonical node table cannot change inside a published core generation —
/// publication is atomic old-or-new — so streaming the whole table on every
/// pin re-reads it for an answer that could not have moved. The entry is keyed
/// by the storage identity plus the full core publication identity, and it is
/// revalidated against a live row count on every reuse: an in-place mutation
/// of the table invalidates the entry instead of hiding behind it, which is
/// the same consistency check the stream itself performs, at O(1).
pub(crate) struct CachedCanonicalSymbolNames {
    storage_path: PathBuf,
    core_generation_id: String,
    core_run_id: String,
    row_count: u32,
    node_names: Arc<HashMap<CoreNodeId, String>>,
}

impl CachedCanonicalSymbolNames {
    /// The complete reuse condition. Every component is load-bearing: the
    /// storage path binds the entry to one database, the core generation and
    /// run bind it to one publication of that database, and the row count is
    /// re-observed on every reuse so a canonical table that moved under a
    /// stable publication cannot be answered from a stale map.
    fn admits_reuse(
        &self,
        storage_path: &Path,
        publication: &codestory_retrieval::RetrievalPublicationIdentity,
        observed_rows: u32,
    ) -> bool {
        self.storage_path == storage_path
            && self.core_generation_id == publication.core_generation_id
            && self.core_run_id == publication.core_run_id
            && self.row_count == observed_rows
    }
}

#[derive(Default)]
pub(crate) struct CanonicalSymbolNamesState {
    cached: Option<CachedCanonicalSymbolNames>,
    /// Full canonical-table streams performed by this controller. Publication-
    /// keyed reuse is only meaningful if it can be observed.
    stream_count: u64,
}

impl CanonicalSymbolNamesState {
    #[cfg(test)]
    pub(crate) fn stream_count(&self) -> u64 {
        self.stream_count
    }

    pub(crate) fn clear(&mut self) {
        self.cached = None;
    }
}

fn canonical_symbol_names_for_session(
    controller: &AppController,
    storage_path: &Path,
    session: &PinnedQuerySession,
) -> Result<Arc<HashMap<CoreNodeId, String>>, ApiError> {
    let publication = session.publication_identity();
    let observed_rows = session
        .storage()
        .get_canonical_search_symbol_count()
        .map_err(|error| {
            ApiError::internal(format!("Failed to count canonical search symbols: {error}"))
        })?;
    {
        let state = controller.canonical_symbol_names.lock();
        if let Some(cached) = state.cached.as_ref()
            && cached.admits_reuse(storage_path, publication, observed_rows)
        {
            return Ok(Arc::clone(&cached.node_names));
        }
    }
    let node_names = Arc::new(
        crate::load_canonical_search_symbols(session.storage(), 10_000, None, |_| Ok(()))?.0,
    );
    let mut state = controller.canonical_symbol_names.lock();
    state.stream_count = state.stream_count.saturating_add(1);
    state.cached = Some(CachedCanonicalSymbolNames {
        storage_path: storage_path.to_path_buf(),
        core_generation_id: publication.core_generation_id.clone(),
        core_run_id: publication.core_run_id.clone(),
        row_count: observed_rows,
        node_names: Arc::clone(&node_names),
    });
    Ok(node_names)
}

impl PinnedRetrievalRead {
    fn begin(controller: &AppController) -> Result<Self, ApiError> {
        let project_root = controller.require_project_root()?;
        let storage_path = controller.require_storage_path()?;
        let session =
            PinnedQuerySession::begin(&project_root, &storage_path, &controller.runtime_config)
                .map_err(map_pinned_query_error)?;
        let node_names = canonical_symbol_names_for_session(controller, &storage_path, &session)?;
        Ok(Self {
            session,
            project_root,
            node_names,
        })
    }

    fn ensure_query_identity(&self, query: &QueryResult, operation: &str) -> Result<(), ApiError> {
        self.session
            .ensure_result_identity(query, operation)
            .map_err(map_pinned_query_error)
    }

    fn revalidate(&self) -> Result<(), ApiError> {
        self.session.revalidate().map_err(map_pinned_query_error)
    }
}

fn map_pinned_query_error(error: AnyhowError) -> ApiError {
    if let Some(error) = crate::services::embedding_api_error(&error) {
        error
    } else if is_retrieval_publication_changed(&error) {
        ApiError::new("publication_changed", error.to_string())
    } else {
        ApiError::new("cache_busy", error.to_string())
    }
}

fn with_pinned_retrieval_read<T>(
    controller: &AppController,
    read: impl FnOnce(&PinnedRetrievalRead) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    if let Some(pinned) = active_pinned_retrieval_read(controller) {
        return read(&pinned);
    }
    let pinned = PinnedRetrievalRead::begin(controller)?;
    let value = read(&pinned)?;
    pinned.revalidate()?;
    Ok(value)
}

pub(crate) fn with_stable_retrieval_publication<T: RetrievalPublicationResponse>(
    controller: &AppController,
    operation: &str,
    mut build: impl FnMut() -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    if let Some(pinned) = active_pinned_retrieval_read(controller) {
        let mut response = build()?;
        response.attach_retrieval_publication(publication_dto(&pinned));
        return Ok(response);
    }
    if !sidecar_retrieval_primary_enabled(controller) {
        return build();
    }
    with_stable_retrieval_publication_inner(controller, operation, build, |_| Ok(()))
}

pub(crate) fn with_pinned_retrieval_publication_value<T>(
    controller: &AppController,
    expected_core_generation_id: &str,
    expected_core_run_id: &str,
    build: impl FnOnce() -> Result<T, ApiError>,
) -> Result<(T, Option<EmbeddingVectorPublicationIdentityDto>), ApiError> {
    // The active pin is checked before the enablement gate, matching
    // `with_stable_retrieval_publication`. A pin only becomes active after that
    // gate admitted it, so an active pin already carries the answer; asking
    // again costs a whole-repository strict-readiness fingerprint pass and can
    // only disagree with the publication this operation is already pinned to.
    if let Some(pinned) = active_pinned_retrieval_read(controller) {
        ensure_pinned_core_publication(&pinned, expected_core_generation_id, expected_core_run_id)?;
        let publication = publication_dto(&pinned);
        let value = build()?;
        return Ok((value, Some(publication)));
    }
    if !sidecar_retrieval_primary_enabled(controller) {
        return build().map(|value| (value, None));
    }

    let pinned = Rc::new(PinnedRetrievalRead::begin(controller)?);
    ensure_pinned_core_publication(&pinned, expected_core_generation_id, expected_core_run_id)?;
    let publication = publication_dto(&pinned);
    with_active_pinned_retrieval_read(controller, Rc::clone(&pinned), || {
        build().and_then(|value| {
            pinned.revalidate()?;
            Ok((value, Some(publication)))
        })
    })
}

fn with_stable_retrieval_publication_inner<T: RetrievalPublicationResponse>(
    controller: &AppController,
    operation: &str,
    mut build: impl FnMut() -> Result<T, ApiError>,
    mut after_retry: impl FnMut(usize) -> Result<(), ApiError>,
) -> Result<T, ApiError> {
    for attempt in 0..RETRIEVAL_PUBLICATION_ATTEMPTS {
        let pinned = Rc::new(PinnedRetrievalRead::begin(controller)?);
        let publication = publication_dto(&pinned);
        let result = with_active_pinned_retrieval_read(controller, Rc::clone(&pinned), || {
            build().and_then(|mut response| {
                response.attach_retrieval_publication(publication.clone());
                pinned.revalidate()?;
                Ok(response)
            })
        });
        match result {
            Err(error)
                if error.code == "publication_changed"
                    && attempt + 1 < RETRIEVAL_PUBLICATION_ATTEMPTS =>
            {
                tracing::debug!(operation, "retrying complete pinned retrieval operation");
                drop(pinned);
                after_retry(attempt + 1)?;
            }
            result => return result,
        }
    }
    unreachable!("bounded retrieval attempts always return")
}

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

/// Whether published retrieval should serve packet and search results.
///
/// - `CODESTORY_RETRIEVAL=1` requires the published agent retrieval generation.
/// - `CODESTORY_RETRIEVAL=0` is unsupported; packet paths fail closed.
/// - Unset: retrieval is available when the manifest exists and the shared
///   per-user embedding server is healthy.
pub(crate) fn sidecar_retrieval_primary_enabled(controller: &AppController) -> bool {
    match retrieval_env_override() {
        Some(false) => {
            tracing::error!("CODESTORY_RETRIEVAL=0 is unsupported; full retrieval is mandatory");
            false
        }
        Some(true) => {
            sidecar_retrieval_eligible(controller) && sidecar_mode_is_required_full(controller)
        }
        None => {
            // Default product path: serve only from full agent-scoped retrieval.
            let auto_on =
                sidecar_retrieval_eligible(controller) && sidecar_mode_is_required_full(controller);
            if auto_on {
                tracing::info!(
                    "retrieval primary auto-on (unset CODESTORY_RETRIEVAL; agent retrieval full)"
                );
            }
            auto_on
        }
    }
}

pub(crate) fn sidecar_retrieval_unavailable_reason(controller: &AppController) -> Option<String> {
    if retrieval_env_override() == Some(false) {
        return Some("CODESTORY_RETRIEVAL=0 is unsupported; full retrieval is mandatory".into());
    }
    if sidecar_retrieval_primary_enabled(controller) {
        return None;
    }
    let Ok(project_root) = controller.require_project_root() else {
        return Some("retrieval requires an open project".into());
    };
    let Ok(storage_path) = controller.require_storage_path() else {
        return Some("retrieval requires an index storage path".into());
    };
    let status =
        sidecar_mode_status_for_runtime(&project_root, &storage_path, &controller.runtime_config);
    let reason = status
        .degraded_reason
        .map(|reason| format!("; reason={reason}"))
        .unwrap_or_default();
    let profile = status.profile.as_deref().unwrap_or("unknown");
    Some(format!(
        "retrieval is unavailable or degraded (profile={profile} mode={}); expected profile=agent mode=full{reason}",
        status.mode
    ))
}

pub(crate) fn sidecar_retrieval_unavailable_error(
    controller: &AppController,
    reason: impl Into<String>,
) -> ApiError {
    let project_root = controller.require_project_root().ok();
    let project = project_root
        .as_ref()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "<project>".to_string());
    let recovery_commands = project_root
        .as_deref()
        .map(|project_root| {
            sidecar_retrieval_recovery_commands_for_runtime(
                project_root,
                &controller.runtime_config,
            )
        })
        .unwrap_or_else(|| sidecar_retrieval_recovery_commands_for_project(&project, None));
    ApiError::retrieval_unavailable(reason, project.clone(), recovery_commands)
}

fn sidecar_retrieval_recovery_commands_for_runtime(
    project_root: &Path,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> Vec<String> {
    let agent_run_id = (runtime.profile == SidecarProfile::Agent)
        .then_some(runtime.run_id.as_deref())
        .flatten();
    sidecar_retrieval_recovery_commands_for_project(&project_root.to_string_lossy(), agent_run_id)
}

fn sidecar_retrieval_recovery_commands_for_project(
    project: &str,
    agent_run_id: Option<&str>,
) -> Vec<String> {
    let project = quote_cli_arg(project);
    let mut activate =
        format!("codestory-cli retrieval index --profile agent --refresh auto --project {project}");
    let mut status = format!("codestory-cli retrieval status --project {project}");
    if let Some(run_id) = agent_run_id {
        activate.push_str(" --run-id ");
        activate.push_str(run_id);
        status.push_str(" --profile agent --run-id ");
        status.push_str(run_id);
    }
    activate.push_str(" --format json");
    status.push_str(" --format json");
    vec![
        activate,
        status,
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
    sidecar_primary_blocks_nucleo_supplement(
        sidecar_retrieval_primary_enabled(controller),
        served_hit_count,
    )
}

pub(crate) fn sidecar_primary_blocks_nucleo_supplement(
    sidecar_primary_enabled: bool,
    _served_hit_count: usize,
) -> bool {
    // Sidecar-primary packets never mix in-process Nucleo, including when the
    // sidecar returned zero hits. Nucleo on an empty sidecar would otherwise
    // become product evidence on a retrieval-claimed path.
    sidecar_primary_enabled
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
    sidecar_status_can_serve_primary(&sidecar_mode_status_for_runtime(
        &project_root,
        &storage_path,
        &controller.runtime_config,
    ))
}

fn sidecar_mode_can_serve_primary(mode: &str) -> bool {
    mode == "full"
}

fn sidecar_status_can_serve_primary(status: &SidecarModeStatus) -> bool {
    status.profile.as_deref() == Some("agent")
        && sidecar_mode_can_serve_primary(&status.mode)
        && status.degraded_reason.is_none()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarModeStatus {
    profile: Option<String>,
    mode: String,
    degraded_reason: Option<String>,
}

fn sidecar_mode_status_for_runtime(
    project_root: &Path,
    storage_path: &Path,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> SidecarModeStatus {
    match strict_sidecar_status_for_runtime(project_root, Some(storage_path), runtime.clone()) {
        Ok(report) => SidecarModeStatus {
            profile: Some(runtime.profile.as_str().to_string()),
            mode: report.retrieval_mode,
            degraded_reason: report.degraded_reason,
        },
        Err(error) => SidecarModeStatus {
            profile: None,
            mode: "unavailable".into(),
            degraded_reason: Some(format!("retrieval_status_error: {error}")),
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
    if let Some(reason) = sidecar_blocking_cancel_reason(query_result) {
        return Some(format!(
            "sidecar retrieval trace `{reason}` is not eligible for primary results"
        ));
    }
    if !query_result.hits.is_empty() && resolved_hits.is_empty() {
        return Some("sidecar retrieval candidates did not resolve to indexed symbols".into());
    }
    None
}

fn sidecar_blocking_cancel_reason(query_result: &QueryResult) -> Option<&str> {
    match query_result.trace.cancel_reason.as_deref() {
        Some("deadline" | "stage_deadline" | "cancelled") => {
            query_result.trace.cancel_reason.as_deref()
        }
        _ => None,
    }
}

pub(crate) fn sidecar_budget_ms(latency_budget_ms: Option<u32>) -> u64 {
    latency_budget_ms
        .map(u64::from)
        .unwrap_or(DEFAULT_SIDECAR_BUDGET_MS)
        .clamp(MIN_PACKET_BATCH_BUDGET_MS, MAX_PACKET_BATCH_BUDGET_MS)
}

fn sidecar_packet_batch_budget_ms(latency_budget_ms: Option<u32>) -> u64 {
    latency_budget_ms
        .map(u64::from)
        .unwrap_or(DEFAULT_PACKET_BATCH_BUDGET_MS)
        .clamp(MIN_PACKET_BATCH_BUDGET_MS, MAX_PACKET_BATCH_BUDGET_MS)
}

fn with_detached_sidecar_query_cache<T>(
    controller: &AppController,
    work: impl FnOnce(&mut codestory_retrieval::RetrievalCache) -> T,
) -> T {
    let (generation, mut cache) = {
        let shared = controller.sidecar_query_cache.lock();
        shared.snapshot()
    };
    let baseline = cache.clone();
    let result = work(&mut cache);
    controller
        .sidecar_query_cache
        .lock()
        .merge_if_current(generation, &baseline, cache);
    result
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
    with_detached_sidecar_query_cache(controller, |cache| {
        execute_retrieval_query_with_cache_for_runtime(
            QueryRequest {
                project_root: &project_root,
                storage_path: &storage_path,
                query,
                budget_ms: Some(sidecar_budget_ms(latency_budget_ms)),
                cancelled: crate::services::active_public_operation_cancellation(),
            },
            cache,
            &controller.runtime_config,
        )
    })
}

pub(crate) fn run_and_resolve_sidecar_query(
    controller: &AppController,
    query: &str,
    max_results: usize,
    latency_budget_ms: Option<u32>,
) -> Result<(QueryResult, SidecarCandidateResolutionOutcome), ApiError> {
    with_pinned_retrieval_read(controller, |pinned| {
        let query_result = with_detached_sidecar_query_cache(controller, |cache| {
            pinned.session.execute_with_cache(
                query,
                Some(sidecar_budget_ms(latency_budget_ms)),
                crate::services::active_public_operation_cancellation(),
                cache,
            )
        })
        .map_err(map_pinned_query_error)?;
        pinned.ensure_query_identity(&query_result, "resolving sidecar candidates")?;
        let resolution =
            resolve_sidecar_candidates_in_read(pinned, &query_result.hits, max_results)?;
        Ok((query_result, resolution))
    })
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
    Retryable {
        error: ApiError,
    },
    Served {
        hits: Vec<SearchHit>,
        packet_hits: Vec<PacketSearchHit>,
        scored_hits: Vec<HybridSearchScoredHit>,
        shadow: RetrievalShadowDto,
    },
}

fn sidecar_primary_error_outcome(error: ApiError) -> SidecarPrimarySearchOutcome {
    if matches!(
        error.code.as_str(),
        "embedding_capacity" | "embedding_retryable" | "cache_busy" | "publication_changed"
    ) {
        SidecarPrimarySearchOutcome::Retryable { error }
    } else {
        SidecarPrimarySearchOutcome::Unavailable {
            reason: format!("retrieval unavailable: {}", error.message),
        }
    }
}

pub(crate) fn try_sidecar_primary_search(
    controller: &AppController,
    prompt: &str,
    max_results: usize,
    latency_budget_ms: Option<u32>,
) -> Option<SidecarPrimarySearchOutcome> {
    if !sidecar_retrieval_primary_enabled(controller) {
        return sidecar_retrieval_unavailable_reason(controller)
            .map(|reason| SidecarPrimarySearchOutcome::Unavailable { reason });
    }
    match run_and_resolve_sidecar_query(controller, prompt, max_results, latency_budget_ms) {
        Ok((query_result, resolution)) => Some(sidecar_primary_search_outcome_from_resolution(
            controller,
            query_result,
            resolution,
        )),
        Err(error) => Some(sidecar_primary_error_outcome(error)),
    }
}

#[cfg(test)]
fn sidecar_primary_search_outcome_from_query_result(
    controller: &AppController,
    query_result: QueryResult,
    max_results: usize,
) -> SidecarPrimarySearchOutcome {
    let resolution =
        match resolve_sidecar_candidates_for_test(controller, &query_result.hits, max_results) {
            Ok(hits) => hits,
            Err(error) => {
                return SidecarPrimarySearchOutcome::Unavailable {
                    reason: format!(
                        "retrieval unavailable: candidate resolution failed: {}",
                        error.message
                    ),
                };
            }
        };
    sidecar_primary_search_outcome_from_resolution(controller, query_result, resolution)
}

fn sidecar_primary_search_outcome_from_resolution(
    controller: &AppController,
    query_result: QueryResult,
    resolution: SidecarCandidateResolutionOutcome,
) -> SidecarPrimarySearchOutcome {
    let resolved_hits = resolution.resolved_hits.clone();
    let shadow = shadow_from_query_result_with_candidate_admission_diagnostics(
        controller,
        query_result.clone(),
        &resolution,
        &resolved_hits,
        &resolved_hits,
    );

    if let Some(reason) = sidecar_primary_result_rejection_reason(&query_result, &resolved_hits) {
        let diagnostic = sidecar_rejection_diagnostic(controller, &query_result, &resolved_hits, 5);
        let reason = format!("{reason}; {diagnostic}");
        return SidecarPrimarySearchOutcome::Rejected { shadow, reason };
    }

    let hits = resolved_hits;

    let scored_hits = hits
        .iter()
        .cloned()
        .map(HybridSearchScoredHit::from_search_hit)
        .collect();
    let packet_hits = resolution.packet_hits;
    debug_assert_eq!(packet_hits.len(), hits.len());

    SidecarPrimarySearchOutcome::Served {
        hits,
        packet_hits,
        scored_hits,
        shadow,
    }
}

pub(crate) fn sidecar_primary_result_rejection_reason(
    query_result: &QueryResult,
    resolved_hits: &[SearchHit],
) -> Option<String> {
    let reason = sidecar_result_rejection_reason(query_result, resolved_hits)?;
    if sidecar_blocking_cancel_reason(query_result).is_some() && !resolved_hits.is_empty() {
        return None;
    }
    Some(reason)
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

#[derive(Debug)]
pub(crate) struct SidecarPacketBatchOutcome {
    pub results: Vec<(String, Vec<PacketSearchHit>)>,
    pub retryable_queries: Vec<String>,
    pub diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
}

pub(crate) struct SidecarCandidateResolutionOutcome {
    pub(crate) resolved_hits: Vec<SearchHit>,
    pub(crate) packet_hits: Vec<PacketSearchHit>,
    unresolved_candidate_count: usize,
    blocking_unresolved_candidate_count: usize,
    attempted_candidate_indices: HashSet<usize>,
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
    let semantic = semantic_stage_degradation(&stage_timings);
    // EV-8: a required query whose dense lane went dark and then resolved nothing produced no
    // evidence, but the sidecar itself reports no blocking cancel — the stage budget, not the
    // query, ran out. Left as `Completed` it would satisfy its query obligation on an empty
    // result. Naming the cancel here is what lets the obligation ledger demote it.
    let semantic_timeout_without_hits =
        semantic.timed_out_zero_hits && resolution.resolved_hits.is_empty();
    let cancel_reason = sidecar_blocking_cancel_reason(query_result)
        .map(str::to_string)
        .or_else(|| {
            semantic_timeout_without_hits.then(|| SEMANTIC_TIMEOUT_ZERO_HITS_CANCEL.to_string())
        });
    PacketSidecarQueryDiagnosticDto {
        query: query_result.query.clone(),
        completion: cancel_reason
            .clone()
            .map_or(PacketQueryCompletionDto::Completed, |reason| {
                PacketQueryCompletionDto::Cancelled { reason }
            }),
        retrieval_mode: query_result.trace.retrieval_mode.clone(),
        sidecar_query_ms: Some(sidecar_query_ms),
        candidate_resolution_ms: Some(candidate_resolution_ms),
        total_elapsed_ms: Some(total_elapsed_ms),
        sidecar_stage_count: u32::try_from(stage_timings.len()).unwrap_or(u32::MAX),
        sidecar_stage_total_ms: Some(sidecar_stage_total_ms),
        batch_query_wall_ms: Some(batch_query_wall_ms),
        candidate_count: u32::try_from(resolution.attempted_candidate_indices.len())
            .unwrap_or(u32::MAX),
        resolved_hit_count: u32::try_from(resolution.resolved_hits.len()).unwrap_or(u32::MAX),
        unresolved_candidate_count: u32::try_from(resolution.unresolved_candidate_count)
            .unwrap_or(u32::MAX),
        blocking_unresolved_candidate_count: u32::try_from(
            resolution.blocking_unresolved_candidate_count,
        )
        .unwrap_or(u32::MAX),
        semantic_stage_timeout_zero_hits: semantic.timed_out_zero_hits,
        semantic_abstained: semantic.abstained,
        diagnostic: cancel_reason
            .map(|reason| format!("sidecar query has blocking cancel reason `{reason}`"))
            .or_else(|| {
                (resolution.unresolved_candidate_count > 0).then(|| {
                    "sidecar candidates did not all resolve to indexed symbols".to_string()
                })
            }),
    }
}

fn search_sidecar_packet_batch_inner(
    controller: &AppController,
    queries: &[(String, usize)],
    latency_budget_ms: Option<u32>,
) -> Result<SidecarPacketBatchOutcome, ApiError> {
    let per_query_budget = sidecar_packet_batch_budget_ms(latency_budget_ms)
        .checked_div(queries.len().max(1) as u64)
        .unwrap_or(100)
        .max(100);
    let batch_queries = queries
        .iter()
        .map(|(query, _)| (query.clone(), per_query_budget))
        .collect::<Vec<_>>();
    with_pinned_retrieval_read(controller, |pinned| {
        let batch_started_at = Instant::now();
        let batch_items = batch_queries
            .iter()
            .map(|(query, budget_ms)| QueryBatchItem {
                query,
                budget_ms: Some(*budget_ms),
            })
            .collect::<Vec<_>>();
        let query_results = with_detached_sidecar_query_cache(controller, |cache| {
            pinned.session.execute_batch_with_cache(
                &batch_items,
                crate::services::active_public_operation_cancellation(),
                cache,
            )
        })
        .map_err(map_pinned_query_error)?;
        for result in &query_results {
            pinned.ensure_query_identity(result, "resolving sidecar packet batch")?;
        }
        build_sidecar_packet_batch_outcome(
            controller,
            queries,
            query_results,
            clamp_elapsed_ms(batch_started_at),
            |query_result, max_results| {
                resolve_sidecar_candidates_in_read(pinned, &query_result.hits, max_results)
            },
        )
    })
}

#[cfg(test)]
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
    build_sidecar_packet_batch_outcome(
        controller,
        queries,
        query_results,
        batch_query_wall_ms,
        |query_result, max_results| {
            resolve_sidecar_candidates_for_test(controller, &query_result.hits, max_results)
        },
    )
}

fn build_sidecar_packet_batch_outcome(
    controller: &AppController,
    queries: &[(String, usize)],
    query_results: Vec<QueryResult>,
    batch_query_wall_ms: u32,
    mut resolve: impl FnMut(&QueryResult, usize) -> Result<SidecarCandidateResolutionOutcome, ApiError>,
) -> Result<SidecarPacketBatchOutcome, ApiError> {
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
    let mut retryable_queries = Vec::new();
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
        if sidecar_blocking_cancel_reason(&query_result) == Some("cancelled") {
            return Err(ApiError::new(
                "cancelled",
                format!("packet fused batch query `{query}` was cancelled"),
            ));
        }
        let sidecar_query_ms = u32::try_from(query_result.trace.elapsed_ms).unwrap_or(u32::MAX);
        let max_results = (*max_results).clamp(1, 50);
        let resolution_started_at = Instant::now();
        let resolution = resolve(&query_result, max_results).map_err(|error| {
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
        let packet_hits = resolution.packet_hits;
        let resolved_hits = resolution.resolved_hits;
        // Bound before the rejection branch below can consume `packet_hits`.
        if let Some(reason) = sidecar_packet_batch_rejection_reason(&query_result, &resolved_hits) {
            if let Some("deadline" | "stage_deadline") =
                sidecar_blocking_cancel_reason(&query_result)
            {
                retryable_queries.push(query.clone());
                // A deadline-cancelled batch query used to contribute
                // NOTHING: every candidate it had already resolved was
                // discarded here, before any scoring, ranking or carry could
                // see it. The single-query path
                // (`sidecar_primary_result_rejection_reason`) already SERVES
                // resolved hits from a cancelled query; that asymmetry was a
                // plain defect with nothing to do with atoms, discarding real
                // retrieval work on every deadline-pressured packet. The
                // batch path now matches those semantics — see
                // [`retained_cancelled_packet_hits`] — and the query is still
                // marked retryable, so the retry runs exactly as before.
                results.push((query.clone(), retained_cancelled_packet_hits(packet_hits)));
                continue;
            }
            let diagnostic =
                sidecar_rejection_diagnostic(controller, &query_result, &resolved_hits, 5);
            return Err(sidecar_retrieval_unavailable_error(
                controller,
                format!(
                    "sidecar retrieval rejected packet batch query `{query}`: {reason}; {diagnostic}"
                ),
            ));
        }
        debug_assert_eq!(packet_hits.len(), resolved_hits.len());
        results.push((query.clone(), packet_hits));
    }
    Ok(SidecarPacketBatchOutcome {
        results,
        retryable_queries,
        diagnostics,
    })
}

/// The resolved hits a DEADLINE-CANCELLED batch query still contributes.
///
/// A cancelled batch query used to contribute NOTHING: every candidate it had
/// already resolved was discarded before scoring, ranking or carry could see
/// it. Gate 9 measured what that costs on a slow shard — all 32 queries
/// cancelled, 327 resolved hits thrown away, and the classes the formulas
/// needed among them. The single-query path already decided this question the
/// other way: `sidecar_primary_result_rejection_reason` SERVES resolved hits
/// from a cancelled query rather than dropping them. That asymmetry was a
/// plain defect, unrelated to atoms — it silently discarded real retrieval
/// work on every deadline-pressured packet — so the batch path now matches
/// the single-query semantics.
///
/// Retention is NOT atom-gated: every resolved hit is kept, subject to every
/// existing downstream limit, and the query is still marked retryable so the
/// retry runs exactly as before. The atom signal only ORDERS the result —
/// resolution rank first, need-first as a tiebreak among equal ranks, then
/// the original resolution order — so that when a downstream limit binds it
/// binds on the identities that occupy the most role positions of the
/// requirement group rather than on whichever tied hit came first. No slot is
/// added anywhere, and nothing here marks a hit as proven (contract rule 4:
/// atom need is a selection input, never a proof input).
fn retained_cancelled_packet_hits(hits: Vec<PacketSearchHit>) -> Vec<PacketSearchHit> {
    let session = crate::agent::packet_candidate::active_packet_proof_session()
        .filter(|session| session.has_atom_needed_identities());
    let need_rank = |hit: &PacketSearchHit| {
        session
            .as_ref()
            .and_then(|session| session.citation_atom_priority(&hit.hit.node_id))
            .map_or(0, |priority| priority + 1)
    };
    let mut retained = hits.into_iter().enumerate().collect::<Vec<_>>();
    retained.sort_by(|(left_rank, left), (right_rank, right)| {
        right
            .hit
            .score
            .partial_cmp(&left.hit.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| need_rank(right).cmp(&need_rank(left)))
            .then(left_rank.cmp(right_rank))
    });
    retained.into_iter().map(|(_, hit)| hit).collect()
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
    if let Some(reason) = sidecar_blocking_cancel_reason(query_result) {
        return Some(format!(
            "sidecar retrieval trace `{reason}` is not eligible for packet batch results"
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
    resolution: &SidecarCandidateResolutionOutcome,
    search_hits: &[SearchHit],
    final_hits: &[SearchHit],
) -> RetrievalShadowDto {
    let resolution_labels = sidecar_candidate_resolution_labels(
        controller,
        &result.hits,
        &resolution.attempted_candidate_indices,
    );
    let admission_labels = sidecar_candidate_admission_labels(
        controller,
        &result.hits,
        &resolution_labels,
        search_hits,
        final_hits,
    );
    shadow_from_query_result_with_counts_and_resolution_labels(
        result,
        resolution.attempted_candidate_indices.len(),
        resolution.resolved_hits.len(),
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
    let storage = controller.open_storage_read_only().ok();
    // Deliberate full-map copy: resolution discovers node ids mid-walk from storage, so an upfront candidate bound would change diagnostic labels.
    let node_names = controller.state.lock().node_names.clone();
    let candidate_summaries: Vec<String> = query_result
        .hits
        .iter()
        .take(max_candidates)
        .enumerate()
        .map(|(index, candidate)| {
            let resolution = candidate_resolution_label(
                project_root.as_deref(),
                storage.as_deref(),
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
    attempted_candidate_indices: &HashSet<usize>,
) -> Vec<String> {
    let project_root = controller.require_project_root().ok();
    let storage = controller.open_storage_read_only().ok();
    // Deliberate full-map copy: resolution discovers node ids mid-walk from storage, so an upfront candidate bound would change diagnostic labels.
    let node_names = controller.state.lock().node_names.clone();
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            if !attempted_candidate_indices.contains(&index) {
                return "not_attempted".to_string();
            }
            candidate_resolution_label(
                project_root.as_deref(),
                storage.as_deref(),
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
    let storage = controller.open_storage_read_only().ok();
    // Deliberate full-map copy: resolution discovers node ids mid-walk from storage, so an upfront candidate bound would change diagnostic labels.
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
                if resolution == "not_attempted" {
                    return SidecarCandidateAdmissionLabel {
                        admission_status: "rejected".to_string(),
                        loss_reason: Some("not_in_resolution_window".to_string()),
                        resolved_node_id: None,
                        search_hit_rank: None,
                        final_rank: None,
                    };
                }
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
            let search_hit_rank = candidate_admission_rank(
                resolved_node_id_text.as_deref(),
                &rel_path,
                &search_nodes,
                &search_paths,
            );
            let final_rank = candidate_admission_rank(
                resolved_node_id_text.as_deref(),
                &rel_path,
                &final_nodes,
                &final_paths,
            );
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

fn candidate_admission_rank(
    resolved_node_id: Option<&str>,
    relative_path: &str,
    ranked_nodes: &HashMap<String, u32>,
    ranked_paths: &HashMap<String, u32>,
) -> Option<u32> {
    match resolved_node_id {
        Some(node_id) => ranked_nodes.get(node_id).copied(),
        None => ranked_paths.get(relative_path).copied(),
    }
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

    let candidates = shadow_candidate_indices(&result.hits, resolution_labels)
        .into_iter()
        .map(|index| {
            let hit = &result.hits[index];
            RetrievalCandidateSummaryDto {
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
            }
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
            .filter(|label| !matches!(label.as_str(), "resolved" | "not_attempted"))
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
            .filter(|(_, label)| !matches!(label.as_str(), "resolved" | "not_attempted"))
            .all(|(candidate, label)| {
                unresolved_candidate_is_diagnostic(candidate, label, has_resolved_hit)
            })
}

fn shadow_candidate_indices(
    candidates: &[CandidateHit],
    resolution_labels: &[String],
) -> Vec<usize> {
    let mut indices = (0..candidates.len().min(MAX_SHADOW_CANDIDATES)).collect::<Vec<_>>();
    let has_resolved_hit = resolution_labels
        .iter()
        .any(|label| label.as_str() == "resolved");
    let blocking_index = candidates
        .iter()
        .zip(resolution_labels)
        .enumerate()
        .skip(MAX_SHADOW_CANDIDATES)
        .find_map(|(index, (candidate, label))| {
            (label != "resolved"
                && label != "not_attempted"
                && !unresolved_candidate_is_diagnostic(candidate, label, has_resolved_hit))
            .then_some(index)
        });
    if let Some(blocking_index) = blocking_index
        && let Some(last_index) = indices.last_mut()
    {
        *last_index = blocking_index;
    }
    indices
}

fn unresolved_candidate_is_diagnostic(
    candidate: &CandidateHit,
    resolution_label: &str,
    has_resolved_hit: bool,
) -> bool {
    bare_dense_anchor_unresolved(candidate, resolution_label)
        || (has_resolved_hit
            && non_parser_backed_file_candidate_unresolved(candidate, resolution_label))
}

fn bare_dense_anchor_unresolved(candidate: &CandidateHit, resolution_label: &str) -> bool {
    resolution_label == "path_unresolvable"
        && candidate.source == CandidateSource::Semantic
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

fn non_parser_backed_file_candidate_unresolved(
    candidate: &CandidateHit,
    resolution_label: &str,
) -> bool {
    // `node_unresolved` is assigned only after the candidate path resolves.
    resolution_label == "node_unresolved"
        && candidate.source == CandidateSource::Lexical
        && candidate.symbol_name.is_none()
        && known_non_symbol_file_path(&candidate.file_path)
}

fn known_non_symbol_file_path(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    let extension = lower.rsplit('.').next();
    matches!(
        extension,
        Some("cfg" | "conf" | "def" | "ini" | "json" | "md" | "markdown" | "toml" | "yaml" | "yml")
    ) || extension
        .is_some_and(|value| value.len() == 1 && matches!(value.as_bytes()[0], b'1'..=b'9'))
        || (extension == Some("zsh")
            && lower
                .split('/')
                .any(|segment| matches!(segment, "complete" | "completion" | "completions")))
}

fn retrieval_stage_timings(trace: &QueryTrace) -> Vec<RetrievalStageTimingDto> {
    trace
        .stages
        .iter()
        .map(|stage| RetrievalStageTimingDto {
            stage: stage.stage.label().to_string(),
            deadline_ms: u32::try_from(stage.budget_ms).ok(),
            elapsed_ms: u32::try_from(stage.elapsed_ms).unwrap_or(u32::MAX),
            admission_wait_ms: u32::try_from(stage.admission_wait_ms).ok(),
            queue_wait_ms: stage.queue_wait_ms.and_then(|ms| u32::try_from(ms).ok()),
            execution_ms: stage.execution_ms.and_then(|ms| u32::try_from(ms).ok()),
            candidates_added: u32::try_from(stage.candidates_added).unwrap_or(u32::MAX),
            marginal_gain: stage.marginal_gain,
            cancel_reason: stage.cancel_reason.clone(),
            cache_hit: stage.cache_hit,
            sidecar_latency_ms: stage
                .execution_ms
                .and_then(|ms| stage.stage.sidecar_latency_ms(ms)),
            degraded: stage.degraded,
            stub_reason: stage.stub_reason.clone(),
            completion_status: match stage.completion_status {
                codestory_retrieval::StageCompletionStatus::Completed => "completed",
                codestory_retrieval::StageCompletionStatus::PendingAfterDeadline => {
                    "pending_after_deadline"
                }
                codestory_retrieval::StageCompletionStatus::CancelledBeforeStart => {
                    "cancelled_before_start"
                }
                codestory_retrieval::StageCompletionStatus::CompletedLate => "completed_late",
                codestory_retrieval::StageCompletionStatus::Skipped => "skipped",
            }
            .into(),
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

/// Stable-partition resolvable candidates ahead of unresolvable ones.
///
/// `path_resolvable` stats the filesystem, so it is decorated once per surviving
/// candidate instead of once per comparison, and the resolved verdict is handed
/// back for the unresolved-candidate label. Retrieval already assigned the
/// canonical fused order, including its exact-definition bucket and stable
/// tie-breaks, so resolution must not create a second score comparator.
fn ordered_sidecar_candidates<F>(
    candidates: &[CandidateHit],
    mut path_resolvable: F,
) -> Vec<(usize, &CandidateHit, bool)>
where
    F: FnMut(&CandidateHit) -> bool,
{
    let mut ordered = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| !is_phantom_sidecar_hit(candidate))
        .map(|(index, candidate)| {
            let resolvable = path_resolvable(candidate);
            (index, candidate, resolvable)
        })
        .collect::<Vec<_>>();
    ordered.sort_by(
        |(left_index, _, left_resolvable), (right_index, _, right_resolvable)| {
            right_resolvable
                .cmp(left_resolvable)
                .then_with(|| left_index.cmp(right_index))
        },
    );
    ordered
}

fn candidate_path_text_is_path_like(path: &str) -> bool {
    let trimmed = path.trim();
    !trimmed.is_empty()
        && !trimmed.contains(':')
        && (trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains('.'))
}

fn normalize_repo_relative_path(project_root: &Path, file_path: &str) -> String {
    let normalized = normalize_storage_path_text(file_path);
    codestory_workspace::workspace_relative_path(project_root, Path::new(&normalized))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or(normalized)
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
    if candidate.target.is_some() {
        return candidate_lookup_paths(project_root, rel_path)
            .into_iter()
            .find_map(|path| storage.get_file_by_path(&path).ok().flatten())
            .map(|file| CoreNodeId(file.id));
    }

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

fn resolve_sidecar_candidates_in_read(
    pinned: &PinnedRetrievalRead,
    candidates: &[CandidateHit],
    max_results: usize,
) -> Result<SidecarCandidateResolutionOutcome, ApiError> {
    resolve_sidecar_candidates_in_storage(
        pinned.session.storage(),
        &pinned.node_names,
        &pinned.project_root,
        candidates,
        max_results,
    )
}

/// One resolved candidate's hydrated graph: edge provenance, the bounded
/// candidate graph, and the per-trail coverage records (R2).
type PacketCandidateGraphHydration = (
    Vec<PacketGraphEdgeProvenance>,
    Option<GraphResponse>,
    Vec<PacketCandidateTrailScan>,
);

const PACKET_CANDIDATE_DIRECTION_NODE_LIMIT: usize = 65;
const PACKET_FILE_STRUCTURAL_TRAIL_DEPTH: u32 = 2;
const PACKET_EXACT_CALL_BOUNDARY_EDGE_LIMIT: u32 = 128;
const PACKET_EXACT_CALL_BOUNDARY_ARTIFACT_PREFIX: &str = "packet-exact-call-boundary-";

/// Node cap of the POST-PASS depth-2 FILE structural trail (round 5.5 item 1
/// residual, option (ii)).
///
/// The store's BFS accessor derives its edge budget from the node cap
/// (`max_nodes × 3`, storage_impl/trail.rs) and breaks out of the traversal
/// the moment that budget is exhausted — at the ROOT that break leaves only
/// the root in the node set, and the accessor's closing endpoint filter then
/// drops every fetched edge, so the artifact comes back EMPTY and is skipped.
/// A real CSS entrypoint has 198+ outgoing structural edges under the uniform
/// `[MEMBER, USAGE, IMPORT]` filter, which crosses the 65-node cap's 195-edge
/// budget and silences the whole trail — taking C1's MODULE-member receipts
/// with it. 130 nodes lifts the edge budget to 390, above entrypoint-scale
/// fanout, so the root's own edges are enumerated and the depth-2 frontier is
/// reached.
///
/// The trail is deliberately NOT split per kind: rule 7's deeper-rooted arm
/// requires the absent kind AND its MEMBER witness in the SAME coverage
/// record, so C3's covering scan must stay one traversal set. The
/// store-accessor pathology itself is a recorded post-acceptance follow-up —
/// it touches every trail consumer.
const PACKET_POST_PASS_STRUCTURAL_NODE_LIMIT: usize = 130;

/// Builds one narrowed scan record (F3 finding 3): the recorded coverage set
/// keeps only the enumerated edges of absence-subject kinds plus — for
/// depth-2 scans — the enumerated MEMBER witness edges. See
/// [`PacketCandidateTrailScan`].
fn packet_trail_scan_record(
    root: &str,
    direction: PacketGraphDirection,
    depth: u32,
    filter: &[EdgeKind],
    trail: &codestory_contracts::graph::TrailResult,
    absence_kinds: &[codestory_contracts::api::EdgeKind],
) -> PacketCandidateTrailScan {
    PacketCandidateTrailScan {
        root: root.to_string(),
        direction,
        depth,
        edge_kinds: filter
            .iter()
            .map(|kind| codestory_contracts::api::EdgeKind::from(*kind))
            .collect(),
        truncated: trail.truncated,
        coverage_edge_ids: trail
            .edges
            .iter()
            .filter(|edge| {
                let kind = codestory_contracts::api::EdgeKind::from(edge.kind);
                absence_kinds.contains(&kind)
                    || (depth >= 2 && kind == codestory_contracts::api::EdgeKind::MEMBER)
            })
            .map(|edge| codestory_contracts::api::EdgeId::from(edge.id))
            .collect(),
    }
}

fn packet_graph_for_resolved_candidate(
    storage: &Store,
    node_names: &HashMap<CoreNodeId, String>,
    node_id: CoreNodeId,
    candidate: &CandidateHit,
) -> Result<PacketCandidateGraphHydration, ApiError> {
    let candidate_node = storage.get_node(node_id).map_err(|error| {
        ApiError::internal(format!(
            "Failed to load packet candidate for CALL hydration: {error}"
        ))
    })?;
    let candidate_kind = candidate_node.as_ref().map(|node| node.kind);
    let hydrate_outgoing_calls = candidate.target.is_none()
        && matches!(
            candidate_kind,
            Some(NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO)
        );
    let specific_evidence = candidate.graph_evidence.as_ref().and_then(|evidence| {
        let edge_kind = evidence.edge_kind?;
        match evidence.direction {
            CandidateGraphDirection::Outgoing => {
                Some((PacketGraphDirection::Outgoing, edge_kind, evidence.hop))
            }
            CandidateGraphDirection::Incoming => {
                Some((PacketGraphDirection::Incoming, edge_kind, evidence.hop))
            }
            CandidateGraphDirection::Anchor => None,
        }
    });

    // R2: one SEPARATE bounded trail per atom-required edge kind for roots the
    // packet's task-class formulas name, under the same per-trail node cap —
    // so a widened kind can never evict the CALL edges other atoms need. FILE
    // roots run the depth-2 uniform [MEMBER, USAGE, IMPORT] structural trail,
    // whose single coverage record carries both the absent kind and the
    // MEMBER witness rule 7's deeper-rooted arm reads. Outside an active
    // packet proof session the plan list stays empty and hydration behaves
    // exactly as before.
    let proof_session = crate::agent::packet_candidate::active_packet_proof_session();
    // F3 REVISE + gate round 2: in-loop widened hydration is restricted to
    // the depth-1 IDENTITY trails R6's promotion actually consumes mid-pass —
    // the kinds whose edges establish the role identities the active spec's
    // formulas join on. FILE roots (C family) run one combined
    // [MEMBER, IMPORT] trail per direction; other rooted kinds run one
    // single-kind trail per identity kind per direction (A family: CLASS
    // roots get [TYPE_USAGE, MEMBER] — the Builder→ConfigType edge is what
    // establishes the beyond-window config type's identity). Every other
    // atom-kind trail and the depth-2 FILE structural trails run in the
    // retained-set POST-PASS (`hydrate_packet_atom_trails_post_pass`), off
    // the sidecar stage clock.
    let mut atom_trail_plans: Vec<(Vec<EdgeKind>, u32, TrailDirection, PacketGraphDirection)> =
        Vec::new();
    let mut absence_kinds: Vec<codestory_contracts::api::EdgeKind> = Vec::new();
    if let Some(spec) = proof_session
        .as_ref()
        .map(|session| &session.hydration)
        .filter(|spec| !spec.is_empty())
    {
        absence_kinds = spec.absence_kinds.clone();
        let identity_filters: Vec<Vec<EdgeKind>> = if candidate_kind == Some(NodeKind::FILE) {
            if spec.file_structural {
                vec![
                    crate::agent::packet_candidate::PACKET_FILE_IDENTITY_TRAIL_KINDS
                        .iter()
                        .map(|kind| EdgeKind::from(*kind))
                        .collect(),
                ]
            } else {
                Vec::new()
            }
        } else if let Some(kind) = candidate_kind {
            spec.identity_trail_kinds_for_root(kind.into())
                .into_iter()
                .map(|kind| vec![EdgeKind::from(kind)])
                .collect()
        } else {
            Vec::new()
        };
        for filter in identity_filters {
            for (direction, packet_direction) in [
                (TrailDirection::Outgoing, PacketGraphDirection::Outgoing),
                (TrailDirection::Incoming, PacketGraphDirection::Incoming),
            ] {
                atom_trail_plans.push((filter.clone(), 1, direction, packet_direction));
            }
        }
    }

    let run_call_trails = specific_evidence.is_some() || hydrate_outgoing_calls;
    if !run_call_trails && atom_trail_plans.is_empty() {
        return Ok((Vec::new(), None, Vec::new()));
    }

    let record_scans = proof_session.is_some();
    let mut scan_records: Vec<PacketCandidateTrailScan> = Vec::new();
    let mut scan_truncated = false;
    let mut scan_omitted_edge_count: u32 = 0;
    let mut seen_incident_edge_ids = HashSet::new();
    // Edge plus its selection origin: `None` = from the CALL trails (legacy
    // selection rules apply), `Some(direction)` = enumerated by an atom trail.
    let mut collected: Vec<(
        codestory_contracts::graph::Edge,
        Option<PacketGraphDirection>,
    )> = Vec::new();
    let record_scan = |scan_records: &mut Vec<PacketCandidateTrailScan>,
                       filter: &[EdgeKind],
                       depth: u32,
                       packet_direction: PacketGraphDirection,
                       trail: &codestory_contracts::graph::TrailResult| {
        scan_records.push(packet_trail_scan_record(
            &node_id.0.to_string(),
            packet_direction,
            depth,
            filter,
            trail,
            &absence_kinds,
        ));
    };

    if run_call_trails {
        let mut edge_filter = vec![EdgeKind::CALL];
        if let Some((_, edge_kind, _)) = specific_evidence
            && !edge_filter.contains(&edge_kind)
        {
            edge_filter.push(edge_kind);
        }
        let bounded_trail = |direction| {
            storage.get_trail(&TrailConfig {
                root_id: node_id,
                depth: 1,
                direction,
                caller_scope: TrailCallerScope::IncludeTestsAndBenches,
                edge_filter: edge_filter.clone(),
                show_utility_calls: true,
                max_nodes: PACKET_CANDIDATE_DIRECTION_NODE_LIMIT,
                ..TrailConfig::default()
            })
        };
        // Scan the two directions independently. The trail accessor bounds materialization before
        // it returns, so high incoming fanout cannot consume the outgoing scan that may carry a
        // packet boundary. A proof outside either scan remains absent and therefore fails closed;
        // the trail's truncation metadata is carried into the candidate graph below.
        let incoming = bounded_trail(TrailDirection::Incoming).map_err(|error| {
            ApiError::internal(format!(
                "Failed to resolve bounded incoming packet candidate graph provenance: {error}"
            ))
        })?;
        let outgoing = bounded_trail(TrailDirection::Outgoing).map_err(|error| {
            ApiError::internal(format!(
                "Failed to resolve bounded outgoing packet candidate graph provenance: {error}"
            ))
        })?;
        scan_truncated = incoming.truncated || outgoing.truncated;
        scan_omitted_edge_count = incoming
            .omitted_edge_count
            .saturating_add(outgoing.omitted_edge_count);
        if record_scans {
            record_scan(
                &mut scan_records,
                &edge_filter,
                1,
                PacketGraphDirection::Incoming,
                &incoming,
            );
            record_scan(
                &mut scan_records,
                &edge_filter,
                1,
                PacketGraphDirection::Outgoing,
                &outgoing,
            );
        }
        for edge in incoming.edges.into_iter().chain(outgoing.edges) {
            if seen_incident_edge_ids.insert(edge.id) {
                collected.push((edge, None));
            }
        }
    }
    for (filter, depth, direction, packet_direction) in &atom_trail_plans {
        let trail = storage
            .get_trail(&TrailConfig {
                root_id: node_id,
                depth: *depth,
                direction: *direction,
                caller_scope: TrailCallerScope::IncludeTestsAndBenches,
                edge_filter: filter.clone(),
                show_utility_calls: true,
                max_nodes: PACKET_CANDIDATE_DIRECTION_NODE_LIMIT,
                ..TrailConfig::default()
            })
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to resolve bounded atom-trail packet candidate hydration: {error}"
                ))
            })?;
        scan_truncated = scan_truncated || trail.truncated;
        scan_omitted_edge_count = scan_omitted_edge_count.saturating_add(trail.omitted_edge_count);
        if record_scans {
            record_scan(&mut scan_records, filter, *depth, *packet_direction, &trail);
        }
        for edge in trail.edges {
            if seen_incident_edge_ids.insert(edge.id) {
                collected.push((edge, Some(*packet_direction)));
            }
        }
    }

    let mut edges = Vec::new();
    for (edge, atom_direction) in collected {
        let mut selected_direction = None;
        if let Some((direction, edge_kind, hop)) = specific_evidence
            && edge.kind == edge_kind
        {
            let (source, target) = edge.effective_endpoints();
            let matches_specific = match direction {
                // The sidecar direction is anchor-relative: an outgoing expansion lands on the
                // target candidate, while an incoming expansion lands on the source candidate.
                PacketGraphDirection::Outgoing => target == node_id,
                PacketGraphDirection::Incoming => source == node_id,
            };
            if matches_specific {
                selected_direction = Some((direction, hop, false, false));
            }
        }
        let (source, _) = edge.effective_endpoints();
        if hydrate_outgoing_calls && edge.kind == EdgeKind::CALL && source == node_id {
            selected_direction.get_or_insert((PacketGraphDirection::Outgoing, 1, true, false));
        }
        if let Some(direction) = atom_direction {
            // Every edge an atom trail enumerated is kept: the trail's scan
            // record claims completeness over exactly this enumeration, and
            // the extras builder refuses the coverage if any of them is
            // missing from the live evidence.
            selected_direction.get_or_insert((direction, 1, true, true));
        }
        if let Some((direction, hop, hydrated, atom_trail)) = selected_direction {
            edges.push((edge, direction, hop, hydrated, atom_trail));
        }
    }
    edges.sort_by(
        |(left, _, _, left_hydrated, _), (right, _, _, right_hydrated, _)| {
            left_hydrated
                .cmp(right_hydrated)
                .then_with(|| {
                    packet_graph_certainty_priority(left.certainty)
                        .cmp(&packet_graph_certainty_priority(right.certainty))
                })
                .then_with(|| left.id.0.cmp(&right.id.0))
        },
    );
    if edges.is_empty() {
        return Ok((Vec::new(), None, scan_records));
    }

    let graph_flags = app_graph_flags();
    let edge_dtos = edges
        .iter()
        .map(|(edge, _, _, _, _)| {
            graph_edge_dto(edge.clone().with_effective_endpoints(), graph_flags)
        })
        .collect::<Vec<_>>();
    let nodes = packet_graph_endpoint_nodes(storage, node_names, node_id, &edge_dtos)?;

    let mut specific_producers = candidate.provenance.clone();
    if let Some(graph_lane) = candidate.lane_scores.graph.as_ref() {
        specific_producers.extend(graph_lane.provenance.iter().cloned());
    }
    specific_producers.sort();
    specific_producers.dedup();
    let provenance = edges
        .iter()
        .zip(edge_dtos.iter())
        .map(|((_, direction, hop, hydrated, atom_trail), edge)| {
            let mut producers = specific_producers.clone();
            if *atom_trail {
                producers.push("atom_trail_hydration".to_string());
                producers.sort();
                producers.dedup();
            } else if *hydrated {
                producers.push("core_incident_call".to_string());
                producers.sort();
                producers.dedup();
            }
            PacketGraphEdgeProvenance {
                edge_id: edge.id.clone(),
                direction: *direction,
                hop: *hop,
                producers,
                certainty: edge.certainty.clone(),
            }
        })
        .collect::<Vec<_>>();
    Ok((
        provenance,
        Some(GraphResponse {
            center_id: node_id.into(),
            nodes,
            edges: edge_dtos,
            truncated: scan_truncated,
            omitted_edge_count: scan_omitted_edge_count,
            canonical_layout: None,
        }),
        scan_records,
    ))
}

/// Hydrates every endpoint of the given edge DTOs into graph nodes, center
/// first, exactly as candidate graphs always did (labels prefer the shared
/// node-name map, falling back to the node's display name).
fn packet_graph_endpoint_nodes(
    storage: &Store,
    node_names: &HashMap<CoreNodeId, String>,
    center_id: CoreNodeId,
    edge_dtos: &[codestory_contracts::api::GraphEdgeDto],
) -> Result<Vec<GraphNodeDto>, ApiError> {
    let mut endpoint_ids = edge_dtos
        .iter()
        .flat_map(|edge| [edge.source.to_core(), edge.target.to_core()])
        .collect::<Result<Vec<_>, _>>()?;
    endpoint_ids.sort_unstable_by_key(|id| id.0);
    endpoint_ids.dedup();
    endpoint_ids.sort_by_key(|id| (*id != center_id, id.0));

    let mut nodes = Vec::with_capacity(endpoint_ids.len());
    for endpoint_id in endpoint_ids {
        let Some(node) = storage.get_node(endpoint_id).map_err(|error| {
            ApiError::internal(format!(
                "Failed to resolve packet candidate graph endpoint: {error}"
            ))
        })?
        else {
            continue;
        };
        let file_path = AppController::file_path_for_node(storage, &node)?;
        let access = storage.get_component_access(node.id).ok().flatten();
        nodes.push(GraphNodeDto {
            id: node.id.into(),
            label: node_names
                .get(&node.id)
                .cloned()
                .unwrap_or_else(|| node_display_name(&node)),
            kind: node.kind.into(),
            depth: u32::from(node.id != center_id),
            label_policy: Some("qualified_or_serialized".to_string()),
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path,
            qualified_name: node.qualified_name,
            member_access: member_access_dto(access),
        });
    }
    Ok(nodes)
}

fn packet_graph_certainty_priority(
    certainty: Option<codestory_contracts::graph::ResolutionCertainty>,
) -> u8 {
    use codestory_contracts::graph::ResolutionCertainty;
    match certainty {
        Some(ResolutionCertainty::Certain) => 0,
        Some(ResolutionCertainty::Probable) => 1,
        Some(ResolutionCertainty::Uncertain) => 2,
        None => 3,
    }
}

/// Artifact id prefix of the post-pass atom-trail hydration graphs.
const PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX: &str = "packet-atom-hydration-";

/// Post-pass trail budget (F3 REVISE), COST-dimensioned rather than
/// trail-counted: one trail costs `edge_filter.len() × depth` units — a proxy
/// for frontier-expansion work, since every depth level re-applies each
/// filter kind to the frontier — so a depth-2 three-kind structural trail
/// costs 6 units while a depth-1 single-kind trail costs 1. Every trail is
/// additionally hard-capped at `PACKET_CANDIDATE_DIRECTION_NODE_LIMIT` (65)
/// nodes, so worst-case materialization is bounded by BUDGET × node-cap
/// regardless of trail shape. 192 units covers 16 FILE roots (12 units each,
/// both directions) or 32 single-kind rooted candidates — above the retained
/// candidate set (~16-50 citations) for the shipped formulas. When the budget
/// binds, roots are dropped from the tail of the citation order,
/// deterministically.
///
/// Node-cap dimension (round 5.5 item 1 residual): the FILE structural trails
/// run at [`PACKET_POST_PASS_STRUCTURAL_NODE_LIMIT`] (130) rather than the 65
/// every other trail keeps, because below that the store accessor retains
/// nothing at all on entrypoint-scale fanout. The budget absorbs the raise
/// unchanged — 192 units still buys the same 16 FILE roots — and the
/// worst-case materialization it bounds becomes 16 roots × 2 directions × 130
/// nodes / 390 edges, i.e. twice the previous structural ceiling and still a
/// fixed, root-count-independent bound. Single-kind rooted trails are
/// untouched at 65 nodes / 195 edges.
const PACKET_ATOM_POST_PASS_COST_BUDGET: usize = 192;

/// R2 post-pass hydration (F3 REVISE): after candidate resolution completes —
/// off the sidecar stage clock — run the remaining atom-kind trails and the
/// depth-2 FILE structural trails over the RETAINED candidate set (the
/// answer's citations, bounded by the stage carry limits), and fill the
/// [`PacketProofSession`] ledger so the proof-evidence extras builder can
/// construct honest coverage records. No-op without an active session, an
/// empty hydration spec, or an unopened storage.
pub(crate) fn hydrate_packet_atom_trails_post_pass(
    controller: &AppController,
    answer: &mut AgentAnswerDto,
) {
    let Some(session) = crate::agent::packet_candidate::active_packet_proof_session() else {
        return;
    };
    if session.hydration.is_empty() {
        return;
    }
    let Ok(storage) = controller.open_storage() else {
        return;
    };
    hydrate_packet_atom_trails_in_storage(&storage, &HashMap::new(), &session, answer);
}

/// Completes exact outgoing CALL boundaries for strict Legacy carriers that already survived
/// retrieval. Generic trail hydration is intentionally unsuitable here: its navigation policy may
/// erase exact resolution fields and its node-shaped cap can lose a lawful edge in a high-fanout
/// caller. This pass reads a fixed raw prefix, admits only fully correlated exact CALL rows, and
/// retains at most one positive witness per declared boundary. Truncation never proves absence.
pub(crate) fn hydrate_packet_exact_call_boundaries_post_pass(
    controller: &AppController,
    flow_requirements: &[FlowRequirement],
    answer: &mut AgentAnswerDto,
) {
    let Ok(storage) = controller.open_storage() else {
        return;
    };
    hydrate_packet_exact_call_boundaries_in_storage(
        &storage,
        &HashMap::new(),
        flow_requirements,
        answer,
    );
}

fn raw_call_is_exact_boundary_candidate(
    edge: &codestory_contracts::graph::Edge,
    source: &codestory_contracts::graph::Node,
) -> bool {
    let Some(file_node_id) = edge.file_node_id else {
        return false;
    };
    let Some(line) = edge.line.filter(|line| *line >= 1) else {
        return false;
    };
    let Some(callsite_identity) = edge.callsite_identity.as_deref() else {
        return false;
    };
    let Some(pre_marker) = callsite_identity.split('|').next() else {
        return false;
    };
    let fields = pre_marker.split(':').collect::<Vec<_>>();
    if fields.len() != 4 {
        return false;
    }
    let (Ok(identity_file), Ok(identity_line), Ok(_column), Ok(identity_target)) = (
        fields[0].parse::<i64>(),
        fields[1].parse::<u32>(),
        fields[2].parse::<u32>(),
        fields[3].parse::<i64>(),
    ) else {
        return false;
    };
    let target = edge.effective_target();
    edge.kind == EdgeKind::CALL
        && edge.certainty == Some(ResolutionCertainty::Certain)
        && edge.effective_source() == source.id
        && edge.resolved_target == Some(target)
        && edge.candidate_targets.is_empty()
        && source.file_node_id == Some(file_node_id)
        && source
            .start_line
            .zip(source.end_line)
            .is_some_and(|(start, end)| start <= line && line <= end)
        && identity_file == file_node_id.0
        && identity_line == line
        && identity_target == edge.target.0
}

fn exact_call_boundary_graph_for_citation(
    storage: &Store,
    node_names: &HashMap<CoreNodeId, String>,
    flow_requirements: &[FlowRequirement],
    citation: &AgentCitationDto,
) -> Result<Option<GraphResponse>, ApiError> {
    let applicable = flow_requirements
        .iter()
        .filter(|requirement| requirement.proof.formula().is_none())
        .filter(|requirement| flow_requirement_call_boundary_is_discoverable(requirement, citation))
        .collect::<Vec<_>>();
    if applicable.is_empty() {
        return Ok(None);
    }
    let Ok(source) = citation.node_id.0.parse::<i64>().map(CoreNodeId) else {
        return Ok(None);
    };
    let Some(source_node) = storage.get_node(source).map_err(|error| {
        ApiError::internal(format!(
            "Failed to load exact packet CALL boundary source: {error}"
        ))
    })?
    else {
        return Ok(None);
    };
    if !matches!(
        source_node.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    ) {
        return Ok(None);
    }
    let bounded = storage
        .get_bounded_raw_call_edges_by_effective_source(
            source,
            PACKET_EXACT_CALL_BOUNDARY_EDGE_LIMIT,
        )
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to load bounded exact packet CALL boundaries: {error}"
            ))
        })?;
    let graph_flags = app_graph_flags();
    let mut selected = Vec::<GraphEdgeDto>::new();
    let mut selected_ids = HashSet::new();
    for requirement in applicable {
        let Some(edge_dto) = bounded.edges.iter().find_map(|edge| {
            if !raw_call_is_exact_boundary_candidate(edge, &source_node) {
                return None;
            }
            let target = edge.effective_target();
            let target_node = storage.get_node(target).ok().flatten()?;
            if !matches!(
                target_node.kind,
                NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
            ) {
                return None;
            }
            let label = node_names
                .get(&target)
                .cloned()
                .unwrap_or_else(|| node_display_name(&target_node));
            let edge_dto = graph_edge_dto(edge.clone().with_effective_endpoints(), graph_flags);
            flow_requirement_call_receipt_is_valid(
                requirement,
                citation,
                &edge_dto,
                &label,
                ApiNodeKind::from(target_node.kind),
            )
            .then_some(edge_dto)
        }) else {
            continue;
        };
        if selected_ids.insert(edge_dto.id.clone()) {
            selected.push(edge_dto);
        }
    }
    if selected.is_empty() {
        return Ok(None);
    }
    let nodes = packet_graph_endpoint_nodes(storage, node_names, source, &selected)?;
    let omitted =
        bounded.edges.len().saturating_sub(selected.len()) + usize::from(bounded.truncated);
    Ok(Some(GraphResponse {
        center_id: source.into(),
        nodes,
        edges: selected,
        truncated: omitted > 0,
        omitted_edge_count: u32::try_from(omitted).unwrap_or(u32::MAX),
        canonical_layout: None,
    }))
}

fn hydrate_packet_exact_call_boundaries_in_storage(
    storage: &Store,
    node_names: &HashMap<CoreNodeId, String>,
    flow_requirements: &[FlowRequirement],
    answer: &mut AgentAnswerDto,
) {
    for citation in &mut answer.citations {
        let Ok(Some(graph)) = exact_call_boundary_graph_for_citation(
            storage,
            node_names,
            flow_requirements,
            citation,
        ) else {
            continue;
        };
        for edge in &graph.edges {
            if !citation.evidence_edge_ids.contains(&edge.id) {
                citation.evidence_edge_ids.insert(0, edge.id.clone());
            }
        }
        citation.evidence_edge_ids.truncate(12);
        let artifact_id = format!(
            "{PACKET_EXACT_CALL_BOUNDARY_ARTIFACT_PREFIX}{}",
            graph.center_id.0
        );
        if !answer.graphs.iter().any(|artifact| match artifact {
            GraphArtifactDto::Uml { id, .. } | GraphArtifactDto::Mermaid { id, .. } => {
                id == &artifact_id
            }
        }) {
            answer.graphs.push(GraphArtifactDto::Uml {
                id: artifact_id.clone(),
                title: "Exact packet CALL boundary".to_string(),
                graph,
            });
        }
        if !answer.subgraph_ids.contains(&artifact_id) {
            answer.subgraph_ids.push(artifact_id);
        }
    }
}

/// Storage-level core of the post-pass, testable with an in-memory store.
///
/// Each retained root gets one self-contained canonical artifact holding
/// every edge its trails enumerated (the coverage claims reference those
/// edges, and self-containment keeps a scan's fate tied to its own artifact).
/// A root whose trails return no edges is skipped entirely — scans included —
/// which is sound because an absence fact's source role can only be bound by
/// positive receipts, so a rootless scan could never be consulted.
fn hydrate_packet_atom_trails_in_storage(
    storage: &Store,
    node_names: &HashMap<CoreNodeId, String>,
    session: &crate::agent::packet_candidate::PacketProofSession,
    answer: &mut AgentAnswerDto,
) {
    let spec = &session.hydration;
    if spec.is_empty() {
        return;
    }
    let live_artifact_ids = answer
        .graphs
        .iter()
        .map(|artifact| match artifact {
            GraphArtifactDto::Uml { id, .. } | GraphArtifactDto::Mermaid { id, .. } => id.clone(),
        })
        .collect::<HashSet<_>>();
    let directions = [
        (TrailDirection::Outgoing, PacketGraphDirection::Outgoing),
        (TrailDirection::Incoming, PacketGraphDirection::Incoming),
    ];
    let graph_flags = app_graph_flags();
    let mut seen_roots: HashSet<i64> = HashSet::new();
    let mut cost_spent = 0usize;
    let mut new_artifacts: Vec<(String, GraphResponse, Vec<PacketCandidateTrailScan>)> = Vec::new();

    // NEED-ORDERED, SKIP-BOUNDED (gate 8). The traversal used to walk plain
    // citation/rank order and `break` the moment a root did not fit the cost
    // budget. Both halves were wrong for exactly the roots this machinery
    // exists to serve: R6 promotion changes WHICH candidates are admitted,
    // never their rank, so rescued roots sit at the TAIL of citation order
    // and a rank-ordered hard break systematically never reached them —
    // structurally the same pathology R6 itself replaced one layer up, which
    // is why nothing changed above it could move the outcome. Their
    // MEMBER/TYPE_USAGE receipts therefore never entered `packet.support`,
    // could not be proven on, could not be protected as atom carriers, and
    // the citation cap dropped them.
    //
    // So: roots are ordered by ATOM NEED first — the session's own
    // multiplicity priority, which is exactly "how many role positions of
    // the active formulas this identity occupies" — and citation order
    // breaks ties, keeping priority-0 roots in their existing relative
    // order behind the needed ones. Nothing outside the session's need-set
    // enters the key: no vocabulary, no rank, no path.
    //
    // And the budget SKIPS rather than breaks: a root whose cost does not
    // fit is passed over and cheaper roots behind it may still be hydrated.
    // The total budget is unchanged, so the cost bound is identical; what
    // changes is that one expensive early root can no longer starve every
    // cheap one behind it.
    let mut ordered_roots: Vec<(i64, usize)> = Vec::new();
    for (citation_index, citation) in answer.citations.iter().enumerate() {
        let Ok(core_id) = citation.node_id.0.parse::<i64>() else {
            continue;
        };
        if !seen_roots.insert(core_id) {
            continue;
        }
        ordered_roots.push((core_id, citation_index));
    }
    ordered_roots.sort_by_key(|(core_id, citation_index)| {
        (
            std::cmp::Reverse(session.promotion_priority(*core_id)),
            *citation_index,
        )
    });

    for (core_id, _) in ordered_roots {
        let root_id = CoreNodeId(core_id);
        let artifact_id = format!("{PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX}{core_id}");
        if live_artifact_ids.contains(&artifact_id) {
            // Idempotent: this root's post-pass view already exists (its
            // ledger entry rode the first pass, first write wins).
            continue;
        }
        let Ok(Some(node)) = storage.get_node(root_id) else {
            continue;
        };
        // (filter, depth, node cap). The FILE structural trail carries the
        // raised cap: below it the store accessor retains nothing at all on
        // entrypoint-scale fanout — see
        // [`PACKET_POST_PASS_STRUCTURAL_NODE_LIMIT`].
        let mut plans: Vec<(Vec<EdgeKind>, u32, usize)> = Vec::new();
        if node.kind == NodeKind::FILE {
            if spec.file_structural {
                plans.push((
                    crate::agent::packet_candidate::PACKET_FILE_STRUCTURAL_TRAIL_KINDS
                        .iter()
                        .map(|kind| EdgeKind::from(*kind))
                        .collect(),
                    PACKET_FILE_STRUCTURAL_TRAIL_DEPTH,
                    PACKET_POST_PASS_STRUCTURAL_NODE_LIMIT,
                ));
            }
        } else {
            for edge_kind in spec.kinds_for_root(node.kind.into()) {
                plans.push((
                    vec![EdgeKind::from(*edge_kind)],
                    1,
                    PACKET_CANDIDATE_DIRECTION_NODE_LIMIT,
                ));
            }
        }
        if plans.is_empty() {
            continue;
        }
        let root_cost = plans
            .iter()
            .map(|(filter, depth, _)| filter.len().saturating_mul(*depth as usize))
            .sum::<usize>()
            .saturating_mul(directions.len());
        if cost_spent.saturating_add(root_cost) > PACKET_ATOM_POST_PASS_COST_BUDGET {
            // Skip, never break: this root does not fit, but a cheaper root
            // behind it still may. The total budget is unchanged.
            continue;
        }
        cost_spent += root_cost;

        let mut scans: Vec<PacketCandidateTrailScan> = Vec::new();
        let mut seen_edge_ids = HashSet::new();
        let mut collected: Vec<(codestory_contracts::graph::Edge, PacketGraphDirection)> =
            Vec::new();
        let mut truncated = false;
        let mut omitted_edge_count: u32 = 0;
        for (filter, depth, max_nodes) in &plans {
            for (direction, packet_direction) in directions {
                // Post-pass hydration is enrichment: a failed trail degrades
                // to absent coverage (fail closed) instead of failing the
                // packet.
                let Ok(trail) = storage.get_trail(&TrailConfig {
                    root_id,
                    depth: *depth,
                    direction,
                    caller_scope: TrailCallerScope::IncludeTestsAndBenches,
                    edge_filter: filter.clone(),
                    show_utility_calls: true,
                    max_nodes: *max_nodes,
                    ..TrailConfig::default()
                }) else {
                    continue;
                };
                truncated = truncated || trail.truncated;
                omitted_edge_count = omitted_edge_count.saturating_add(trail.omitted_edge_count);
                scans.push(packet_trail_scan_record(
                    &core_id.to_string(),
                    packet_direction,
                    *depth,
                    filter,
                    &trail,
                    &spec.absence_kinds,
                ));
                for edge in trail.edges {
                    if seen_edge_ids.insert(edge.id) {
                        collected.push((edge, packet_direction));
                    }
                }
            }
        }
        if collected.is_empty() {
            continue;
        }
        let edge_dtos = collected
            .iter()
            .map(|(edge, _)| graph_edge_dto(edge.clone().with_effective_endpoints(), graph_flags))
            .collect::<Vec<_>>();
        let Ok(nodes) = packet_graph_endpoint_nodes(storage, node_names, root_id, &edge_dtos)
        else {
            continue;
        };
        new_artifacts.push((
            artifact_id,
            GraphResponse {
                center_id: root_id.into(),
                nodes,
                edges: edge_dtos,
                truncated,
                omitted_edge_count,
                canonical_layout: None,
            },
            scans,
        ));
    }
    for (artifact_id, graph, scans) in new_artifacts {
        session.record_artifact_scans(&artifact_id, &scans);
        answer.graphs.push(GraphArtifactDto::Uml {
            id: artifact_id,
            title: "Packet atom trail hydration".to_string(),
            graph,
        });
    }
}

/// R6 — atom-driven admission at the candidate-resolution boundary.
///
/// The materialized `Vec` + hard `break` is replaced by a re-prioritizable
/// pending queue: at each step the next candidate is the earliest (by base
/// order) pending candidate whose promotion key matches a receipt-established
/// identity, else the next in base order. After each in-loop hydration, newly
/// established identities — exact in-loop resolutions and IMPORT/MEMBER/USAGE
/// effective endpoints from hydrated trails — re-prioritize the remaining
/// queue. The resolution-attempt budget (`max_results` resolved hits) is
/// preserved exactly; only MEMBERSHIP changes. The outer path-resolvability
/// sort is deliberately demoted from invariant to base order: promoted
/// candidates jump it, everything unpromoted keeps it, and promoted candidates
/// keep it among themselves. Dedup key and unresolved-candidate accounting are
/// unchanged; displaced tail candidates end un-attempted exactly as cap-cut
/// candidates do today.
///
/// Promotion keys are identity-only: (a) `CandidateHit.node_id` equal to an
/// atom-needed identity; (b) for file-shaped candidates (`target.is_some()`,
/// where `node_id` is absent), the canonical file node id derived from the
/// candidate's declared path via the in-crate `storage.get_file_by_path`
/// lookup — the route the final contract review chose over exporting the
/// indexer-private `canonical_file_node_id_for_path` (recorded here per that
/// adjudication). `symbol_name` never participates; no substring, token, or
/// similarity operation exists anywhere in the key.
///
/// PROMOTION IS ATOM-NEED-GATED (contract rev 5.3, gate round 3) and
/// CROSS-CONTAINER-RESTRICTED (rev 5.4): an identity promotes only when a
/// still-unproven material atom of the active formulas REQUIRES it — it is a
/// role-constrained endpoint of a hydrated edge matching one of the
/// formulas' IMPORT or TYPE_USAGE patterns (membership/usage kinds discharge
/// atoms as receipts but never drive admission); the need-set is maintained
/// by [`PacketProofSession::record_atom_needed_identities`], and the C
/// bootstrap's import-closure identities arrive through exactly this route
/// because the C IMPORT facts are role-to-role patterns.
/// Identities that merely exist — exact in-loop resolutions included —
/// never promote, and with no active formula-bearing requirements promotion
/// is INERT: admission is bit-identical to pre-R6 behavior. The former key
/// (c) (`graph_evidence` edge identity) is subsumed: an edge identity can
/// only be atom-needed through its endpoints, which keys (a)/(b) already
/// cover.
///
/// Gate round 2, finding 1: the need-set lives in the thread-scoped
/// [`PacketProofSession`], NOT per call — the bootstrap chain establishes
/// identities while resolving one sidecar query's candidates and must
/// promote candidates sitting in OTHER queries' windows (base resolves in
/// query X; the animation stylesheet sits at rank ~29 of query Y). The batch
/// order is fixed, so later queries see earlier identities while earlier
/// queries cannot retroactively benefit — a deterministic, adjudicated
/// asymmetry. Without an active session a throwaway per-call session (empty
/// pattern list, permanently empty need-set) keeps promotion inert.
///
/// Round 5.5 item 2 bounds the gate from both ends, atom-derived on each:
/// (a) PER-ROLE PER-QUERY SLOTS — a candidate jumps the queue only through a
/// promotion role no earlier promotion in THIS query already spent, so a
/// re-flooded need-set can displace at most one candidate per formula role
/// per query (A: 2, C: 4, M and all-Legacy: 0, structurally); and (b) a
/// QUERY-BOUNDARY GROUP CHECKPOINT — once the public group matcher proves a
/// requirement against the typed receipts accumulated in-loop, that
/// requirement's promotion patterns retire and stop driving admission. Both
/// silence promotion only: base-order admission, the resolution-attempt
/// budget, dedup, and unresolved accounting are untouched, so the strictest
/// possible outcome of either bound is exactly pre-R6 admission.
fn resolve_sidecar_candidates_in_storage(
    storage: &Store,
    node_names: &HashMap<CoreNodeId, String>,
    project_root: &Path,
    candidates: &[CandidateHit],
    max_results: usize,
) -> Result<SidecarCandidateResolutionOutcome, ApiError> {
    let mut hits = Vec::new();
    let mut packet_hits = Vec::new();
    let mut unresolved_candidates = Vec::new();
    let mut attempted_candidate_indices = HashSet::new();
    let mut seen = HashSet::new();
    let mut pending = ordered_sidecar_candidates(candidates, |candidate| {
        candidate_path_resolvable(project_root, &candidate.file_path)
    });

    // The cross-query promotion need-set scope (see the doc comment above).
    let identity_scope = crate::agent::packet_candidate::active_packet_proof_session()
        .unwrap_or_else(|| Rc::new(crate::agent::packet_candidate::PacketProofSession::default()));
    let mut admission_trace = identity_scope.trace_enabled().then(|| {
        crate::agent::packet_candidate::PacketQueryAdmissionTrace {
            query_index: identity_scope.next_query_index(),
            ..Default::default()
        }
    });

    // Round 5.5 item 2a — the per-role promotion slots this query has spent.
    // Roles are the endpoints of the formulas' cross-container patterns, so
    // the bound is atom-derived (A: 2, C: 4, M and all-Legacy: 0 — no
    // cross-container pattern, no slot, no promotion). A slot is spent when
    // a candidate JUMPS the queue, whether or not it goes on to resolve:
    // displacement is paid at selection, so that is where it is accounted.
    let mut spent_promotion_roles: Vec<codestory_agent::packet_proof_atoms::ProofRole> = Vec::new();

    while hits.len() < max_results && !pending.is_empty() {
        // Gate 6 — need-set PRIORITY BY ATOM-ROLE MULTIPLICITY. Volume was
        // never the residual: with hundreds of equally-needed identities the
        // slots went to whatever base order surfaced first, so the chain that
        // could complete a group-consistent proof was never admitted. The
        // slot that is about to be filled therefore goes to the pending
        // candidate occupying the MOST distinct (requirement, role)
        // positions, ties broken by base order and then by stable identity —
        // a total, deterministic key with no vocabulary, file position, or
        // repo-specific constant in it. This decides WHICH candidate fills a
        // slot; the per-role slot bound above still decides how many.
        //
        // Cost: one pass over the pending queue per admitted candidate, on
        // identities the session caches — the same order of work the
        // previous earliest-match scan already paid when nothing was
        // promotable.
        let promotion = if !identity_scope.promotion_is_active() {
            None
        } else {
            pending
                .iter()
                .enumerate()
                .filter_map(|(position, (_, candidate, _))| {
                    let identity = candidate_promotion_identity(
                        storage,
                        project_root,
                        candidate,
                        &identity_scope,
                    )?;
                    let role =
                        identity_scope.free_promotion_role(identity, &spent_promotion_roles)?;
                    Some((
                        position,
                        role,
                        identity_scope.promotion_priority(identity),
                        identity,
                    ))
                })
                .min_by_key(|(position, _, priority, identity)| {
                    (std::cmp::Reverse(*priority), *position, *identity)
                })
                .map(|(position, role, _, _)| (position, role))
        };
        let promoted_position = promotion.map(|(position, _)| position);
        if let Some((_, role)) = promotion {
            spent_promotion_roles.push(role);
        }
        let (candidate_index, candidate, path_resolvable) =
            pending.remove(promoted_position.unwrap_or(0));
        attempted_candidate_indices.insert(candidate_index);
        let rel_path = normalize_repo_relative_path(project_root, &candidate.file_path);
        let Some(node_id) =
            resolve_candidate_node_id(storage, node_names, project_root, &rel_path, candidate)
        else {
            let label = if path_resolvable {
                "node_unresolved"
            } else {
                "path_unresolvable"
            };
            unresolved_candidates.push((candidate, label));
            continue;
        };
        let dedupe_key = node_id.0.to_string();
        if !seen.insert(dedupe_key) {
            continue;
        }
        let Some(hit) =
            AppController::build_search_hit(storage, node_names, node_id, candidate.score)?
        else {
            unresolved_candidates.push((candidate, "hit_build_failed"));
            continue;
        };
        let hit = classify_resolved_candidate_hit(hit, candidate);
        let (graph_provenance, graph, trail_scans) =
            packet_graph_for_resolved_candidate(storage, node_names, node_id, candidate)?;
        // Re-prioritization input (rev 5.3): the hydrated trails' typed
        // edges, matched against the active formulas' patterns — only the
        // role-constrained endpoints of matching edges join the need-set,
        // which accumulates in the session and is visible to every later
        // query of the same packet. Exact resolutions establish nothing by
        // themselves.
        if let Some(graph) = graph.as_ref() {
            identity_scope.record_atom_needed_identities(graph);
        }
        if let Some(trace) = admission_trace.as_mut() {
            trace
                .admitted
                .push((node_id.0.to_string(), promoted_position.is_some()));
        }
        packet_hits.push(PacketSearchHit {
            hit: hit.clone(),
            graph_provenance,
            graph,
            trail_scans,
        });
        hits.push(hit);
    }

    // Env-gated R6 admission trace (gate round 4): attribute the
    // un-attempted remainder — identity derivation here runs ONLY when the
    // step-trace artifact is armed, never on a production stage clock.
    if let Some(mut trace) = admission_trace {
        for (_, candidate, _) in &pending {
            let identity =
                candidate_promotion_identity(storage, project_root, candidate, &identity_scope);
            let needed_at_query_end =
                identity.is_some_and(|identity| identity_scope.identity_is_atom_needed(identity));
            let slot_free_at_query_end = identity.is_some_and(|identity| {
                identity_scope
                    .free_promotion_role(identity, &spent_promotion_roles)
                    .is_some()
            });
            trace
                .unattempted
                .push((identity, needed_at_query_end, slot_free_at_query_end));
        }
        trace.promotion_roles_used = spent_promotion_roles;
        identity_scope.record_query_admissions(trace);
    }

    // Round 5.5 item 2b — the QUERY BOUNDARY. The group matcher runs over the
    // typed receipts this query accumulated (plus every earlier query's) and
    // retires the requirements it proves, so their promotion patterns stop
    // driving admission from the next query on. Never gated on tracing, and
    // a no-op without cross-container patterns.
    identity_scope.checkpoint_group_retirement();

    let has_resolved_hit = !hits.is_empty();
    let unresolved_candidate_count = unresolved_candidates.len();
    let blocking_unresolved_candidate_count = unresolved_candidates
        .iter()
        .filter(|(candidate, label)| {
            !unresolved_candidate_is_diagnostic(candidate, label, has_resolved_hit)
        })
        .count();

    Ok(SidecarCandidateResolutionOutcome {
        resolved_hits: hits,
        packet_hits,
        unresolved_candidate_count,
        blocking_unresolved_candidate_count,
        attempted_candidate_indices,
    })
}

/// The identity-only promotion key of one pending candidate (R6): the parsed
/// `node_id` for symbol candidates, or the canonical file node id derived
/// from the candidate's declared path for file-shaped candidates — an exact
/// identity derivation through `storage.get_file_by_path`, never similarity
/// matching. `symbol_name` and free path text never participate. File
/// derivations are cached in the session by normalized relative path, so
/// large candidate pools re-scanned across a packet's queries pay the
/// storage lookup once (stage-clock hygiene).
fn candidate_promotion_identity(
    storage: &Store,
    project_root: &Path,
    candidate: &CandidateHit,
    identity_scope: &crate::agent::packet_candidate::PacketProofSession,
) -> Option<i64> {
    if candidate.target.is_some() {
        let rel_path = normalize_repo_relative_path(project_root, &candidate.file_path);
        return identity_scope.cached_file_identity(&rel_path, || {
            candidate_lookup_paths(project_root, &rel_path)
                .into_iter()
                .find_map(|path| storage.get_file_by_path(&path).ok().flatten())
                .map(|file| file.id)
        });
    }
    candidate
        .node_id
        .as_deref()
        .and_then(|raw| raw.parse::<i64>().ok())
}

fn classify_resolved_candidate_hit(mut hit: SearchHit, candidate: &CandidateHit) -> SearchHit {
    hit.score_breakdown = Some(score_breakdown_for_candidate(candidate));
    if candidate.target.is_some() {
        hit.origin = SearchHitOrigin::TextMatch;
        hit.target = candidate.target.clone();
        hit.kind = ApiNodeKind::FILE;
        hit.file_path = Some(candidate.file_path.clone());
        hit.line = candidate.start_line;
        hit.resolvable = false;
        hit.source_excerpt = candidate.source_excerpt.clone();
    }
    decorate_search_hit_evidence(&mut hit);
    hit
}

#[cfg(test)]
fn resolve_sidecar_candidates_for_test(
    controller: &AppController,
    candidates: &[CandidateHit],
    max_results: usize,
) -> Result<SidecarCandidateResolutionOutcome, ApiError> {
    let storage = controller.open_storage()?;
    let project_root = controller.require_project_root()?;
    let node_names = storage
        .get_nodes()
        .map_err(|error| ApiError::internal(format!("load test nodes: {error}")))?
        .into_iter()
        .map(|node| (node.id, crate::node_display_name(&node)))
        .collect();
    resolve_sidecar_candidates_in_storage(
        &storage,
        &node_names,
        &project_root,
        candidates,
        max_results,
    )
}

fn score_breakdown_for_candidate(candidate: &CandidateHit) -> RetrievalScoreBreakdownDto {
    let provenance = candidate_provenance_labels(candidate);
    let (lexical, semantic, graph) = candidate
        .rank_features
        .as_ref()
        .map(|features| (features.lexical, features.semantic, features.scip_distance))
        .unwrap_or_else(|| {
            let lexical = candidate
                .lane_scores
                .lexical
                .as_ref()
                .map(|evidence| evidence.raw_score);
            let semantic = candidate
                .lane_scores
                .semantic
                .as_ref()
                .map(|evidence| evidence.raw_score);
            let graph = candidate
                .lane_scores
                .graph
                .as_ref()
                .map(|evidence| evidence.raw_score);
            if lexical.is_some() || semantic.is_some() || graph.is_some() {
                (
                    lexical.unwrap_or(0.0),
                    semantic.unwrap_or(0.0),
                    graph.unwrap_or(0.0),
                )
            } else {
                match candidate.source {
                    CandidateSource::Lexical | CandidateSource::Legacy => {
                        (candidate.score, 0.0, 0.0)
                    }
                    CandidateSource::Semantic => (0.0, candidate.score, 0.0),
                    CandidateSource::Scip => (0.0, 0.0, candidate.score),
                }
            }
        });
    RetrievalScoreBreakdownDto {
        lexical,
        semantic,
        graph,
        total: candidate.score,
        tier_cap: None,
        boosts: Vec::new(),
        dampening: Vec::new(),
        final_rank_reason: Some(codestory_retrieval::RANKING_POLICY_VERSION.into()),
        provenance,
    }
}

fn candidate_provenance_labels(candidate: &CandidateHit) -> Vec<String> {
    if !candidate.provenance.is_empty() {
        return candidate.provenance.clone();
    }
    let label = match candidate.source {
        CandidateSource::Lexical => "lexical_source",
        CandidateSource::Semantic => "dense_anchor",
        CandidateSource::Scip => "scip_candidate",
        CandidateSource::Legacy => "legacy",
    };
    vec![label.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::packet_evidence::PacketEvidenceTier;
    use crate::test_support::{git, git_available};
    use codestory_contracts::api::{
        AgentCitationDto, NodeId, NodeKind as ApiNodeKind, PacketTaskClassDto, SearchHitOrigin,
        SearchTargetDto,
    };
    use codestory_retrieval::{
        CandidateHit, QueryTrace, RetrievalCacheKey, RetrievalStageKind, StageTrace,
        classify_query, project_id_for_root, rank_candidates,
        test_support::{publish_zero_dense_pinned_query_fixture, retrieval_manifest_fixture},
    };

    #[derive(Debug, Default)]
    struct TestPublicationResponse {
        publication: Option<EmbeddingVectorPublicationIdentityDto>,
    }

    impl RetrievalPublicationResponse for TestPublicationResponse {
        fn attach_retrieval_publication(
            &mut self,
            publication: EmbeddingVectorPublicationIdentityDto,
        ) {
            self.publication = Some(publication);
        }
    }

    struct PinnedOperationFixture {
        _project: tempfile::TempDir,
        _storage: tempfile::TempDir,
        _retrieval_cache: tempfile::TempDir,
        storage_path: PathBuf,
        controller: AppController,
    }

    fn publish_test_complete_core(
        store: &mut Store,
        project_root: &Path,
        publication: &codestory_store::IndexPublicationRecord,
    ) {
        codestory_retrieval::test_support::publish_complete_core_fixture(
            store,
            project_root,
            publication,
        )
        .expect("publish complete core generation");
    }

    fn pinned_operation_fixture() -> PinnedOperationFixture {
        use codestory_store::{IndexPublicationMode, IndexPublicationRecord};

        let project = tempfile::tempdir().expect("project");
        let storage = tempfile::tempdir().expect("storage");
        let retrieval_cache = tempfile::tempdir().expect("retrieval cache");
        let storage_path = storage.path().join("codestory.db");
        let mut store = Store::open(&storage_path).expect("open storage");
        let publication = IndexPublicationRecord {
            generation: 1,
            generation_id: "11111111-1111-4111-8111-111111111111".into(),
            run_id: "run-one".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        publish_test_complete_core(&mut store, project.path(), &publication);
        drop(store);
        let runtime = codestory_retrieval::with_test_cache_root(retrieval_cache.path(), || {
            SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Local,
            )
        });
        publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
            .expect("publish strict retrieval fixture");
        let controller = AppController::new_with_config(runtime);
        {
            let mut state = controller.state.lock();
            state.project_root = Some(project.path().to_path_buf());
            state.storage_path = Some(storage_path.clone());
        }
        PinnedOperationFixture {
            _project: project,
            _storage: storage,
            _retrieval_cache: retrieval_cache,
            storage_path,
            controller,
        }
    }

    fn env_test_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::process_env_test_lock()
    }

    fn undecorated_search_hit_for_candidate(candidate: &CandidateHit) -> SearchHit {
        SearchHit {
            node_id: NodeId("candidate".to_string()),
            display_name: candidate
                .symbol_name
                .clone()
                .unwrap_or_else(|| candidate.file_path.clone()),
            kind: ApiNodeKind::FUNCTION,
            file_path: Some(candidate.file_path.clone()),
            line: candidate.start_line,
            score: candidate.score,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            source_excerpt: None,
            verification_targets: Vec::new(),
            score_breakdown: None,
        }
    }

    fn search_hit_for_candidate(candidate: &CandidateHit) -> SearchHit {
        classify_resolved_candidate_hit(undecorated_search_hit_for_candidate(candidate), candidate)
    }

    fn retrieval_cache_key_for_test(query_fingerprint: &str) -> RetrievalCacheKey {
        RetrievalCacheKey {
            core_generation_id: None,
            core_run_id: None,
            project_id: "abc".into(),
            lexical_version: "v1".into(),
            semantic_generation: "codestory_abc".into(),
            scip_revision: None,
            sidecar_generation: Some("abc-hash".into()),
            sidecar_input_hash: Some("hash".into()),
            sidecar_schema_version: Some(1),
            projection_count: Some(1),
            query_fingerprint: query_fingerprint.into(),
        }
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
    fn complete_operation_retries_drift_and_traces_the_retried_publication() {
        use codestory_store::{IndexPublicationMode, IndexPublicationRecord};

        let fixture = pinned_operation_fixture();
        let mut build_calls = 0usize;
        let mut retry_calls = 0usize;
        let response = with_stable_retrieval_publication_inner(
            &fixture.controller,
            "test response",
            || {
                build_calls += 1;
                assert!(
                    active_pinned_retrieval_read(&fixture.controller).is_some(),
                    "response assembly must retain the operation pin"
                );
                if build_calls == 1 {
                    let mut writer = Store::open(&fixture.storage_path).expect("open drift writer");
                    let publication = IndexPublicationRecord {
                        generation: 2,
                        generation_id: "22222222-2222-4222-8222-222222222222".into(),
                        run_id: "run-two".into(),
                        mode: IndexPublicationMode::Full,
                        published_at_epoch_ms: 2,
                    };
                    publish_test_complete_core(
                        &mut writer,
                        fixture
                            .controller
                            .require_project_root()
                            .expect("project root")
                            .as_path(),
                        &publication,
                    );
                }
                Ok(TestPublicationResponse::default())
            },
            |_| {
                retry_calls += 1;
                publish_zero_dense_pinned_query_fixture(
                    fixture.controller.require_project_root()?.as_path(),
                    &fixture.storage_path,
                    &fixture.controller.runtime_config,
                )
                .map(|_| ())
                .map_err(|error| ApiError::internal(format!("repair retry fixture: {error}")))
            },
        )
        .expect("second complete attempt should succeed");

        assert_eq!(build_calls, 2);
        assert_eq!(retry_calls, 1);
        let publication = response.publication.expect("response publication metadata");
        assert_eq!(
            publication.core_generation_id,
            "22222222-2222-4222-8222-222222222222"
        );
        assert_eq!(publication.core_run_id, "run-two");
        assert!(!publication.retrieval_generation.is_empty());
        assert!(!publication.retrieval_input_hash.is_empty());
        assert!(!publication.semantic_generation.is_empty());
        assert!(active_pinned_retrieval_read(&fixture.controller).is_none());
    }

    fn canonical_stream_count(controller: &AppController) -> u64 {
        controller.canonical_symbol_names.lock().stream_count()
    }

    /// The canonical node table cannot move inside a published core
    /// generation, so a second pin on the same publication must reuse the map
    /// instead of streaming the whole table again.
    #[test]
    fn a_second_pin_on_one_publication_reuses_the_canonical_symbol_map() {
        let fixture = pinned_operation_fixture();

        let first = PinnedRetrievalRead::begin(&fixture.controller).expect("first pin");
        let first_names = Arc::clone(&first.node_names);
        drop(first);
        assert_eq!(canonical_stream_count(&fixture.controller), 1);

        let second = PinnedRetrievalRead::begin(&fixture.controller).expect("second pin");
        assert_eq!(
            canonical_stream_count(&fixture.controller),
            1,
            "a pin on an unchanged publication must not restream the canonical table"
        );
        assert_eq!(
            *second.node_names, *first_names,
            "the reused map must be the map the stream produced"
        );
        assert!(
            Arc::ptr_eq(&second.node_names, &first_names),
            "the reused map must be the cached allocation, not a fresh clone"
        );
    }

    /// Publication-keyed means keyed by the publication: a new core generation
    /// describes a different canonical table and must be streamed again.
    #[test]
    fn a_new_core_publication_restreams_the_canonical_symbol_map() {
        use codestory_store::{IndexPublicationMode, IndexPublicationRecord};

        let fixture = pinned_operation_fixture();
        PinnedRetrievalRead::begin(&fixture.controller).expect("first pin");
        assert_eq!(canonical_stream_count(&fixture.controller), 1);

        let mut writer = Store::open(&fixture.storage_path).expect("open publication writer");
        let publication = IndexPublicationRecord {
            generation: 2,
            generation_id: "22222222-2222-4222-8222-222222222222".into(),
            run_id: "run-two".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 2,
        };
        publish_test_complete_core(
            &mut writer,
            fixture
                .controller
                .require_project_root()
                .expect("project root")
                .as_path(),
            &publication,
        );
        drop(writer);
        publish_zero_dense_pinned_query_fixture(
            fixture
                .controller
                .require_project_root()
                .expect("project root")
                .as_path(),
            &fixture.storage_path,
            &fixture.controller.runtime_config,
        )
        .expect("republish the retrieval fixture for the new core generation");

        PinnedRetrievalRead::begin(&fixture.controller).expect("pin the new publication");
        assert_eq!(
            canonical_stream_count(&fixture.controller),
            2,
            "a different core publication must not be answered from the previous map"
        );
    }

    /// Fail closed: every component of the reuse condition must be able to
    /// refuse on its own, including the live row count that makes an in-place
    /// canonical-table mutation invalidate the entry instead of hiding behind
    /// it.
    #[test]
    fn each_component_of_the_canonical_reuse_condition_can_refuse_on_its_own() {
        let cached = CachedCanonicalSymbolNames {
            storage_path: PathBuf::from("/cache/codestory.db"),
            core_generation_id: "11111111-1111-4111-8111-111111111111".into(),
            core_run_id: "run-one".into(),
            row_count: 12,
            node_names: Arc::new(HashMap::new()),
        };
        let publication = codestory_retrieval::RetrievalPublicationIdentity {
            core_generation_id: cached.core_generation_id.clone(),
            core_run_id: cached.core_run_id.clone(),
            sidecar_generation: "sidecar-one".into(),
            sidecar_input_hash: "hash-one".into(),
            semantic_generation: "codestory_one".into(),
        };
        assert!(cached.admits_reuse(&cached.storage_path.clone(), &publication, 12));

        assert!(
            !cached.admits_reuse(Path::new("/other/codestory.db"), &publication, 12),
            "a map read from another database must never be reused"
        );
        let mut other_generation = publication.clone();
        other_generation.core_generation_id = "22222222-2222-4222-8222-222222222222".into();
        assert!(!cached.admits_reuse(&cached.storage_path.clone(), &other_generation, 12));
        let mut other_run = publication.clone();
        other_run.core_run_id = "run-two".into();
        assert!(!cached.admits_reuse(&cached.storage_path.clone(), &other_run, 12));
        assert!(
            !cached.admits_reuse(&cached.storage_path.clone(), &publication, 13),
            "a canonical table that gained or lost rows must be restreamed"
        );
    }

    #[test]
    fn nested_complete_operation_attaches_the_outer_pinned_publication() {
        let fixture = pinned_operation_fixture();
        let pinned = Rc::new(
            PinnedRetrievalRead::begin(&fixture.controller).expect("begin outer retrieval pin"),
        );
        let expected = publication_dto(&pinned);

        let response =
            with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
                with_stable_retrieval_publication(&fixture.controller, "nested response", || {
                    Ok(TestPublicationResponse::default())
                })
            })
            .expect("nested operation");

        assert_eq!(response.publication, Some(expected));
    }

    /// Every helper that can reuse an operation's pin must actually reuse it.
    ///
    /// `with_pinned_retrieval_publication_value` is the helper the public
    /// operation path uses, and its active-pin short-circuit is what keeps a
    /// wrapped request from beginning a second pin. `PinnedRetrievalRead::begin`
    /// runs strict readiness, so a missed reuse is a whole extra
    /// whole-repository pass. Measure the pin's own pass, then require that
    /// running the value helper *inside* that pin adds none.
    ///
    /// Deleting the short-circuit costs the operation both things this asserts:
    /// the pinned publication it was already entitled to report, and — wherever
    /// retrieval is primary — a second `PinnedRetrievalRead::begin`. The
    /// end-to-end count is proved on the real packet path in
    /// `services::activation_tests`.
    ///
    /// This asserts a difference, not an absolute: the absolute count of
    /// readiness passes a warm operation pays is larger than one and is not
    /// what this branch changes.
    #[test]
    fn the_value_helper_borrows_an_active_pin_instead_of_paying_for_a_second() {
        let fixture = pinned_operation_fixture();
        let _scope = codestory_workspace::SourceFreshnessScope::enter();
        let pinned = Rc::new(
            PinnedRetrievalRead::begin(&fixture.controller).expect("begin the operation pin"),
        );
        let after_pin = codestory_workspace::source_freshness_counts()
            .expect("an armed operation scope reports counts")
            .readiness_fingerprint_passes;
        assert!(
            after_pin > 0,
            "beginning a retrieval pin runs strict readiness, which is the pass this \
             counter exists to make visible"
        );

        let publication =
            with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
                with_pinned_retrieval_publication_value(
                    &fixture.controller,
                    &pinned.session.publication_identity().core_generation_id,
                    &pinned.session.publication_identity().core_run_id,
                    || Ok(()),
                )
            })
            .expect("the value helper must borrow the operation's pin")
            .1;
        assert!(
            publication.is_some(),
            "borrowing the pin must still report the pinned retrieval publication"
        );

        assert_eq!(
            codestory_workspace::source_freshness_counts()
                .expect("armed scope")
                .readiness_fingerprint_passes,
            after_pin,
            "borrowing the active pin must not buy another readiness fingerprint pass"
        );
    }

    /// The counters are scoped to the operation, never to the process.
    #[test]
    fn the_pass_counter_does_not_outlive_the_operation_scope() {
        let fixture = pinned_operation_fixture();
        let scope = codestory_workspace::SourceFreshnessScope::enter();
        let _pinned = PinnedRetrievalRead::begin(&fixture.controller).expect("begin a pin");
        assert!(
            crate::source_freshness_telemetry_for_operation()
                .expect("telemetry inside the scope")
                .readiness_fingerprint_passes
                > 0
        );
        drop(scope);
        assert_eq!(
            codestory_workspace::source_freshness_counts(),
            None,
            "the pass counter must not outlive the operation"
        );
        assert_eq!(crate::source_freshness_telemetry_for_operation(), None);
    }

    #[test]
    fn cancelled_complete_operation_releases_active_and_retention_pins() {
        use std::sync::mpsc;
        use std::time::Duration;

        let fixture = pinned_operation_fixture();
        let error = with_stable_retrieval_publication_inner(
            &fixture.controller,
            "cancelled response",
            || Err::<TestPublicationResponse, _>(ApiError::new("cancelled", "request cancelled")),
            |_| Ok(()),
        )
        .expect_err("cancellation must leave the operation");
        assert_eq!(error.code, "cancelled");
        assert!(active_pinned_retrieval_read(&fixture.controller).is_none());

        let project_id = sidecar_project_id_for_root(
            fixture
                .controller
                .require_project_root()
                .expect("project root")
                .as_path(),
        );
        let state_file = fixture.controller.runtime_config.layout.state_file.clone();
        let (sender, receiver) = mpsc::channel();
        let probe = std::thread::spawn(move || {
            let result =
                codestory_retrieval::GenerationRetentionLock::acquire(&state_file, &project_id)
                    .map(drop)
                    .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("cancelled response must release the query generation lease")
            .expect("acquire exclusive generation lease after cancellation");
        probe.join().expect("retention probe thread");
    }

    #[test]
    fn agent_adapter_preserves_publication_changed_for_operation_retry() {
        match sidecar_primary_error_outcome(ApiError::new(
            "publication_changed",
            "generation drift",
        )) {
            SidecarPrimarySearchOutcome::Retryable { error } => {
                assert_eq!(error.code, "publication_changed");
                assert_eq!(error.message, "generation drift");
            }
            _ => panic!("publication drift must remain retryable"),
        }
    }

    #[test]
    fn detached_sidecar_query_cache_does_not_hold_mutex_during_work() {
        let controller = AppController::new();
        let first = retrieval_cache_key_for_test("first");
        let second = retrieval_cache_key_for_test("second");
        controller.sidecar_query_cache.lock().insert(
            first.clone(),
            vec![CandidateHit::lexical_stub("src/first.rs", 1.0)],
        );

        with_detached_sidecar_query_cache(&controller, |cache| {
            assert!(
                controller.sidecar_query_cache.try_lock().is_some(),
                "sidecar query cache mutex should not be held during retrieval work"
            );
            assert_eq!(
                cache.get(&first).expect("detached cache carries entries")[0].file_path,
                "src/first.rs"
            );
            cache.insert(
                second.clone(),
                vec![CandidateHit::lexical_stub("src/second.rs", 1.0)],
            );
        });

        let cache = controller.sidecar_query_cache.lock();
        assert_eq!(
            cache
                .get(&first)
                .expect("original cache entry should merge back")[0]
                .file_path,
            "src/first.rs"
        );
        assert_eq!(
            cache
                .get(&second)
                .expect("new cache entry should merge back")[0]
                .file_path,
            "src/second.rs"
        );
    }

    #[test]
    fn detached_sidecar_query_cache_skips_merge_after_invalidation() {
        let controller = AppController::new();
        let first = retrieval_cache_key_for_test("first");
        let second = retrieval_cache_key_for_test("second");
        controller.sidecar_query_cache.lock().insert(
            first.clone(),
            vec![CandidateHit::lexical_stub("src/first.rs", 1.0)],
        );

        with_detached_sidecar_query_cache(&controller, |cache| {
            controller.sidecar_query_cache.lock().clear();
            cache.insert(
                second.clone(),
                vec![CandidateHit::lexical_stub("src/second.rs", 1.0)],
            );
        });

        let cache = controller.sidecar_query_cache.lock();
        assert!(
            cache.get(&first).is_none(),
            "clear during detached work should invalidate original entries"
        );
        assert!(
            cache.get(&second).is_none(),
            "detached entries must not merge after cache invalidation"
        );
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

    #[cfg(windows)]
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
    fn sidecar_candidate_order_evaluates_path_resolution_once_per_candidate() {
        let candidates = vec![
            CandidateHit::lexical_stub("ok/equal-a.rs", 1.0),
            CandidateHit::lexical_stub("ok/equal-b.rs", 1.0),
            CandidateHit::lexical_stub("missing/high.rs", 100.0),
            CandidateHit::lexical_stub("lexical:phantom", 500.0),
        ];
        let mut evaluations = 0usize;
        let ordered = ordered_sidecar_candidates(&candidates, |candidate| {
            evaluations += 1;
            candidate.file_path.starts_with("ok/")
        });
        assert_eq!(
            evaluations, 3,
            "path resolution must run once per surviving candidate"
        );
        assert_eq!(
            ordered
                .iter()
                .map(|(index, candidate, resolvable)| (
                    *index,
                    candidate.file_path.as_str(),
                    *resolvable
                ))
                .collect::<Vec<_>>(),
            vec![
                (0, "ok/equal-a.rs", true),
                (1, "ok/equal-b.rs", true),
                (2, "missing/high.rs", false),
            ],
            "resolvability stays the primary key and equal scores keep input order"
        );

        let nan_candidates = vec![
            CandidateHit::lexical_stub("ok/nan.rs", f32::NAN),
            CandidateHit::lexical_stub("ok/finite.rs", 2.0),
        ];
        let ordered = ordered_sidecar_candidates(&nan_candidates, |_| true);
        assert_eq!(
            ordered
                .iter()
                .map(|(_, candidate, _)| candidate.file_path.as_str())
                .collect::<Vec<_>>(),
            vec!["ok/nan.rs", "ok/finite.rs"],
            "a NaN score must stay comparator-equal and stable"
        );
    }

    #[test]
    fn sidecar_candidate_order_preserves_canonical_rank_within_resolution_buckets() {
        let scores = [
            1.0_f32,
            f32::NAN,
            1.0,
            50.0,
            -3.0,
            f32::NAN,
            0.0,
            50.0,
            f32::INFINITY,
        ];
        let resolvable = |path: &str| path.starts_with("ok/");
        for window in 1..=scores.len() {
            for offset in 0..=(scores.len() - window) {
                let candidates = scores[offset..offset + window]
                    .iter()
                    .enumerate()
                    .map(|(index, score)| {
                        let prefix = if index.is_multiple_of(3) { "ok" } else { "no" };
                        CandidateHit::lexical_stub(format!("{prefix}/candidate-{index}.rs"), *score)
                    })
                    .collect::<Vec<_>>();

                let expected = candidates
                    .iter()
                    .enumerate()
                    .filter(|(_, candidate)| resolvable(&candidate.file_path))
                    .chain(
                        candidates
                            .iter()
                            .enumerate()
                            .filter(|(_, candidate)| !resolvable(&candidate.file_path)),
                    )
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();

                let current = ordered_sidecar_candidates(&candidates, |candidate| {
                    resolvable(&candidate.file_path)
                });

                assert_eq!(
                    current
                        .iter()
                        .map(|(index, _, _)| *index)
                        .collect::<Vec<_>>(),
                    expected,
                    "candidate order moved for window={window} offset={offset}"
                );
            }
        }
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
    fn whole_file_lexical_candidate_resolves_to_typed_file_range_evidence() {
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
                line_count: 3,
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
                    serialized_name: "unrelated_symbol".to_string(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(2),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");

        let mut candidate =
            CandidateHit::with_source("src/lib.rs", None, 0.8, CandidateSource::Lexical);
        candidate.start_line = Some(2);
        candidate.target = Some(SearchTargetDto::FileRange {
            file_path: "src/lib.rs".to_string(),
            start_byte: 12,
            end_byte: 19,
        });
        candidate.source_excerpt = Some("fn needle() {}".to_string());
        candidate.provenance = vec!["lexical_source".to_string()];

        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[candidate],
            1,
        )
        .expect("resolve typed lexical candidate");
        let hit = outcome.resolved_hits.first().expect("resolved file hit");

        assert_eq!(hit.node_id, NodeId("1".to_string()));
        assert_eq!(hit.kind, ApiNodeKind::FILE);
        assert_eq!(hit.origin, SearchHitOrigin::TextMatch);
        assert_eq!(hit.line, Some(2));
        assert_eq!(
            hit.target,
            Some(SearchTargetDto::FileRange {
                file_path: "src/lib.rs".to_string(),
                start_byte: 12,
                end_byte: 19,
            })
        );
        assert_eq!(hit.source_excerpt.as_deref(), Some("fn needle() {}"));
        assert_eq!(hit.evidence_tier, Some(PacketEvidenceTier::LexicalSource));
        assert_eq!(
            hit.resolution_status,
            Some(crate::agent::packet_evidence::PacketEvidenceResolution::SourceRangeOnly)
        );
    }

    #[test]
    fn packet_candidate_keeps_exact_scip_edge_provenance_without_public_hit_fields() {
        use codestory_retrieval::CandidateGraphEvidence;
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("requests/sessions.py"),
                language: "python".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 10,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        storage
            .insert_nodes_batch(&[
                codestory_contracts::graph::Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::FILE,
                    serialized_name: "requests/sessions.py".into(),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(2),
                    kind: NodeKind::METHOD,
                    serialized_name: "Session.request".into(),
                    qualified_name: Some("Session.request".into()),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(2),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(3),
                    kind: NodeKind::METHOD,
                    serialized_name: "Session.send".into(),
                    qualified_name: Some("Session.send".into()),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(5),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_edges_batch(&[codestory_contracts::graph::Edge {
                id: codestory_contracts::graph::EdgeId(7),
                source: CoreNodeId(2),
                target: CoreNodeId(3),
                kind: codestory_contracts::graph::EdgeKind::CALL,
                resolved_target: Some(CoreNodeId(3)),
                certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
                ..Default::default()
            }])
            .expect("insert edge");

        let mut candidate = CandidateHit::with_source(
            "requests/sessions.py",
            Some("Session.send".into()),
            0.8,
            CandidateSource::Scip,
        );
        candidate.node_id = Some("3".into());
        candidate.provenance = vec!["scip_graph_projection".into(), "graph_neighbor".into()];
        candidate.graph_evidence = Some(CandidateGraphEvidence {
            edge_kind: Some(codestory_contracts::graph::EdgeKind::CALL),
            direction: CandidateGraphDirection::Outgoing,
            hop: 1,
            fanout: 1,
            edge_weight: 1.0,
            direction_weight: 1.0,
        });

        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[candidate],
            1,
        )
        .expect("resolve packet candidate");
        assert_eq!(outcome.resolved_hits.len(), 1);
        let packet_hit = outcome.packet_hits.first().expect("packet hit");
        assert_eq!(
            packet_hit
                .graph_provenance
                .iter()
                .map(|provenance| provenance.edge_id.0.as_str())
                .collect::<Vec<_>>(),
            ["7"]
        );
        assert_eq!(
            packet_hit.graph_provenance[0].direction,
            PacketGraphDirection::Outgoing
        );
        assert_eq!(packet_hit.graph_provenance[0].hop, 1);
        assert_eq!(
            packet_hit.graph_provenance[0].certainty.as_deref(),
            Some("certain")
        );
        assert!(
            packet_hit.graph_provenance[0]
                .producers
                .iter()
                .any(|producer| producer == "scip_graph_projection")
        );
        assert_eq!(packet_hit.citation(true).evidence_edge_ids[0].0, "7");
    }

    #[test]
    fn packet_candidate_keeps_more_than_twenty_specific_incoming_and_late_outgoing_callsites() {
        use codestory_retrieval::CandidateGraphEvidence;
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("src/server.js"),
                language: "javascript".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 32,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        let mut nodes = vec![
            codestory_contracts::graph::Node {
                id: CoreNodeId(1),
                kind: NodeKind::FILE,
                serialized_name: "src/server.js".into(),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(2),
                kind: NodeKind::METHOD,
                serialized_name: "response.send".into(),
                qualified_name: Some("response.send".into()),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(2),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(24),
                kind: NodeKind::METHOD,
                serialized_name: "response.json".into(),
                qualified_name: Some("response.json".into()),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(1),
                ..Default::default()
            },
        ];
        nodes.extend((0..21).map(|index| codestory_contracts::graph::Node {
            id: CoreNodeId(3 + index),
            kind: NodeKind::UNKNOWN,
            serialized_name: if index == 16 {
                "end".into()
            } else {
                format!("boundary_{index}")
            },
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(3 + index as u32),
            ..Default::default()
        }));
        nodes.extend((0..80).map(|index| codestory_contracts::graph::Node {
            id: CoreNodeId(3_000 + index),
            kind: NodeKind::METHOD,
            serialized_name: format!("high_fanout_incoming_{index}"),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(50 + index as u32),
            ..Default::default()
        }));
        nodes.extend((0..21).map(|index| codestory_contracts::graph::Node {
            id: CoreNodeId(1_000 + index),
            kind: NodeKind::METHOD,
            serialized_name: format!("incoming_{index}"),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(25 + index as u32),
            ..Default::default()
        }));
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        let mut edges = (0..21)
            .map(|index| codestory_contracts::graph::Edge {
                id: codestory_contracts::graph::EdgeId(100 + index),
                source: CoreNodeId(2),
                target: CoreNodeId(3 + index),
                kind: EdgeKind::CALL,
                file_node_id: Some(CoreNodeId(1)),
                line: Some(3 + index as u32),
                callsite_identity: Some(format!(
                    "src/server.js:{}:1:{}|syntax:js-member-call",
                    3 + index,
                    3 + index
                )),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        edges.push(codestory_contracts::graph::Edge {
            id: codestory_contracts::graph::EdgeId(7),
            source: CoreNodeId(24),
            target: CoreNodeId(2),
            kind: EdgeKind::CALL,
            resolved_target: Some(CoreNodeId(2)),
            certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
            file_node_id: Some(CoreNodeId(1)),
            line: Some(1),
            ..Default::default()
        });
        edges.extend((0..21).map(|index| codestory_contracts::graph::Edge {
            id: codestory_contracts::graph::EdgeId(2_000 + index),
            source: CoreNodeId(1_000 + index),
            target: CoreNodeId(2),
            kind: EdgeKind::CALL,
            resolved_target: Some(CoreNodeId(2)),
            certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
            file_node_id: Some(CoreNodeId(1)),
            line: Some(25 + index as u32),
            ..Default::default()
        }));
        edges.extend((0..80).map(|index| codestory_contracts::graph::Edge {
            id: codestory_contracts::graph::EdgeId(4_000 + index),
            source: CoreNodeId(3_000 + index),
            target: CoreNodeId(2),
            kind: EdgeKind::CALL,
            resolved_target: Some(CoreNodeId(2)),
            certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
            file_node_id: Some(CoreNodeId(1)),
            line: Some(50 + index as u32),
            ..Default::default()
        }));
        storage.insert_edges_batch(&edges).expect("insert edges");

        let mut candidate = CandidateHit::with_source(
            "src/server.js",
            Some("response.send".into()),
            0.8,
            CandidateSource::Scip,
        );
        candidate.node_id = Some("2".into());
        candidate.provenance = vec!["scip_graph_projection".into()];
        candidate.graph_evidence = Some(CandidateGraphEvidence {
            edge_kind: Some(EdgeKind::CALL),
            direction: CandidateGraphDirection::Outgoing,
            hop: 1,
            fanout: 1,
            edge_weight: 1.0,
            direction_weight: 1.0,
        });

        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[candidate],
            1,
        )
        .expect("resolve packet candidate");
        let packet_hit = outcome.packet_hits.first().expect("packet hit");
        let graph = packet_hit.graph.as_ref().expect("incident CALL graph");

        assert_eq!(graph.edges.len(), 85);
        assert_eq!(packet_hit.graph_provenance.len(), 85);
        assert!(graph.truncated);
        assert_eq!(graph.omitted_edge_count, 38);
        assert!(graph.edges.iter().any(|edge| edge.id.0 == "7"));
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.id.0 == "116" && edge.target == NodeId("19".into())),
            "the response end edge occurs after the old 12-edge cutoff"
        );
        let specific = packet_hit
            .graph_provenance
            .iter()
            .find(|provenance| provenance.edge_id.0 == "7")
            .expect("specific incoming proof");
        assert!(
            specific
                .producers
                .iter()
                .any(|producer| producer == "scip_graph_projection")
        );
        let hydrated = packet_hit
            .graph_provenance
            .iter()
            .find(|provenance| provenance.edge_id.0 == "116")
            .expect("hydrated outgoing end proof");
        assert_eq!(hydrated.direction, PacketGraphDirection::Outgoing);
        assert_eq!(hydrated.hop, 1);
        assert!(
            hydrated
                .producers
                .iter()
                .any(|producer| producer == "core_incident_call")
        );
        assert!(packet_hit.has_proof_call_provenance());
    }

    fn exact_boundary_test_citation(id: i64, display_name: &str) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(id.to_string()),
            display_name: display_name.to_string(),
            kind: ApiNodeKind::FUNCTION,
            file_path: Some("src/runtime.c".to_string()),
            line: Some(2),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
            source_excerpt: None,
        }
    }

    fn exact_boundary_test_requirements() -> Vec<FlowRequirement> {
        Vec::new()
    }

    fn exact_boundary_test_edge(
        id: i64,
        source: i64,
        target: i64,
        line: u32,
    ) -> codestory_contracts::graph::Edge {
        codestory_contracts::graph::Edge {
            id: codestory_contracts::graph::EdgeId(id),
            source: CoreNodeId(source),
            target: CoreNodeId(target),
            kind: EdgeKind::CALL,
            file_node_id: Some(CoreNodeId(1)),
            line: Some(line),
            resolved_target: Some(CoreNodeId(target)),
            certainty: Some(ResolutionCertainty::Certain),
            callsite_identity: Some(format!("1:{line}:1:{target}|syntax:c-call")),
            ..Default::default()
        }
    }

    #[test]
    fn exact_boundary_post_pass_recovers_only_correlated_router_and_loop_witnesses() {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("src/runtime.c"),
                language: "c".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 300,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        storage
            .insert_file(&FileInfo {
                id: 2,
                path: PathBuf::from("src/other.c"),
                language: "c".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 300,
                file_role: FileRole::Source,
            })
            .expect("insert other file");
        let mut nodes = vec![
            codestory_contracts::graph::Node {
                id: CoreNodeId(1),
                kind: NodeKind::FILE,
                serialized_name: "src/runtime.c".into(),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(2),
                kind: NodeKind::FILE,
                serialized_name: "src/other.c".into(),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(10),
                kind: NodeKind::FUNCTION,
                serialized_name: "processCommand".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(2),
                end_line: Some(150),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(20),
                kind: NodeKind::FUNCTION,
                serialized_name: "rejectCommand".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(200),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(30),
                kind: NodeKind::FUNCTION,
                serialized_name: "recordCommandMetrics".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(210),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(40),
                kind: NodeKind::FUNCTION,
                serialized_name: "aeMain".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(220),
                end_line: Some(225),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(41),
                kind: NodeKind::FUNCTION,
                serialized_name: "EventLoop.processEvents".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(230),
                ..Default::default()
            },
        ];
        nodes.extend((0..70).map(|index| codestory_contracts::graph::Node {
            id: CoreNodeId(100 + index),
            kind: NodeKind::FUNCTION,
            serialized_name: format!("recordMetric{index}"),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(10 + index as u32),
            ..Default::default()
        }));
        storage.insert_nodes_batch(&nodes).expect("insert nodes");

        let mut edges = (0..70)
            .map(|index| exact_boundary_test_edge(index + 1, 10, 100 + index, 10 + index as u32))
            .collect::<Vec<_>>();
        let mut probable = exact_boundary_test_edge(80, 10, 20, 90);
        probable.certainty = Some(ResolutionCertainty::Probable);
        edges.push(probable);
        let mut candidate_bearing = exact_boundary_test_edge(81, 10, 20, 91);
        candidate_bearing.candidate_targets = vec![CoreNodeId(30)];
        edges.push(candidate_bearing);
        let mut unresolved = exact_boundary_test_edge(82, 10, 20, 92);
        unresolved.resolved_target = None;
        edges.push(unresolved);
        let mut malformed = exact_boundary_test_edge(83, 10, 20, 93);
        malformed.callsite_identity = Some("syntax:c-call".into());
        edges.push(malformed);
        edges.push(exact_boundary_test_edge(84, 10, 30, 94));
        edges.push(exact_boundary_test_edge(85, 10, 20, 151));
        let mut wrong_file = exact_boundary_test_edge(86, 10, 20, 101);
        wrong_file.file_node_id = Some(CoreNodeId(2));
        wrong_file.callsite_identity = Some("2:101:1:20|syntax:c-call".into());
        edges.push(wrong_file);
        edges.push(exact_boundary_test_edge(90, 10, 20, 100));
        edges.push(exact_boundary_test_edge(91, 40, 41, 221));
        storage.insert_edges_batch(&edges).expect("insert edges");

        let requirements = exact_boundary_test_requirements();
        let router = exact_call_boundary_graph_for_citation(
            &storage,
            &HashMap::new(),
            &requirements,
            &exact_boundary_test_citation(10, "processCommand"),
        )
        .expect("router hydration")
        .expect("exact router boundary");
        assert_eq!(
            router
                .edges
                .iter()
                .map(|edge| edge.id.0.as_str())
                .collect::<Vec<_>>(),
            ["90"],
            "high fanout must not hide the one exact routing witness; probable, candidate-bearing, unresolved, malformed, and wrong-target rows remain excluded"
        );

        let loop_driver = exact_call_boundary_graph_for_citation(
            &storage,
            &HashMap::new(),
            &requirements,
            &exact_boundary_test_citation(40, "aeMain"),
        )
        .expect("loop hydration")
        .expect("exact loop boundary");
        assert_eq!(loop_driver.edges[0].id.0, "91");

        let mut answer = sidecar_answer_with_citation_node("10");
        answer.citations = vec![
            exact_boundary_test_citation(10, "processCommand"),
            exact_boundary_test_citation(40, "aeMain"),
        ];
        hydrate_packet_exact_call_boundaries_in_storage(
            &storage,
            &HashMap::new(),
            &requirements,
            &mut answer,
        );
        assert_eq!(answer.citations[0].evidence_edge_ids[0].0, "90");
        assert_eq!(answer.citations[1].evidence_edge_ids[0].0, "91");
        assert_eq!(answer.graphs.len(), 2);
        assert_eq!(answer.subgraph_ids.len(), 2);

        assert!(
            exact_call_boundary_graph_for_citation(
                &storage,
                &HashMap::new(),
                &requirements,
                &exact_boundary_test_citation(40, "Connection.rebindEventLoop"),
            )
            .expect("hostile carrier")
            .is_none(),
            "a rebind wrapper must not enter exact boundary hydration"
        );
    }

    #[test]
    fn exact_boundary_post_pass_never_claims_a_match_beyond_its_fixed_raw_prefix() {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("src/runtime.c"),
                language: "c".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 400,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        let mut nodes = vec![
            codestory_contracts::graph::Node {
                id: CoreNodeId(1),
                kind: NodeKind::FILE,
                serialized_name: "src/runtime.c".into(),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(10),
                kind: NodeKind::FUNCTION,
                serialized_name: "processCommand".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(2),
                end_line: Some(400),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(20),
                kind: NodeKind::FUNCTION,
                serialized_name: "rejectCommand".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(300),
                ..Default::default()
            },
        ];
        nodes.extend((0..PACKET_EXACT_CALL_BOUNDARY_EDGE_LIMIT).map(|index| {
            codestory_contracts::graph::Node {
                id: CoreNodeId(100 + i64::from(index)),
                kind: NodeKind::FUNCTION,
                serialized_name: format!("recordMetric{index}"),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(10 + index),
                ..Default::default()
            }
        }));
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        let mut edges = (0..PACKET_EXACT_CALL_BOUNDARY_EDGE_LIMIT)
            .map(|index| {
                exact_boundary_test_edge(
                    1 + i64::from(index),
                    10,
                    100 + i64::from(index),
                    10 + index,
                )
            })
            .collect::<Vec<_>>();
        edges.push(exact_boundary_test_edge(10_000, 10, 20, 350));
        storage.insert_edges_batch(&edges).expect("insert edges");

        assert!(
            exact_call_boundary_graph_for_citation(
                &storage,
                &HashMap::new(),
                &exact_boundary_test_requirements(),
                &exact_boundary_test_citation(10, "processCommand"),
            )
            .expect("bounded hydration")
            .is_none(),
            "a lawful edge beyond the fixed raw prefix remains unproven; truncation is never absence or authority"
        );
    }

    #[test]
    fn unresolved_sidecar_candidates_are_diagnostic_only() {
        let result = QueryResult {
            publication_identity: None,
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
            packet_hits: Vec::new(),
            unresolved_candidate_count: 1,
            blocking_unresolved_candidate_count: 1,
            attempted_candidate_indices: HashSet::from([0]),
        };

        let diagnostic = packet_sidecar_query_diagnostic(&result, &resolution, 2, 1, 3);

        assert_eq!(diagnostic.candidate_count, 1);
        assert_eq!(diagnostic.resolved_hit_count, 0);
        assert_eq!(diagnostic.unresolved_candidate_count, 1);
        assert_eq!(diagnostic.total_elapsed_ms, Some(3));
        assert!(diagnostic.diagnostic.is_some());
    }

    fn semantic_stage_trace(
        completion_status: codestory_retrieval::StageCompletionStatus,
        candidates_added: usize,
    ) -> codestory_retrieval::StageTrace {
        codestory_retrieval::StageTrace {
            stage: codestory_retrieval::RetrievalStageKind::Stage1bSemantic,
            budget_ms: 40,
            elapsed_ms: 40,
            admission_wait_ms: 0,
            queue_wait_ms: None,
            execution_ms: None,
            candidates_added,
            marginal_gain: 0.0,
            cancel_reason: Some("stage_deadline".into()),
            cache_hit: false,
            degraded: false,
            stub_reason: None,
            completion_status,
        }
    }

    fn query_result_with_stages(stages: Vec<codestory_retrieval::StageTrace>) -> QueryResult {
        QueryResult {
            publication_identity: None,
            query: "how does activation admit a lease".into(),
            features: classify_query("how does activation admit a lease"),
            hits: Vec::new(),
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 100,
                elapsed_ms: 40,
                cancel_reason: None,
                cache_hit: false,
                stages,
            },
        }
    }

    fn empty_resolution() -> SidecarCandidateResolutionOutcome {
        SidecarCandidateResolutionOutcome {
            resolved_hits: Vec::new(),
            packet_hits: Vec::new(),
            unresolved_candidate_count: 0,
            blocking_unresolved_candidate_count: 0,
            attempted_candidate_indices: HashSet::new(),
        }
    }

    /// EV-8. The sidecar reports no blocking cancel when only a *stage* runs out of budget, so a
    /// query whose dense lane went dark and then resolved nothing used to arrive as `Completed`
    /// and satisfy its query obligation on an empty result.
    #[test]
    fn a_semantic_stage_timeout_with_no_resolved_hits_cancels_the_query() {
        let result = query_result_with_stages(vec![semantic_stage_trace(
            codestory_retrieval::StageCompletionStatus::PendingAfterDeadline,
            0,
        )]);

        let diagnostic = packet_sidecar_query_diagnostic(&result, &empty_resolution(), 40, 1, 41);

        assert_eq!(
            diagnostic.completion,
            PacketQueryCompletionDto::Cancelled {
                reason: SEMANTIC_TIMEOUT_ZERO_HITS_CANCEL.to_string()
            },
            "{diagnostic:?}"
        );
        assert!(
            diagnostic.semantic_stage_timeout_zero_hits,
            "{diagnostic:?}"
        );
    }

    /// The demotion is about lost evidence, not about the stage clock. A query that still
    /// resolved hits produced evidence and must stay `Completed`.
    #[test]
    fn a_semantic_stage_timeout_that_still_resolved_hits_stays_completed() {
        let result = query_result_with_stages(vec![semantic_stage_trace(
            codestory_retrieval::StageCompletionStatus::PendingAfterDeadline,
            0,
        )]);
        let mut resolution = empty_resolution();
        resolution.resolved_hits = vec![undecorated_search_hit_for_candidate(
            &CandidateHit::with_source(
                "crates/codestory-runtime/src/services.rs",
                Some("activate_once".to_string()),
                0.9,
                CandidateSource::Lexical,
            ),
        )];

        let diagnostic = packet_sidecar_query_diagnostic(&result, &resolution, 40, 1, 41);

        assert_eq!(
            diagnostic.completion,
            PacketQueryCompletionDto::Completed,
            "{diagnostic:?}"
        );
        assert!(
            diagnostic.semantic_stage_timeout_zero_hits,
            "the lost stage is still counted even when the query recovered: {diagnostic:?}"
        );
    }

    /// A deliberately skipped dense lane on a repository with no dense anchors is correct
    /// behavior. It is counted, not cancelled.
    #[test]
    fn a_skipped_semantic_stage_abstains_without_cancelling_the_query() {
        let mut skipped =
            semantic_stage_trace(codestory_retrieval::StageCompletionStatus::Skipped, 0);
        skipped.cancel_reason = Some("zero_dense_anchors".into());
        let result = query_result_with_stages(vec![skipped]);

        let diagnostic = packet_sidecar_query_diagnostic(&result, &empty_resolution(), 1, 1, 2);

        assert_eq!(diagnostic.completion, PacketQueryCompletionDto::Completed);
        assert!(diagnostic.semantic_abstained, "{diagnostic:?}");
        assert!(
            !diagnostic.semantic_stage_timeout_zero_hits,
            "{diagnostic:?}"
        );
    }

    #[test]
    fn shadow_maps_unavailable_trace() {
        let shadow = shadow_from_query_result(QueryResult {
            publication_identity: None,
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
            publication_identity: None,
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
                    stage: RetrievalStageKind::Stage1Lexical,
                    budget_ms: 120,
                    elapsed_ms: 20,
                    admission_wait_ms: 0,
                    queue_wait_ms: Some(1),
                    execution_ms: Some(19),
                    candidates_added: 2,
                    marginal_gain: 0.4,
                    cancel_reason: None,
                    cache_hit: false,
                    degraded: false,
                    stub_reason: None,
                    completion_status: codestory_retrieval::StageCompletionStatus::Completed,
                }],
            },
        });
        assert_eq!(shadow.retrieval_mode, "full");
        assert_eq!(shadow.stage_timings.len(), 1);
        assert_eq!(shadow.stage_timings[0].stage, "stage1_lexical");
        assert_eq!(shadow.candidates.len(), 2);
        assert_eq!(shadow.would_rank, vec!["src/a.rs", "src/b.rs"]);
    }

    #[test]
    fn score_breakdown_reports_fused_rank_features_and_provenance() {
        let mut candidate = CandidateHit::with_source(
            "src/service.rs",
            Some("ExtensionService".into()),
            0.91,
            CandidateSource::Lexical,
        );
        candidate.provenance = vec![
            "lexical_source".into(),
            "dense_anchor".into(),
            "graph_neighbor".into(),
        ];
        candidate.rank_features = Some(codestory_retrieval::RankFeatures {
            ranking_policy: codestory_retrieval::RANKING_POLICY_VERSION.into(),
            lexical: 0.91,
            semantic: 0.82,
            scip_distance: 0.5,
            file_role_prior: 0.72,
            definition_quality: 0.85,
            token_overlap: 0.25,
            text_quality: 0.81,
            requested_role_agreement: 0.75,
        });

        let breakdown = score_breakdown_for_candidate(&candidate);

        assert_eq!(breakdown.lexical, 0.91);
        assert_eq!(breakdown.semantic, 0.82);
        assert_eq!(breakdown.graph, 0.5);
        assert_eq!(
            breakdown.provenance,
            vec![
                "lexical_source".to_string(),
                "dense_anchor".to_string(),
                "graph_neighbor".to_string()
            ]
        );
    }

    #[test]
    fn score_breakdown_does_not_export_graph_for_pure_lexical_candidate() {
        let mut candidate = CandidateHit::with_source(
            "src/service.rs",
            Some("Service".into()),
            0.78,
            CandidateSource::Lexical,
        );
        candidate.provenance = vec!["lexical_source".into()];
        let ranked = rank_candidates(&classify_query("explain service startup"), vec![candidate]);
        let candidate = ranked.first().expect("ranked candidate");

        let breakdown = score_breakdown_for_candidate(candidate);
        let hit = search_hit_for_candidate(candidate);

        assert!(breakdown.lexical > 0.0);
        assert_eq!(breakdown.semantic, 0.0);
        assert_eq!(breakdown.graph, 0.0);
        assert_eq!(hit.evidence_tier, Some(PacketEvidenceTier::LexicalSource));
    }

    #[test]
    fn score_breakdown_does_not_export_graph_for_pure_dense_candidate() {
        let mut candidate = CandidateHit::with_source(
            "src/search.rs",
            Some("SearchService".into()),
            0.86,
            CandidateSource::Semantic,
        );
        candidate.provenance = vec!["dense_anchor".into()];
        let ranked = rank_candidates(&classify_query("explain search service"), vec![candidate]);
        let candidate = ranked.first().expect("ranked candidate");

        let breakdown = score_breakdown_for_candidate(candidate);
        let hit = search_hit_for_candidate(candidate);

        assert_eq!(breakdown.lexical, 0.0);
        assert!(breakdown.semantic > 0.0);
        assert_eq!(breakdown.graph, 0.0);
        assert_eq!(hit.evidence_tier, Some(PacketEvidenceTier::DenseSemantic));
        assert_eq!(hit.eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn scored_hit_adapter_never_reports_total_as_lexical() {
        let candidate = CandidateHit::with_source(
            "src/search.rs",
            Some("SearchService".into()),
            0.86,
            CandidateSource::Semantic,
        );
        let ranked = rank_candidates(&classify_query("explain search service"), vec![candidate]);
        let hit = search_hit_for_candidate(ranked.first().expect("candidate"));

        let scored = HybridSearchScoredHit::from_search_hit(hit);

        assert_eq!(scored.lexical_score, 0.0);
        assert_eq!(scored.semantic_score, 0.86);
        assert!(scored.total_score > 0.0);
    }

    #[test]
    fn candidate_evidence_is_classified_once_after_lane_context_is_attached() {
        use crate::agent::packet_evidence::PacketEvidenceResolution;

        let mut lexical = CandidateHit::with_source(
            "src/service.rs",
            Some("Service".into()),
            0.8,
            CandidateSource::Lexical,
        );
        lexical.provenance = vec!["lexical_source".into()];
        lexical.start_line = Some(1);
        lexical.target = Some(SearchTargetDto::FileRange {
            file_path: "src/service.rs".into(),
            start_byte: 0,
            end_byte: 10,
        });
        let neutral = classify_resolved_candidate_hit(
            undecorated_search_hit_for_candidate(&lexical),
            &lexical,
        );
        assert_eq!(
            neutral.evidence_tier,
            Some(PacketEvidenceTier::LexicalSource)
        );
        assert_eq!(neutral.evidence_producer.as_deref(), Some("lexical_source"));

        let mut structural = undecorated_search_hit_for_candidate(&lexical);
        structural.evidence_tier = Some(PacketEvidenceTier::StructuralText);
        structural.evidence_producer = Some("structural_markdown_collector".into());
        structural.resolution_status = Some(PacketEvidenceResolution::SourceRangeOnly);
        structural.eligible_for_sufficiency = Some(false);
        let structural = classify_resolved_candidate_hit(structural, &lexical);
        assert_eq!(
            structural.evidence_tier,
            Some(PacketEvidenceTier::StructuralText)
        );
        assert_eq!(
            structural.evidence_producer.as_deref(),
            Some("structural_markdown_collector")
        );
        assert_eq!(
            structural.resolution_status,
            Some(PacketEvidenceResolution::SourceRangeOnly)
        );
        assert_eq!(structural.eligible_for_sufficiency, Some(false));

        let mut exact = undecorated_search_hit_for_candidate(&lexical);
        exact.evidence_tier = Some(PacketEvidenceTier::ExactSource);
        exact.evidence_producer = Some("openapi_endpoint_schema".into());
        exact.resolution_status = Some(PacketEvidenceResolution::SourceRangeOnly);
        exact.eligible_for_sufficiency = Some(false);
        let exact = classify_resolved_candidate_hit(exact, &lexical);
        assert_eq!(exact.evidence_tier, Some(PacketEvidenceTier::ExactSource));
        assert_eq!(
            exact.resolution_status,
            Some(PacketEvidenceResolution::SourceRangeOnly)
        );
        assert_eq!(exact.eligible_for_sufficiency, Some(false));

        let mut affinity = CandidateHit::with_source(
            "src/service.rs",
            Some("ServiceImpl".into()),
            0.8,
            CandidateSource::Scip,
        );
        affinity.provenance = vec!["same_file_name_affinity".into()];
        let affinity = rank_candidates(&classify_query("ServiceImpl"), vec![affinity])
            .into_iter()
            .next()
            .expect("ranked affinity candidate");
        let affinity_hit = search_hit_for_candidate(&affinity);
        assert_eq!(
            affinity_hit
                .score_breakdown
                .as_ref()
                .map(|breakdown| breakdown.graph),
            Some(0.0)
        );
        assert_ne!(
            affinity_hit.evidence_tier,
            Some(PacketEvidenceTier::ResolvedGraph)
        );
    }

    #[test]
    fn stage_two_reference_adjacency_is_published_as_resolved_graph_evidence() {
        let mut adjacency = CandidateHit::with_source(
            "src/client.rs",
            Some("parse_client".into()),
            0.65,
            CandidateSource::Scip,
        );
        adjacency.node_id = Some("4".into());
        adjacency.start_line = Some(50);
        adjacency.scip_hop_distance = Some(1);
        // Exactly what the sidecar stage stamps: the artifact's evidence source
        // plus the stage's own public provenance label.
        adjacency.provenance = vec![
            "scip_graph_projection".into(),
            RetrievalStageKind::Stage2ScipExpand
                .provenance_label()
                .expect("stage 2 publishes a provenance label")
                .to_string(),
        ];
        let adjacency = rank_candidates(&classify_query("parse_client"), vec![adjacency])
            .into_iter()
            .next()
            .expect("ranked adjacency candidate");

        let hit = search_hit_for_candidate(&adjacency);

        assert_eq!(
            hit.score_breakdown
                .as_ref()
                .map(|breakdown| breakdown.graph),
            Some(0.65),
            "validated hop-1 reference adjacency publishes the graph feature: {hit:?}"
        );
        assert_eq!(
            hit.evidence_tier,
            Some(PacketEvidenceTier::ResolvedGraph),
            "stage-2 adjacency is graph evidence again: {hit:?}"
        );
    }

    #[test]
    fn shadow_candidate_summaries_include_loss_point_resolution() {
        let mut candidate = CandidateHit::with_source(
            "semantic:handler",
            Some("handler".into()),
            0.5,
            CandidateSource::Semantic,
        );
        candidate.start_line = Some(42);
        let shadow = shadow_from_query_result_with_counts_and_resolution_labels(
            QueryResult {
                publication_identity: None,
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
            publication_identity: None,
            query: "apii".into(),
            features: classify_query("apii"),
            hits: vec![CandidateHit::with_source(
                "apii",
                Some("apii".to_string()),
                0.5,
                CandidateSource::Semantic,
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
            publication_identity: None,
            query: "StringUtils".into(),
            features: classify_query("StringUtils"),
            hits: vec![CandidateHit::with_source(
                "main/java/org/apache/commons/lang3/Missing.java",
                Some("StringUtils".to_string()),
                0.5,
                CandidateSource::Semantic,
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
    fn shadow_marks_non_parser_backed_file_candidates_diagnostic_only_with_source_hits() {
        let shadow = shadow_from_query_result_with_counts_and_resolution_labels(
            QueryResult {
                publication_identity: None,
                query: "form validation".into(),
                features: classify_query("form validation"),
                hits: vec![
                    CandidateHit::with_source(
                        "config/form-validation.json",
                        None,
                        0.9,
                        CandidateSource::Lexical,
                    ),
                    CandidateHit::with_source(
                        "docs/man/tool.1",
                        None,
                        0.85,
                        CandidateSource::Lexical,
                    ),
                    CandidateHit::with_source(
                        "scripts/completions/tool.zsh",
                        None,
                        0.825,
                        CandidateSource::Lexical,
                    ),
                    CandidateHit::with_source(
                        "src/forms/validation.rs",
                        Some("email".into()),
                        0.8,
                        CandidateSource::Scip,
                    ),
                    CandidateHit::with_source(
                        "tests/support/harness.tcl",
                        None,
                        0.7,
                        CandidateSource::Lexical,
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
            4,
            1,
            &[
                "node_unresolved".to_string(),
                "node_unresolved".to_string(),
                "node_unresolved".to_string(),
                "resolved".to_string(),
                "not_attempted".to_string(),
            ],
            &[],
        );
        let value = serde_json::to_value(&shadow).expect("serialize shadow");
        assert_eq!(shadow.unresolved_candidate_count, 3);
        assert_eq!(value["diagnostic_only"], true);
    }

    #[test]
    fn shadow_keeps_blocking_unresolved_candidate_visible() {
        let mut hits = vec![CandidateHit::with_source(
            "config/application.json",
            None,
            0.9,
            CandidateSource::Lexical,
        )];
        let mut resolution_labels = vec!["node_unresolved".to_string()];
        for index in 1..MAX_SHADOW_CANDIDATES {
            hits.push(CandidateHit::with_source(
                format!("src/module_{index}.rs"),
                Some(format!("module_{index}")),
                0.8,
                CandidateSource::Scip,
            ));
            resolution_labels.push("resolved".to_string());
        }
        hits.push(CandidateHit::with_source(
            "missing/application.json",
            None,
            0.7,
            CandidateSource::Lexical,
        ));
        resolution_labels.push("path_unresolvable".to_string());

        let summary_indices = shadow_candidate_indices(&hits, &resolution_labels);
        assert_eq!(summary_indices.len(), MAX_SHADOW_CANDIDATES);
        assert_eq!(summary_indices.last(), Some(&MAX_SHADOW_CANDIDATES));
        assert!(!unresolved_candidates_are_diagnostic_only(
            &hits,
            &resolution_labels,
            2,
        ));

        let source_candidate =
            CandidateHit::with_source("src/tool.zsh", None, 0.9, CandidateSource::Lexical);
        assert!(!unresolved_candidate_is_diagnostic(
            &source_candidate,
            "node_unresolved",
            true,
        ));
    }

    #[test]
    fn shadow_candidate_summaries_include_admission_diagnostics() {
        let shadow = shadow_from_query_result_with_counts_and_resolution_labels(
            QueryResult {
                publication_identity: None,
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
                        CandidateSource::Lexical,
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
    fn resolved_candidate_admission_never_borrows_another_symbol_rank_from_its_file() {
        let ranked_nodes = HashMap::from([("kept".to_string(), 1)]);
        let ranked_paths = HashMap::from([("src/shared.rs".to_string(), 1)]);

        assert_eq!(
            candidate_admission_rank(
                Some("dropped"),
                "src/shared.rs",
                &ranked_nodes,
                &ranked_paths,
            ),
            None
        );
        assert_eq!(
            candidate_admission_rank(None, "src/shared.rs", &ranked_nodes, &ranked_paths),
            Some(1),
            "path fallback remains available only when no symbol identity was resolved"
        );
    }

    #[test]
    fn sidecar_budget_respects_latency_cap() {
        assert_eq!(sidecar_budget_ms(Some(400)), 1_000);
        assert_eq!(sidecar_budget_ms(Some(5_000)), 5_000);
        assert_eq!(sidecar_budget_ms(None), 1_500);
        assert_eq!(sidecar_budget_ms(Some(250_000)), 120_000);
    }

    #[test]
    fn packet_batch_budget_uses_packet_latency_budget() {
        assert_eq!(
            sidecar_packet_batch_budget_ms(None),
            DEFAULT_PACKET_BATCH_BUDGET_MS
        );
        assert_eq!(sidecar_packet_batch_budget_ms(Some(18_000)), 18_000);
        assert_eq!(sidecar_packet_batch_budget_ms(Some(5_000)), 5_000);
        assert_eq!(sidecar_packet_batch_budget_ms(Some(5)), 1_000);
        assert_eq!(
            sidecar_packet_batch_budget_ms(Some(250_000)),
            MAX_PACKET_BATCH_BUDGET_MS
        );
    }

    #[test]
    fn packet_batch_marks_only_deadlines_retryable_and_rejects_cancellation() {
        use codestory_retrieval::classify_query;

        let query_result = |cancel_reason: Option<&str>| QueryResult {
            publication_identity: None,
            query: "handler".into(),
            features: classify_query("handler"),
            hits: Vec::new(),
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 1_000,
                elapsed_ms: 1,
                cancel_reason: cancel_reason.map(str::to_string),
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        let empty_resolution = |_: &QueryResult, _: usize| {
            Ok(SidecarCandidateResolutionOutcome {
                resolved_hits: Vec::new(),
                packet_hits: Vec::new(),
                unresolved_candidate_count: 0,
                blocking_unresolved_candidate_count: 0,
                attempted_candidate_indices: HashSet::new(),
            })
        };
        let controller = AppController::new();
        let queries = vec![("handler".to_string(), 5)];

        let empty = build_sidecar_packet_batch_outcome(
            &controller,
            &queries,
            vec![query_result(None)],
            1,
            empty_resolution,
        )
        .expect("ordinary empty result");
        assert!(empty.retryable_queries.is_empty());

        for reason in ["deadline", "stage_deadline"] {
            let deadline = build_sidecar_packet_batch_outcome(
                &controller,
                &queries,
                vec![query_result(Some(reason))],
                1,
                empty_resolution,
            )
            .expect("deadline result");
            assert_eq!(deadline.retryable_queries, ["handler"]);
            assert!(deadline.results[0].1.is_empty());
        }

        let cancelled = build_sidecar_packet_batch_outcome(
            &controller,
            &queries,
            vec![query_result(Some("cancelled"))],
            1,
            empty_resolution,
        )
        .expect_err("public cancellation must not become an empty successful batch");
        assert_eq!(cancelled.code, "cancelled");
    }

    #[test]
    fn recovery_commands_quote_shell_sensitive_project_paths() {
        let commands =
            sidecar_retrieval_recovery_commands_for_project(r"C:\tmp\cost$cache`tick's repo", None);

        #[cfg(windows)]
        let expected_project = r"'C:/tmp/cost$cache`tick''s repo'";
        #[cfg(not(windows))]
        let expected_project = r"'C:/tmp/cost$cache`tick'\''s repo'";

        assert!(
            commands
                .first()
                .is_some_and(|command| command.contains("retrieval index")),
            "retrieval recovery should start with artifact publication: {commands:?}"
        );
        assert!(
            commands
                .iter()
                .all(|command| command.contains(&format!("--project {expected_project}"))),
            "all recovery commands should quote the project path literally: {commands:?}"
        );
    }

    #[test]
    fn recovery_commands_preserve_agent_run_id_for_readiness_and_status() {
        let commands =
            sidecar_retrieval_recovery_commands_for_project("C:/repo", Some("packet-search-eval"));

        assert!(
            commands
                .first()
                .is_some_and(|command| command.contains("retrieval index")
                    && command.contains("--run-id packet-search-eval")),
            "retrieval activation should keep the selected agent run id: {commands:?}"
        );
        assert!(
            commands
                .get(1)
                .is_some_and(|command| command.contains("retrieval status")
                    && command.contains("--profile agent --run-id packet-search-eval")),
            "retrieval status should keep the selected agent profile/run id: {commands:?}"
        );
        assert!(
            commands
                .get(2)
                .is_some_and(|command| command
                    == "codestory-cli doctor --project \"C:/repo\" --format markdown"),
            "doctor does not accept profile/run-id flags, so the hint should remain parseable: {commands:?}"
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
        let agent_full_but_dead = SidecarModeStatus {
            profile: Some("agent".into()),
            mode: "full".into(),
            degraded_reason: Some("embedding_runtime_unavailable: connection refused".into()),
        };

        assert!(
            !sidecar_status_can_serve_primary(&local_full),
            "local/default full sidecar must not serve packet/search/context primary retrieval"
        );
        assert!(sidecar_status_can_serve_primary(&agent_full));
        assert!(!sidecar_status_can_serve_primary(&agent_full_but_dead));
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
    fn retrieval_status_rejects_stale_manifest_before_engine_start() {
        let project = tempfile::tempdir().expect("project");
        let storage_dir = tempfile::tempdir().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let runtime = SidecarRuntimeConfig::for_project_auto(project.path());
        let project_id = project_id_for_root(project.path());
        let hash = "deadbeefcafebabe";
        let mut storage = Store::open(&storage_path).expect("open storage");
        let mut manifest = retrieval_manifest_fixture(&project_id, hash);
        manifest.embedding_backend = Some("stale-backend".into());
        storage
            .upsert_retrieval_index_manifest(&manifest)
            .expect("manifest");

        let status = sidecar_mode_status_for_runtime(project.path(), &storage_path, &runtime);

        assert_eq!(status.mode, "full");
        let reason = status.degraded_reason.expect("unavailable reason");
        assert!(
            reason.starts_with("retrieval_manifest_stale:"),
            "expected static manifest validation before engine startup, got: {reason}"
        );
    }

    fn upsert_manifest(storage_path: &Path, project_id: &str) {
        let hash = "deadbeefcafebabe";
        let generation = format!("{project_id}-{hash}");
        let semantic_generation = format!("codestory_{project_id}_{hash}");
        let built_at_epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_millis() as i64;
        let mut storage = Store::open(storage_path).expect("open storage");
        storage
            .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                project_id: project_id.into(),
                lexical_version: codestory_retrieval::LEXICAL_INDEX_VERSION.into(),
                semantic_generation,
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
                semantic_policy_version: Some(codestory_retrieval::SEMANTIC_POLICY_VERSION.into()),
                graph_artifact_hash: Some("graph-test-hash".into()),
                dense_reason_counts_json: Some("{}".into()),
                precise_semantic_import_status: None,
                precise_semantic_import_reason: None,
                precise_semantic_import_revision: None,
                precise_semantic_import_producer: None,
            })
            .expect("manifest");
    }

    #[test]
    fn pinned_read_resolves_the_original_generation_and_rejects_publication_drift() {
        use codestory_retrieval::CandidateSource;
        use codestory_store::{FileInfo, FileRole, IndexPublicationMode, IndexPublicationRecord};

        let project = tempfile::tempdir().expect("project");
        let source_path = project.path().join("src/lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "fn original() {}\n").expect("write source");
        let storage_dir = tempfile::tempdir().expect("storage");
        let retrieval_cache = tempfile::tempdir().expect("retrieval cache");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = sidecar_project_id_for_root(project.path());

        let mut storage = Store::open(&storage_path).expect("open storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: source_path.clone(),
                language: "rust".into(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        let original_node = codestory_contracts::graph::Node {
            id: CoreNodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "original".into(),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(1),
            ..Default::default()
        };
        storage
            .insert_nodes_batch(&[
                codestory_contracts::graph::Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::FILE,
                    serialized_name: source_path.to_string_lossy().into_owned(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(1),
                    ..Default::default()
                },
                original_node,
            ])
            .expect("insert nodes");
        let first_publication = IndexPublicationRecord {
            generation: 1,
            generation_id: "11111111-1111-4111-8111-111111111111".into(),
            run_id: "run-one".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        publish_test_complete_core(&mut storage, project.path(), &first_publication);
        drop(storage);

        let runtime = codestory_retrieval::with_test_cache_root(retrieval_cache.path(), || {
            SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Local,
            )
        });
        publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
            .expect("publish strict first retrieval generation");

        let controller = AppController::new_with_config(runtime);
        {
            let mut state = controller.state.lock();
            state.project_root = Some(project.path().to_path_buf());
            state.storage_path = Some(storage_path.clone());
        }
        let pinned = PinnedRetrievalRead::begin(&controller).expect("pin first publication");

        let mut writer = Store::open(&storage_path).expect("open publication writer");
        writer
            .insert_nodes_batch(&[codestory_contracts::graph::Node {
                id: CoreNodeId(2),
                kind: NodeKind::FUNCTION,
                serialized_name: "replacement".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(1),
                ..Default::default()
            }])
            .expect("reuse numeric id in replacement generation");
        let replacement_publication = IndexPublicationRecord {
            generation: 2,
            generation_id: "22222222-2222-4222-8222-222222222222".into(),
            run_id: "run-two".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 2,
        };
        publish_test_complete_core(&mut writer, project.path(), &replacement_publication);
        let replacement_manifest = retrieval_manifest_fixture(&project_id, "second");
        writer
            .upsert_retrieval_index_manifest(&replacement_manifest)
            .expect("publish replacement manifest");
        drop(writer);

        let mut candidate = CandidateHit::with_source(
            "src/lib.rs",
            Some("original".into()),
            1.0,
            CandidateSource::Scip,
        );
        candidate.node_id = Some("2".into());
        let resolution = resolve_sidecar_candidates_in_read(&pinned, &[candidate], 1)
            .expect("resolve against pinned snapshot");
        assert_eq!(resolution.resolved_hits.len(), 1);
        assert_eq!(resolution.resolved_hits[0].display_name, "original");

        let error = pinned
            .revalidate()
            .expect_err("publication drift must reject the result");
        assert_eq!(error.code, "publication_changed");
    }

    fn git_project() -> Option<tempfile::TempDir> {
        if !git_available() {
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

    #[test]
    fn sidecar_result_allows_empty_full_mode_and_rejects_unresolved_candidates() {
        use codestory_retrieval::{CandidateSource, classify_query};

        let empty_full = QueryResult {
            publication_identity: None,
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
            publication_identity: None,
            query: "handler".into(),
            features: classify_query("handler"),
            hits: vec![CandidateHit::with_source(
                "semantic:handler",
                Some("handler".into()),
                0.5,
                CandidateSource::Semantic,
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
    fn sidecar_result_rejects_blocking_cancel_reasons_even_with_resolved_hits() {
        use codestory_retrieval::{CandidateSource, classify_query};

        for reason in ["deadline", "stage_deadline", "cancelled"] {
            let candidate = CandidateHit::with_source(
                "src/handler.rs",
                Some("handler".into()),
                0.9,
                CandidateSource::Lexical,
            );
            let resolved_hit = search_hit_for_candidate(&candidate);
            let result = QueryResult {
                publication_identity: None,
                query: "handler".into(),
                features: classify_query("handler"),
                hits: vec![candidate],
                trace: QueryTrace {
                    retrieval_mode: "full".into(),
                    degraded_reason: None,
                    total_budget_ms: 500,
                    elapsed_ms: 100,
                    cancel_reason: Some(reason.into()),
                    cache_hit: false,
                    stages: Vec::new(),
                },
            };

            let expected =
                format!("sidecar retrieval trace `{reason}` is not eligible for primary results");
            assert_eq!(
                sidecar_result_rejection_reason(&result, &[resolved_hit]).as_deref(),
                Some(expected.as_str())
            );
        }
    }

    #[test]
    fn sidecar_search_result_rejects_non_full_modes_even_without_candidates() {
        use codestory_retrieval::classify_query;

        for mode in ["no_semantic", "no_scip", "lexical_only", "unavailable"] {
            let result = QueryResult {
                publication_identity: None,
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
            publication_identity: None,
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
            packet_hits: Vec::new(),
            unresolved_candidate_count: 0,
            blocking_unresolved_candidate_count: 0,
            attempted_candidate_indices: HashSet::new(),
        };
        let empty_diagnostic =
            packet_sidecar_query_diagnostic(&empty_full, &empty_resolution, 1, 0, 1);
        assert_eq!(empty_diagnostic.candidate_count, 0);
        assert_eq!(empty_diagnostic.resolved_hit_count, 0);
        assert_eq!(empty_diagnostic.unresolved_candidate_count, 0);
        assert!(empty_diagnostic.diagnostic.is_none());
        assert_eq!(
            empty_diagnostic.completion,
            PacketQueryCompletionDto::Completed
        );

        let unresolved = QueryResult {
            publication_identity: None,
            query: "handler".into(),
            features: classify_query("handler"),
            hits: vec![CandidateHit::with_source(
                "semantic:handler",
                Some("handler".into()),
                0.5,
                CandidateSource::Semantic,
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
            packet_hits: Vec::new(),
            unresolved_candidate_count: 1,
            blocking_unresolved_candidate_count: 1,
            attempted_candidate_indices: HashSet::from([0]),
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
        assert_eq!(
            unresolved_diagnostic.completion,
            PacketQueryCompletionDto::Completed
        );

        let cancelled = QueryResult {
            publication_identity: None,
            query: "handler".into(),
            features: classify_query("handler"),
            hits: vec![CandidateHit::with_source(
                "src/handler.rs",
                Some("handler".into()),
                0.9,
                CandidateSource::Lexical,
            )],
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 100,
                cancel_reason: Some("stage_deadline".into()),
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        let cancelled_hit = search_hit_for_candidate(&cancelled.hits[0]);
        let cancelled_resolution = SidecarCandidateResolutionOutcome {
            resolved_hits: vec![cancelled_hit.clone()],
            packet_hits: vec![PacketSearchHit::without_graph(cancelled_hit)],
            unresolved_candidate_count: 0,
            blocking_unresolved_candidate_count: 0,
            attempted_candidate_indices: HashSet::from([0]),
        };
        let cancelled_diagnostic =
            packet_sidecar_query_diagnostic(&cancelled, &cancelled_resolution, 100, 1, 101);
        assert_eq!(cancelled_diagnostic.resolved_hit_count, 1);
        assert_eq!(
            cancelled_diagnostic.diagnostic.as_deref(),
            Some("sidecar query has blocking cancel reason `stage_deadline`")
        );
        assert_eq!(
            cancelled_diagnostic.completion,
            PacketQueryCompletionDto::Cancelled {
                reason: "stage_deadline".to_string()
            }
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
            publication_identity: None,
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

        let resolution = resolve_sidecar_candidates_for_test(&controller, &query_result.hits, 1)
            .expect("resolve sidecar candidates");
        assert_eq!(resolution.attempted_candidate_indices.len(), 1);
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

        let mixed_resolution =
            resolve_sidecar_candidates_for_test(&controller, &query_result.hits, 2)
                .expect("resolve mixed sidecar candidates");
        assert_eq!(mixed_resolution.attempted_candidate_indices.len(), 2);
        assert_eq!(mixed_resolution.resolved_hits.len(), 1);
        assert_eq!(mixed_resolution.unresolved_candidate_count, 1);
        assert_eq!(mixed_resolution.blocking_unresolved_candidate_count, 1);

        let mixed_diagnostic =
            packet_sidecar_query_diagnostic(&query_result, &mixed_resolution, 1, 0, 1);
        assert_eq!(mixed_diagnostic.resolved_hit_count, 1);
        assert_eq!(mixed_diagnostic.unresolved_candidate_count, 1);
        assert_eq!(mixed_diagnostic.blocking_unresolved_candidate_count, 1);
    }

    #[test]
    fn packet_batch_rejects_unavailable_sidecar_mode() {
        use codestory_retrieval::{CandidateSource, classify_query};

        let unavailable = QueryResult {
            publication_identity: None,
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
            publication_identity: None,
            query: "handler".into(),
            features: classify_query("handler"),
            hits: vec![CandidateHit::with_source(
                "semantic:handler",
                Some("handler".into()),
                0.5,
                CandidateSource::Semantic,
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
                assert_eq!(batch, &[("helpers".to_string(), 1_000)]);
                Ok(vec![QueryResult {
                    publication_identity: None,
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
                        total_budget_ms: 1_000,
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
                        publication_identity: None,
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
                assert_eq!(batch, &[("handler".to_string(), 1_000)]);
                Ok(vec![QueryResult {
                    publication_identity: None,
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
                        total_budget_ms: 1_000,
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
            publication_identity: None,
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
    fn sidecar_primary_search_serves_cancelled_full_trace_with_resolved_hits() {
        use codestory_retrieval::CandidateSource;
        use codestory_store::{FileInfo, FileRole, Store};

        let temp = tempfile::tempdir().expect("tempdir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("storage parent"))
            .expect("create storage parent");
        let source_path = temp.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn packaged_agent_proof() {}\n").expect("write source");

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
                        serialized_name: "packaged_agent_proof".to_string(),
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
        let mut candidate = CandidateHit::with_source(
            source_path.to_string_lossy().to_string(),
            Some("packaged_agent_proof".to_string()),
            0.9,
            CandidateSource::Scip,
        );
        candidate.node_id = Some("2".to_string());
        let query_result = QueryResult {
            publication_identity: None,
            query: "Explain how CodeStory validates packaged agent readiness.".into(),
            features: classify_query("Explain how CodeStory validates packaged agent readiness."),
            hits: vec![candidate],
            trace: QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 290,
                cancel_reason: Some("stage_deadline".into()),
                cache_hit: false,
                stages: Vec::new(),
            },
        };

        let outcome =
            sidecar_primary_search_outcome_from_query_result(&controller, query_result, 5);

        match outcome {
            SidecarPrimarySearchOutcome::Served {
                hits,
                packet_hits,
                shadow,
                ..
            } => {
                assert_eq!(hits.len(), 1);
                assert_eq!(packet_hits.len(), 1);
                assert_eq!(packet_hits[0].hit.node_id, hits[0].node_id);
                assert_eq!(shadow.cancel_reason.as_deref(), Some("stage_deadline"));
                assert_eq!(shadow.resolved_hit_count, 1);
            }
            SidecarPrimarySearchOutcome::Rejected { reason, .. } => {
                panic!("resolved cancelled packet primary trace should serve: {reason}")
            }
            SidecarPrimarySearchOutcome::Unavailable { reason } => {
                panic!("resolved cancelled packet primary trace should stay available: {reason}")
            }
            SidecarPrimarySearchOutcome::Retryable { error } => {
                panic!("resolved cancelled packet primary trace should not retry: {error:?}")
            }
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

    // -----------------------------------------------------------------------
    // Stage 4: R6 admission queue and R2 widened hydration
    // -----------------------------------------------------------------------

    /// In-memory storage shaped like the C-chain bootstrap: a base stylesheet
    /// whose file-rooted trails expose a selector member, a depth-2 var
    /// usage, and the incoming IMPORT from the animation stylesheet.
    fn css_bootstrap_storage() -> Store {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        for (id, path) in [
            (1, "styles/_base.css"),
            (2, "styles/animate.css"),
            (4, "src/other.rs"),
        ] {
            storage
                .insert_file(&FileInfo {
                    id,
                    path: PathBuf::from(path),
                    language: "css".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 40,
                    file_role: FileRole::Source,
                })
                .expect("insert file");
        }
        storage
            .insert_nodes_batch(&[
                codestory_contracts::graph::Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::FILE,
                    serialized_name: "styles/_base.css".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(1),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(2),
                    kind: NodeKind::FILE,
                    serialized_name: "styles/animate.css".into(),
                    file_node_id: Some(CoreNodeId(2)),
                    start_line: Some(1),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(3),
                    kind: NodeKind::CONSTANT,
                    serialized_name: ".hero".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(5),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(4),
                    kind: NodeKind::FILE,
                    serialized_name: "src/other.rs".into(),
                    file_node_id: Some(CoreNodeId(4)),
                    start_line: Some(1),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(5),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "unrelated_filler".into(),
                    file_node_id: Some(CoreNodeId(4)),
                    start_line: Some(2),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(6),
                    kind: NodeKind::VARIABLE,
                    serialized_name: "--hero-color".into(),
                    file_node_id: Some(CoreNodeId(2)),
                    start_line: Some(3),
                    ..Default::default()
                },
                // Decoy (rev 5.3): a FIELD member matches no C typed-relation
                // pattern, so its identity must never join the need-set.
                codestory_contracts::graph::Node {
                    id: CoreNodeId(9),
                    kind: NodeKind::FIELD,
                    serialized_name: "decoy-field".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(9),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_edges_batch(&[
                codestory_contracts::graph::Edge {
                    id: codestory_contracts::graph::EdgeId(101),
                    source: CoreNodeId(1),
                    target: CoreNodeId(3),
                    kind: EdgeKind::MEMBER,
                    file_node_id: Some(CoreNodeId(1)),
                    ..Default::default()
                },
                codestory_contracts::graph::Edge {
                    id: codestory_contracts::graph::EdgeId(102),
                    source: CoreNodeId(2),
                    target: CoreNodeId(1),
                    kind: EdgeKind::IMPORT,
                    file_node_id: Some(CoreNodeId(2)),
                    ..Default::default()
                },
                codestory_contracts::graph::Edge {
                    id: codestory_contracts::graph::EdgeId(103),
                    source: CoreNodeId(3),
                    target: CoreNodeId(6),
                    kind: EdgeKind::USAGE,
                    file_node_id: Some(CoreNodeId(1)),
                    ..Default::default()
                },
                codestory_contracts::graph::Edge {
                    id: codestory_contracts::graph::EdgeId(104),
                    source: CoreNodeId(1),
                    target: CoreNodeId(9),
                    kind: EdgeKind::MEMBER,
                    file_node_id: Some(CoreNodeId(1)),
                    ..Default::default()
                },
            ])
            .expect("insert edges");
        storage
    }

    fn file_shaped_candidate(path: &str) -> CandidateHit {
        let mut candidate = CandidateHit::with_source(path, None, 0.6, CandidateSource::Lexical);
        candidate.target = Some(SearchTargetDto::FileRange {
            file_path: path.to_string(),
            start_byte: 0,
            end_byte: 10,
        });
        candidate
    }

    fn node_candidate(path: &str, node_id: &str, symbol_name: &str) -> CandidateHit {
        let mut candidate = CandidateHit::with_source(
            path,
            Some(symbol_name.to_string()),
            0.5,
            CandidateSource::Scip,
        );
        candidate.node_id = Some(node_id.to_string());
        candidate
    }

    /// A session carrying the REAL C-family spec (patterns included), derived
    /// from the css question exactly as the orchestrator derives it — the
    /// rev 5.3 need-gate matches hydrated edges against these patterns.
    fn file_structural_session() -> Rc<crate::agent::packet_candidate::PacketProofSession> {
        let requirements =
            Vec::new();
        let spec = crate::agent::packet_candidate::packet_atom_hydration_spec(&requirements);
        assert!(
            !spec.promotion_patterns.is_empty(),
            "the css question must derive C promotion patterns"
        );
        Rc::new(crate::agent::packet_candidate::PacketProofSession::new(
            spec,
        ))
    }

    /// R6 negative first: with no receipt-established identities (no packet
    /// session, so hydration exposes no structural endpoints), admission is
    /// pure base order and the budget cuts the tail exactly as before.
    /// With the session installed, the base stylesheet's file-rooted trails
    /// establish the animation file's canonical id through the incoming
    /// IMPORT, the late file candidate is promoted over the filler, the
    /// displaced filler ends un-attempted like a cap-cut candidate, and the
    /// whole outcome is deterministic across runs.
    #[test]
    fn r6_established_import_identity_promotes_late_file_candidate_deterministically() {
        let storage = css_bootstrap_storage();
        let candidates = vec![
            file_shaped_candidate("styles/_base.css"),
            node_candidate("src/other.rs", "5", "unrelated_filler"),
            file_shaped_candidate("styles/animate.css"),
        ];

        // Base order without a session: the filler consumes the second slot.
        let unpromoted = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &candidates,
            2,
        )
        .expect("resolve without session");
        assert_eq!(
            unpromoted
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["1", "5"],
            "without established identities admission stays base order"
        );
        assert_eq!(
            unpromoted.attempted_candidate_indices,
            HashSet::from([0, 1])
        );

        let run = || {
            let session = file_structural_session();
            let _guard =
                crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
            resolve_sidecar_candidates_in_storage(
                &storage,
                &HashMap::new(),
                Path::new("."),
                &candidates,
                2,
            )
            .expect("resolve with session")
        };
        let promoted = run();
        assert_eq!(
            promoted
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"],
            "the IMPORT-established canonical file id must promote the late candidate"
        );
        assert_eq!(promoted.attempted_candidate_indices, HashSet::from([0, 2]));
        assert_eq!(
            promoted.unresolved_candidate_count, 0,
            "the displaced filler is un-attempted, not unresolved — cap-cut semantics"
        );

        // Determinism: identical inputs yield identical outcomes.
        let second = run();
        assert_eq!(
            promoted
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.clone())
                .collect::<Vec<_>>(),
            second
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            promoted.attempted_candidate_indices,
            second.attempted_candidate_indices
        );

        // F3 REVISE: the IN-LOOP hydration is bounded to the depth-1
        // identity-establishing [IMPORT] trails (gate 5c: MEMBER dropped —
        // it feeds nothing under rev 5.4 and its fanout shares the trail
        // accessor's edge budget) — depth-2 structural coverage belongs to
        // the post-pass, never to the stage clock.
        let base_hit = promoted
            .packet_hits
            .iter()
            .find(|hit| hit.hit.node_id.0 == "1")
            .expect("base stylesheet packet hit");
        let scans = &base_hit.trail_scans;
        assert_eq!(scans.len(), 2, "one identity scan per direction: {scans:?}");
        for scan in scans {
            assert_eq!(scan.root, "1");
            assert_eq!(scan.depth, 1, "in-loop trails stay at depth 1: {scan:?}");
            assert_eq!(
                scan.edge_kinds,
                vec![codestory_contracts::api::EdgeKind::IMPORT]
            );
            assert!(!scan.truncated);
        }
        let graph = base_hit.graph.as_ref().expect("hydrated identity graph");
        assert!(
            graph.edges.iter().any(|edge| edge.id.0 == "102"),
            "the incoming IMPORT identity edge must be retained in the candidate graph"
        );
        for structural in ["101", "103", "104"] {
            assert!(
                !graph.edges.iter().any(|edge| edge.id.0 == structural),
                "MEMBER/USAGE edge {structural} must NOT be hydrated on the stage clock"
            );
        }
    }

    /// R6 negative: the promotion key is identity-only. Two node-id-bearing
    /// candidates that differ only in symbol_name and file_path receive
    /// identical promotion treatment — swapping their names changes nothing
    /// but base order.
    #[test]
    fn r6_promotion_key_ignores_symbol_names_and_paths() {
        let storage = css_bootstrap_storage();
        let outcome_for = |first_name: &str, second_name: &str| {
            let candidates = vec![
                file_shaped_candidate("styles/_base.css"),
                node_candidate("src/other.rs", "5", "unrelated_filler"),
                // Rev 5.4: the promotable identity is the IMPORT-established
                // entrypoint file node (2) — cross-container. Names and
                // paths on the two carriers differ arbitrarily.
                node_candidate("styles/animate.css", "2", first_name),
                node_candidate("completely/else.css", "2", second_name),
            ];
            let session = file_structural_session();
            let _guard =
                crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
            let outcome = resolve_sidecar_candidates_in_storage(
                &storage,
                &HashMap::new(),
                Path::new("."),
                &candidates,
                2,
            )
            .expect("resolve");
            (
                outcome
                    .resolved_hits
                    .iter()
                    .map(|hit| hit.node_id.0.clone())
                    .collect::<Vec<_>>(),
                outcome.attempted_candidate_indices,
            )
        };
        // The entrypoint node 2 is established through the base file's
        // incoming IMPORT; the earliest pending candidate with that identity
        // is promoted regardless of its display strings.
        let (first_hits, first_attempted) = outcome_for("animate", "zzz_unrelated");
        let (second_hits, second_attempted) = outcome_for("zzz_unrelated", "animate");
        assert_eq!(first_hits, ["1", "2"]);
        assert_eq!(first_hits, second_hits);
        assert_eq!(first_attempted, HashSet::from([0, 2]));
        assert_eq!(first_attempted, second_attempted);
    }

    /// R2: widened kinds run one separate bounded trail each, so a
    /// high-fanout widened kind saturates its own trail (and reports
    /// truncation for rule 7) while every CALL edge survives untouched.
    #[test]
    fn r2_widened_member_fanout_cannot_evict_call_and_reports_truncation() {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("src/hub.rs"),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 400,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        let mut nodes = vec![
            codestory_contracts::graph::Node {
                id: CoreNodeId(1),
                kind: NodeKind::FILE,
                serialized_name: "src/hub.rs".into(),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(10),
                kind: NodeKind::FUNCTION,
                serialized_name: "hub".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(2),
                ..Default::default()
            },
        ];
        nodes.extend((0..3).map(|index| codestory_contracts::graph::Node {
            id: CoreNodeId(20 + index),
            kind: NodeKind::FUNCTION,
            serialized_name: format!("callee_{index}"),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(10 + index as u32),
            ..Default::default()
        }));
        nodes.extend((0..80).map(|index| codestory_contracts::graph::Node {
            id: CoreNodeId(100 + index),
            kind: NodeKind::CLASS,
            serialized_name: format!("owner_{index}"),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(50 + index as u32),
            ..Default::default()
        }));
        nodes.extend((0..2).map(|index| codestory_contracts::graph::Node {
            id: CoreNodeId(200 + index),
            kind: NodeKind::VARIABLE,
            serialized_name: format!("used_{index}"),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(200 + index as u32),
            ..Default::default()
        }));
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        let mut edges = (0..3)
            .map(|index| codestory_contracts::graph::Edge {
                id: codestory_contracts::graph::EdgeId(300 + index),
                source: CoreNodeId(10),
                target: CoreNodeId(20 + index),
                kind: EdgeKind::CALL,
                certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
                file_node_id: Some(CoreNodeId(1)),
                line: Some(10 + index as u32),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        edges.extend((0..80).map(|index| codestory_contracts::graph::Edge {
            id: codestory_contracts::graph::EdgeId(1_000 + index),
            source: CoreNodeId(100 + index),
            target: CoreNodeId(10),
            kind: EdgeKind::MEMBER,
            file_node_id: Some(CoreNodeId(1)),
            ..Default::default()
        }));
        edges.extend((0..2).map(|index| codestory_contracts::graph::Edge {
            id: codestory_contracts::graph::EdgeId(500 + index),
            source: CoreNodeId(10),
            target: CoreNodeId(200 + index),
            kind: EdgeKind::USAGE,
            file_node_id: Some(CoreNodeId(1)),
            ..Default::default()
        }));
        storage.insert_edges_batch(&edges).expect("insert edges");

        let session = Rc::new(crate::agent::packet_candidate::PacketProofSession::new(
            crate::agent::packet_candidate::PacketAtomHydrationSpec {
                rooted: vec![(
                    ApiNodeKind::FUNCTION,
                    vec![
                        codestory_contracts::api::EdgeKind::MEMBER,
                        codestory_contracts::api::EdgeKind::USAGE,
                    ],
                )],
                file_structural: false,
                absence_kinds: vec![codestory_contracts::api::EdgeKind::USAGE],
                promotion_patterns: Vec::new(),
                role_scoring_patterns: Vec::new(),
                formulas: Vec::new(),
            },
        ));
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));

        // Gate round 2, in-loop bound: the stage clock runs the baseline
        // CALL trails plus the depth-1 IDENTITY kinds only — MEMBER (an
        // identity establisher, its saturating fanout recording rule-7
        // truncation in-loop) runs; USAGE (not an identity kind) must NOT.
        let candidate = node_candidate("src/hub.rs", "10", "hub");
        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[candidate],
            1,
        )
        .expect("resolve hub candidate");
        let packet_hit = outcome.packet_hits.first().expect("packet hit");
        let in_loop_graph = packet_hit.graph.as_ref().expect("graph");
        for call_edge in ["300", "301", "302"] {
            assert!(
                in_loop_graph
                    .edges
                    .iter()
                    .any(|edge| edge.id.0 == call_edge),
                "the baseline CALL hydration must retain CALL edge {call_edge}"
            );
        }
        // Gate 5c: MEMBER and USAGE are not cross-container kinds, so NO
        // widened identity trail runs in-loop for this spec — the stage
        // clock carries exactly the baseline CALL hydration; the MEMBER and
        // USAGE trails (and their rule-7 truncation records) belong to the
        // post-pass below.
        assert!(
            !in_loop_graph
                .edges
                .iter()
                .any(|edge| edge.kind != codestory_contracts::api::EdgeKind::CALL),
            "only baseline CALL edges may hydrate on the stage clock"
        );
        assert!(
            packet_hit
                .trail_scans
                .iter()
                .all(|scan| scan.edge_kinds == vec![codestory_contracts::api::EdgeKind::CALL]),
            "in-loop scans are the baseline CALL trails only: {:?}",
            packet_hit.trail_scans
        );

        // POST-PASS: the full atom-kind trails (including USAGE) run over
        // the retained set, fill the ledger, keep every CALL edge untouched,
        // and record truncation honestly for rule 7.
        let mut answer = sidecar_answer_with_citation_node("10");
        crate::agent::packet_candidate::merge_packet_candidate_graph_for_requirements(
            &mut answer,
            packet_hit,
            &[],
        );
        let call_edges_before = answer
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
                GraphArtifactDto::Mermaid { .. } => None,
            })
            .flatten()
            .filter(|edge| edge.kind == codestory_contracts::api::EdgeKind::CALL)
            .count();
        hydrate_packet_atom_trails_in_storage(&storage, &HashMap::new(), &session, &mut answer);

        let post_pass = answer
            .graphs
            .iter()
            .find_map(|artifact| match artifact {
                GraphArtifactDto::Uml { id, graph, .. }
                    if id.starts_with(PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX) =>
                {
                    Some(graph)
                }
                _ => None,
            })
            .expect("post-pass hydration artifact");
        assert!(
            post_pass
                .edges
                .iter()
                .any(|edge| edge.kind == codestory_contracts::api::EdgeKind::MEMBER),
            "the post-pass runs the widened MEMBER trails"
        );
        assert!(
            post_pass
                .edges
                .iter()
                .any(|edge| edge.kind == codestory_contracts::api::EdgeKind::USAGE),
            "the post-pass runs the deferred USAGE trails"
        );
        assert!(post_pass.truncated, "80 members overflow the 65-node cap");
        let call_edges_after = answer
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
                GraphArtifactDto::Mermaid { .. } => None,
            })
            .flatten()
            .filter(|edge| edge.kind == codestory_contracts::api::EdgeKind::CALL)
            .count();
        assert_eq!(
            call_edges_before, call_edges_after,
            "the post-pass never evicts CALL evidence"
        );

        let ledger = session.artifact_scans();
        let (_, scans) = ledger
            .iter()
            .find(|(artifact_id, _)| artifact_id.starts_with(PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX))
            .expect("post-pass ledger entry");
        let member_incoming = scans
            .iter()
            .find(|scan| {
                scan.direction == PacketGraphDirection::Incoming
                    && scan.edge_kinds == vec![codestory_contracts::api::EdgeKind::MEMBER]
            })
            .expect("incoming MEMBER scan record");
        assert!(
            member_incoming.truncated,
            "an over-cap scan must record truncation for rule 7: {member_incoming:?}"
        );
        let member_outgoing = scans
            .iter()
            .find(|scan| {
                scan.direction == PacketGraphDirection::Outgoing
                    && scan.edge_kinds == vec![codestory_contracts::api::EdgeKind::MEMBER]
            })
            .expect("outgoing MEMBER scan record");
        assert!(
            !member_outgoing.truncated && member_outgoing.coverage_edge_ids.is_empty(),
            "an empty untruncated scan is recorded — absence facts need it: {member_outgoing:?}"
        );

        // Idempotence: a second post-pass changes nothing.
        let graphs_snapshot = serde_json::to_value(&answer.graphs).expect("graphs");
        hydrate_packet_atom_trails_in_storage(&storage, &HashMap::new(), &session, &mut answer);
        assert_eq!(
            serde_json::to_value(&answer.graphs).expect("graphs"),
            graphs_snapshot
        );
        assert_eq!(session.artifact_scans().len(), ledger.len());
    }

    /// Minimal answer fixture with one citation, for post-pass tests.
    fn sidecar_answer_with_citation_node(node_id: &str) -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "post-pass".into(),
            prompt: "post-pass".into(),
            summary: String::new(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![codestory_contracts::api::AgentCitationDto {
                node_id: NodeId(node_id.to_string()),
                display_name: format!("node-{node_id}"),
                kind: ApiNodeKind::FUNCTION,
                file_path: Some("src/hub.rs".into()),
                line: Some(2),
                score: 0.9,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                subgraph_id: None,
                evidence_edge_ids: Vec::new(),
                retrieval_score_breakdown: None,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: Some(true),
                source_excerpt: None,
            }],
            subgraph_ids: Vec::new(),
            retrieval_version: "sidecar".into(),
            graphs: Vec::new(),
            source_coverage: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "post-pass".into(),
                retrieval_publication: None,
                resolved_profile: codestory_contracts::api::AgentRetrievalPresetDto::Architecture,
                policy_mode: codestory_contracts::api::AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 0,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                semantic_stage_timeout_zero_hits: 0,
                semantic_abstained_count: 0,
                annotations: Vec::new(),
                packet_claim_profile_telemetry: None,
                source_freshness_telemetry: None,
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        }
    }

    /// The post-pass over a retained FILE citation runs the depth-2 uniform
    /// structural trails, fills the ledger with NARROWED coverage sets (the
    /// absence-subject USAGE edges plus the depth-2 MEMBER witnesses — never
    /// the incidental IMPORT edge), and stays within its cost budget.
    #[test]
    fn post_pass_fills_ledger_with_narrowed_coverage_for_retained_file_roots() {
        let storage = css_bootstrap_storage();
        let session = file_structural_session();
        let mut answer = sidecar_answer_with_citation_node("1");
        answer.citations[0].kind = ApiNodeKind::FILE;
        answer.citations[0].file_path = Some("styles/_base.css".into());

        hydrate_packet_atom_trails_in_storage(&storage, &HashMap::new(), &session, &mut answer);

        let post_pass = answer
            .graphs
            .iter()
            .find_map(|artifact| match artifact {
                GraphArtifactDto::Uml { id, graph, .. }
                    if id == &format!("{PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX}1") =>
                {
                    Some(graph)
                }
                _ => None,
            })
            .expect("post-pass artifact for the retained stylesheet");
        for required in ["101", "102", "103"] {
            assert!(
                post_pass.edges.iter().any(|edge| edge.id.0 == required),
                "structural edge {required} must be hydrated by the post-pass"
            );
        }

        let ledger = session.artifact_scans();
        let (_, scans) = ledger
            .iter()
            .find(|(artifact_id, _)| {
                artifact_id == &format!("{PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX}1")
            })
            .expect("ledger entry for the post-pass artifact");
        assert_eq!(scans.len(), 2, "one depth-2 scan per direction: {scans:?}");
        let outgoing = scans
            .iter()
            .find(|scan| scan.direction == PacketGraphDirection::Outgoing)
            .expect("outgoing structural scan");
        assert_eq!(outgoing.depth, 2);
        assert_eq!(
            outgoing.edge_kinds,
            vec![
                codestory_contracts::api::EdgeKind::MEMBER,
                codestory_contracts::api::EdgeKind::USAGE,
                codestory_contracts::api::EdgeKind::IMPORT,
            ]
        );
        assert!(!outgoing.truncated);
        // Narrowing (F3 finding 3): USAGE 103 (absence subject) and MEMBER
        // 101 (depth-2 witness) are recorded; the IMPORT edges are not.
        let mut recorded = outgoing
            .coverage_edge_ids
            .iter()
            .map(|edge_id| edge_id.0.as_str())
            .collect::<Vec<_>>();
        recorded.sort_unstable();
        assert_eq!(
            recorded,
            ["101", "103", "104"],
            "the narrowed set is the absence subject plus the MEMBER witnesses"
        );
        let incoming = scans
            .iter()
            .find(|scan| scan.direction == PacketGraphDirection::Incoming)
            .expect("incoming structural scan");
        assert!(
            !incoming
                .coverage_edge_ids
                .iter()
                .any(|edge_id| edge_id.0 == "102"),
            "the incidental IMPORT edge never joins a coverage set: {incoming:?}"
        );
    }

    /// Gate round 2, finding 1 — the cross-query bootstrap shape the
    /// single-pass test cannot catch: identities established while resolving
    /// QUERY 1's candidates must promote candidates sitting in QUERY 2's
    /// window, because the R6 promotion state lives in the packet-scoped
    /// session, not per resolution call. Negative first: without a shared
    /// session the second query falls back to base order.
    #[test]
    fn r6_identities_established_in_one_query_promote_candidates_in_later_queries() {
        let storage = css_bootstrap_storage();
        let query_one = vec![file_shaped_candidate("styles/_base.css")];
        let query_two = vec![
            node_candidate("src/other.rs", "5", "unrelated_filler"),
            // Decoy (rev 5.3): node 9 was hydrated as a MEMBER endpoint in
            // query 1, but its FIELD-kind edge matches no unproven C atom
            // pattern — an identity that merely exists must not promote.
            node_candidate("styles/_base.css", "9", "decoy_field"),
            file_shaped_candidate("styles/animate.css"),
        ];

        // Without an installed session each call gets a throwaway identity
        // scope: query 2 never sees query 1's identities and admits by base
        // order.
        {
            let session = file_structural_session();
            let _guard =
                crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
            resolve_sidecar_candidates_in_storage(
                &storage,
                &HashMap::new(),
                Path::new("."),
                &query_one,
                1,
            )
            .expect("resolve query one");
        }
        let isolated = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &query_two,
            1,
        )
        .expect("resolve query two without shared session");
        assert_eq!(
            isolated
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["5"],
            "without cross-query identity state the filler wins by base order"
        );

        // With ONE session across both queries, the base stylesheet's
        // identity trails in query 1 establish the animation file's canonical
        // id (incoming IMPORT), and query 2 promotes it over the filler.
        let session = file_structural_session();
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
        let first = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &query_one,
            1,
        )
        .expect("resolve query one");
        assert_eq!(first.resolved_hits.len(), 1);
        let second = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &query_two,
            1,
        )
        .expect("resolve query two under the shared session");
        assert_eq!(
            second
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["2"],
            "query 1's atom-needed identity must promote query 2's beyond-window candidate"
        );
        assert_eq!(
            second.attempted_candidate_indices,
            HashSet::from([2]),
            "the pattern-matched entrypoint promotes; the decoy and the filler are displaced"
        );
    }

    /// Rev 5.3 point 2 — all-Legacy inertness: with no formula-bearing
    /// requirements the promotion need-set can never populate, and admission
    /// under an installed session is bit-identical to no session at all —
    /// same resolved set, same order, same attempted indices.
    #[test]
    fn r6_all_legacy_session_admission_is_bit_identical_to_no_session() {
        let storage = css_bootstrap_storage();
        let candidates = vec![
            file_shaped_candidate("styles/_base.css"),
            node_candidate("src/other.rs", "5", "unrelated_filler"),
            file_shaped_candidate("styles/animate.css"),
        ];
        let baseline = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &candidates,
            2,
        )
        .expect("resolve without session");

        let legacy_requirements =
            Vec::new();
        let spec = crate::agent::packet_candidate::packet_atom_hydration_spec(&legacy_requirements);
        assert!(
            spec.promotion_patterns.is_empty(),
            "all-Legacy requirements must derive no promotion patterns"
        );
        // Round 5.5 item 2: no cross-container pattern means no promotion
        // SLOT either, so admission cannot even express a promotion.
        assert!(
            spec.promotion_role_slots().is_empty(),
            "all-Legacy requirements must derive no promotion slots"
        );
        assert!(
            spec.role_scoring_patterns.is_empty(),
            "all-Legacy requirements carry no typed pattern to score with either"
        );
        let session = Rc::new(crate::agent::packet_candidate::PacketProofSession::new(
            spec,
        ));
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
        let under_session = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &candidates,
            2,
        )
        .expect("resolve under all-Legacy session");

        assert_eq!(
            baseline
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.clone())
                .collect::<Vec<_>>(),
            under_session
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.clone())
                .collect::<Vec<_>>(),
            "all-Legacy admission must be bit-identical to pre-R6 behavior"
        );
        assert_eq!(
            baseline.attempted_candidate_indices,
            under_session.attempted_candidate_indices
        );
        assert_eq!(
            baseline.unresolved_candidate_count,
            under_session.unresolved_candidate_count
        );
        assert!(
            !session.has_atom_needed_identities(),
            "no pattern, no need — the set must stay empty"
        );
        assert!(
            !session.promotion_is_active(),
            "promotion stays structurally inert for all-Legacy packets"
        );
        assert!(
            session.retired_requirements().is_empty(),
            "the query-boundary checkpoint is a no-op without formulas"
        );
    }

    /// Rev 5.3 point 3 — M-shard no-displacement: the M atoms join only
    /// FlowOwner (the CALL source, already baseline-hydrated); M3's dispatch
    /// target is an `Any` endpoint, so even rich need-set accumulation from
    /// matching dispatch edges promotes nothing and admission stays
    /// identical to no session.
    #[test]
    fn r6_m_shard_accumulation_produces_no_displacement() {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("src/logger.php"),
                language: "php".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 60,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        storage
            .insert_nodes_batch(&[
                codestory_contracts::graph::Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::FILE,
                    serialized_name: "src/logger.php".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(1),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(10),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "invokeHandlers".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(8),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(20),
                    kind: NodeKind::METHOD,
                    serialized_name: "Handler.handle".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(30),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(5),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "unrelated_filler".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(50),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_edges_batch(&[codestory_contracts::graph::Edge {
                id: codestory_contracts::graph::EdgeId(600),
                source: CoreNodeId(10),
                target: CoreNodeId(20),
                kind: EdgeKind::CALL,
                resolved_target: Some(CoreNodeId(20)),
                certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
                callsite_identity: Some(
                    "src/logger.php:10:5:handle|syntax:php-call|receiver-owner:handler|receiver-binding:loop-element@8-14"
                        .to_string(),
                ),
                file_node_id: Some(CoreNodeId(1)),
                line: Some(10),
                ..Default::default()
            }])
            .expect("insert edge");

        let candidates = vec![
            node_candidate("src/logger.php", "10", "invokeHandlers"),
            node_candidate("src/logger.php", "5", "unrelated_filler"),
            node_candidate("src/logger.php", "20", "Handler.handle"),
        ];
        let baseline = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &candidates,
            2,
        )
        .expect("resolve without session");

        let m_requirements =
            Vec::new();
        let spec = crate::agent::packet_candidate::packet_atom_hydration_spec(&m_requirements);
        // Rev 5.4: the M formula names only CALL — no cross-container kind —
        // so it derives ZERO promotion patterns and admission is
        // structurally inert, not merely endpoint-shaped.
        assert!(
            spec.promotion_patterns.is_empty(),
            "CALL is not cross-container; the M spec must derive no promotion patterns"
        );
        // Round 5.5 item 2: zero cross-container patterns → zero promotion
        // slots → the M shard is structurally unchanged, not merely quiet.
        assert!(
            spec.promotion_role_slots().is_empty(),
            "the M spec must derive no promotion slots"
        );
        assert!(
            !spec.role_scoring_patterns.is_empty(),
            "the M formulas do carry typed patterns — what makes the shard inert \
             is the absent CROSS-CONTAINER pattern, not an absent formula"
        );
        let session = Rc::new(crate::agent::packet_candidate::PacketProofSession::new(
            spec,
        ));
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
        let under_session = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &candidates,
            2,
        )
        .expect("resolve under M session");

        assert!(
            !session.has_atom_needed_identities(),
            "rev 5.4: a CALL-only formula accumulates nothing at all"
        );
        assert!(
            !session.identity_is_atom_needed(20),
            "M3's dispatch target never becomes atom-needed"
        );
        // Gate 6: with no promotion pattern the scoring path is never
        // entered at all — no attribution, no score, no ordering decision.
        for identity in [10, 20, 5] {
            assert_eq!(
                session.promotion_priority(identity),
                0,
                "zero promotion patterns means zero scoring work: {identity}"
            );
        }
        assert_eq!(
            baseline
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.clone())
                .collect::<Vec<_>>(),
            under_session
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.clone())
                .collect::<Vec<_>>(),
            "rich M accumulation must produce no displacement"
        );
        assert_eq!(
            baseline.attempted_candidate_indices,
            under_session.attempted_candidate_indices
        );
        assert!(
            !session.promotion_is_active(),
            "promotion stays structurally inert for the M shard"
        );
        assert!(
            session.retired_requirements().is_empty(),
            "with no promotion pattern there is nothing the checkpoint could retire"
        );
    }

    /// Gate round 2, finding 2 — the A-shard bootstrap: a CLASS root under an
    /// A-family spec runs depth-1 [TYPE_USAGE, MEMBER] identity trails
    /// in-loop, the certain TYPE_USAGE edge establishes the config type's
    /// identity, and the TypeMap-shaped candidate beyond the window is
    /// promoted over the filler.
    #[test]
    fn r6_a_shard_type_usage_identity_trail_promotes_beyond_window_candidate() {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("src/builder.cs"),
                language: "csharp".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 60,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        storage
            .insert_nodes_batch(&[
                codestory_contracts::graph::Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::FILE,
                    serialized_name: "src/builder.cs".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(1),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(10),
                    kind: NodeKind::CLASS,
                    serialized_name: "TypeMapPlanBuilder".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(5),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(30),
                    kind: NodeKind::CLASS,
                    serialized_name: "TypeMap".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(30),
                    ..Default::default()
                },
                // Gate 6: a lone configuration TARGET — one role position.
                codestory_contracts::graph::Node {
                    id: CoreNodeId(32),
                    kind: NodeKind::CLASS,
                    serialized_name: "ResolutionContext".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(32),
                    ..Default::default()
                },
                codestory_contracts::graph::Node {
                    id: CoreNodeId(5),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "unrelated_filler".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(50),
                    ..Default::default()
                },
                // Decoy (rev 5.3): a FIELD member of the builder — hydrated by
                // the MEMBER identity trail, but A3's MEMBER pattern names
                // METHOD targets, so this identity is never atom-needed.
                codestory_contracts::graph::Node {
                    id: CoreNodeId(50),
                    kind: NodeKind::FIELD,
                    serialized_name: "decoy_field".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(9),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_edges_batch(&[
                codestory_contracts::graph::Edge {
                    id: codestory_contracts::graph::EdgeId(400),
                    source: CoreNodeId(10),
                    target: CoreNodeId(30),
                    kind: EdgeKind::TYPE_USAGE,
                    certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
                    file_node_id: Some(CoreNodeId(1)),
                    line: Some(7),
                    ..Default::default()
                },
                codestory_contracts::graph::Edge {
                    id: codestory_contracts::graph::EdgeId(401),
                    source: CoreNodeId(10),
                    target: CoreNodeId(50),
                    kind: EdgeKind::MEMBER,
                    file_node_id: Some(CoreNodeId(1)),
                    ..Default::default()
                },
                // Gate 6: the plan type also stands in the SOURCE position of
                // the config atom, giving it two role positions to the lone
                // target's one.
                codestory_contracts::graph::Edge {
                    id: codestory_contracts::graph::EdgeId(402),
                    source: CoreNodeId(30),
                    target: CoreNodeId(10),
                    kind: EdgeKind::TYPE_USAGE,
                    certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
                    file_node_id: Some(CoreNodeId(1)),
                    line: Some(31),
                    ..Default::default()
                },
                codestory_contracts::graph::Edge {
                    id: codestory_contracts::graph::EdgeId(403),
                    source: CoreNodeId(10),
                    target: CoreNodeId(32),
                    kind: EdgeKind::TYPE_USAGE,
                    certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
                    file_node_id: Some(CoreNodeId(1)),
                    line: Some(33),
                    ..Default::default()
                },
            ])
            .expect("insert edges");

        let requirements =
            Vec::new();
        let session = Rc::new(crate::agent::packet_candidate::PacketProofSession::new(
            crate::agent::packet_candidate::packet_atom_hydration_spec(&requirements),
        ));
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
        let candidates = vec![
            node_candidate("src/builder.cs", "10", "TypeMapPlanBuilder"),
            node_candidate("src/builder.cs", "5", "unrelated_filler"),
            // Decoy (rev 5.3): hydrated as a MEMBER endpoint, but matching no
            // unproven A atom pattern — it must NOT promote.
            node_candidate("src/builder.cs", "50", "decoy_field"),
            // Gate 6: the lone configuration target sits EARLIER in base
            // order than the two-position identity behind it.
            node_candidate("src/builder.cs", "32", "ResolutionContext"),
            node_candidate("src/builder.cs", "30", "TypeMap"),
        ];
        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &candidates,
            2,
        )
        .expect("resolve A-shard candidates");
        assert_eq!(
            outcome
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["10", "30"],
            "the atom-needed TYPE_USAGE identity must promote TypeMap over filler and decoy"
        );
        assert_eq!(outcome.attempted_candidate_indices, HashSet::from([0, 4]));
        assert!(
            !session.identity_is_atom_needed(50),
            "the decoy MEMBER endpoint matches no A pattern and is never atom-needed"
        );
        // Gate 6: multiplicity, not base order, decided which identity got
        // the ConfigType-family slot.
        assert_eq!(
            (
                session.promotion_priority(30),
                session.promotion_priority(32)
            ),
            (2, 1),
            "the two-position plan type outranks the lone configuration target"
        );
        assert!(
            session.identity_is_atom_needed(32),
            "the lone target is still needed — it was outranked, not excluded"
        );

        let builder_hit = outcome
            .packet_hits
            .iter()
            .find(|hit| hit.hit.node_id.0 == "10")
            .expect("builder packet hit");
        let graph = builder_hit.graph.as_ref().expect("identity graph");
        assert!(
            graph.edges.iter().any(|edge| edge.id.0 == "400"),
            "the TYPE_USAGE identity edge must be hydrated in-loop"
        );
        for scan in &builder_hit.trail_scans {
            assert_eq!(scan.depth, 1, "identity trails stay depth-1: {scan:?}");
            assert_eq!(
                scan.edge_kinds.len(),
                1,
                "non-FILE identity trails are single-kind: {scan:?}"
            );
        }
        let scanned_kinds = builder_hit
            .trail_scans
            .iter()
            .map(|scan| scan.edge_kinds[0])
            .collect::<HashSet<_>>();
        assert_eq!(
            scanned_kinds,
            HashSet::from([codestory_contracts::api::EdgeKind::TYPE_USAGE]),
            "gate 5c: in-loop identity kinds are the rooted kinds ∩ cross-container set"
        );
    }

    /// Gate round 4 telemetry: the armed session records the need-set with
    /// per-id pattern provenance, per-query admission decisions with the
    /// promoted flag, and the derived why-not attribution for the
    /// un-attempted remainder — rendered into the `r6_session` step-trace
    /// section.
    #[test]
    fn r6_session_trace_records_need_set_provenance_and_admission_decisions() {
        let storage = css_bootstrap_storage();
        let requirements =
            Vec::new();
        let session = Rc::new(
            crate::agent::packet_candidate::PacketProofSession::new(
                crate::agent::packet_candidate::packet_atom_hydration_spec(&requirements),
            )
            .with_trace_enabled(),
        );
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
        let query_one = vec![file_shaped_candidate("styles/_base.css")];
        let query_two = vec![
            node_candidate("src/other.rs", "5", "unrelated_filler"),
            file_shaped_candidate("styles/animate.css"),
        ];
        resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &query_one,
            1,
        )
        .expect("resolve query one");
        resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &query_two,
            1,
        )
        .expect("resolve query two");

        let trace = session.r6_trace_json();
        assert!(trace["promotion_pattern_count"].as_u64().unwrap() > 0);
        let need_set = trace["need_set"].as_array().expect("need_set");
        assert!(
            need_set.iter().any(|entry| {
                entry["node_id"].as_i64() == Some(2)
                    && entry["pattern_kind"].as_str() == Some("IMPORT")
            }),
            "the entrypoint identity carries its IMPORT pattern provenance: {need_set:?}"
        );
        let admissions = trace["query_admissions"].as_array().expect("admissions");
        assert_eq!(admissions.len(), 2, "one admission record per query");
        assert_eq!(admissions[0]["query_index"].as_u64(), Some(0));
        let q1_admitted = admissions[0]["admitted"].as_array().unwrap();
        assert_eq!(q1_admitted[0]["node_id"].as_str(), Some("1"));
        assert_eq!(q1_admitted[0]["promoted"].as_bool(), Some(false));
        let q2_admitted = admissions[1]["admitted"].as_array().unwrap();
        assert_eq!(q2_admitted[0]["node_id"].as_str(), Some("2"));
        assert_eq!(
            q2_admitted[0]["promoted"].as_bool(),
            Some(true),
            "the cross-query promotion is attributed"
        );
        let q2_unattempted = admissions[1]["unattempted"].as_array().unwrap();
        assert!(
            q2_unattempted
                .iter()
                .any(|entry| entry["why_not"].as_str() == Some("not_in_need_set")),
            "the displaced filler is attributed: {q2_unattempted:?}"
        );
        assert!(
            !trace["identity_hydrations"].as_array().unwrap().is_empty(),
            "identity-trail hydrations are summarized per root"
        );
    }

    /// Gate 9 item 2 — the fourth selection. A deadline-cancelled batch query
    /// used to contribute NOTHING: every candidate it had already resolved
    /// was discarded before scoring, ranking or carry could see it. The
    /// AutoMapper shard measured 32 of 32 queries cancelled and 327 resolved
    /// hits thrown away, while the single-query path was already serving such
    /// hits — a plain asymmetry, unrelated to atoms.
    ///
    /// Retention is therefore NOT atom-gated: every resolved hit survives,
    /// ordered by resolution rank, with the atom signal breaking ties among
    /// equal ranks only.
    #[test]
    fn deadline_cancelled_queries_retain_resolved_hits_ordered_by_rank() {
        let hit_with = |id: i64, score: f32| {
            let mut hit = PacketSearchHit::without_graph(SearchHit {
                node_id: NodeId(id.to_string()),
                display_name: format!("symbol_{id}"),
                kind: ApiNodeKind::CLASS,
                file_path: Some(format!("src/f{id}.rs")),
                line: Some(1),
                score,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                match_quality: None,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: Some(true),
                source_excerpt: None,
                verification_targets: Vec::new(),
                score_breakdown: None,
            });
            hit.hit.score = score;
            hit
        };
        // Ranks 0.9 / 0.5 / 0.5 / 0.2: one clear leader, one tied pair, one
        // trailer.
        let hits = || {
            vec![
                hit_with(1, 0.9),
                hit_with(2, 0.5),
                hit_with(3, 0.5),
                hit_with(4, 0.2),
            ]
        };
        let order = |hits: Vec<PacketSearchHit>| {
            retained_cancelled_packet_hits(hits)
                .into_iter()
                .map(|hit| hit.hit.node_id.0.clone())
                .collect::<Vec<_>>()
        };

        assert_eq!(
            order(hits()),
            ["1", "2", "3", "4"],
            "with no session every resolved hit is retained in rank order — \
             the single-query path's semantics"
        );

        let legacy_requirements =
            Vec::new();
        let legacy = Rc::new(crate::agent::packet_candidate::PacketProofSession::new(
            crate::agent::packet_candidate::packet_atom_hydration_spec(&legacy_requirements),
        ));
        {
            let _guard =
                crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&legacy));
            assert_eq!(
                order(hits()),
                ["1", "2", "3", "4"],
                "an all-Legacy packet has no need-set, so the order is pure rank"
            );
        }

        // Node 3 is atom-needed and TIED with node 2 at 0.5: the atom signal
        // breaks that tie and nothing else moves. The clear leader keeps its
        // place and the trailer keeps its place — need never overtakes rank.
        let session = session_needing("3", "9");
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
        assert!(session.identity_is_atom_needed(3));
        assert_eq!(
            order(hits()),
            ["1", "3", "2", "4"],
            "need-first applies only among equal ranks"
        );
        assert_eq!(order(hits()), order(hits()), "and it is deterministic");
    }

    /// A C-family session with the R6 trace armed, so the per-query
    /// promotion SLOT accounting is observable in assertions.
    fn traced_file_structural_session() -> Rc<crate::agent::packet_candidate::PacketProofSession> {
        let requirements =
            Vec::new();
        Rc::new(
            crate::agent::packet_candidate::PacketProofSession::new(
                crate::agent::packet_candidate::packet_atom_hydration_spec(&requirements),
            )
            .with_trace_enabled(),
        )
    }

    /// The promotion roles one traced query spent, in consumption order.
    fn promotion_roles_used(
        session: &crate::agent::packet_candidate::PacketProofSession,
        query_index: usize,
    ) -> Vec<String> {
        session.r6_trace_json()["query_admissions"][query_index]["promotion_roles_used"]
            .as_array()
            .expect("promotion_roles_used")
            .iter()
            .map(|role| role.as_str().expect("role").to_string())
            .collect()
    }

    /// One entrypoint stylesheet importing `targets` sibling stylesheets,
    /// plus an unrelated filler symbol — the C-shard import-closure shape.
    fn css_entrypoint_closure_storage(targets: i64) -> Store {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        let mut files = vec![
            (1, "styles/entry.css", "css"),
            (900, "src/other.rs", "rust"),
        ];
        let target_paths = (0..targets)
            .map(|index| (100 + index, format!("styles/t{index:02}.css")))
            .collect::<Vec<_>>();
        for (id, path) in &target_paths {
            files.push((*id, path.as_str(), "css"));
        }
        for (id, path, language) in files {
            storage
                .insert_file(&FileInfo {
                    id,
                    path: PathBuf::from(path),
                    language: language.to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 40,
                    file_role: FileRole::Source,
                })
                .expect("insert file");
        }
        let mut nodes = vec![
            codestory_contracts::graph::Node {
                id: CoreNodeId(1),
                kind: NodeKind::FILE,
                serialized_name: "styles/entry.css".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(1),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(900),
                kind: NodeKind::FILE,
                serialized_name: "src/other.rs".into(),
                file_node_id: Some(CoreNodeId(900)),
                start_line: Some(1),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(5),
                kind: NodeKind::FUNCTION,
                serialized_name: "unrelated_filler".into(),
                file_node_id: Some(CoreNodeId(900)),
                start_line: Some(2),
                ..Default::default()
            },
        ];
        nodes.extend(
            target_paths
                .iter()
                .map(|(id, path)| codestory_contracts::graph::Node {
                    id: CoreNodeId(*id),
                    kind: NodeKind::FILE,
                    serialized_name: path.clone(),
                    file_node_id: Some(CoreNodeId(*id)),
                    start_line: Some(1),
                    ..Default::default()
                }),
        );
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        let edges = (0..targets)
            .map(|index| codestory_contracts::graph::Edge {
                id: codestory_contracts::graph::EdgeId(2_000 + index),
                source: CoreNodeId(1),
                target: CoreNodeId(100 + index),
                kind: EdgeKind::IMPORT,
                file_node_id: Some(CoreNodeId(1)),
                line: Some(2 + index as u32),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        storage.insert_edges_batch(&edges).expect("insert edges");
        storage
    }

    /// Round 5.5 item 2a — C shard: promotion is capped at FOUR per query,
    /// the atom-derived slot count (the entrypoint role plus the three
    /// source-file roles the C IMPORT patterns name). A fifth atom-needed
    /// candidate finds no free slot and admission falls back to base order
    /// for the rest of the query — retirement and slots silence promotion
    /// only, they never change base-order admission.
    #[test]
    fn r6_c_shard_promotions_are_capped_at_the_four_atom_derived_role_slots() {
        let storage = css_entrypoint_closure_storage(6);
        let session = traced_file_structural_session();
        assert_eq!(
            session.hydration.promotion_role_slots().len(),
            4,
            "the C formulas derive exactly four promotion slots"
        );
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));

        // Query 0 bootstraps: the entrypoint resolves in base order and its
        // depth-1 IMPORT identity trail establishes the closure.
        resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[file_shaped_candidate("styles/entry.css")],
            1,
        )
        .expect("bootstrap query");
        assert!(
            promotion_roles_used(&session, 0).is_empty(),
            "the bootstrap query has nothing to promote yet"
        );

        // Query 1 offers five atom-needed identities behind a filler: four
        // source/entrypoint slots exist, so exactly four promotions happen.
        let candidates = vec![
            node_candidate("src/other.rs", "5", "unrelated_filler"),
            node_candidate("styles/t00.css", "100", "t00"),
            node_candidate("styles/t01.css", "101", "t01"),
            node_candidate("styles/t02.css", "102", "t02"),
            node_candidate("styles/entry.css", "1", "entry"),
            node_candidate("styles/t03.css", "103", "t03"),
        ];
        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &candidates,
            6,
        )
        .expect("slot-bounded query");
        assert_eq!(
            promotion_roles_used(&session, 1),
            vec!["VarsSource", "BaseSource", "AnimSource", "Entrypoint"],
            "each of the four atom-derived roles is spent exactly once"
        );
        assert_eq!(
            outcome
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["100", "101", "102", "1", "5", "103"],
            "after the four slots are spent admission returns to base order"
        );
        assert!(
            session.identity_is_atom_needed(103),
            "the fifth identity is still needed — it simply had no free slot"
        );

        // Slots are PER QUERY: the next query re-opens them.
        let next = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[
                node_candidate("src/other.rs", "5", "unrelated_filler"),
                node_candidate("styles/t03.css", "103", "t03"),
            ],
            1,
        )
        .expect("next query");
        assert_eq!(
            next.resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["103"],
            "a fresh query re-opens the source slots"
        );
        assert!(
            session.retired_requirements().is_empty(),
            "the C requirements each carry a carrier-range atom, so nothing can \
             retire mid-retrieval — the need-gate keeps hunting"
        );

        // Telemetry: a needed identity left un-attempted because its roles
        // were all spent is attributed to the SLOT bound, not to the
        // resolution budget — the two are different diagnoses at the gate.
        resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[
                node_candidate("styles/t00.css", "100", "t00"),
                node_candidate("styles/t01.css", "101", "t01"),
                node_candidate("styles/t02.css", "102", "t02"),
                node_candidate("styles/t03.css", "103", "t03"),
                node_candidate("styles/t04.css", "104", "t04"),
            ],
            4,
        )
        .expect("slot-exhaustion query");
        let unattempted = session.r6_trace_json()["query_admissions"][3]["unattempted"].clone();
        assert_eq!(
            unattempted
                .as_array()
                .expect("unattempted")
                .iter()
                .filter(|entry| entry["why_not"].as_str() == Some("slot_exhausted"))
                .count(),
            1,
            "the identity whose every role was spent is attributed to the slot \
             bound: {unattempted:?}"
        );
    }

    /// Round 5.5 item 2a — C shard pace: four slots per query leave the gate
    /// 5c measurement (12 promotions across 9 queries) intact, and no query
    /// ever exceeds its slot count.
    #[test]
    fn r6_c_shard_role_slots_preserve_the_gate_pace_across_nine_queries() {
        let storage = css_entrypoint_closure_storage(20);
        let session = traced_file_structural_session();
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
        resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[file_shaped_candidate("styles/entry.css")],
            1,
        )
        .expect("bootstrap query");

        let mut promotions = 0usize;
        for query in 0..9 {
            let first = 100 + query * 2;
            let candidates = vec![
                node_candidate("src/other.rs", "5", "unrelated_filler"),
                node_candidate(
                    &format!("styles/t{:02}.css", query * 2),
                    &first.to_string(),
                    "target",
                ),
                node_candidate(
                    &format!("styles/t{:02}.css", query * 2 + 1),
                    &(first + 1).to_string(),
                    "target",
                ),
            ];
            resolve_sidecar_candidates_in_storage(
                &storage,
                &HashMap::new(),
                Path::new("."),
                &candidates,
                2,
            )
            .expect("pace query");
            let spent = promotion_roles_used(&session, query + 1);
            assert!(
                spent.len() <= 4,
                "no query may exceed its four atom-derived slots: {spent:?}"
            );
            promotions += spent.len();
        }
        assert!(
            promotions >= 12,
            "the gate 5c pace (12 promotions across 9 queries) must survive the \
             slot bound; observed {promotions}"
        );
        // Gate 6 guard: multiplicity introduces NO import-order or
        // file-position preference. Every pure import target occupies the
        // same role positions, so their scores are equal and base order
        // alone separates them — exactly as before.
        let priorities = (100..120)
            .map(|identity| session.promotion_priority(identity))
            .collect::<HashSet<_>>();
        assert_eq!(
            priorities.len(),
            1,
            "import targets must be indistinguishable by score: {priorities:?}"
        );
    }

    /// Round 5.5 item 2a — A shard: TWO slots per query (Builder and
    /// ConfigType, the endpoints of A1's TYPE_USAGE pattern). The
    /// TypeMap-shaped identity still promotes while its slot is free, a
    /// second config-type identity finds none, and the next query re-opens
    /// both.
    #[test]
    fn r6_a_shard_promotions_are_capped_at_the_two_atom_derived_role_slots() {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("src/builder.cs"),
                language: "csharp".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 90,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        let mut nodes = vec![codestory_contracts::graph::Node {
            id: CoreNodeId(1),
            kind: NodeKind::FILE,
            serialized_name: "src/builder.cs".into(),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(1),
            ..Default::default()
        }];
        for (id, name) in [
            (10, "TypeMapPlanBuilder"),
            (11, "MapperConfiguration"),
            (30, "TypeMap"),
            (31, "TypeMapPlan"),
        ] {
            nodes.push(codestory_contracts::graph::Node {
                id: CoreNodeId(id),
                kind: NodeKind::CLASS,
                serialized_name: name.into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(id as u32),
                ..Default::default()
            });
        }
        for id in [5, 6] {
            nodes.push(codestory_contracts::graph::Node {
                id: CoreNodeId(id),
                kind: NodeKind::FUNCTION,
                serialized_name: format!("unrelated_filler_{id}"),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(60 + id as u32),
                ..Default::default()
            });
        }
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        storage
            .insert_edges_batch(
                &[(400, 10, 30), (401, 10, 31), (403, 11, 10)]
                    .into_iter()
                    .map(|(id, source, target)| codestory_contracts::graph::Edge {
                        id: codestory_contracts::graph::EdgeId(id),
                        source: CoreNodeId(source),
                        target: CoreNodeId(target),
                        kind: EdgeKind::TYPE_USAGE,
                        certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
                        file_node_id: Some(CoreNodeId(1)),
                        line: Some(7),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>(),
            )
            .expect("insert edges");

        let requirements =
            Vec::new();
        let session = Rc::new(
            crate::agent::packet_candidate::PacketProofSession::new(
                crate::agent::packet_candidate::packet_atom_hydration_spec(&requirements),
            )
            .with_trace_enabled(),
        );
        assert_eq!(
            session.hydration.promotion_role_slots().len(),
            2,
            "the A formulas derive exactly two promotion slots"
        );
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));

        let candidates = vec![
            node_candidate("src/builder.cs", "10", "TypeMapPlanBuilder"),
            node_candidate("src/builder.cs", "5", "unrelated_filler_5"),
            node_candidate("src/builder.cs", "6", "unrelated_filler_6"),
            node_candidate("src/builder.cs", "30", "TypeMap"),
            node_candidate("src/builder.cs", "11", "MapperConfiguration"),
            node_candidate("src/builder.cs", "31", "TypeMapPlan"),
        ];
        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &candidates,
            4,
        )
        .expect("A-shard slot-bounded query");
        assert_eq!(
            promotion_roles_used(&session, 0),
            vec!["ConfigType", "Builder"],
            "each of the two atom-derived roles is spent exactly once"
        );
        assert_eq!(
            outcome
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["10", "30", "11", "5"],
            "the TypeMap-shaped identity promotes while its slot is free; the \
             second config-type identity waits and base order resumes"
        );
        assert!(
            session.identity_is_atom_needed(31),
            "the unpromoted config type stays needed — it lacked a free slot"
        );

        let next = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[
                node_candidate("src/builder.cs", "6", "unrelated_filler_6"),
                node_candidate("src/builder.cs", "31", "TypeMapPlan"),
            ],
            1,
        )
        .expect("A-shard next query");
        assert_eq!(
            next.resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["31"],
            "a fresh query re-opens the ConfigType slot"
        );
        assert!(
            session.retired_requirements().is_empty(),
            "mapper_config also requires a carrier range, which cannot discharge \
             mid-retrieval — the need-gate keeps hunting"
        );
    }

    /// An A-shaped store whose bootstrap class is incident to both
    /// directions of the TYPE_USAGE relation, so one hydration establishes a
    /// MULTI-POSITION identity (source and target of the config atom) beside
    /// lone-target identities.
    fn mapper_multiplicity_storage() -> Store {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("src/mapper.cs"),
                language: "csharp".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 200,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        let mut nodes = vec![
            codestory_contracts::graph::Node {
                id: CoreNodeId(1),
                kind: NodeKind::FILE,
                serialized_name: "src/mapper.cs".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(1),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(5),
                kind: NodeKind::FUNCTION,
                serialized_name: "unrelated_filler".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(150),
                ..Default::default()
            },
        ];
        for (id, name) in [
            (20, "MapperConfiguration"),
            (40, "ResolutionContext"),
            (41, "Conventions"),
            (50, "TypeMapPlanBuilder"),
        ] {
            nodes.push(codestory_contracts::graph::Node {
                id: CoreNodeId(id),
                kind: NodeKind::CLASS,
                serialized_name: name.into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(id as u32),
                ..Default::default()
            });
        }
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        storage
            .insert_edges_batch(
                &[
                    // Lone configuration targets: one role position each.
                    (400, 20, 40),
                    (401, 20, 41),
                    // The chain identity: target of one config edge AND
                    // source of another, i.e. two role positions.
                    (402, 20, 50),
                    (403, 50, 20),
                ]
                .into_iter()
                .map(|(id, source, target)| codestory_contracts::graph::Edge {
                    id: codestory_contracts::graph::EdgeId(id),
                    source: CoreNodeId(source),
                    target: CoreNodeId(target),
                    kind: EdgeKind::TYPE_USAGE,
                    certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
                    file_node_id: Some(CoreNodeId(1)),
                    line: Some(7),
                    ..Default::default()
                })
                .collect::<Vec<_>>(),
            )
            .expect("insert edges");
        storage
    }

    fn mapper_session() -> Rc<crate::agent::packet_candidate::PacketProofSession> {
        let requirements =
            Vec::new();
        Rc::new(crate::agent::packet_candidate::PacketProofSession::new(
            crate::agent::packet_candidate::packet_atom_hydration_spec(&requirements),
        ))
    }

    /// Gate 6 — the slot goes to ATOM-ROLE MULTIPLICITY, not base order.
    /// With hundreds of equally-needed identities the earliest-match rule
    /// spent its slots on whatever surfaced first; the identity that stands
    /// in two role positions of the requirement group — the one that can
    /// complete a group-consistent proof — now takes the slot even though it
    /// sits LATER in base order.
    #[test]
    fn r6_promotion_priority_prefers_multi_role_identities_over_base_order() {
        let storage = mapper_multiplicity_storage();
        let session = mapper_session();
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
        let candidates = vec![
            node_candidate("src/mapper.cs", "20", "MapperConfiguration"),
            node_candidate("src/mapper.cs", "5", "unrelated_filler"),
            // Lone configuration target, EARLIER in base order.
            node_candidate("src/mapper.cs", "40", "ResolutionContext"),
            // Two role positions, LATER in base order.
            node_candidate("src/mapper.cs", "50", "TypeMapPlanBuilder"),
        ];
        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &candidates,
            2,
        )
        .expect("priority-ordered resolve");

        assert_eq!(
            session.promotion_priority(50),
            2,
            "the chain identity stands in both role positions"
        );
        assert_eq!(
            session.promotion_priority(40),
            1,
            "the lone target stands in one"
        );
        assert_eq!(
            outcome
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["20", "50"],
            "the slot goes to the higher-multiplicity identity; under the old \
             earliest-match rule it would have gone to node 40"
        );
        assert_eq!(outcome.attempted_candidate_indices, HashSet::from([0, 3]));
    }

    /// Gate 6 — the tie-break chain below multiplicity: equal scores fall
    /// back to BASE ORDER, then to stable identity, and the whole decision
    /// is deterministic across runs.
    #[test]
    fn r6_equal_priority_falls_back_to_base_order_deterministically() {
        let storage = mapper_multiplicity_storage();
        let run = || {
            let session = mapper_session();
            let _guard =
                crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
            let candidates = vec![
                node_candidate("src/mapper.cs", "20", "MapperConfiguration"),
                node_candidate("src/mapper.cs", "5", "unrelated_filler"),
                // Two lone targets, identical scores: base order decides.
                node_candidate("src/mapper.cs", "41", "Conventions"),
                node_candidate("src/mapper.cs", "40", "ResolutionContext"),
            ];
            let outcome = resolve_sidecar_candidates_in_storage(
                &storage,
                &HashMap::new(),
                Path::new("."),
                &candidates,
                2,
            )
            .expect("tie-break resolve");
            assert_eq!(
                session.promotion_priority(40),
                session.promotion_priority(41)
            );
            (
                outcome
                    .resolved_hits
                    .iter()
                    .map(|hit| hit.node_id.0.clone())
                    .collect::<Vec<_>>(),
                outcome.attempted_candidate_indices,
            )
        };
        let (first_hits, first_attempted) = run();
        assert_eq!(
            first_hits,
            ["20", "41"],
            "equal multiplicity keeps the earlier base-order candidate"
        );
        let (second_hits, second_attempted) = run();
        assert_eq!(first_hits, second_hits, "the decision is deterministic");
        assert_eq!(first_attempted, second_attempted);
    }

    /// Round 5.5 item 2b — the query-boundary group checkpoint. A formula
    /// whose requirement is satisfiable by TYPED atoms alone is proven by the
    /// public group matcher over the receipts the first query accumulated, so
    /// its promotion patterns RETIRE and the second query admits in pure base
    /// order. The identical run under the real C spec — whose requirements
    /// also carry carrier-range atoms that cannot discharge mid-retrieval —
    /// keeps promoting, which is the fail-closed half of the property:
    /// retirement is exactly as strict as the proof layer.
    #[test]
    fn r6_group_checkpointed_retirement_stops_promotion_at_the_next_query_boundary() {
        use codestory_agent::packet_proof_atoms::{
            FlowProofFormula, ProofAtomId, ProofAtomSpec, ProofEndpointPattern, ProofFactPattern,
            ProofRole, TypedRelationPattern,
        };

        // A typed-only probe formula: one IMPORT fact, no source-aspect or
        // absence atom, so accumulated typed receipts alone can prove it.
        static RETIREMENT_PROBE_FORMULA: FlowProofFormula = FlowProofFormula {
            atoms: &[ProofAtomSpec {
                id: ProofAtomId::C2,
                requirement: "retirement_probe",
                facts: &[ProofFactPattern::TypedRelation(TypedRelationPattern {
                    kind: codestory_contracts::api::EdgeKind::IMPORT,
                    source: ProofEndpointPattern::Role(ProofRole::Entrypoint),
                    target: ProofEndpointPattern::Role(ProofRole::VarsSource),
                    target_kind: Some(ApiNodeKind::FILE),
                    markers: &[],
                    target_distinct_from_source: false,
                })],
            }],
            distinct_roles: &[],
        };
        let ProofFactPattern::TypedRelation(probe_pattern) =
            &RETIREMENT_PROBE_FORMULA.atoms[0].facts[0]
        else {
            panic!("the probe formula carries one typed-relation fact");
        };
        let probe_promotion_pattern = crate::agent::packet_candidate::PacketPromotionPattern {
            requirement: "retirement_probe",
            pattern: probe_pattern,
            source_roles: vec![ProofRole::Entrypoint],
            target_roles: vec![ProofRole::VarsSource],
        };

        let storage = css_bootstrap_storage();
        let query_one = vec![file_shaped_candidate("styles/_base.css")];
        let query_two = vec![
            node_candidate("src/other.rs", "5", "unrelated_filler"),
            file_shaped_candidate("styles/animate.css"),
        ];
        let run = |session: Rc<crate::agent::packet_candidate::PacketProofSession>| {
            let _guard =
                crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
            resolve_sidecar_candidates_in_storage(
                &storage,
                &HashMap::new(),
                Path::new("."),
                &query_one,
                1,
            )
            .expect("query one");
            let second = resolve_sidecar_candidates_in_storage(
                &storage,
                &HashMap::new(),
                Path::new("."),
                &query_two,
                1,
            )
            .expect("query two");
            (
                session.retired_requirements(),
                second
                    .resolved_hits
                    .iter()
                    .map(|hit| hit.node_id.0.clone())
                    .collect::<Vec<_>>(),
            )
        };

        let probe_session = Rc::new(crate::agent::packet_candidate::PacketProofSession::new(
            crate::agent::packet_candidate::PacketAtomHydrationSpec {
                rooted: Vec::new(),
                file_structural: true,
                absence_kinds: Vec::new(),
                promotion_patterns: vec![probe_promotion_pattern.clone()],
                role_scoring_patterns: vec![probe_promotion_pattern],
                formulas: vec![crate::agent::packet_candidate::PacketProofFormulaRef(
                    &RETIREMENT_PROBE_FORMULA,
                )],
            },
        ));
        let (retired, probe_hits) = run(Rc::clone(&probe_session));
        assert_eq!(
            retired,
            vec!["retirement_probe"],
            "the group matcher proves the typed-only requirement at the query boundary"
        );
        assert!(
            !probe_session.promotion_is_active(),
            "a fully retired pattern set silences the need-gate"
        );
        assert_eq!(
            probe_hits,
            ["5"],
            "after retirement the second query admits in pure base order"
        );
        // Monotone and deterministic: re-running the checkpoint with the same
        // receipts changes nothing.
        probe_session.checkpoint_group_retirement();
        assert_eq!(
            probe_session.retired_requirements(),
            vec!["retirement_probe"]
        );

        let (c_retired, c_hits) = run(file_structural_session());
        assert!(
            c_retired.is_empty(),
            "the shipped C requirements cannot retire mid-retrieval: their \
             carrier-range and anchored atoms fail closed without anchors"
        );
        assert_eq!(
            c_hits,
            ["2"],
            "with nothing retired the need-gate still promotes the entrypoint"
        );
    }

    /// `file_count` stylesheets, each owning one selector — enough FILE and
    /// structural roots to make the post-pass cost budget bind.
    fn post_pass_budget_storage(file_count: i64) -> Store {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for index in 1..=file_count {
            let path = format!("styles/f{index:02}.css");
            storage
                .insert_file(&FileInfo {
                    id: index,
                    path: PathBuf::from(&path),
                    language: "css".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 20,
                    file_role: FileRole::Source,
                })
                .expect("insert file");
            nodes.push(codestory_contracts::graph::Node {
                id: CoreNodeId(index),
                kind: NodeKind::FILE,
                serialized_name: path,
                file_node_id: Some(CoreNodeId(index)),
                start_line: Some(1),
                ..Default::default()
            });
            nodes.push(codestory_contracts::graph::Node {
                id: CoreNodeId(1_000 + index),
                kind: NodeKind::CONSTANT,
                serialized_name: format!(".sel{index:02}"),
                file_node_id: Some(CoreNodeId(index)),
                start_line: Some(3),
                ..Default::default()
            });
            edges.push(codestory_contracts::graph::Edge {
                id: codestory_contracts::graph::EdgeId(5_000 + index),
                source: CoreNodeId(index),
                target: CoreNodeId(1_000 + index),
                kind: EdgeKind::MEMBER,
                file_node_id: Some(CoreNodeId(index)),
                ..Default::default()
            });
        }
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        storage.insert_edges_batch(&edges).expect("insert edges");
        storage
    }

    fn answer_citing_nodes(node_ids: &[i64]) -> AgentAnswerDto {
        let mut answer = sidecar_answer_with_citation_node("0");
        answer.citations.clear();
        for node_id in node_ids {
            let citation = sidecar_answer_with_citation_node(&node_id.to_string())
                .citations
                .remove(0);
            answer.citations.push(citation);
        }
        answer
    }

    fn post_pass_artifact_ids(answer: &AgentAnswerDto) -> Vec<String> {
        answer
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                GraphArtifactDto::Uml { id, .. }
                    if id.starts_with(PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX) =>
                {
                    Some(id.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// A C-family session with two identities already in the promotion
    /// need-set — the shape R6 works to rescue.
    fn session_needing(
        source: &str,
        target: &str,
    ) -> Rc<crate::agent::packet_candidate::PacketProofSession> {
        let session = file_structural_session();
        let node = |id: &str| codestory_contracts::api::GraphNodeDto {
            id: NodeId(id.into()),
            label: id.into(),
            kind: ApiNodeKind::FILE,
            depth: 1,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: None,
            qualified_name: None,
            member_access: None,
        };
        session.record_atom_needed_identities(&GraphResponse {
            center_id: NodeId(source.into()),
            nodes: vec![node(source), node(target)],
            edges: vec![codestory_contracts::api::GraphEdgeDto {
                id: codestory_contracts::api::EdgeId("import-1".into()),
                source: NodeId(source.into()),
                target: NodeId(target.into()),
                kind: codestory_contracts::api::EdgeKind::IMPORT,
                certainty: None,
                confidence: None,
                callsite_identity: None,
                candidate_targets: Vec::new(),
            }],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        });
        session
    }

    /// Gate 8 — the post-pass is NEED-ORDERED, not rank-ordered. R6 promotion
    /// changes which candidates are admitted, never their rank, so rescued
    /// roots land at the TAIL of citation order; under the old rank-ordered
    /// walk with a hard budget `break` they were systematically the roots the
    /// traversal never reached, and their receipts never entered the support.
    /// Here the two atom-needed roots sit LAST among 18 citations while the
    /// budget only affords 16 — and they are hydrated while priority-0 roots
    /// ahead of them are the ones dropped.
    #[test]
    fn post_pass_hydrates_atom_needed_roots_before_rank_order() {
        let storage = post_pass_budget_storage(18);
        let session = session_needing("18", "17");
        assert!(session.promotion_priority(18) > 0 && session.promotion_priority(17) > 0);
        assert_eq!(session.promotion_priority(1), 0);

        // 18 FILE roots at 12 units each = 216 against a 192-unit budget.
        let citations = (1..=18).collect::<Vec<_>>();
        let mut answer = answer_citing_nodes(&citations);
        hydrate_packet_atom_trails_in_storage(&storage, &HashMap::new(), &session, &mut answer);
        let artifacts = post_pass_artifact_ids(&answer);
        assert_eq!(
            artifacts.len(),
            16,
            "the cost budget still affords exactly 16 FILE roots: {artifacts:?}"
        );
        for needed in [17, 18] {
            assert!(
                artifacts.contains(&format!("{PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX}{needed}")),
                "the atom-needed root at the tail of citation order must be hydrated: {needed}"
            );
        }
        for dropped in [15, 16] {
            assert!(
                !artifacts.contains(&format!("{PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX}{dropped}")),
                "a priority-0 root is what the budget drops now: {dropped}"
            );
        }
        assert_eq!(
            artifacts[0],
            format!("{PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX}17"),
            "need-ordered roots come first, and citation order breaks their tie"
        );
        assert_eq!(
            artifacts[1],
            format!("{PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX}18")
        );
    }

    /// Gate 8 — determinism of the need-ordered traversal: identical inputs
    /// produce an identical artifact set in an identical order.
    #[test]
    fn post_pass_need_ordering_is_deterministic() {
        let storage = post_pass_budget_storage(18);
        let citations = (1..=18).collect::<Vec<_>>();
        let run = || {
            let session = session_needing("18", "17");
            let mut answer = answer_citing_nodes(&citations);
            hydrate_packet_atom_trails_in_storage(&storage, &HashMap::new(), &session, &mut answer);
            post_pass_artifact_ids(&answer)
        };
        let first = run();
        let second = run();
        assert_eq!(first, second, "the traversal order must be reproducible");
        assert_eq!(first.len(), 16);
    }

    /// Gate 8 — SKIP, never BREAK. A root whose cost does not fit is passed
    /// over and cheaper roots behind it are still hydrated; the total budget
    /// is unchanged, so this only stops one expensive root from starving
    /// everything behind it.
    #[test]
    fn post_pass_skips_an_unaffordable_root_and_keeps_hydrating_cheaper_ones() {
        let storage = post_pass_budget_storage(17);
        let session = file_structural_session();

        // 15 FILE roots (12 each = 180) + one structural root (4) = 184 of
        // 192. The next FILE root costs 12 and cannot fit; the structural
        // root behind it costs 4 and still can.
        let mut citations = (1..=15).collect::<Vec<_>>();
        citations.push(1_001); // CONSTANT root, 4 units
        citations.push(16); // FILE root, 12 units — must be SKIPPED
        citations.push(1_002); // CONSTANT root, 4 units — must still hydrate
        let mut answer = answer_citing_nodes(&citations);
        hydrate_packet_atom_trails_in_storage(&storage, &HashMap::new(), &session, &mut answer);
        let artifacts = post_pass_artifact_ids(&answer);

        assert!(
            !artifacts.contains(&format!("{PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX}16")),
            "the unaffordable FILE root is skipped: {artifacts:?}"
        );
        assert!(
            artifacts.contains(&format!("{PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX}1002")),
            "a cheaper root behind it must still be hydrated — the old hard \
             break would have ended the traversal here: {artifacts:?}"
        );
        assert_eq!(
            artifacts.len(),
            17,
            "15 file roots plus both structural roots: {artifacts:?}"
        );
    }

    /// Round 5.5 item 1 residual (option ii): the POST-PASS depth-2 FILE
    /// structural trail survives entrypoint-scale fanout. Under the old
    /// 65-node cap the store accessor's edge budget (`max_nodes × 3` = 195)
    /// is exhausted at the root, the traversal breaks with only the root in
    /// the node set, and the closing endpoint filter drops EVERY edge — the
    /// artifact comes back empty and C1's MODULE-member receipts die with it.
    /// The raised cap keeps one traversal set carrying
    /// `[MEMBER, USAGE, IMPORT]` at depth 2, which is what rule 7's
    /// deeper-rooted arm requires.
    #[test]
    fn post_pass_structural_trail_survives_entrypoint_scale_fanout() {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("styles/entry.css"),
                language: "css".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 400,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        let mut nodes = vec![codestory_contracts::graph::Node {
            id: CoreNodeId(1),
            kind: NodeKind::FILE,
            serialized_name: "styles/entry.css".into(),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(1),
            ..Default::default()
        }];
        // 99 MODULE import-statement members + 99 imported files = the 198
        // outgoing structural edges a real entrypoint carries.
        nodes.extend((0..99).map(|index| codestory_contracts::graph::Node {
            id: CoreNodeId(1_000 + index),
            kind: NodeKind::MODULE,
            serialized_name: format!("@import {index:02}"),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(1 + index as u32),
            ..Default::default()
        }));
        nodes.extend((0..99).map(|index| codestory_contracts::graph::Node {
            id: CoreNodeId(2_000 + index),
            kind: NodeKind::FILE,
            serialized_name: format!("styles/imported_{index:02}.css"),
            file_node_id: Some(CoreNodeId(2_000 + index)),
            start_line: Some(1),
            ..Default::default()
        }));
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        // Interleaved ids so any retained prefix carries both kinds.
        let mut edges = Vec::new();
        for index in 0..99 {
            edges.push(codestory_contracts::graph::Edge {
                id: codestory_contracts::graph::EdgeId(10_000 + index * 2),
                source: CoreNodeId(1),
                target: CoreNodeId(1_000 + index),
                kind: EdgeKind::MEMBER,
                file_node_id: Some(CoreNodeId(1)),
                ..Default::default()
            });
            edges.push(codestory_contracts::graph::Edge {
                id: codestory_contracts::graph::EdgeId(10_001 + index * 2),
                source: CoreNodeId(1),
                target: CoreNodeId(2_000 + index),
                kind: EdgeKind::IMPORT,
                file_node_id: Some(CoreNodeId(1)),
                ..Default::default()
            });
        }
        storage.insert_edges_batch(&edges).expect("insert edges");

        // The pathology this fix routes around, pinned at the store boundary:
        // at the old cap the same trail enumerates 198 edges and returns NONE.
        let filter = crate::agent::packet_candidate::PACKET_FILE_STRUCTURAL_TRAIL_KINDS
            .iter()
            .map(|kind| EdgeKind::from(*kind))
            .collect::<Vec<_>>();
        let starved = storage
            .get_trail(&TrailConfig {
                root_id: CoreNodeId(1),
                depth: PACKET_FILE_STRUCTURAL_TRAIL_DEPTH,
                direction: TrailDirection::Outgoing,
                caller_scope: TrailCallerScope::IncludeTestsAndBenches,
                edge_filter: filter.clone(),
                show_utility_calls: true,
                max_nodes: PACKET_CANDIDATE_DIRECTION_NODE_LIMIT,
                ..TrailConfig::default()
            })
            .expect("starved trail");
        assert!(
            starved.edges.is_empty(),
            "the 65-node cap's edge budget starves this root — the artifact \
             would be empty and skipped"
        );

        let session = file_structural_session();
        let mut answer = sidecar_answer_with_citation_node("1");
        hydrate_packet_atom_trails_in_storage(&storage, &HashMap::new(), &session, &mut answer);
        let post_pass = answer
            .graphs
            .iter()
            .find_map(|artifact| match artifact {
                GraphArtifactDto::Uml { id, graph, .. }
                    if id.starts_with(PACKET_ATOM_HYDRATION_ARTIFACT_PREFIX) =>
                {
                    Some(graph)
                }
                _ => None,
            })
            .expect("post-pass hydration artifact");
        assert!(
            !post_pass.edges.is_empty(),
            "the raised structural cap must keep the entrypoint's trail alive"
        );
        for kind in [
            codestory_contracts::api::EdgeKind::MEMBER,
            codestory_contracts::api::EdgeKind::IMPORT,
        ] {
            assert!(
                post_pass.edges.iter().any(|edge| edge.kind == kind),
                "the single traversal set must carry {kind:?} edges"
            );
        }
        let scans = session.artifact_scans();
        let (_, recorded) = scans.first().expect("ledger entry for the entrypoint root");
        assert!(
            recorded.iter().any(|scan| {
                scan.root == "1"
                    && scan.depth == PACKET_FILE_STRUCTURAL_TRAIL_DEPTH
                    && scan.edge_kinds
                        == crate::agent::packet_candidate::PACKET_FILE_STRUCTURAL_TRAIL_KINDS
                            .to_vec()
            }),
            "rule 7 needs ONE depth-2 [MEMBER, USAGE, IMPORT] traversal set: {recorded:?}"
        );
    }

    /// Gate 5c, item 1: an outgoing IMPORT identity trail with more targets
    /// than the 65-node trail cap truncates — and its RETAINED edges still
    /// contribute their identities to the need-set (truncation bars absence
    /// claims, never positive identity receipts), so a bounce-shaped
    /// beyond-window candidate whose file sits early in the import closure
    /// promotes.
    #[test]
    fn r6_truncated_import_trail_still_contributes_retained_identities() {
        use codestory_store::{FileInfo, FileRole};

        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("source/animate.css"),
                language: "css".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 120,
                file_role: FileRole::Source,
            })
            .expect("insert entrypoint file");
        storage
            .insert_file(&FileInfo {
                id: 4,
                path: PathBuf::from("src/other.rs"),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 10,
                file_role: FileRole::Source,
            })
            .expect("insert filler file");
        let mut nodes = vec![
            codestory_contracts::graph::Node {
                id: CoreNodeId(1),
                kind: NodeKind::FILE,
                serialized_name: "source/animate.css".into(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(1),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(4),
                kind: NodeKind::FILE,
                serialized_name: "src/other.rs".into(),
                file_node_id: Some(CoreNodeId(4)),
                start_line: Some(1),
                ..Default::default()
            },
            codestory_contracts::graph::Node {
                id: CoreNodeId(5),
                kind: NodeKind::FUNCTION,
                serialized_name: "unrelated_filler".into(),
                file_node_id: Some(CoreNodeId(4)),
                start_line: Some(2),
                ..Default::default()
            },
        ];
        // 99 import-target FILE nodes — well beyond the 65-node trail cap.
        nodes.extend((0..99).map(|index| codestory_contracts::graph::Node {
            id: CoreNodeId(1_000 + index),
            kind: NodeKind::FILE,
            serialized_name: format!("source/group/target_{index:02}.css"),
            file_node_id: Some(CoreNodeId(1_000 + index)),
            start_line: Some(1),
            ..Default::default()
        }));
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        let edges = (0..99)
            .map(|index| codestory_contracts::graph::Edge {
                id: codestory_contracts::graph::EdgeId(2_000 + index),
                source: CoreNodeId(1),
                target: CoreNodeId(1_000 + index),
                kind: EdgeKind::IMPORT,
                file_node_id: Some(CoreNodeId(1)),
                line: Some(2 + index as u32),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        storage.insert_edges_batch(&edges).expect("insert edges");

        let session = file_structural_session();
        let _guard =
            crate::agent::packet_candidate::install_packet_proof_session(Rc::clone(&session));
        // The bounce-shaped candidate: the 3rd import target — early in the
        // closure, comfortably inside the trail's retained prefix, but
        // beyond the resolution window without promotion.
        let candidates = vec![
            file_shaped_candidate("source/animate.css"),
            node_candidate("src/other.rs", "5", "unrelated_filler"),
            node_candidate("source/group/target_02.css", "1002", "bounce_shaped"),
        ];
        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &candidates,
            2,
        )
        .expect("resolve over-cap entrypoint");
        assert_eq!(
            outcome
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["1", "1002"],
            "the retained import target must promote over the filler"
        );

        let entry_hit = outcome
            .packet_hits
            .iter()
            .find(|hit| hit.hit.node_id.0 == "1")
            .expect("entrypoint packet hit");
        let outgoing = entry_hit
            .trail_scans
            .iter()
            .find(|scan| scan.direction == PacketGraphDirection::Outgoing)
            .expect("outgoing IMPORT identity scan");
        assert!(
            outgoing.truncated,
            "99 targets overflow the 65-node cap: {outgoing:?}"
        );
        let retained_imports = entry_hit
            .graph
            .as_ref()
            .expect("entrypoint graph")
            .edges
            .iter()
            .filter(|edge| edge.kind == codestory_contracts::api::EdgeKind::IMPORT)
            .count();
        assert!(
            retained_imports >= 60,
            "the truncated trail must still retain its edge prefix: {retained_imports}"
        );
        assert!(
            session.identity_is_atom_needed(1_002),
            "retained-edge identities contribute despite truncation"
        );
        assert!(
            !session.identity_is_atom_needed(1_098),
            "identities beyond the retained prefix are not established (fail closed)"
        );
    }

    #[test]
    fn empty_sidecar_primary_does_not_admit_nucleo_as_product_evidence() {
        assert!(
            sidecar_primary_blocks_nucleo_supplement(true, 0),
            "zero sidecar hits must not open the Nucleo supplement"
        );
        assert!(
            sidecar_primary_blocks_nucleo_supplement(true, 4),
            "sidecar-primary packets never mix in-process Nucleo"
        );
        assert!(
            !sidecar_primary_blocks_nucleo_supplement(false, 0),
            "Nucleo remains available when sidecar-primary is off"
        );
    }
}
