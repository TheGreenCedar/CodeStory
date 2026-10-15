//! Mandatory sidecar retrieval integration for packet and agent ask paths.

use super::packet_candidate::PacketRetrievalQuery;
use codestory_retrieval::LexicalAdmissionPolicy;

use crate::agent::nucleo_policy::with_sidecar_primary_retrieval;
use crate::agent::packet_candidate::{
    PacketAdmissionDecision, PacketGraphDirection, PacketGraphEdgeProvenance, PacketSearchHit,
    active_packet_proof_session,
};
use crate::agent::packet_degradation::semantic_stage_degradation;
use crate::agent::packet_evidence::decorate_search_hit_evidence;
use crate::{
    AppController, HybridSearchScoredHit, app_graph_flags, graph_edge_dto, member_access_dto,
    node_display_name,
};
use anyhow::Error as AnyhowError;
use codestory_contracts::api::NodeKind as ApiNodeKind;
use codestory_contracts::api::{
    AgentAnswerDto, AgentPacketDto, ApiError, ApiErrorDetails,
    EmbeddingVectorPublicationIdentityDto, GraphNodeDto, GraphResponse, PacketQueryCompletionDto,
    PacketSidecarQueryDiagnosticDto, RetrievalCandidateResolutionCountDto,
    RetrievalCandidateSummaryDto, RetrievalScoreBreakdownDto, RetrievalShadowDto,
    RetrievalStageTimingDto, SearchHit, SearchHitOrigin, SearchResultsDto,
};
use codestory_contracts::compilation::PacketAdmissionGapKindV1;
use codestory_contracts::graph::{
    EdgeKind, NodeId as CoreNodeId, NodeKind, TrailCallerScope, TrailConfig, TrailDirection,
};
#[cfg(test)]
use codestory_retrieval::SidecarRuntimeConfig;
use codestory_retrieval::{
    CandidateGraphDirection, CandidateHit, CandidateSource, PinnedQuerySession, QueryBatchItem,
    QueryRequest, QueryResult, QueryTrace, SidecarProfile, deferred_full_readiness_stop_message,
    execute_retrieval_query_with_cache_for_runtime, is_phantom_sidecar_hit,
    is_retrieval_publication_changed, retrieval_stop_message, sidecar_project_id_for_root,
    strict_descriptor_sidecar_status_for_runtime, strict_sidecar_status_for_runtime,
};
use codestory_store::Store;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    storage_path: PathBuf,
    node_names: RefCell<Option<Arc<HashMap<CoreNodeId, String>>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinnedRetrievalScope {
    Full,
    PacketDescriptor,
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

pub(super) fn read_admitted_published_file(
    controller: &AppController,
    receipt: &codestory_contracts::compilation::PacketAdmissionReceiptV1,
) -> Result<Option<codestory_retrieval::PublishedFileSourceWitness>, ApiError> {
    let pinned = active_pinned_retrieval_read(controller)
        .ok_or_else(|| ApiError::internal("Published file source is missing its operation pin."))?;
    let session = active_packet_proof_session().ok_or_else(|| {
        ApiError::invalid_argument("Published file source lacks an active packet admission.")
    })?;
    if !session.authorizes_published_file(receipt, pinned.session.publication_identity()) {
        return Err(ApiError::invalid_argument(
            "Published file source lacks sealed lexical admission.",
        ));
    }
    let path = receipt
        .stable_identity
        .strip_prefix("path:")
        .ok_or_else(|| {
            ApiError::invalid_argument("Published file source requires a file identity.")
        })?;
    pinned
        .session
        .read_published_file_source(
            path,
            crate::services::active_public_operation_cancellation(),
        )
        .map_err(|error| {
            ApiError::internal(format!("Published file source unavailable: {error:#}"))
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

#[cfg(feature = "benchmark-support")]
impl RetrievalPublicationResponse for super::orchestrator::BenchmarkPacketExecution {
    fn attach_retrieval_publication(&mut self, publication: EmbeddingVectorPublicationIdentityDto) {
        self.packet.attach_retrieval_publication(publication);
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
        crate::load_canonical_search_symbols(&session.storage(), 10_000, None, |_| Ok(()))?.0,
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
        Self::begin_with_scope(controller, PinnedRetrievalScope::Full)
    }

    fn begin_packet_descriptor(controller: &AppController) -> Result<Self, ApiError> {
        Self::begin_with_scope(controller, PinnedRetrievalScope::PacketDescriptor)
    }

    fn begin_with_scope(
        controller: &AppController,
        scope: PinnedRetrievalScope,
    ) -> Result<Self, ApiError> {
        let mut span = super::packet_batch::observe_packet_operation_span(
            super::packet_batch::PacketOperationObservationSpan::PinBegin,
        );
        let result = (|| {
            let project_root = controller.require_project_root()?;
            let storage_path = controller.require_storage_path()?;
            let session = match scope {
                PinnedRetrievalScope::Full => PinnedQuerySession::begin(
                    &project_root,
                    &storage_path,
                    &controller.runtime_config,
                ),
                PinnedRetrievalScope::PacketDescriptor => {
                    PinnedQuerySession::begin_packet_descriptor(
                        &project_root,
                        &storage_path,
                        &controller.runtime_config,
                    )
                }
            }
            .map_err(map_pinned_query_error)?;
            Ok(Self {
                session,
                project_root,
                storage_path,
                node_names: RefCell::new(None),
            })
        })();
        if result.is_ok() {
            span.finish_success();
        }
        result
    }

    fn canonical_node_names(
        &self,
        controller: &AppController,
    ) -> Result<Arc<HashMap<CoreNodeId, String>>, ApiError> {
        if let Some(node_names) = self.node_names.borrow().as_ref() {
            return Ok(Arc::clone(node_names));
        }
        let node_names =
            canonical_symbol_names_for_session(controller, &self.storage_path, &self.session)?;
        self.node_names.replace(Some(Arc::clone(&node_names)));
        Ok(node_names)
    }

    fn ensure_query_identity(&self, query: &QueryResult, operation: &str) -> Result<(), ApiError> {
        self.session
            .ensure_result_identity(query, operation)
            .map_err(map_pinned_query_error)
    }

    fn revalidate(&self) -> Result<(), ApiError> {
        let mut span = super::packet_batch::observe_packet_operation_span(
            super::packet_batch::PacketOperationObservationSpan::PinRevalidation,
        );
        let result = self.session.revalidate().map_err(map_pinned_query_error);
        if result.is_ok() {
            span.finish_success();
        }
        result
    }

    fn revalidate_after_response(&self) -> Result<(), ApiError> {
        let mut span = super::packet_batch::observe_packet_operation_span(
            super::packet_batch::PacketOperationObservationSpan::PinRevalidation,
        );
        let result = self.session.revalidate_after_response().map_err(|error| {
            if let Some(fence) =
                error.downcast_ref::<codestory_retrieval::PublishedFileSourceFenceError>()
            {
                let code = match fence {
                    codestory_retrieval::PublishedFileSourceFenceError::Cancelled => "cancelled",
                    codestory_retrieval::PublishedFileSourceFenceError::Changed { .. } => {
                        "source_stale"
                    }
                };
                ApiError::new(code, error.to_string())
            } else {
                map_pinned_query_error(error)
            }
        });
        if result.is_ok() {
            span.finish_success();
        }
        result
    }
}

fn map_pinned_query_error(error: AnyhowError) -> ApiError {
    map_pinned_query_error_at(error, PinnedRetrievalStopBoundary::Unknown)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinnedRetrievalStopBoundary {
    Unknown,
    DeferredFullReadiness,
}

fn map_pinned_query_error_at(
    error: AnyhowError,
    boundary: PinnedRetrievalStopBoundary,
) -> ApiError {
    if let Some(error) = crate::services::embedding_api_error(&error) {
        error
    } else if is_retrieval_publication_changed(&error) {
        ApiError::new("publication_changed", error.to_string())
    } else if let Some(message) = match boundary {
        PinnedRetrievalStopBoundary::Unknown => retrieval_stop_message(&error),
        PinnedRetrievalStopBoundary::DeferredFullReadiness => {
            deferred_full_readiness_stop_message(&error)
        }
    } {
        ApiError::new("cache_busy", message)
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
    with_stable_retrieval_publication_inner(
        controller,
        operation,
        PinnedRetrievalScope::Full,
        build,
        |_| Ok(()),
    )
}

/// Pin the retrieval publication for a packet without opening the graph lane
/// before descriptor admission. Packet compilation performs the ordinary full
/// readiness check later, after the packet-wide admission session is sealed.
pub(crate) fn with_stable_packet_retrieval_publication<T: RetrievalPublicationResponse>(
    controller: &AppController,
    operation: &str,
    expected_core_generation_id: &str,
    expected_core_run_id: &str,
    mut build: impl FnMut() -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    if let Some(pinned) = active_pinned_retrieval_read(controller) {
        ensure_pinned_core_publication(&pinned, expected_core_generation_id, expected_core_run_id)?;
        let mut response = build()?;
        response.attach_retrieval_publication(publication_dto(&pinned));
        return Ok(response);
    }
    if !sidecar_descriptor_retrieval_enabled(controller) {
        return build();
    }
    with_stable_retrieval_publication_inner(
        controller,
        operation,
        PinnedRetrievalScope::PacketDescriptor,
        || {
            let pinned = active_pinned_retrieval_read(controller).ok_or_else(|| {
                ApiError::internal("packet descriptor retrieval ran without its publication pin")
            })?;
            ensure_pinned_core_publication(
                &pinned,
                expected_core_generation_id,
                expected_core_run_id,
            )?;
            build()
        },
        |_| Ok(()),
    )
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
    // This helper owns Retrieval-class public admission. An unavailable
    // generation cannot lend a pin to its response builder, even when Core is
    // fresh. Preserve the readiness reason instead of running an unpinned body.
    if let Some(reason) = sidecar_retrieval_unavailable_reason(controller) {
        return Err(sidecar_retrieval_unavailable_error(controller, reason));
    }

    let pinned = Rc::new(PinnedRetrievalRead::begin(controller)?);
    ensure_pinned_core_publication(&pinned, expected_core_generation_id, expected_core_run_id)?;
    let publication = publication_dto(&pinned);
    with_active_pinned_retrieval_read(controller, Rc::clone(&pinned), || {
        build().and_then(|value| {
            pinned.revalidate_after_response()?;
            Ok((value, Some(publication)))
        })
    })
}

fn with_stable_retrieval_publication_inner<T: RetrievalPublicationResponse>(
    controller: &AppController,
    operation: &str,
    scope: PinnedRetrievalScope,
    mut build: impl FnMut() -> Result<T, ApiError>,
    mut after_retry: impl FnMut(usize) -> Result<(), ApiError>,
) -> Result<T, ApiError> {
    for attempt in 0..RETRIEVAL_PUBLICATION_ATTEMPTS {
        let pinned = Rc::new(match scope {
            PinnedRetrievalScope::Full => PinnedRetrievalRead::begin(controller)?,
            PinnedRetrievalScope::PacketDescriptor => {
                PinnedRetrievalRead::begin_packet_descriptor(controller)?
            }
        });
        let publication = publication_dto(&pinned);
        let result = with_active_pinned_retrieval_read(controller, Rc::clone(&pinned), || {
            build().and_then(|mut response| {
                response.attach_retrieval_publication(publication.clone());
                pinned.revalidate_after_response()?;
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
    let eligible = || {
        if controller.runtime_config.profile == SidecarProfile::Agent
            && let Some(pinned) = active_pinned_retrieval_read(controller)
            && let Some(readiness) = pinned.session.validate_primary_readiness_reuse()
        {
            return readiness.is_ok();
        }
        sidecar_retrieval_eligible(controller) && sidecar_mode_is_required_full(controller)
    };
    match retrieval_env_override() {
        Some(false) => {
            tracing::error!("CODESTORY_RETRIEVAL=0 is unsupported; full retrieval is mandatory");
            false
        }
        Some(true) => eligible(),
        None => {
            // Default product path: serve only from full agent-scoped retrieval.
            let auto_on = eligible();
            if auto_on {
                tracing::info!(
                    "retrieval primary auto-on (unset CODESTORY_RETRIEVAL; agent retrieval full)"
                );
            }
            auto_on
        }
    }
}

fn sidecar_descriptor_retrieval_enabled(controller: &AppController) -> bool {
    if retrieval_env_override() == Some(false) {
        return false;
    }
    // The outer packet boundary admitted and pinned this exact publication
    // using descriptor-scoped readiness. Re-running project-wide status while
    // the pin is active cannot strengthen that identity; the descriptor batch
    // performs its own component health check against the pinned manifest.
    if active_pinned_retrieval_read(controller).is_some() {
        return true;
    }
    if !sidecar_retrieval_eligible(controller) {
        return false;
    }
    let Ok(project_root) = controller.require_project_root() else {
        return false;
    };
    let Ok(storage_path) = controller.require_storage_path() else {
        return false;
    };
    sidecar_status_can_serve_primary(&sidecar_descriptor_mode_status_for_runtime(
        &project_root,
        &storage_path,
        &controller.runtime_config,
    ))
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

pub(crate) fn sidecar_query_rejection_error(
    controller: &AppController,
    retrieval_mode: &str,
    cancel_reason: Option<&str>,
    reason: impl Into<String>,
) -> ApiError {
    if sidecar_mode_can_serve_primary(retrieval_mode)
        && let Some(stop @ ("deadline" | "stage_deadline" | "cancelled")) = cancel_reason
    {
        return sidecar_request_stop_error(controller, stop, reason);
    }
    sidecar_retrieval_unavailable_error(controller, reason)
}

pub(crate) fn sidecar_request_stop_error(
    controller: &AppController,
    stop: &str,
    reason: impl Into<String>,
) -> ApiError {
    let mut details = ApiErrorDetails::cause(stop);
    details.failed_layer = Some("retrieval_query".into());
    details.project = Some(
        controller
            .require_project_root()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|_| "<project>".into()),
    );
    let action = if stop == "cancelled" {
        "The request was cancelled; no index repair is required."
    } else {
        "Retry the request or narrow the question to a more specific target."
    };
    ApiError::with_details(
        "retrieval_unavailable",
        format!("{}; {action}", reason.into()),
        details,
    )
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
    if !codestory_store::core_database_exists(storage_path).unwrap_or(false) {
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

fn sidecar_descriptor_mode_status_for_runtime(
    project_root: &Path,
    storage_path: &Path,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> SidecarModeStatus {
    match strict_descriptor_sidecar_status_for_runtime(
        project_root,
        Some(storage_path),
        runtime.clone(),
    ) {
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

/// Run every generic packet query through the descriptor-only sidecar path,
/// rank the combined identities once, and seal packet admission before any
/// candidate source or graph hydration begins. The packet session retains
/// these exact query results under its retrieval policy and publication pin;
/// later resolution reuses them and may hydrate only identities admitted here.
#[cfg(test)]
pub(crate) fn preadmit_packet_descriptor_queries(
    controller: &AppController,
    queries: &[String],
    latency_budget_ms: Option<u32>,
) -> Result<(), ApiError> {
    let typed = queries
        .iter()
        .map(|query| PacketRetrievalQuery::strict(query))
        .collect::<Vec<_>>();
    preadmit_packet_descriptor_queries_with_policy(controller, &typed, latency_budget_ms)
}

pub(crate) fn preadmit_packet_descriptor_queries_with_policy(
    controller: &AppController,
    queries: &[PacketRetrievalQuery],
    latency_budget_ms: Option<u32>,
) -> Result<(), ApiError> {
    let session = active_packet_proof_session().ok_or_else(|| {
        ApiError::internal("packet descriptor admission requires an active packet session")
    })?;
    let readiness_budget_ms = latency_budget_ms
        .map(u64::from)
        .unwrap_or(DEFAULT_PACKET_BATCH_BUDGET_MS)
        .min(MAX_PACKET_BATCH_BUDGET_MS);
    let descriptor_budget_ms = sidecar_packet_batch_budget_ms(latency_budget_ms);
    let request_cancelled = crate::services::active_public_operation_cancellation()
        .unwrap_or_else(codestory_retrieval::cancellation_flag);
    let readiness_deadline = Instant::now()
        .checked_add(Duration::from_millis(readiness_budget_ms))
        .expect("bounded packet descriptor deadline");
    if queries.is_empty() || session.remaining_hydration_slots() == 0 {
        let seal_started = Instant::now();
        session.seal_retrieval_admission();
        let admission_seal_wall_ms = packet_descriptor_phase_wall_ms(seal_started);
        let readiness_started = Instant::now();
        let readiness = if let Some(pinned) = active_pinned_retrieval_read(controller) {
            pinned
                .session
                .validate_full_readiness_with_control(
                    readiness_deadline,
                    Arc::clone(&request_cancelled),
                )
                .map_err(|error| {
                    map_pinned_query_error_at(
                        error,
                        PinnedRetrievalStopBoundary::DeferredFullReadiness,
                    )
                })
        } else {
            Ok(())
        };
        let deferred_readiness_wall_ms = packet_descriptor_phase_wall_ms(readiness_started);
        super::packet_batch::observe_packet_descriptor_preadmit_runtime_phases(
            0,
            admission_seal_wall_ms,
            deferred_readiness_wall_ms,
        );
        readiness?;
        return Ok(());
    }
    if !sidecar_descriptor_retrieval_enabled(controller) {
        let reason = if retrieval_env_override() == Some(false) {
            "CODESTORY_RETRIEVAL=0 is unsupported; packet descriptor retrieval is mandatory"
                .to_string()
        } else if let (Ok(project_root), Ok(storage_path)) = (
            controller.require_project_root(),
            controller.require_storage_path(),
        ) {
            let status = sidecar_descriptor_mode_status_for_runtime(
                &project_root,
                &storage_path,
                &controller.runtime_config,
            );
            format!(
                "packet descriptor retrieval is unavailable (profile={} mode={} reason={})",
                status.profile.as_deref().unwrap_or("unknown"),
                status.mode,
                status.degraded_reason.as_deref().unwrap_or("unknown")
            )
        } else {
            "packet descriptor retrieval requires an open indexed project".to_string()
        };
        return Err(sidecar_retrieval_unavailable_error(controller, reason));
    }

    with_pinned_retrieval_read(controller, |pinned| {
        let query_plan_started = Instant::now();
        let per_query_budget = descriptor_budget_ms
            .checked_div(queries.len().max(1) as u64)
            .unwrap_or(100)
            .max(100);
        let batch_items = queries
            .iter()
            .map(|query| QueryBatchItem {
                lexical_policy: query.lexical_policy,
                query: &query.query,
                budget_ms: Some(per_query_budget),
            })
            .collect::<Vec<_>>();
        let query_plan_wall_ms = packet_descriptor_phase_wall_ms(query_plan_started);
        let include_dense_semantic = session.includes_dense_semantic();
        let query_results = with_detached_sidecar_query_cache(controller, |cache| {
            #[cfg(feature = "benchmark-support")]
            if !include_dense_semantic {
                return pinned
                    .session
                    .execute_packet_descriptor_batch_without_dense_semantic_for_benchmark_with_observation_and_cache(
                        &batch_items,
                        Some(Arc::clone(&request_cancelled)),
                        cache,
                    );
            }
            pinned
                .session
                .execute_packet_descriptor_batch_with_observation_and_cache(
                    &batch_items,
                    Some(Arc::clone(&request_cancelled)),
                    cache,
                )
        })
        .map_err(map_pinned_query_error)?;
        let (query_results, descriptor_observation) = query_results;
        #[cfg(any(test, feature = "benchmark-support"))]
        for result in &query_results {
            session.record_descriptor_trace(&result.trace);
        }
        if query_results.len() != queries.len() {
            return Err(sidecar_retrieval_unavailable_error(
                controller,
                format!(
                    "packet descriptor batch returned {} results for {} queries",
                    query_results.len(),
                    queries.len()
                ),
            ));
        }

        for (expected_query, result) in queries.iter().zip(&query_results) {
            if result.query != expected_query.query
                || result.features.lexical_policy != expected_query.lexical_policy
            {
                return Err(sidecar_retrieval_unavailable_error(
                    controller,
                    format!(
                        "packet descriptor batch query mismatch expected `{}` got `{}`",
                        expected_query.query, result.query
                    ),
                ));
            }
            pinned.ensure_query_identity(result, "admitting packet descriptors")?;
        }
        for (expected, result) in queries.iter().zip(&query_results) {
            session
                .retain_descriptor_result_with_policy(
                    result,
                    expected.lexical_policy,
                    include_dense_semantic,
                    pinned.session.publication_identity(),
                )
                .map_err(|reason| {
                    ApiError::internal(format!(
                        "packet descriptor retention violated its coherent session: {reason}"
                    ))
                })?;
        }
        // Observe the returned eligible view order before max-score pooling.
        // Bounded metadata only; this cannot rerank, read or admit a candidate.
        let mut view_rows = Vec::new();
        let mut view_eligible_counts = Vec::new();
        for result in query_results.iter().take(4) {
            let mut rows = Vec::new();
            let mut eligible = 0;
            for (rank, hit) in result.hits.iter().enumerate() {
                if is_phantom_sidecar_hit(hit) {
                    continue;
                }
                if let Some(descriptor) = hit.packet_descriptor() {
                    eligible += 1;
                    if rows.len() < 16 {
                        rows.push((
                            descriptor.stable_identity,
                            rank,
                            descriptor.retrieval_score.value.to_bits(),
                        ));
                    }
                }
            }
            view_rows.push(rows);
            view_eligible_counts.push(eligible);
        }
        super::packet_batch::observe_packet_raf_query_views(
            &view_rows,
            &view_eligible_counts,
            query_results.len(),
        );
        let seal_started = Instant::now();
        admit_packet_candidate_descriptors_bound(
            &session,
            query_results.iter().flat_map(|result| result.hits.iter()),
            Some(&pinned.session.publication_identity().sidecar_input_hash),
        );
        let admission_seal_wall_ms = packet_descriptor_phase_wall_ms(seal_started);
        // The repository-wide freshness and core/vector attestation checks are
        // intentionally deferred until the one packet admission session is
        // sealed. They may inspect repository records, so running them before
        // this point would violate the descriptor-first boundary.
        let readiness_started = Instant::now();
        let readiness = pinned
            .session
            .validate_full_readiness_with_control(
                readiness_deadline,
                Arc::clone(&request_cancelled),
            )
            .map_err(|error| {
                map_pinned_query_error_at(error, PinnedRetrievalStopBoundary::DeferredFullReadiness)
            });
        let deferred_readiness_wall_ms = packet_descriptor_phase_wall_ms(readiness_started);
        if let Some(observation) = descriptor_observation {
            super::packet_batch::observe_packet_descriptor_preadmission(
                observation.query_count,
                observation.health_resolution_wall_ms,
                observation.query_batch_wall_ms,
                query_plan_wall_ms,
                observation.lexical_wall_ms,
                observation.dense_semantic_wall_ms,
                admission_seal_wall_ms,
                deferred_readiness_wall_ms,
            );
        }
        readiness?;
        Ok(())
    })
}

fn packet_descriptor_phase_wall_ms(started_at: Instant) -> u64 {
    let elapsed = started_at.elapsed();
    let millis = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    millis.saturating_add(u64::from(!elapsed.subsec_nanos().is_multiple_of(1_000_000)))
}

#[cfg(test)]
fn admit_packet_candidate_descriptors<'a>(
    session: &crate::agent::packet_candidate::PacketProofSession,
    candidates: impl IntoIterator<Item = &'a CandidateHit>,
) {
    admit_packet_candidate_descriptors_bound(session, candidates, None);
}

fn admit_packet_candidate_descriptors_bound<'a>(
    session: &crate::agent::packet_candidate::PacketProofSession,
    candidates: impl IntoIterator<Item = &'a CandidateHit>,
    input_hash: Option<&str>,
) {
    let mut descriptors = Vec::new();
    for candidate in candidates
        .into_iter()
        .filter(|candidate| !is_phantom_sidecar_hit(candidate))
    {
        if let Some(descriptor) = candidate.packet_descriptor() {
            descriptors.push(descriptor);
        } else {
            let stable_identity = candidate.packet_stable_identity();
            let kind = if stable_identity.is_some() {
                PacketAdmissionGapKindV1::SourceBoundMissing
            } else {
                PacketAdmissionGapKindV1::StableIdentityMissing
            };
            session.record_ineligible_candidate(kind, None);
        }
    }
    descriptors.sort_by(|left, right| {
        right
            .retrieval_score
            .value
            .total_cmp(&left.retrieval_score.value)
            .then_with(|| left.stable_identity.cmp(&right.stable_identity))
    });
    session.retain_replay_ranked_descriptors(&descriptors);
    let mut ranked_identities = Vec::new();
    let mut ranked_seen = HashSet::new();
    for descriptor in &descriptors {
        if ranked_seen.insert(descriptor.stable_identity.clone()) {
            ranked_identities.push(descriptor.stable_identity.clone());
        }
    }
    let descriptors = published_call_admission_order(
        &descriptors,
        &session
            .receipts()
            .into_iter()
            .map(|r| r.stable_identity)
            .collect(),
        input_hash,
    );
    admit_published_call_descriptors(session, &descriptors);
    session.seal_retrieval_admission();
    let admitted_identities = session
        .receipts()
        .into_iter()
        .map(|receipt| receipt.stable_identity)
        .collect::<Vec<_>>();
    super::packet_batch::observe_packet_raf_ranked_admitted(
        &ranked_identities,
        &admitted_identities,
    );
}

/// Admission remains count/source bounded. Retaining metadata is a separate
/// pass over the same validated pool, so a late exact owner cannot lose its
/// navigation annotation when a new identity first exceeds the count cap.
fn admit_published_call_descriptors(
    session: &crate::agent::packet_candidate::PacketProofSession,
    descriptors: &[codestory_contracts::compilation::PacketCandidateDescriptorV1],
) {
    for descriptor in descriptors {
        match session.admit_descriptor(descriptor) {
            PacketAdmissionDecision::CountBudgetExceeded
                if session.remaining_hydration_slots() == 0 =>
            {
                break;
            }
            _ => {}
        }
    }
    for descriptor in descriptors {
        session.retain_admitted_call_annotation(descriptor);
    }
}

/// Validate and deduplicate one published eligible pool without changing new
/// identity retrieval rank. Optional call hints remain annotations only. This
/// reserves no extra source, performs no read, and cannot introduce a new target.
fn published_call_admission_order(
    ranked: &[codestory_contracts::compilation::PacketCandidateDescriptorV1],
    already_admitted: &HashSet<String>,
    input_hash: Option<&str>,
) -> Vec<codestory_contracts::compilation::PacketCandidateDescriptorV1> {
    use codestory_contracts::compilation::*;
    let mut unique = Vec::<PacketCandidateDescriptorV1>::new();
    let mut indices = HashMap::<String, usize>::new();
    let mut conflicts = HashSet::new();
    for row in ranked {
        let mut row = row.clone();
        if row.published_calls_conflicted {
            conflicts.insert(row.stable_identity.clone());
            row.published_calls = None;
        }
        row.published_calls = row.published_calls.filter(|m| {
            Some(m.sidecar_input_hash.as_str()) == input_hash
                && input_hash.is_some()
                && m.neighborhood.is_valid_for(&row.stable_identity)
        });
        if let Some(&index) = indices.get(&row.stable_identity) {
            let previous = &mut unique[index];
            if conflicts.contains(&row.stable_identity) {
                previous.published_calls = None;
                previous.published_calls_conflicted = true;
            }
            match (&previous.published_calls, &row.published_calls) {
                (None, Some(_)) if !conflicts.contains(&row.stable_identity) => {
                    previous.published_calls = row.published_calls
                }
                (Some(a), Some(b)) if a != b => {
                    previous.published_calls = None;
                    previous.published_calls_conflicted = true;
                    conflicts.insert(row.stable_identity);
                }
                _ => {}
            }
        } else {
            indices.insert(row.stable_identity.clone(), unique.len());
            unique.push(row);
        }
    }
    // Already-issued owners retain their existing session receipts and are
    // annotated in the separate full-pool pass. All new identities retain the
    // unique versioned retrieval order; optional topology cannot evict them.
    unique.sort_by_key(|row| already_admitted.contains(&row.stable_identity));
    unique
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn replay_published_call_admissions(
    ranked: &[codestory_contracts::compilation::PacketCandidateDescriptorV1],
    input_hash: &str,
) -> (
    Vec<codestory_contracts::compilation::PacketAdmissionReceiptV1>,
    Vec<codestory_contracts::compilation::PacketDirectedRelationV1>,
) {
    let session = crate::agent::packet_candidate::PacketProofSession::new();
    let ordered = published_call_admission_order(ranked, &HashSet::new(), Some(input_hash));
    admit_published_call_descriptors(&session, &ordered);
    session.seal_retrieval_admission();
    session.reserve_source_pages(&std::collections::HashSet::new());
    (session.receipts(), session.admitted_call_navigation())
}

pub(crate) fn run_and_resolve_sidecar_query(
    controller: &AppController,
    query: &str,
    max_results: usize,
    latency_budget_ms: Option<u32>,
) -> Result<(QueryResult, SidecarCandidateResolutionOutcome), ApiError> {
    run_and_resolve_sidecar_query_with_policy(
        controller,
        query,
        max_results,
        latency_budget_ms,
        LexicalAdmissionPolicy::StrictLiteralV1,
    )
}

pub(crate) fn run_and_resolve_sidecar_query_with_policy(
    controller: &AppController,
    query: &str,
    max_results: usize,
    latency_budget_ms: Option<u32>,
    lexical_policy: LexicalAdmissionPolicy,
) -> Result<(QueryResult, SidecarCandidateResolutionOutcome), ApiError> {
    with_pinned_retrieval_read(controller, |pinned| {
        let packet_session = crate::agent::packet_candidate::active_packet_proof_session();
        let include_dense_semantic = packet_session
            .as_ref()
            .is_none_or(|session| session.includes_dense_semantic());
        let retained = packet_session.as_ref().and_then(|session| {
            session.descriptor_result_with_policy(
                query,
                lexical_policy,
                include_dense_semantic,
                pinned.session.publication_identity(),
            )
        });
        if retained.is_none()
            && packet_session
                .as_ref()
                .is_some_and(|session| session.retrieval_admission_is_sealed())
        {
            return Err(ApiError::internal(
                "sealed packet is missing its admitted query/policy result",
            ));
        }
        if packet_session.is_none() && lexical_policy != LexicalAdmissionPolicy::StrictLiteralV1 {
            return Err(ApiError::internal(
                "broad question retrieval requires typed packet origin",
            ));
        }
        let _executed_query = retained.is_none();
        let query_result = if let Some(result) = retained {
            result
        } else {
            with_detached_sidecar_query_cache(controller, |cache| {
                if packet_session.is_some() {
                    #[cfg(feature = "benchmark-support")]
                    if !include_dense_semantic {
                        return pinned
                        .session
                        .execute_packet_descriptors_without_dense_semantic_for_benchmark_with_admission_policy(
                            query,
                            lexical_policy,
                            Some(sidecar_budget_ms(latency_budget_ms)),
                            crate::services::active_public_operation_cancellation(),
                            cache,
                        );
                    }
                    pinned.session.execute_packet_descriptors_with_admission_policy(
                        query,
                        lexical_policy,
                        Some(sidecar_budget_ms(latency_budget_ms)),
                        crate::services::active_public_operation_cancellation(),
                        cache,
                    )
                } else {
                    pinned.session.execute_with_cache(
                        query,
                        Some(sidecar_budget_ms(latency_budget_ms)),
                        crate::services::active_public_operation_cancellation(),
                        cache,
                    )
                }
            })
            .map_err(map_pinned_query_error)?
        };
        #[cfg(any(test, feature = "benchmark-support"))]
        if _executed_query && let Some(session) = packet_session {
            session.record_descriptor_trace(&query_result.trace);
        }
        if query_result.features.lexical_policy != lexical_policy {
            return Err(ApiError::internal(
                "sidecar result executed a different lexical admission policy",
            ));
        }
        pinned.ensure_query_identity(&query_result, "resolving sidecar candidates")?;
        let resolution = resolve_sidecar_candidates_in_read(
            controller,
            pinned,
            &query_result.hits,
            max_results,
        )?;
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

pub(crate) fn try_sidecar_primary_search_with_policy(
    controller: &AppController,
    prompt: &str,
    max_results: usize,
    latency_budget_ms: Option<u32>,
    lexical_policy: LexicalAdmissionPolicy,
) -> Option<SidecarPrimarySearchOutcome> {
    if !sidecar_retrieval_primary_enabled(controller) {
        return sidecar_retrieval_unavailable_reason(controller)
            .map(|reason| SidecarPrimarySearchOutcome::Unavailable { reason });
    }
    match run_and_resolve_sidecar_query_with_policy(
        controller,
        prompt,
        max_results,
        latency_budget_ms,
        lexical_policy,
    ) {
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
    let can_serve_unindexed_lexical_artifacts = resolution.unindexed_lexical_artifacts_only
        && sidecar_mode_can_serve_primary(&query_result.trace.retrieval_mode)
        && sidecar_blocking_cancel_reason(&query_result).is_none();
    let resolved_hits = resolution.resolved_hits.clone();
    let mut shadow = shadow_from_query_result_with_candidate_admission_diagnostics(
        controller,
        query_result.clone(),
        &resolution,
        &resolved_hits,
        &resolved_hits,
    );
    if can_serve_unindexed_lexical_artifacts {
        shadow.diagnostic_only = true;
    }

    if let Some(reason) = sidecar_primary_result_rejection_reason(&query_result, &resolved_hits)
        && !can_serve_unindexed_lexical_artifacts
    {
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
    pub retained_deadline_queries: Vec<String>,
    pub diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
}

pub(crate) struct SidecarCandidateResolutionOutcome {
    pub(crate) resolved_hits: Vec<SearchHit>,
    pub(crate) packet_hits: Vec<PacketSearchHit>,
    unresolved_candidate_count: usize,
    blocking_unresolved_candidate_count: usize,
    attempted_candidate_indices: HashSet<usize>,
    unindexed_lexical_artifacts_only: bool,
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
    // query, ran out. Left as `Completed` it would misreport an empty result as complete.
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
        let packet_session = active_packet_proof_session();
        let include_dense_semantic = packet_session
            .as_ref()
            .is_none_or(|session| session.includes_dense_semantic());
        let mut query_results = vec![None; batch_queries.len()];
        let mut missing_indices = Vec::new();
        let mut retained_queries = HashSet::new();
        for (index, (query, _)) in batch_queries.iter().enumerate() {
            let retained = packet_session.as_ref().and_then(|session| {
                session.descriptor_result(
                    query,
                    include_dense_semantic,
                    pinned.session.publication_identity(),
                )
            });
            if let Some(result) = retained {
                retained_queries.insert(query.clone());
                query_results[index] = Some(result);
            } else {
                missing_indices.push(index);
            }
        }
        if !missing_indices.is_empty()
            && packet_session
                .as_ref()
                .is_some_and(|session| session.retrieval_admission_is_sealed())
        {
            return Err(ApiError::internal(
                "sealed packet is missing a strict free-query result",
            ));
        }
        let missing_items = missing_indices
            .iter()
            .map(|index| QueryBatchItem {
                lexical_policy: codestory_retrieval::LexicalAdmissionPolicy::StrictLiteralV1,
                query: batch_queries[*index].0.as_str(),
                budget_ms: Some(batch_queries[*index].1),
            })
            .collect::<Vec<_>>();
        let executed_results = if missing_items.is_empty() {
            Vec::new()
        } else {
            with_detached_sidecar_query_cache(controller, |cache| {
            #[cfg(feature = "benchmark-support")]
                if packet_session.is_some() && !include_dense_semantic {
                return pinned
                    .session
                    .execute_packet_descriptor_batch_without_dense_semantic_for_benchmark_with_cache(
                            &missing_items,
                        crate::services::active_public_operation_cancellation(),
                        cache,
                    );
                }
                pinned.session.execute_packet_descriptor_batch_with_cache(
                    &missing_items,
                    crate::services::active_public_operation_cancellation(),
                    cache,
                )
            })
            .map_err(map_pinned_query_error)?
        };
        if executed_results.len() != missing_indices.len() {
            return Err(sidecar_retrieval_unavailable_error(
                controller,
                format!(
                    "packet descriptor batch returned {} results for {} missing queries",
                    executed_results.len(),
                    missing_indices.len()
                ),
            ));
        }
        #[cfg(any(test, feature = "benchmark-support"))]
        if let Some(session) = packet_session.as_ref() {
            for result in &executed_results {
                session.record_descriptor_trace(&result.trace);
            }
        }
        for (index, result) in missing_indices.into_iter().zip(executed_results) {
            query_results[index] = Some(result);
        }
        let query_results = query_results
            .into_iter()
            .enumerate()
            .map(|(index, result)| {
                result.ok_or_else(|| {
                    ApiError::internal(format!(
                        "packet descriptor result {index} was neither retained nor executed"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        for result in &query_results {
            if result.features.lexical_policy != LexicalAdmissionPolicy::StrictLiteralV1 {
                return Err(ApiError::internal(
                    "free-query batch received a non-strict result",
                ));
            }
            pinned.ensure_query_identity(result, "resolving sidecar packet batch")?;
        }
        build_sidecar_packet_batch_outcome(
            controller,
            queries,
            query_results,
            clamp_elapsed_ms(batch_started_at),
            &retained_queries,
            |query_result, max_results| {
                resolve_sidecar_candidates_in_read(
                    controller,
                    pinned,
                    &query_result.hits,
                    max_results,
                )
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
        &HashSet::new(),
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
    retained_queries: &HashSet<String>,
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
    // Authenticate the entire positional batch before hydrating even its first hit.
    for ((expected_query, _), result) in queries.iter().zip(&query_results) {
        if result.query != *expected_query
            || result.features.lexical_policy != LexicalAdmissionPolicy::StrictLiteralV1
        {
            return Err(ApiError::internal("free-query batch query/policy mismatch"));
        }
    }
    let mut results = Vec::with_capacity(queries.len());
    let mut retryable_queries = Vec::new();
    let mut retained_deadline_queries = Vec::new();
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
        let max_results = (*max_results).clamp(
            1,
            codestory_contracts::compilation::INTERIM_MAX_ADMITTED_CANDIDATES,
        );
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
                if retained_queries.contains(query) {
                    retained_deadline_queries.push(query.clone());
                } else {
                    retryable_queries.push(query.clone());
                }
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
                // retryable only when it was freshly executed. A retained
                // descriptor is immutable and replay cannot recover its deadline.
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
        retained_deadline_queries,
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
/// existing downstream limit. Only freshly executed queries remain retryable;
/// retained descriptor deadlines cannot be recovered by replay. The atom signal
/// only ORDERS the result —
/// resolution rank first, need-first as a tiebreak among equal ranks, then
/// the original resolution order — so that when a downstream limit binds it
/// binds on the identities that occupy the most role positions of the
/// requirement group rather than on whichever tied hit came first. No slot is
/// added anywhere, and nothing here marks a hit as proven (contract rule 4:
/// atom need is a selection input, never a proof input).
fn retained_cancelled_packet_hits(hits: Vec<PacketSearchHit>) -> Vec<PacketSearchHit> {
    let mut retained = hits.into_iter().enumerate().collect::<Vec<_>>();
    retained.sort_by(|(left_rank, left), (right_rank, right)| {
        right
            .hit
            .score
            .partial_cmp(&left.hit.score)
            .unwrap_or(std::cmp::Ordering::Equal)
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
    let normalized = normalize_storage_path_text(file_path);
    if normalized.trim().is_empty() {
        return false;
    }
    let source_rooted = source_root_candidate_path(&normalized);
    std::iter::once(normalized.as_str())
        .chain(source_rooted.as_deref())
        .any(|candidate| {
            matches!(
                codestory_workspace::resolve_project_relative_path(
                    project_root,
                    Path::new(candidate)
                ),
                Ok(codestory_workspace::ProjectRelativePathResolution::Existing { .. })
            )
        })
}

fn candidate_is_unindexed_lexical_artifact(
    storage: &Store,
    project_root: &Path,
    candidate: &CandidateHit,
) -> bool {
    if candidate.source != CandidateSource::Lexical
        || candidate.node_id.is_some()
        || candidate.symbol_name.is_some()
        || candidate.qualified_name.is_some()
        || candidate.target.is_some()
    {
        return false;
    }

    let normalized = normalize_repo_relative_path(project_root, &candidate.file_path);
    if normalized.trim().is_empty() {
        return false;
    }
    let source_rooted = source_root_candidate_path(&normalized);
    let existing_outside_core_route = std::iter::once(normalized.as_str())
        .chain(source_rooted.as_deref())
        .any(|path| {
            matches!(
                codestory_workspace::resolve_project_relative_path(project_root, Path::new(path)),
                Ok(codestory_workspace::ProjectRelativePathResolution::Existing {
                    relative,
                    ..
                }) if !codestory_workspace::has_supported_source_route(&relative)
            )
        });
    existing_outside_core_route
        && candidate_lookup_paths(project_root, &normalized)
            .into_iter()
            .all(|path| matches!(storage.get_file_by_path(&path), Ok(None)))
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
    controller: &AppController,
    pinned: &PinnedRetrievalRead,
    candidates: &[CandidateHit],
    max_results: usize,
) -> Result<SidecarCandidateResolutionOutcome, ApiError> {
    if active_packet_proof_session().is_some() {
        // Packet descriptors carry stable node identities. Canonical-name
        // streaming is repository-wide core hydration and therefore cannot
        // happen before or on behalf of a rejected packet candidate.
        return resolve_sidecar_candidates_in_storage(
            &pinned.session.storage(),
            &HashMap::new(),
            &pinned.project_root,
            candidates,
            max_results,
        );
    }
    let node_names = pinned.canonical_node_names(controller)?;
    resolve_sidecar_candidates_in_storage(
        &pinned.session.storage(),
        &node_names,
        &pinned.project_root,
        candidates,
        max_results,
    )
}

/// One resolved candidate's hydrated CALL graph: edge provenance and the
/// bounded candidate graph.
type PacketCandidateGraphHydration = (Vec<PacketGraphEdgeProvenance>, Option<GraphResponse>);

const PACKET_CANDIDATE_DIRECTION_NODE_LIMIT: usize = 65;
const PACKET_CANDIDATE_RAW_EDGE_LIMIT: usize = PACKET_CANDIDATE_DIRECTION_NODE_LIMIT * 6;

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

    let run_call_trails = specific_evidence.is_some() || hydrate_outgoing_calls;
    if !run_call_trails {
        return Ok((Vec::new(), None));
    }

    let mut edge_filter = vec![EdgeKind::CALL];
    if let Some((_, edge_kind, _)) = specific_evidence
        && !edge_filter.contains(&edge_kind)
    {
        edge_filter.push(edge_kind);
    }
    let identity_scope = active_packet_proof_session();
    let (collected, scan_truncated, scan_omitted_edge_count) =
        if let Some(identity_scope) = identity_scope.as_ref() {
            // Trail traversal materializes every endpoint node before it returns
            // the incident edges. A packet may inspect only identities admitted
            // by the packet-wide descriptor gate, so use the edge-only view and
            // discard unadmitted endpoints before any node or file hydration.
            let incident = storage
                .get_bounded_raw_incident_edges(node_id, PACKET_CANDIDATE_RAW_EDGE_LIMIT)
                .map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to resolve bounded packet candidate edge provenance: {error}"
                    ))
                })?;
            let mut omitted = u32::from(incident.truncated);
            let mut collected = Vec::new();
            for edge in incident.edges {
                let (source, target) = edge.effective_endpoints();
                if !edge_filter.contains(&edge.kind)
                    || (source != node_id && target != node_id)
                    || !identity_scope.is_admitted_node(source)
                    || !identity_scope.is_admitted_node(target)
                {
                    omitted = omitted.saturating_add(1);
                    continue;
                }
                collected.push(edge);
            }
            (collected, incident.truncated, omitted)
        } else {
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
            let scan_truncated = incoming.truncated || outgoing.truncated;
            let scan_omitted_edge_count = incoming
                .omitted_edge_count
                .saturating_add(outgoing.omitted_edge_count);
            let mut seen_incident_edge_ids = HashSet::new();
            let mut collected = Vec::new();
            for edge in incoming.edges.into_iter().chain(outgoing.edges) {
                if seen_incident_edge_ids.insert(edge.id) {
                    collected.push(edge);
                }
            }
            (collected, scan_truncated, scan_omitted_edge_count)
        };

    let mut edges = Vec::new();
    for edge in collected {
        let mut selected_direction = None;
        if let Some((direction, edge_kind, hop)) = specific_evidence
            && edge.kind == edge_kind
        {
            let (source, target) = edge.effective_endpoints();
            let matches_specific = match direction {
                PacketGraphDirection::Outgoing => target == node_id,
                PacketGraphDirection::Incoming => source == node_id,
            };
            if matches_specific {
                selected_direction = Some((direction, hop, false));
            }
        }
        let (source, _) = edge.effective_endpoints();
        if hydrate_outgoing_calls && edge.kind == EdgeKind::CALL && source == node_id {
            selected_direction.get_or_insert((PacketGraphDirection::Outgoing, 1, true));
        }
        if let Some((direction, hop, hydrated)) = selected_direction {
            edges.push((edge, direction, hop, hydrated));
        }
    }
    edges.sort_by(
        |(left, _, _, left_hydrated), (right, _, _, right_hydrated)| {
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
        return Ok((Vec::new(), None));
    }

    let graph_flags = app_graph_flags();
    let edge_dtos = edges
        .iter()
        .map(|(edge, _, _, _)| graph_edge_dto(edge.clone().with_effective_endpoints(), graph_flags))
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
        .map(|((_, direction, hop, hydrated), edge)| {
            let mut producers = specific_producers.clone();
            if *hydrated {
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
    let identity_scope = crate::agent::packet_candidate::active_packet_proof_session();
    let mut pending = if identity_scope.is_some() {
        // Packet admission consumes the sidecar's versioned order and descriptor
        // metadata only. Even a filesystem stat would inspect an unadmitted
        // candidate, so path validation waits until after admission.
        candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| !is_phantom_sidecar_hit(candidate))
            .map(|(index, candidate)| (index, candidate, false))
            .collect::<Vec<_>>()
    } else {
        ordered_sidecar_candidates(candidates, |candidate| {
            candidate_path_resolvable(project_root, &candidate.file_path)
        })
    };
    if identity_scope.is_some() {
        pending.sort_by(|left, right| {
            let left_candidate = left.1;
            let right_candidate = right.1;
            right_candidate
                .score
                .total_cmp(&left_candidate.score)
                .then_with(|| left_candidate.node_id.cmp(&right_candidate.node_id))
                .then_with(|| left_candidate.file_path.cmp(&right_candidate.file_path))
        });
    }
    let max_results = if identity_scope.is_some() {
        max_results.min(codestory_contracts::compilation::INTERIM_MAX_ADMITTED_CANDIDATES)
    } else {
        max_results
    };
    let mut admitted: Vec<(CoreNodeId, &CandidateHit)> = Vec::new();

    while admitted.len() < max_results && !pending.is_empty() {
        let (candidate_index, candidate, path_resolvable) = pending.remove(0);
        attempted_candidate_indices.insert(candidate_index);
        let rel_path = normalize_repo_relative_path(project_root, &candidate.file_path);
        let descriptor = if let Some(identity_scope) = identity_scope.as_ref() {
            match candidate.packet_descriptor() {
                Some(descriptor) => Some(descriptor),
                None => {
                    let stable_identity = candidate.packet_stable_identity();
                    let kind = if stable_identity.is_some() {
                        PacketAdmissionGapKindV1::SourceBoundMissing
                    } else {
                        PacketAdmissionGapKindV1::StableIdentityMissing
                    };
                    identity_scope.record_ineligible_candidate(kind, None);
                    unresolved_candidates.push((
                        candidate,
                        if matches!(kind, PacketAdmissionGapKindV1::StableIdentityMissing) {
                            "stable_identity_missing"
                        } else {
                            "source_bound_missing"
                        },
                    ));
                    continue;
                }
            }
        } else {
            None
        };
        let descriptor_was_preadmitted = if let (Some(identity_scope), Some(descriptor)) =
            (identity_scope.as_ref(), descriptor.as_ref())
        {
            // The descriptor batch sealed the packet-wide session before this
            // hydration path began. A candidate missing from that admitted set
            // is rejected before even a path lookup or node read.
            match identity_scope.admit_descriptor(descriptor) {
                PacketAdmissionDecision::AlreadyAdmitted => true,
                PacketAdmissionDecision::Admitted => false,
                PacketAdmissionDecision::CountBudgetExceeded => {
                    unresolved_candidates.push((candidate, "candidate_count_exceeded"));
                    continue;
                }
                PacketAdmissionDecision::SourceBudgetExceeded => {
                    unresolved_candidates.push((candidate, "source_budget_exceeded"));
                    continue;
                }
            }
        } else {
            false
        };
        let node_id = if let Some(descriptor) = descriptor.as_ref() {
            if let Some(raw_id) = descriptor.stable_identity.strip_prefix("node:") {
                raw_id.parse::<i64>().ok().map(CoreNodeId)
            } else if let Some(path) = descriptor.stable_identity.strip_prefix("path:") {
                // A sealed path admits this file, not an inferred callable or
                // a source-root alias. Use the same exact file lookup as
                // subsequent packet source hydration.
                super::packet_compiler::find_admitted_file(storage, project_root, path)?
                    .map(|file| CoreNodeId(file.id))
            } else {
                None
            }
        } else {
            resolve_candidate_node_id(storage, node_names, project_root, &rel_path, candidate)
        };
        let Some(node_id) = node_id else {
            let admitted_unindexed_lexical_artifact = descriptor_was_preadmitted
                && candidate_is_unindexed_lexical_artifact(storage, project_root, candidate);
            let label = if admitted_unindexed_lexical_artifact {
                "node_unresolved"
            } else if path_resolvable {
                if identity_scope.is_some() {
                    "stable_identity_missing"
                } else {
                    "node_unresolved"
                }
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
        admitted.push((node_id, candidate));
    }

    for (node_id, candidate) in admitted {
        let Some(hit) =
            AppController::build_search_hit(storage, node_names, node_id, candidate.score)?
        else {
            unresolved_candidates.push((candidate, "hit_build_failed"));
            continue;
        };
        let hit = classify_resolved_candidate_hit(hit, candidate);
        let (graph_provenance, graph) =
            packet_graph_for_resolved_candidate(storage, node_names, node_id, candidate)?;
        packet_hits.push(PacketSearchHit {
            hit: hit.clone(),
            graph_provenance,
            graph,
        });
        hits.push(hit);
    }

    let has_resolved_hit = !hits.is_empty();
    let unresolved_candidate_count = unresolved_candidates.len();
    let unindexed_lexical_artifacts_only = !candidates.is_empty()
        && hits.is_empty()
        && unresolved_candidate_count == candidates.len()
        && attempted_candidate_indices.len() == candidates.len()
        && unresolved_candidates.iter().all(|(candidate, label)| {
            *label == "node_unresolved"
                && candidate_is_unindexed_lexical_artifact(storage, project_root, candidate)
        });
    let blocking_unresolved_candidate_count = if unindexed_lexical_artifacts_only {
        0
    } else {
        unresolved_candidates
            .iter()
            .filter(|(candidate, label)| {
                !unresolved_candidate_is_diagnostic(candidate, label, has_resolved_hit)
            })
            .count()
    };

    Ok(SidecarCandidateResolutionOutcome {
        resolved_hits: hits,
        packet_hits,
        unresolved_candidate_count,
        blocking_unresolved_candidate_count,
        attempted_candidate_indices,
        unindexed_lexical_artifacts_only,
    })
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
        AgentPacketRequestDto, NodeId, NodeKind as ApiNodeKind, PacketBudgetModeDto,
        PacketDispositionKindDto, SearchHitOrigin, SearchTargetDto,
    };
    use codestory_retrieval::{
        CandidateHit, CandidateSource, QueryTrace, RetrievalCacheKey, RetrievalStageKind,
        StageTrace, classify_query, project_id_for_root, rank_candidates,
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
        pinned_operation_fixture_with_profile(codestory_retrieval::SidecarProfile::Local)
    }

    fn pinned_operation_fixture_with_profile(
        profile: codestory_retrieval::SidecarProfile,
    ) -> PinnedOperationFixture {
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
            SidecarRuntimeConfig::for_project_profile(Some(project.path()), profile)
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

    fn public_packet_fixture_with_unindexed_lexical_artifact() -> PinnedOperationFixture {
        public_packet_fixture_with_lexical_files(&[(
            "LICENSE",
            "The control plane assumes the control plane role, and worker nodes use the worker node role.\n",
        )])
    }

    fn public_packet_fixture_with_lexical_files(files: &[(&str, &str)]) -> PinnedOperationFixture {
        let project = tempfile::tempdir().expect("project");
        let storage = tempfile::tempdir().expect("storage");
        let retrieval_cache = tempfile::tempdir().expect("retrieval cache");
        let storage_path = storage.path().join("codestory.db");
        std::fs::write(
            project.path().join("metadata.rs"),
            "// public packet freshness anchor\n",
        )
        .expect("write indexed freshness anchor");
        for (path, content) in files {
            let path = project.path().join(path);
            std::fs::create_dir_all(path.parent().expect("artifact parent"))
                .expect("create artifact parent");
            std::fs::write(path, content).expect("write unindexed lexical artifact");
        }
        let mut runtime = codestory_retrieval::with_test_cache_root(retrieval_cache.path(), || {
            SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Agent,
            )
        });
        assert_eq!(
            crate::semantic_projection::semantic_doc_scope_from_value(
                &runtime.retrieval.semantic_doc_scope
            ),
            crate::semantic_projection::SemanticDocScope::DurableSymbols,
            "zero-dense public fixture requires the unchanged durable semantic scope",
        );
        runtime.embedding.allow_cpu = true;
        let controller = AppController::new_with_config(runtime.clone());
        controller
            .project_service()
            .open_project_summary_with_storage_path(
                project.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("bind public packet fixture");
        controller
            .run_indexing_blocking_without_runtime_refresh(
                codestory_contracts::api::IndexMode::Full,
            )
            .expect("publish public-ready complete core");
        let manifest =
            publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
                .expect("publish public-ready retrieval fixture");
        assert_eq!(
            manifest.dense_projection_count,
            Some(0),
            "public packet fixture must stay zero-dense and must not execute embeddings"
        );

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
            PinnedRetrievalScope::Full,
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
        assert_eq!(
            canonical_stream_count(&fixture.controller),
            0,
            "pinning alone must not stream repository node records"
        );
        let first_names = first
            .canonical_node_names(&fixture.controller)
            .expect("load first canonical map");
        drop(first);
        assert_eq!(canonical_stream_count(&fixture.controller), 1);

        let second = PinnedRetrievalRead::begin(&fixture.controller).expect("second pin");
        assert_eq!(
            canonical_stream_count(&fixture.controller),
            1,
            "pinning alone must not restream the canonical table"
        );
        let second_names = second
            .canonical_node_names(&fixture.controller)
            .expect("reuse canonical map");
        assert_eq!(
            canonical_stream_count(&fixture.controller),
            1,
            "a pin on an unchanged publication must not restream the canonical table"
        );
        assert_eq!(
            *second_names, *first_names,
            "the reused map must be the map the stream produced"
        );
        assert!(
            Arc::ptr_eq(&second_names, &first_names),
            "the reused map must be the cached allocation, not a fresh clone"
        );
    }

    #[test]
    fn rejected_packet_candidate_does_not_stream_repository_identity_records() {
        use crate::agent::packet_candidate::{
            PacketAdmissionDecision, PacketProofSession, install_packet_proof_session,
        };

        let fixture = pinned_operation_fixture();
        let pinned = PinnedRetrievalRead::begin(&fixture.controller).expect("packet pin");
        let admission = Rc::new(PacketProofSession::new());
        for index in 0..codestory_contracts::compilation::INTERIM_MAX_ADMITTED_CANDIDATES {
            assert_eq!(
                admission.admit(&format!("node:{index}"), 1),
                PacketAdmissionDecision::Admitted
            );
        }
        let _guard = install_packet_proof_session(Rc::clone(&admission));
        let mut rejected = CandidateHit::with_source(
            "src/never-opened.rs",
            Some("NeverOpened".into()),
            1.0,
            CandidateSource::Lexical,
        );
        rejected.node_id = Some("17".into());
        rejected.source_bytes_upper_bound = Some(1);

        let result =
            resolve_sidecar_candidates_in_read(&fixture.controller, &pinned, &[rejected], 1)
                .expect("budget rejection is diagnostic");

        assert!(result.resolved_hits.is_empty());
        assert_eq!(
            canonical_stream_count(&fixture.controller),
            0,
            "the seventeenth identity must not trigger canonical node hydration"
        );
    }

    /// Publication-keyed means keyed by the publication: a new core generation
    /// describes a different canonical table and must be streamed again.
    #[test]
    fn a_new_core_publication_restreams_the_canonical_symbol_map() {
        use codestory_store::{IndexPublicationMode, IndexPublicationRecord};

        let fixture = pinned_operation_fixture();
        PinnedRetrievalRead::begin(&fixture.controller)
            .expect("first pin")
            .canonical_node_names(&fixture.controller)
            .expect("load first canonical map");
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

        PinnedRetrievalRead::begin(&fixture.controller)
            .expect("pin the new publication")
            .canonical_node_names(&fixture.controller)
            .expect("load replacement canonical map");
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
            artifact_scope: None,
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

    #[test]
    fn packet_descriptor_pin_refuses_a_different_core_before_building() {
        let fixture = pinned_operation_fixture();
        let mut build_calls = 0;
        let pinned = Rc::new(
            PinnedRetrievalRead::begin_packet_descriptor(&fixture.controller)
                .expect("begin packet descriptor pin"),
        );

        let error =
            with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
                with_stable_packet_retrieval_publication(
                    &fixture.controller,
                    "packet response",
                    "ffffffff-ffff-4fff-8fff-ffffffffffff",
                    "foreign-run",
                    || {
                        build_calls += 1;
                        Ok(TestPublicationResponse::default())
                    },
                )
            })
            .expect_err("a descriptor publication from another core must be rejected");

        assert_eq!(error.code, "publication_changed");
        assert_eq!(build_calls, 0);
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

    #[test]
    fn primary_eligibility_reuses_only_a_healthy_validated_agent_pin() {
        let _lock = env_test_lock();
        let fixture =
            pinned_operation_fixture_with_profile(codestory_retrieval::SidecarProfile::Agent);
        let _scope = codestory_workspace::SourceFreshnessScope::enter();
        let pinned = Rc::new(
            PinnedRetrievalRead::begin(&fixture.controller).expect("begin validated Agent pin"),
        );
        let after_pin = codestory_workspace::source_freshness_counts()
            .expect("armed scope")
            .readiness_fingerprint_passes;

        with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
            assert!(
                sidecar_retrieval_primary_enabled(&fixture.controller),
                "the validated Agent pin remains eligible for primary retrieval"
            );
        });
        assert_eq!(
            codestory_workspace::source_freshness_counts()
                .expect("armed scope")
                .readiness_fingerprint_passes,
            after_pin,
            "primary eligibility must not re-fingerprint an already validated operation pin"
        );

        let foreign_controller =
            AppController::new_with_config(fixture.controller.runtime_config.as_ref().clone());
        {
            let mut state = foreign_controller.state.lock();
            state.project_root = Some(fixture._project.path().to_owned());
            state.storage_path = Some(fixture.storage_path.clone());
        }
        with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
            assert!(sidecar_retrieval_primary_enabled(&foreign_controller));
        });
        let after_foreign = codestory_workspace::source_freshness_counts()
            .expect("armed scope")
            .readiness_fingerprint_passes;
        assert!(
            after_foreign > after_pin,
            "a different controller must obtain its own ordinary readiness proof"
        );

        let descriptor = Rc::new(
            PinnedRetrievalRead::begin_packet_descriptor(&fixture.controller)
                .expect("begin an unvalidated descriptor pin"),
        );
        let before_descriptor_gate = codestory_workspace::source_freshness_counts()
            .expect("armed scope")
            .readiness_fingerprint_passes;
        with_active_pinned_retrieval_read(&fixture.controller, descriptor, || {
            assert!(sidecar_retrieval_primary_enabled(&fixture.controller));
        });
        assert!(
            codestory_workspace::source_freshness_counts()
                .expect("armed scope")
                .readiness_fingerprint_passes
                > before_descriptor_gate,
            "a descriptor pin without full validation must retain ordinary readiness"
        );

        let local_fixture = pinned_operation_fixture();
        let local_pin = Rc::new(
            PinnedRetrievalRead::begin(&local_fixture.controller)
                .expect("begin a validated Local-profile pin"),
        );
        with_active_pinned_retrieval_read(&local_fixture.controller, local_pin, || {
            assert!(
                !sidecar_retrieval_primary_enabled(&local_fixture.controller),
                "a validated Local pin must not become Agent primary retrieval"
            );
        });

        std::fs::rename(
            &fixture.controller.runtime_config.layout.lexical_data_dir,
            fixture._retrieval_cache.path().join("unavailable-lexical"),
        )
        .expect("withdraw only this owned fixture's lexical component");
        with_active_pinned_retrieval_read(&fixture.controller, pinned, || {
            assert!(
                !sidecar_retrieval_primary_enabled(&fixture.controller),
                "a validated pin must not hide a component that became unavailable"
            );
        });
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
            PinnedRetrievalScope::Full,
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
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
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
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
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

    #[test]
    fn candidate_path_resolution_admits_only_selected_project_root_extensionless_file() {
        let project = tempfile::tempdir().expect("project");
        let foreign = tempfile::tempdir().expect("foreign project");
        std::fs::write(project.path().join("LICENSE"), "selected\n").expect("write selected file");
        let foreign_path = foreign.path().join("LICENSE");
        std::fs::write(&foreign_path, "foreign\n").expect("write foreign file");

        assert_eq!(
            (
                candidate_path_resolvable(project.path(), "LICENSE"),
                candidate_path_resolvable(project.path(), &foreign_path.to_string_lossy()),
            ),
            (true, false),
            "extensionless admission must bind an existing file to the selected project"
        );
    }

    #[test]
    fn candidate_path_resolution_does_not_borrow_foreign_process_cwd_file() {
        let project = tempfile::tempdir().expect("selected project");
        let cwd_file = Path::new("Cargo.toml");
        assert!(
            cwd_file.is_file(),
            "Cargo runs this crate's unit tests from the crate root"
        );
        assert!(!project.path().join(cwd_file).exists());

        assert!(
            !candidate_path_resolvable(project.path(), &cwd_file.to_string_lossy()),
            "candidate lookup must not borrow a file from the process working directory"
        );
    }

    #[test]
    fn candidate_path_resolution_preserves_nonblank_spaced_filename() {
        let project = tempfile::tempdir().expect("selected project");
        std::fs::write(project.path().join(" LICENSE"), "spaced\n").expect("write spaced filename");

        assert_eq!(
            (
                candidate_path_resolvable(project.path(), " LICENSE"),
                candidate_path_resolvable(project.path(), "LICENSE"),
            ),
            (true, false),
            "ordinary resolution must preserve the actual filename instead of borrowing its trimmed neighbor"
        );
    }

    #[test]
    fn ordinary_candidate_order_prioritizes_selected_extensionless_over_foreign_cwd_file() {
        let project = tempfile::tempdir().expect("project");
        std::fs::write(project.path().join("LICENSE"), "selected\n").expect("write selected file");
        let cwd_file = Path::new("Cargo.toml");
        assert!(cwd_file.is_file(), "expected crate-root Cargo.toml");
        assert!(!project.path().join(cwd_file).exists());

        let candidates = vec![
            CandidateHit::with_source("LICENSE", None, 1.0, CandidateSource::Lexical),
            CandidateHit::with_source("Cargo.toml", None, 0.5, CandidateSource::Lexical),
        ];
        let ordered = ordered_sidecar_candidates(&candidates, |candidate| {
            candidate_path_resolvable(project.path(), &candidate.file_path)
        });

        assert_eq!(ordered[0].1.file_path, "LICENSE");
        assert!(ordered[0].2);
        assert_eq!(ordered[1].1.file_path, "Cargo.toml");
        assert!(!ordered[1].2);
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

    /// Moved from tests/retrieval_primary_rejection.rs: phantom-only results
    /// must hit the runtime's typed refusal path, and a real file must reach
    /// the ordinary resolution path instead.
    #[test]
    fn phantom_only_candidates_are_detected() {
        let phantoms = vec![
            CandidateHit::with_source(
                "lexical:handler",
                Some("handler".into()),
                0.5,
                CandidateSource::Lexical,
            ),
            CandidateHit::with_source(
                "semantic:handler",
                Some("handler".into()),
                0.55,
                CandidateSource::Semantic,
            ),
        ];
        assert!(codestory_retrieval::phantom_sidecar_candidates_only(
            &phantoms
        ));

        let node_names = HashMap::new();
        for phantom in &phantoms {
            assert_eq!(
                candidate_resolution_label(None, None, &node_names, phantom),
                "phantom_hit",
                "phantom candidates must be refused before any path or storage work"
            );
        }
        let ordered = ordered_sidecar_candidates(&phantoms, |_| {
            panic!("phantom hits must be filtered before path resolution")
        });
        assert!(ordered.is_empty());

        // Real-file control: an existing source file is not a phantom, reaches
        // path resolution, and then proceeds into node lookup.
        let temp = tempfile::tempdir().expect("project root");
        let source = temp.path().join("src/lib.rs");
        std::fs::create_dir_all(source.parent().expect("src parent")).expect("mkdir src");
        std::fs::write(&source, "fn lib() {}\n").expect("write source");
        let real = CandidateHit::lexical_stub("src/lib.rs", 0.9);
        assert!(!codestory_retrieval::phantom_sidecar_candidates_only(
            std::slice::from_ref(&real)
        ));

        let storage = Store::new_in_memory().expect("storage");
        assert_eq!(
            candidate_resolution_label(Some(temp.path()), Some(&storage), &node_names, &real),
            "node_unresolved",
            "a real resolvable file must reach node lookup, not the phantom refusal"
        );
        assert_eq!(ordered_sidecar_candidates(&[real], |_| true).len(), 1);
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
            &[candidate.clone()],
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
    fn ordinary_search_resolution_is_not_limited_by_packet_admission() {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};
        use codestory_store::{FileInfo, FileRole};

        let project = tempfile::tempdir().expect("isolated source project");
        let path = project.path().join("src/lib.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            (2..=21)
                .map(|id| format!("fn source_{id}() {{}}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let mut storage = Store::new_in_memory().expect("storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: path.clone(),
                language: "rust".into(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 20,
                file_role: FileRole::Source,
            })
            .unwrap();
        let mut nodes = vec![codestory_contracts::graph::Node {
            id: CoreNodeId(1),
            kind: NodeKind::FILE,
            serialized_name: path.to_string_lossy().into_owned(),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(1),
            ..Default::default()
        }];
        let candidates = (2..=21)
            .map(|id| {
                let name = format!("source_{id}");
                nodes.push(codestory_contracts::graph::Node {
                    id: CoreNodeId(id),
                    kind: NodeKind::FUNCTION,
                    serialized_name: name.clone(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some((id - 1) as u32),
                    ..Default::default()
                });
                let mut candidate = CandidateHit::with_source(
                    "src/lib.rs",
                    Some(name),
                    1.0 - id as f32 / 100.0,
                    CandidateSource::Lexical,
                );
                candidate.node_id = Some(id.to_string());
                candidate.source_bytes_upper_bound = Some(64);
                candidate
            })
            .collect::<Vec<_>>();
        storage.insert_nodes_batch(&nodes).unwrap();

        assert!(crate::agent::packet_candidate::active_packet_proof_session().is_none());
        for requested in [1, 16, 20] {
            let ordinary = resolve_sidecar_candidates_in_storage(
                &storage,
                &HashMap::new(),
                project.path(),
                &candidates,
                requested,
            )
            .expect("ordinary sidecar resolution");
            assert_eq!(
                ordinary.resolved_hits.len(),
                requested,
                "ordinary resolution must honor its own result window"
            );
            assert_eq!(
                ordinary
                    .resolved_hits
                    .iter()
                    .map(|hit| hit.node_id.0.clone())
                    .collect::<Vec<_>>(),
                (2..2 + requested)
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
            );
            assert_eq!(ordinary.unresolved_candidate_count, 0);
        }

        let session = Rc::new(PacketProofSession::new());
        let _guard = install_packet_proof_session(Rc::clone(&session));
        let packet = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            project.path(),
            &candidates,
            20,
        )
        .expect("bounded packet resolution");
        assert_eq!(
            packet.resolved_hits.len(),
            codestory_contracts::compilation::INTERIM_MAX_ADMITTED_CANDIDATES
        );
        assert_eq!(
            session.receipts().len(),
            codestory_contracts::compilation::INTERIM_MAX_ADMITTED_CANDIDATES
        );
        assert_eq!(
            packet
                .resolved_hits
                .iter()
                .map(|hit| hit.node_id.0.clone())
                .collect::<Vec<_>>(),
            (2..18).map(|id| id.to_string()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn sealed_path_descriptors_keep_file_identity_and_exact_path_lookup() {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};
        use codestory_store::{FileInfo, FileRole};

        let project = tempfile::tempdir().expect("isolated project");
        let mut storage = Store::new_in_memory().expect("storage");
        for (id, path) in [
            (1, project.path().join("src/first.rs")),
            (4, PathBuf::from("src/second.rs")),
            (7, PathBuf::from("src/main/java/Entry.java")),
        ] {
            storage
                .insert_file(&FileInfo {
                    id,
                    path: path.clone(),
                    language: "rust".into(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 20,
                    file_role: FileRole::Source,
                })
                .unwrap();
            storage
                .insert_nodes_batch(&[
                    codestory_contracts::graph::Node {
                        id: CoreNodeId(id),
                        kind: NodeKind::FILE,
                        serialized_name: path.to_string_lossy().into_owned(),
                        file_node_id: Some(CoreNodeId(id)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                    codestory_contracts::graph::Node {
                        id: CoreNodeId(id + 1),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "first_callable".into(),
                        file_node_id: Some(CoreNodeId(id)),
                        start_line: Some(2),
                        end_line: Some(3),
                        ..Default::default()
                    },
                    codestory_contracts::graph::Node {
                        id: CoreNodeId(id + 2),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "later_callable".into(),
                        file_node_id: Some(CoreNodeId(id)),
                        start_line: Some(10),
                        end_line: Some(12),
                        ..Default::default()
                    },
                ])
                .unwrap();
        }
        for (path, file_id) in [("./src//first.rs", 1), ("src/second.rs", 4)] {
            for symbol in [None, Some("later_callable")] {
                let mut file_candidate = CandidateHit::with_source(
                    path,
                    symbol.map(str::to_owned),
                    1.0,
                    CandidateSource::Lexical,
                );
                file_candidate.source_bytes_upper_bound = Some(64);
                let mut canonical = CandidateHit::with_source(
                    path,
                    Some("later_callable".into()),
                    0.8,
                    CandidateSource::Lexical,
                );
                canonical.node_id = Some((file_id + 2).to_string());
                canonical.source_bytes_upper_bound = Some(64);
                let candidates = [file_candidate, canonical];
                let session = Rc::new(PacketProofSession::new());
                admit_packet_candidate_descriptors(&session, candidates.iter());
                assert!(session.retrieval_admission_is_sealed());
                let receipts = session.receipts();
                assert_eq!(receipts.len(), 2);
                assert_eq!(
                    receipts[0].stable_identity,
                    format!("path:{}", path.trim_start_matches("./").replace("//", "/"))
                );
                assert_eq!(receipts[1].stable_identity, format!("node:{}", file_id + 2));
                let _guard = install_packet_proof_session(Rc::clone(&session));
                let outcome = resolve_sidecar_candidates_in_storage(
                    &storage,
                    &HashMap::new(),
                    project.path(),
                    &candidates,
                    2,
                )
                .expect("resolve sealed file and canonical node descriptors");
                assert_eq!(outcome.resolved_hits.len(), 2);
                assert_eq!(outcome.resolved_hits[0].node_id.0, file_id.to_string());
                assert_eq!(outcome.resolved_hits[0].kind, ApiNodeKind::FILE);
                assert_eq!(
                    outcome.resolved_hits[1].node_id.0,
                    (file_id + 2).to_string()
                );
                assert_eq!(outcome.resolved_hits[1].kind, ApiNodeKind::FUNCTION);
                assert!(outcome.packet_hits[0].graph.is_none());
                assert!(outcome.packet_hits[0].graph_provenance.is_empty());
                assert_eq!(session.receipts(), receipts);
            }
        }
        // A metadata path cannot select a similarly spelled source-root alias.
        let mut alias =
            CandidateHit::with_source("main/java/Entry.java", None, 1.0, CandidateSource::Lexical);
        alias.source_bytes_upper_bound = Some(64);
        let session = Rc::new(PacketProofSession::new());
        admit_packet_candidate_descriptors(&session, [&alias]);
        let _guard = install_packet_proof_session(Rc::clone(&session));
        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            project.path(),
            &[alias],
            1,
        )
        .expect("unresolved exact path remains a diagnostic");
        assert!(outcome.resolved_hits.is_empty());
        assert_eq!(outcome.unresolved_candidate_count, 1);
        assert_eq!(
            session.receipts()[0].stable_identity,
            "path:main/java/Entry.java"
        );
    }

    #[test]
    fn seventeenth_packet_identity_is_rejected_before_core_hydration() {
        use crate::agent::packet_candidate::{
            PacketAdmissionDecision, PacketProofSession, install_packet_proof_session,
        };

        let storage_root = tempfile::tempdir().expect("storage root");
        let storage_path = storage_root.path().join("codestory.db");
        let storage = Store::open(&storage_path).expect("storage");
        let poison = rusqlite::Connection::open(&storage_path).expect("poison connection");
        poison
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DROP TABLE edge;
                 DROP TABLE node;
                 DROP TABLE file;",
            )
            .expect("remove every core hydration table");
        drop(poison);
        let session = Rc::new(PacketProofSession::new());
        for index in 0..codestory_contracts::compilation::INTERIM_MAX_ADMITTED_CANDIDATES {
            assert_eq!(
                session.admit(
                    &format!("node:{index}"),
                    codestory_contracts::compilation::INTERIM_SOURCE_ROW_UPPER_BOUND,
                ),
                PacketAdmissionDecision::Admitted
            );
        }
        let _guard = install_packet_proof_session(Rc::clone(&session));
        let mut seventeenth = CandidateHit::with_source(
            "src/never-opened.rs",
            Some("NeverOpened".to_string()),
            1.0,
            CandidateSource::Lexical,
        );
        seventeenth.node_id = Some("17".to_string());
        seventeenth.source_bytes_upper_bound =
            Some(codestory_contracts::compilation::INTERIM_SOURCE_ROW_UPPER_BOUND as u32);

        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[seventeenth],
            1,
        )
        .expect("count rejection is diagnostic, not a storage failure");

        assert!(outcome.resolved_hits.is_empty());
        assert!(outcome.packet_hits.is_empty());
        assert_eq!(outcome.unresolved_candidate_count, 1);
        assert_eq!(*session.hydrated_admissions.borrow(), 16);
    }

    #[test]
    fn published_call_admission_skips_unaffordable_rank_without_refunds() {
        let mut rows = (1..=24)
            .map(|id| {
                let mut hit = CandidateHit::lexical_stub("src/opaque", 1.0);
                hit.node_id = Some(id.to_string());
                hit.source_bytes_upper_bound = Some(512);
                hit.packet_descriptor().unwrap()
            })
            .collect::<Vec<_>>();
        let session = crate::agent::packet_candidate::PacketProofSession::new();
        let initial = session.remaining_source_bytes();
        rows[0].source_bytes_upper_bound = Some((initial + 1) as u32);
        let ordered = published_call_admission_order(&rows, &HashSet::new(), Some("h"));
        admit_published_call_descriptors(&session, &ordered);
        assert_eq!(
            session
                .receipts()
                .iter()
                .map(|r| r.stable_identity.clone())
                .collect::<Vec<_>>(),
            (2..=17).map(|id| format!("node:{id}")).collect::<Vec<_>>()
        );
        assert_eq!(session.remaining_source_bytes(), initial - 16 * 512);
        assert_eq!(session.remaining_hydration_slots(), 0);
        assert!(session.gaps().iter().any(|g| g.kind
            == codestory_contracts::compilation::PacketAdmissionGapKindV1::SourceBudgetExceeded));
    }

    #[test]
    fn published_call_hints_cannot_evict_feasible_retrieval_prefix() {
        use codestory_contracts::compilation::*;
        let mut rows = (1..=24)
            .map(|id| {
                let mut hit = CandidateHit::lexical_stub("src/opaque", 1.0);
                hit.node_id = Some(id.to_string());
                hit.source_bytes_upper_bound = Some(512);
                hit.packet_descriptor().unwrap()
            })
            .collect::<Vec<_>>();
        for (seed, target) in [(0, 20), (1, 23), (2, 24)] {
            rows[seed].published_calls = Some(PacketPublishedCallsV1 {
                sidecar_input_hash: "sealed".into(),
                neighborhood: PacketCallNeighborhoodV1 {
                    schema_version: 1,
                    source_identity: rows[seed].stable_identity.clone(),
                    truncated: true,
                    targets: vec![PacketCallTargetV1 {
                        stable_identity: format!("node:{target}"),
                        representative_edge_id: target.to_string(),
                        callsite_count: 32,
                        certainty: PacketRelationCertaintyV1::Uncertain,
                        minimum_confidence: Some(0.2),
                    }],
                },
            });
        }
        let session = crate::agent::packet_candidate::PacketProofSession::new();
        let ordered = published_call_admission_order(&rows, &HashSet::new(), Some("sealed"));
        admit_published_call_descriptors(&session, &ordered);
        assert_eq!(
            session
                .receipts()
                .iter()
                .map(|r| r.stable_identity.clone())
                .collect::<Vec<_>>(),
            (1..=16).map(|id| format!("node:{id}")).collect::<Vec<_>>()
        );
        assert!(
            ordered[0]
                .published_calls
                .as_ref()
                .unwrap()
                .neighborhood
                .truncated
        );
    }

    #[test]
    fn published_call_partial_positive_hints_and_zero_gain_rank_fallback() {
        use codestory_contracts::compilation::*;
        let make = |id: u32| {
            let mut h = CandidateHit::lexical_stub("src/opaque", 1.0);
            h.node_id = Some(id.to_string());
            h.source_bytes_upper_bound = Some(512);
            h.packet_descriptor().unwrap()
        };
        let mut rows = (1..=24).map(make).collect::<Vec<_>>();
        let target = |id: u32| PacketCallTargetV1 {
            stable_identity: format!("node:{id}"),
            representative_edge_id: id.to_string(),
            callsite_count: 1,
            certainty: PacketRelationCertaintyV1::Uncertain,
            minimum_confidence: Some(0.2),
        };
        rows[0].published_calls = Some(PacketPublishedCallsV1 {
            sidecar_input_hash: "h".into(),
            neighborhood: PacketCallNeighborhoodV1 {
                schema_version: 1,
                source_identity: "node:1".into(),
                truncated: true,
                targets: vec![target(20)],
            },
        });
        let positive = published_call_admission_order(&rows, &HashSet::new(), Some("h"));
        assert_eq!(positive[8].stable_identity, "node:9");
        assert!(
            positive[0]
                .published_calls
                .as_ref()
                .unwrap()
                .neighborhood
                .truncated,
            "incomplete coverage remains explicit"
        );
        rows[0]
            .published_calls
            .as_mut()
            .unwrap()
            .neighborhood
            .targets = vec![target(2), target(20)];
        // Lexicographic target identities, including a prefix-contained target.
        assert_eq!(
            published_call_admission_order(&rows, &HashSet::new(), Some("h"))[8].stable_identity,
            "node:9",
            "already covered seed cannot displace higher-ranked nonneighbors"
        );
        rows[0]
            .published_calls
            .as_mut()
            .unwrap()
            .neighborhood
            .targets
            .clear();
        assert_eq!(
            published_call_admission_order(&rows, &HashSet::new(), Some("h")),
            rows,
            "empty partial hints preserve original rank"
        );
    }

    #[test]
    fn published_call_seed_coverage_resists_hubs_and_symbol_spelling() {
        use codestory_contracts::compilation::*;
        let make = |id: u32| {
            let mut hit = CandidateHit::lexical_stub("src/anonymous.rs", 1.0);
            hit.node_id = Some(id.to_string());
            hit.source_bytes_upper_bound = Some(512);
            hit.packet_descriptor().unwrap()
        };
        let mut rows = (1..=24).map(make).collect::<Vec<_>>();
        let targets = |ids: Vec<u32>| {
            let mut t = ids
                .into_iter()
                .map(|id| PacketCallTargetV1 {
                    stable_identity: format!("node:{id}"),
                    representative_edge_id: id.to_string(),
                    callsite_count: 1,
                    certainty: PacketRelationCertaintyV1::Probable,
                    minimum_confidence: Some(0.5),
                })
                .collect::<Vec<_>>();
            t.sort_by(|a, b| a.stable_identity.cmp(&b.stable_identity));
            t
        };
        for (i, ids) in [(0, (9..=24).collect()), (1, vec![23]), (2, vec![24])] {
            rows[i].published_calls = Some(PacketPublishedCallsV1 {
                sidecar_input_hash: "sealed".into(),
                neighborhood: PacketCallNeighborhoodV1 {
                    schema_version: 1,
                    source_identity: rows[i].stable_identity.clone(),
                    truncated: false,
                    targets: targets(ids),
                },
            });
        }
        let ordered = published_call_admission_order(&rows, &HashSet::new(), Some("sealed"));
        assert_eq!(ordered[8].stable_identity, "node:9");
        assert_eq!(ordered[9].stable_identity, "node:10");
        let identities = ordered
            .iter()
            .map(|d| d.stable_identity.clone())
            .collect::<Vec<_>>();
        for row in &mut rows {
            row.symbol = Some("unrelated opaque spelling".into());
            row.path = "other/language/fixture.xyz".into();
            if let Some(m) = row.published_calls.as_mut() {
                for t in &mut m.neighborhood.targets {
                    t.callsite_count = 32;
                }
            }
        }
        assert_eq!(
            published_call_admission_order(&rows, &HashSet::new(), Some("sealed"))
                .iter()
                .map(|d| d.stable_identity.clone())
                .collect::<Vec<_>>(),
            identities,
            "selection must not inspect names, paths, language, or repeated callsite counts"
        );
    }

    #[test]
    fn published_call_late_exact_owner_retains_annotation_after_count_break() {
        use codestory_contracts::compilation::*;
        let rows = (1..=24)
            .map(|id| {
                let mut hit = CandidateHit::lexical_stub("src/opaque", 1.0);
                hit.node_id = Some(id.to_string());
                hit.source_bytes_upper_bound = Some(512);
                hit.packet_descriptor().unwrap()
            })
            .collect::<Vec<_>>();
        let mut rows = rows;
        rows[23].published_calls = Some(PacketPublishedCallsV1 {
            sidecar_input_hash: "h".into(),
            neighborhood: PacketCallNeighborhoodV1 {
                schema_version: 1,
                source_identity: "node:24".into(),
                truncated: true,
                targets: vec![PacketCallTargetV1 {
                    stable_identity: "node:15".into(),
                    representative_edge_id: "1".into(),
                    callsite_count: 1,
                    certainty: PacketRelationCertaintyV1::Probable,
                    minimum_confidence: None,
                }],
            },
        });
        let session = crate::agent::packet_candidate::PacketProofSession::new();
        assert_eq!(
            session.admit_exact_selector("node:24", 1, 0),
            PacketAdmissionDecision::Admitted
        );
        let ordered =
            published_call_admission_order(&rows, &HashSet::from(["node:24".into()]), Some("h"));
        let mut probe_count = 1;
        let mut rejection = None;
        for (i, row) in ordered.iter().enumerate() {
            if row.stable_identity == "node:24" {
                continue;
            }
            if probe_count == 16 {
                rejection = Some(i);
                break;
            }
            probe_count += 1;
        }
        assert!(
            rejection.unwrap()
                < ordered
                    .iter()
                    .position(|r| r.stable_identity == "node:24")
                    .unwrap(),
            "negative control must put exact metadata behind the first rejected new ID"
        );
        admit_published_call_descriptors(&session, &ordered);
        assert_eq!(session.receipts().len(), 16);
        assert!(
            session
                .receipts()
                .iter()
                .any(|r| r.stable_identity == "node:24"
                    && r.origin == PacketAdmissionOriginV1::ExactTypedSelector)
        );
        let links = session.admitted_call_navigation();
        assert!(links.iter().any(|l| l.from_identity == "node:24"
            && l.to_identity == "node:15"
            && l.certainty == PacketRelationCertaintyV1::Probable));
    }

    #[test]
    fn published_call_admission_falls_back_and_preserves_conflicts_and_exact_slots() {
        use codestory_contracts::compilation::*;
        let make = |id: u32| {
            let mut hit = CandidateHit::lexical_stub("src/worker.rs", 1.0);
            hit.node_id = Some(id.to_string());
            hit.source_bytes_upper_bound = Some(512);
            hit.packet_descriptor().unwrap()
        };
        let mut rows = (1..=24).map(make).collect::<Vec<_>>();
        let metadata = |target: u32| PacketPublishedCallsV1 {
            sidecar_input_hash: "bound".into(),
            neighborhood: PacketCallNeighborhoodV1 {
                schema_version: PACKET_CALL_DESCRIPTOR_SCHEMA_V1,
                source_identity: "node:1".into(),
                truncated: false,
                targets: vec![PacketCallTargetV1 {
                    stable_identity: format!("node:{target}"),
                    representative_edge_id: "1".into(),
                    callsite_count: 1,
                    certainty: PacketRelationCertaintyV1::Unknown,
                    minimum_confidence: None,
                }],
            },
        };
        let none = HashSet::new();
        assert_eq!(
            published_call_admission_order(&rows, &none, Some("bound")),
            rows
        );
        rows[0].published_calls = Some(metadata(20));
        let ordered = published_call_admission_order(&rows, &none, Some("bound"));
        assert_eq!(ordered[8].stable_identity, "node:9");
        assert_eq!(
            published_call_admission_order(&rows, &none, Some("wrong"))[8].stable_identity,
            "node:9"
        );
        assert!(
            published_call_admission_order(&rows, &none, Some("wrong"))[0]
                .published_calls
                .is_none()
        );
        for invalid in 0..2 {
            let mut skew = rows.clone();
            let m = skew[0].published_calls.as_mut().unwrap();
            match invalid {
                0 => m.neighborhood.source_identity = "node:2".into(),
                _ => m.neighborhood.targets[0].stable_identity = "node:999".into(),
            }
            if invalid == 0 {
                assert!(
                    published_call_admission_order(&skew, &none, Some("bound"))[0]
                        .published_calls
                        .is_none()
                );
            }
            assert_eq!(
                published_call_admission_order(&skew, &none, Some("bound"))[8].stable_identity,
                "node:9"
            );
        }
        let mut duplicate = rows[0].clone();
        duplicate.published_calls = Some(metadata(21));
        let conflicted =
            published_call_admission_order(&[rows[0].clone(), duplicate], &none, Some("bound"));
        assert!(conflicted[0].published_calls_conflicted);
        assert!(conflicted[0].published_calls.is_none());
        let twice = published_call_admission_order(
            &[conflicted[0].clone(), rows[0].clone()],
            &none,
            Some("bound"),
        );
        assert!(twice[0].published_calls_conflicted);
        assert!(twice[0].published_calls.is_none());
        let exact = (1..=8)
            .map(|id| format!("node:{id}"))
            .collect::<HashSet<_>>();
        let after_exact = published_call_admission_order(&rows, &exact, Some("bound"));
        assert_eq!(
            after_exact[..4]
                .iter()
                .map(|d| d.stable_identity.as_str())
                .collect::<Vec<_>>(),
            vec!["node:9", "node:10", "node:11", "node:12"]
        );
        assert_eq!(after_exact[4].stable_identity, "node:13");
        let session = crate::agent::packet_candidate::PacketProofSession::new();
        for id in 1..=8 {
            assert_eq!(
                session.admit_exact_selector(&format!("node:{id}"), 1, id),
                PacketAdmissionDecision::Admitted
            );
        }
        for descriptor in after_exact {
            if matches!(
                session.admit_descriptor(&descriptor),
                PacketAdmissionDecision::CountBudgetExceeded
            ) {
                break;
            }
        }
        assert_eq!(session.receipts().len(), 16);
        assert!(
            session.receipts()[..8]
                .iter()
                .all(|r| r.origin == PacketAdmissionOriginV1::ExactTypedSelector)
        );
    }

    #[test]
    fn packet_admission_consumes_versioned_retrieval_score_order() {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};

        let storage = Store::new_in_memory().expect("storage");
        let session = Rc::new(PacketProofSession::new());
        let _guard = install_packet_proof_session(Rc::clone(&session));
        let mut low = CandidateHit::with_source(
            "src/low.rs",
            Some("Low".to_string()),
            0.1,
            CandidateSource::Lexical,
        );
        low.node_id = Some("1".to_string());
        low.source_bytes_upper_bound = Some(64);
        let mut high = CandidateHit::with_source(
            "src/high.rs",
            Some("High".to_string()),
            0.9,
            CandidateSource::Lexical,
        );
        high.node_id = Some("2".to_string());
        high.source_bytes_upper_bound = Some(64);

        let expected_score_version = high.packet_descriptor().unwrap().retrieval_score.version;

        let _ = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[low, high],
            1,
        )
        .expect("missing core rows are diagnostic after descriptor admission");

        assert_eq!(session.receipts().len(), 1);
        assert_eq!(session.receipts()[0].stable_identity, "node:2");
        assert_eq!(session.receipts()[0].score_version, expected_score_version);
    }

    #[test]
    fn descriptor_admission_ranks_candidates_across_query_batches_before_sealing() {
        use crate::agent::packet_candidate::{PacketAdmissionDecision, PacketProofSession};

        let session = PacketProofSession::new();
        for index in 0..15 {
            assert_eq!(
                session.admit_exact_selector(&format!("node:exact-{index}"), 1, index),
                PacketAdmissionDecision::Admitted
            );
        }
        let mut low = CandidateHit::with_source(
            "src/low.rs",
            Some("Low".to_string()),
            0.1,
            CandidateSource::Lexical,
        );
        low.node_id = Some("1".to_string());
        low.source_bytes_upper_bound = Some(64);
        let mut high = CandidateHit::with_source(
            "src/high.rs",
            Some("High".to_string()),
            0.9,
            CandidateSource::Semantic,
        );
        high.node_id = Some("2".to_string());
        high.source_bytes_upper_bound = Some(64);

        admit_packet_candidate_descriptors(&session, [&low, &high]);

        assert_eq!(session.receipts().len(), 16);
        assert_eq!(session.receipts()[15].stable_identity, "node:2");
        assert_eq!(
            session.admit_descriptor(&low.packet_descriptor().expect("complete low descriptor")),
            PacketAdmissionDecision::CountBudgetExceeded,
            "the sealed session must not admit a late lower-scoring query candidate"
        );
    }

    #[test]
    fn descriptor_admission_records_private_raf_copresence_observation() {
        use crate::agent::packet_batch::{
            enter_packet_latency_scope, enter_packet_public_operation_observation,
            packet_operation_observation_for_test, raf_identity_digest_for_test,
        };
        use crate::agent::packet_candidate::PacketProofSession;

        let session = PacketProofSession::new();
        let mut low = CandidateHit::with_source(
            "src/low.rs",
            Some("Low".to_string()),
            0.1,
            CandidateSource::Lexical,
        );
        low.node_id = Some("1".to_string());
        low.source_bytes_upper_bound = Some(64);
        let mut high = CandidateHit::with_source(
            "src/high.rs",
            Some("High".to_string()),
            0.9,
            CandidateSource::Semantic,
        );
        high.node_id = Some("2".to_string());
        high.source_bytes_upper_bound = Some(64);

        let _latency = enter_packet_latency_scope(Some(2_000));
        {
            let _packet = enter_packet_public_operation_observation("packet");
            admit_packet_candidate_descriptors(&session, [&low, &high]);
        }

        let observed = packet_operation_observation_for_test()
            .expect("owned packet observation retains raf digests");
        assert_eq!(observed.raf_ranked_admitted_observed_count, 1);
        assert_eq!(observed.raf_ranked_pool_count, 2);
        assert_eq!(observed.raf_ranked_recorded_count, 2);
        assert_eq!(observed.raf_admitted_count, 2);
        assert_eq!(
            observed.raf_ranked_identity_digests,
            [
                raf_identity_digest_for_test("node:2"),
                raf_identity_digest_for_test("node:1"),
            ]
            .join(",")
        );
        assert_eq!(
            observed.raf_admitted_identity_digests,
            [
                raf_identity_digest_for_test("node:2"),
                raf_identity_digest_for_test("node:1"),
            ]
            .join(",")
        );
        assert_eq!(session.receipts()[0].stable_identity, "node:2");
        assert_eq!(session.receipts()[1].stable_identity, "node:1");
    }

    #[test]
    fn packet_descriptor_zero_remaining_budget_expires_before_readiness() {
        use crate::agent::packet_batch::{
            enter_packet_latency_scope, enter_packet_public_operation_observation,
            packet_operation_observation_for_test,
        };
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};

        let fixture = pinned_operation_fixture();
        let pinned = Rc::new(
            PinnedRetrievalRead::begin_packet_descriptor(&fixture.controller)
                .expect("begin packet descriptor pin"),
        );
        let session = Rc::new(PacketProofSession::new());

        with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
            let _latency = enter_packet_latency_scope(Some(1_000));
            let _packet = enter_packet_public_operation_observation("packet");
            let _guard = install_packet_proof_session(Rc::clone(&session));
            let error = preadmit_packet_descriptor_queries(&fixture.controller, &[], Some(0))
                .expect_err("zero remaining allowance must expire before readiness");
            assert_eq!(error.code, "cache_busy");
            assert_eq!(
                error.message,
                "retrieval stopped: reason=deadline stage=deferred_full_readiness_entry"
            );
            let observed = packet_operation_observation_for_test()
                .expect("zero-budget empty path still records runtime phase walls");
            assert_eq!(observed.descriptor_preadmission_observed_count, 0);
            assert_eq!(
                observed.descriptor_preadmit_runtime_phases_observed_count,
                1
            );
            assert_eq!(observed.descriptor_query_batch_wall_ms, 0);
            assert_eq!(observed.descriptor_lexical_wall_ms, 0);
            assert_eq!(observed.descriptor_dense_semantic_wall_ms, 0);
        });
    }

    #[test]
    fn packet_descriptor_subphase_walls_populate_on_successful_preadmit() {
        use crate::agent::packet_batch::{
            enter_packet_latency_scope, enter_packet_public_operation_observation,
            observe_packet_descriptor_remaining_before_handoff,
            packet_operation_observation_for_test,
        };
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};

        let fixture = pinned_operation_fixture();
        let pinned = Rc::new(
            PinnedRetrievalRead::begin_packet_descriptor(&fixture.controller)
                .expect("begin packet descriptor pin"),
        );
        let session = Rc::new(PacketProofSession::new());
        let query = "renamed lattice transition";

        with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
            let _latency = enter_packet_latency_scope(Some(18_000));
            let _packet = enter_packet_public_operation_observation("packet");
            let _guard = install_packet_proof_session(Rc::clone(&session));
            preadmit_packet_descriptor_queries(&fixture.controller, &[query.to_string()], None)
                .expect("successful descriptor preadmission");
            observe_packet_descriptor_remaining_before_handoff(12_000);

            let observed = packet_operation_observation_for_test()
                .expect("owned packet observation retains descriptor sub-phase walls");
            assert_eq!(observed.descriptor_preadmission_observed_count, 1);
            assert_eq!(observed.descriptor_preadmission_query_count, 1);
            assert_eq!(
                observed.descriptor_preadmit_runtime_phases_observed_count,
                0
            );
            assert!(
                observed.descriptor_query_batch_wall_ms > 0
                    || observed.descriptor_lexical_wall_ms > 0
                    || observed.descriptor_health_resolution_wall_ms > 0,
                "successful preadmit must retain at least one measured descriptor wall"
            );
            assert_eq!(observed.descriptor_remaining_before_handoff_ms, 12_000);
        });
    }

    #[test]
    fn same_text_policy_results_retain_and_resolve_in_both_orders_and_refuse_sealed_miss() {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};
        for order in [
            [
                LexicalAdmissionPolicy::StrictLiteralV1,
                LexicalAdmissionPolicy::BroadQuestionV1,
            ],
            [
                LexicalAdmissionPolicy::BroadQuestionV1,
                LexicalAdmissionPolicy::StrictLiteralV1,
            ],
        ] {
            let fixture = pinned_operation_fixture();
            let pinned = Rc::new(
                PinnedRetrievalRead::begin_packet_descriptor(&fixture.controller).expect("pin"),
            );
            let session = Rc::new(PacketProofSession::new());
            let query = "renamed lattice transition";
            let requests = order.map(|lexical_policy| PacketRetrievalQuery {
                query: query.to_string(),
                lexical_policy,
            });
            with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
                let _guard = install_packet_proof_session(Rc::clone(&session));
                preadmit_packet_descriptor_queries_with_policy(
                    &fixture.controller,
                    &requests,
                    None,
                )
                .expect("typed preadmit");
                assert!(session.retrieval_admission_is_sealed());
                assert_eq!(
                    session.benchmark_retrieval_proof().descriptor_query_count,
                    2
                );
                for policy in order {
                    let (result, _) = run_and_resolve_sidecar_query_with_policy(
                        &fixture.controller,
                        query,
                        4,
                        None,
                        policy,
                    )
                    .expect("retained typed resolution");
                    assert_eq!(result.features.lexical_policy, policy);
                    let wrong_policy = if policy == LexicalAdmissionPolicy::StrictLiteralV1 {
                        LexicalAdmissionPolicy::BroadQuestionV1
                    } else {
                        LexicalAdmissionPolicy::StrictLiteralV1
                    };
                    assert!(
                        PacketProofSession::new()
                            .retain_descriptor_result_with_policy(
                                &result,
                                wrong_policy,
                                true,
                                pinned.session.publication_identity(),
                            )
                            .is_err(),
                        "expected policy must authenticate actual execution"
                    );
                }
                assert_eq!(
                    session.benchmark_retrieval_proof().descriptor_query_count,
                    2
                );
                assert!(
                    run_and_resolve_sidecar_query_with_policy(
                        &fixture.controller,
                        "not preadmitted",
                        4,
                        None,
                        LexicalAdmissionPolicy::BroadQuestionV1,
                    )
                    .is_err()
                );
                assert!(
                    search_sidecar_packet_batch_inner(
                        &fixture.controller,
                        &[("not preadmitted".into(), 4)],
                        None,
                    )
                    .is_err(),
                    "strict free-query misses also refuse a post-seal rerun"
                );
                assert_eq!(
                    session.benchmark_retrieval_proof().descriptor_query_count,
                    2
                );
            });
        }
    }

    #[test]
    fn preadmitted_descriptor_result_is_reused_by_initial_resolution() {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};

        let fixture = pinned_operation_fixture();
        let pinned = Rc::new(
            PinnedRetrievalRead::begin_packet_descriptor(&fixture.controller)
                .expect("begin packet descriptor pin"),
        );
        let session = Rc::new(PacketProofSession::new());
        let query = "renamed lattice transition";

        with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
            let _guard = install_packet_proof_session(Rc::clone(&session));
            preadmit_packet_descriptor_queries(&fixture.controller, &[query.to_string()], None)
                .expect("preadmit renamed descriptor query");
            assert_eq!(
                session.benchmark_retrieval_proof().descriptor_query_count,
                1,
                "preadmission executes the descriptor query once"
            );

            let (result, _) = run_and_resolve_sidecar_query(&fixture.controller, query, 4, None)
                .expect("resolve the preadmitted descriptor result");
            assert_eq!(result.query, query);
            assert_eq!(
                session.benchmark_retrieval_proof().descriptor_query_count,
                1,
                "initial resolution must reuse the preadmitted result instead of executing retrieval again"
            );

            let publication = pinned.session.publication_identity().clone();
            let mut different_publication = publication.clone();
            different_publication
                .sidecar_generation
                .push_str("-different");
            assert!(
                session
                    .descriptor_result("another query", true, &publication)
                    .is_none()
            );
            assert!(
                session
                    .descriptor_result(query, false, &publication)
                    .is_none()
            );
            assert!(
                session
                    .descriptor_result(query, true, &different_publication)
                    .is_none()
            );
            assert!(
                PacketProofSession::new()
                    .descriptor_result(query, true, &publication)
                    .is_none(),
                "a retained descriptor result is scoped to one packet session"
            );

            let mut mismatched_result = result;
            mismatched_result.publication_identity = Some(different_publication);
            assert!(
                PacketProofSession::new()
                    .retain_descriptor_result(&mismatched_result, true, &publication)
                    .is_err(),
                "the retained result must authenticate the packet's exact publication"
            );
        });
    }

    #[test]
    fn preadmitted_license_descriptor_preserves_the_internal_artifact_classification() {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};

        let fixture = public_packet_fixture_with_unindexed_lexical_artifact();
        let project_root = fixture._project.path();
        let core = Store::open_read_only(&fixture.storage_path).expect("open pinned core");
        let pinned = Rc::new(
            PinnedRetrievalRead::begin_packet_descriptor(&fixture.controller)
                .expect("begin packet descriptor pin"),
        );
        let session = Rc::new(PacketProofSession::new());
        let query =
            "Which role is assumed by the control plane, and which is used by worker nodes?";

        with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
            let _guard = install_packet_proof_session(Rc::clone(&session));
            preadmit_packet_descriptor_queries(&fixture.controller, &[query.to_string()], None)
                .expect("preadmit LICENSE descriptor query");
            let publication = pinned.session.publication_identity().clone();
            let retained = session
                .descriptor_result(query, true, &publication)
                .expect("retain exact preadmitted query result");
            assert_eq!(retained.query, query);
            assert_eq!(retained.publication_identity.as_ref(), Some(&publication));
            assert_eq!(retained.hits.len(), 1);
            let candidate = &retained.hits[0];
            assert_eq!(candidate.file_path, "LICENSE");
            assert_eq!(candidate.source, CandidateSource::Lexical);
            assert!(candidate.node_id.is_none());
            assert!(candidate.symbol_name.is_none());
            assert!(candidate.qualified_name.is_none());
            assert!(candidate.target.is_none());
            assert!(candidate.packet_descriptor().is_some());
            assert!(candidate_is_unindexed_lexical_artifact(
                &core,
                project_root,
                candidate,
            ));

            let (resolved_result, resolution) =
                run_and_resolve_sidecar_query(&fixture.controller, query, 4, None)
                    .expect("resolve the preadmitted LICENSE descriptor result");
            assert_eq!(
                resolved_result.publication_identity.as_ref(),
                Some(&publication)
            );
            assert_eq!(resolved_result.hits.len(), 1);
            assert_eq!(
                session.benchmark_retrieval_proof().descriptor_query_count,
                1
            );
            assert!(resolution.resolved_hits.is_empty());
            assert!(resolution.packet_hits.is_empty());
            assert_eq!(resolution.unresolved_candidate_count, 1);
            assert_eq!(resolution.blocking_unresolved_candidate_count, 0);
            assert_eq!(resolution.attempted_candidate_indices, HashSet::from([0]));
            assert!(
                resolution.unindexed_lexical_artifacts_only,
                "successful descriptor admission must preserve the narrow unindexed-artifact classification"
            );
        });
    }

    #[test]
    fn packet_admission_rejects_hostile_unindexed_artifact_shapes() {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};

        let fixture = pinned_operation_fixture();
        let project_root = fixture._project.path();
        std::fs::create_dir_all(project_root.join("src")).expect("create source directory");
        std::fs::write(project_root.join("LICENSE"), "license text\n")
            .expect("write extensionless artifact");
        std::fs::write(
            project_root.join("src/unindexed.rs"),
            "pub fn unindexed() {}\n",
        )
        .expect("write parser-routed source");
        let outside = tempfile::tempdir().expect("outside root");
        let outside_file = outside.path().join("FOREIGN");
        std::fs::write(&outside_file, "foreign text\n").expect("write foreign file");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_file, project_root.join("ESCAPE"))
            .expect("create escaping symlink");
        let storage = Store::open_read_only(&fixture.storage_path).expect("open pinned core");
        let candidate = |path: String, source: CandidateSource| {
            let mut candidate = CandidateHit::with_source(path, None, 0.8, source);
            candidate.source_bytes_upper_bound = Some(64);
            candidate
        };

        let regular_artifact = candidate("LICENSE".into(), CandidateSource::Lexical);
        {
            let session = Rc::new(PacketProofSession::new());
            let _guard = install_packet_proof_session(Rc::clone(&session));
            let outcome = resolve_sidecar_candidates_in_storage(
                &storage,
                &HashMap::new(),
                project_root,
                std::slice::from_ref(&regular_artifact),
                1,
            )
            .expect("resolve freshly admitted artifact descriptor");
            assert_eq!(session.receipts().len(), 1);
            assert_eq!(outcome.blocking_unresolved_candidate_count, 1);
            assert!(!outcome.unindexed_lexical_artifacts_only);
        }
        {
            let session = Rc::new(PacketProofSession::new());
            session.seal_retrieval_admission();
            let _guard = install_packet_proof_session(session);
            let outcome = resolve_sidecar_candidates_in_storage(
                &storage,
                &HashMap::new(),
                project_root,
                &[regular_artifact],
                1,
            )
            .expect("reject sealed-unadmitted artifact descriptor");
            assert_eq!(outcome.blocking_unresolved_candidate_count, 1);
            assert!(!outcome.unindexed_lexical_artifacts_only);
        }

        let mut rows = vec![
            (
                "parser-routed source",
                true,
                candidate("src/unindexed.rs".into(), CandidateSource::Lexical),
            ),
            (
                "missing path",
                true,
                candidate("MISSING".into(), CandidateSource::Lexical),
            ),
            (
                "foreign path",
                false,
                candidate(
                    outside_file.to_string_lossy().into_owned(),
                    CandidateSource::Lexical,
                ),
            ),
        ];
        #[cfg(unix)]
        rows.push((
            "escaping symlink",
            true,
            candidate("ESCAPE".into(), CandidateSource::Lexical),
        ));
        let mut targeted = candidate("LICENSE".into(), CandidateSource::Lexical);
        targeted.target = Some(SearchTargetDto::FileRange {
            file_path: "LICENSE".into(),
            start_byte: 0,
            end_byte: 7,
        });
        rows.push(("target-bearing artifact", true, targeted));
        let mut symbol = candidate("LICENSE".into(), CandidateSource::Lexical);
        symbol.symbol_name = Some("license".into());
        rows.push(("symbol-bearing artifact", true, symbol));
        let mut semantic = candidate("LICENSE".into(), CandidateSource::Semantic);
        semantic.node_id = Some("999999".into());
        rows.push(("semantic candidate", true, semantic));
        let mut malformed = candidate("LICENSE".into(), CandidateSource::Lexical);
        malformed.node_id = Some("not-an-id".into());
        rows.push(("malformed node identity", false, malformed));

        for (name, preadmitted, candidate) in rows {
            assert_eq!(
                candidate.packet_descriptor().is_some(),
                preadmitted,
                "{name} descriptor eligibility"
            );
            let session = Rc::new(PacketProofSession::new());
            admit_packet_candidate_descriptors(&session, [&candidate]);
            assert_eq!(
                session.receipts().len(),
                if preadmitted { 1 } else { 0 },
                "{name} preadmission receipt"
            );
            let _guard = install_packet_proof_session(session);
            let outcome = resolve_sidecar_candidates_in_storage(
                &storage,
                &HashMap::new(),
                project_root,
                &[candidate],
                1,
            )
            .unwrap_or_else(|error| panic!("{name} resolution failed: {error:?}"));
            assert!(outcome.resolved_hits.is_empty(), "{name}");
            assert_eq!(outcome.unresolved_candidate_count, 1, "{name}");
            assert_eq!(outcome.blocking_unresolved_candidate_count, 1, "{name}");
            assert!(!outcome.unindexed_lexical_artifacts_only, "{name}");
        }
    }

    #[test]
    fn preadmitted_descriptor_result_is_reused_by_planned_query_batch() {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};

        let fixture = pinned_operation_fixture();
        let pinned = Rc::new(
            PinnedRetrievalRead::begin_packet_descriptor(&fixture.controller)
                .expect("begin packet descriptor pin"),
        );
        let session = Rc::new(PacketProofSession::new());
        let query = "renamed lattice continuation";

        with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
            let _guard = install_packet_proof_session(Rc::clone(&session));
            preadmit_packet_descriptor_queries(&fixture.controller, &[query.to_string()], None)
                .expect("preadmit planned descriptor query");
            assert_eq!(
                session.benchmark_retrieval_proof().descriptor_query_count,
                1
            );

            let outcome = search_sidecar_packet_batch_inner(
                &fixture.controller,
                &[(query.to_string(), 4)],
                None,
            )
            .expect("resolve the preadmitted planned query");
            assert_eq!(outcome.results.len(), 1);
            assert_eq!(outcome.results[0].0, query);
            assert_eq!(
                session.benchmark_retrieval_proof().descriptor_query_count,
                1,
                "planned query resolution must reuse the preadmitted result"
            );
        });
    }

    #[test]
    fn fresh_strict_query_executes_outside_packet_proof_session() {
        use codestory_contracts::api::PacketQueryCompletionDto;

        assert!(active_packet_proof_session().is_none());
        let fixture = public_packet_fixture_with_unindexed_lexical_artifact();
        assert!(active_packet_proof_session().is_none());
        let query = "metadata";
        let outcome = search_sidecar_packet_batch_inner(
            &fixture.controller,
            &[(query.into(), 4)],
            Some(18_000),
        )
        .expect("a fresh strict query outside a packet session remains eligible");

        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].0, query);
        assert!(
            !outcome.results[0].1.is_empty(),
            "the fresh fixture must produce real strict-query hits"
        );
        assert_eq!(outcome.diagnostics.len(), 1);
        assert_eq!(outcome.diagnostics[0].query, query);
        assert_eq!(
            outcome.diagnostics[0].completion,
            PacketQueryCompletionDto::Completed
        );
        assert!(outcome.retained_deadline_queries.is_empty());
        assert!(outcome.retryable_queries.is_empty());
        assert!(active_packet_proof_session().is_none());
    }

    #[test]
    fn planned_query_preserves_sealed_deadline_hits_without_phantom_retry() {
        use crate::agent::packet_batch::{PacketLatencyBudget, run_packet_planned_subqueries};
        use crate::agent::packet_budget::packet_budget_limits;
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};
        use crate::agent::packet_plan::build_packet_plan_with_extra;
        use crate::agent::trace::TraceRecorder;
        use codestory_contracts::api::{
            AgentAnswerDto, AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto,
            PacketQueryCompletionDto,
        };

        for reason in ["stage_deadline", "deadline", "cancelled"] {
            let fixture = public_packet_fixture_with_unindexed_lexical_artifact();
            let pinned = Rc::new(
                PinnedRetrievalRead::begin_packet_descriptor(&fixture.controller)
                    .expect("begin packet descriptor pin"),
            );
            let session = Rc::new(PacketProofSession::new());
            let query = "public packet freshness anchor";
            with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
                let _guard = install_packet_proof_session(Rc::clone(&session));
                // Execute real descriptors, then supply a deadline before the admission seal.
                // No already-sealed receipt is changed to create this fault.
                let mut results = with_detached_sidecar_query_cache(&fixture.controller, |cache| {
                    pinned.session.execute_packet_descriptor_batch_with_cache(
                        &[QueryBatchItem {
                            lexical_policy:
                                codestory_retrieval::LexicalAdmissionPolicy::StrictLiteralV1,
                            query,
                            budget_ms: Some(18_000),
                        }],
                        None,
                        cache,
                    )
                })
                .expect("execute complete fixture descriptor");
                let mut result = results.pop().expect("one descriptor query result");
                assert!(results.is_empty());
                assert_eq!(result.trace.retrieval_mode, "full");
                assert!(
                    !result.hits.is_empty(),
                    "deadline fixture must retain real candidates"
                );
                session.record_descriptor_trace(&result.trace);
                result.trace.cancel_reason = Some(reason.into());
                let publication = pinned.session.publication_identity();
                session
                    .retain_descriptor_result(&result, true, publication)
                    .expect("retain deadline result before sealing");
                admit_packet_candidate_descriptors(&session, result.hits.iter());
                assert!(!session.receipts().is_empty());
                let resolution = resolve_sidecar_candidates_in_read(
                    &fixture.controller,
                    &pinned,
                    &result.hits,
                    4,
                )
                .expect("resolve indexed metadata source before planned call");
                assert!(
                    !resolution.packet_hits.is_empty(),
                    "fixture must resolve indexed hits"
                );

                let sealed = serde_json::to_value(&result).expect("serialize sealed result");
                let plan = build_packet_plan_with_extra(
                    "generic question",
                    PacketBudgetModeDto::Standard,
                    &[query.into()],
                );
                let mut answer = AgentAnswerDto {
                    answer_id: "sealed-deadline".into(),
                    prompt: "generic question".into(),
                    summary: String::new(),
                    freshness: None,
                    source_coverage: Vec::new(),
                    focused_source: None,
                    sections: Vec::new(),
                    citations: Vec::new(),
                    subgraph_ids: Vec::new(),
                    retrieval_version: "fixture".into(),
                    graphs: Vec::new(),
                    retrieval_trace: TraceRecorder::new(Some(18_000)).finish(
                        "sealed-deadline".into(),
                        AgentRetrievalPresetDto::Architecture,
                        AgentRetrievalPolicyModeDto::LatencyFirst,
                    ),
                };
                let latency = PacketLatencyBudget::new(Some(18_000));
                assert!(latency.remaining_for_handoff().is_some());
                let mut exhausted_answer = answer.clone();
                let planned = run_packet_planned_subqueries(
                    &fixture.controller,
                    &plan,
                    PacketBudgetModeDto::Standard,
                    &packet_budget_limits(PacketBudgetModeDto::Standard),
                    false,
                    latency,
                    &mut answer,
                );
                if reason == "cancelled" {
                    assert_eq!(
                        planned
                            .expect_err("retained cancellation must propagate")
                            .code,
                        "cancelled"
                    );
                    assert_eq!(
                        session.benchmark_retrieval_proof().descriptor_query_count,
                        1
                    );
                    assert!(answer.citations.is_empty());
                    return;
                }
                planned.expect("resolve retained deadline evidence");
                assert_eq!(
                    session.benchmark_retrieval_proof().descriptor_query_count,
                    1,
                    "a sealed descriptor must not execute retrieval again"
                );
                assert!(
                    !answer.citations.is_empty(),
                    "resolved deadline hits must survive"
                );
                assert_eq!(
                    answer.retrieval_trace.packet_sidecar_diagnostics.len(),
                    1,
                    "a retained deadline is one resolution, not a retry result"
                );
                let diagnostic = &answer.retrieval_trace.packet_sidecar_diagnostics[0];
                assert_eq!(diagnostic.query, query);
                assert_eq!(
                    diagnostic.completion,
                    PacketQueryCompletionDto::Cancelled {
                        reason: reason.into()
                    }
                );
                assert!(diagnostic.resolved_hit_count > 0);
                assert_eq!(answer.retrieval_trace.steps.len(), 1);
                assert!(answer.retrieval_trace.annotations.iter().any(|note|
                    note.text.contains("packet_fused_deadline_retry_omitted reason=sealed_descriptor count=1 retained_hits=1")));

                assert!(
                    answer
                        .retrieval_trace
                        .annotations
                        .iter()
                        .all(|note| !note.text.contains("packet_fused_blocking_cancel_retry")),
                    "sealed result replay must not claim retry execution or exhaustion: {:?}",
                    answer.retrieval_trace.annotations
                );

                run_packet_planned_subqueries(
                    &fixture.controller,
                    &plan,
                    PacketBudgetModeDto::Standard,
                    &packet_budget_limits(PacketBudgetModeDto::Standard),
                    false,
                    PacketLatencyBudget {
                        started_at: Instant::now()
                            .checked_sub(Duration::from_secs(2))
                            .expect("backdate exhausted allowance"),
                        target_ms: 1_000,
                    },
                    &mut exhausted_answer,
                )
                .expect("exhausted allowance must skip before dispatch");
                assert_eq!(
                    session.benchmark_retrieval_proof().descriptor_query_count,
                    1
                );
                assert!(exhausted_answer.citations.is_empty());
                assert!(
                    exhausted_answer
                        .retrieval_trace
                        .packet_sidecar_diagnostics
                        .is_empty()
                );
                assert!(
                    exhausted_answer
                        .retrieval_trace
                        .annotations
                        .iter()
                        .any(|note| note.text.contains(
                            "packet_material_queries skipped reason=latency_budget_exhausted"
                        ))
                );

                let error = search_sidecar_packet_batch_inner(
                    &fixture.controller,
                    &[("metadata".into(), 4)],
                    Some(18_000),
                )
                .expect_err("sealed session must refuse an unretained strict query");
                assert_eq!(error.code, "internal");
                assert_eq!(
                    error.message,
                    "sealed packet is missing a strict free-query result"
                );
                assert_eq!(
                    session.benchmark_retrieval_proof().descriptor_query_count,
                    1,
                    "a sealed miss must not execute another descriptor query"
                );

                assert_eq!(
                    serde_json::to_value(
                        session
                            .descriptor_result(query, true, publication)
                            .expect("sealed descriptor remains available")
                    )
                    .expect("serialize retained result"),
                    sealed
                );
            });
        }
    }

    #[test]
    fn ineligible_sidecar_descriptor_never_exports_an_unauthenticated_identity() {
        use crate::agent::packet_candidate::PacketProofSession;

        let session = PacketProofSession::new();
        let mut malformed = CandidateHit::with_source(
            "src/malformed.rs",
            Some("Malformed".to_string()),
            0.9,
            CandidateSource::Lexical,
        );
        malformed.node_id = Some("not-an-id".to_string());
        malformed.source_bytes_upper_bound = Some(64);
        let mut missing_bound = CandidateHit::with_source(
            "src/missing.rs",
            Some("Missing".to_string()),
            0.8,
            CandidateSource::Lexical,
        );
        missing_bound.node_id = Some("42".to_string());

        admit_packet_candidate_descriptors(&session, [&malformed, &missing_bound]);

        assert!(session.receipts().is_empty());
        assert_eq!(session.gaps().len(), 2);
        assert!(
            session
                .gaps()
                .iter()
                .all(|gap| gap.stable_identity.is_none()),
            "unverified sidecar identities leaked into public continuation input"
        );
    }

    #[test]
    fn packet_graph_never_hydrates_or_projects_an_unadmitted_endpoint() {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};
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
        candidate.source_bytes_upper_bound = Some(512);

        let session = Rc::new(PacketProofSession::new());
        let guard = install_packet_proof_session(Rc::clone(&session));

        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[candidate.clone()],
            1,
        )
        .expect("resolve packet candidate");
        assert_eq!(outcome.resolved_hits.len(), 1);
        let packet_hit = outcome.packet_hits.first().expect("packet hit");
        assert!(packet_hit.graph_provenance.is_empty());
        assert!(packet_hit.graph.is_none());
        drop(guard);

        let mut admitted_peer = CandidateHit::with_source(
            "requests/sessions.py",
            Some("Session.request".into()),
            0.7,
            CandidateSource::Scip,
        );
        admitted_peer.node_id = Some("2".into());
        admitted_peer.source_bytes_upper_bound = Some(512);
        let session = Rc::new(PacketProofSession::new());
        let _guard = install_packet_proof_session(session);
        let outcome = resolve_sidecar_candidates_in_storage(
            &storage,
            &HashMap::new(),
            Path::new("."),
            &[candidate, admitted_peer],
            2,
        )
        .expect("resolve both admitted packet candidates");
        let packet_hit = outcome
            .packet_hits
            .iter()
            .find(|hit| hit.hit.node_id.0 == "3")
            .expect("target packet hit");
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
            unindexed_lexical_artifacts_only: false,
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
            unindexed_lexical_artifacts_only: false,
        }
    }

    /// EV-8. The sidecar reports no blocking cancel when only a *stage* runs out of budget, so a
    /// query whose dense lane went dark and then resolved nothing used to arrive as `Completed`
    /// and report an empty result as complete.
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
        assert_eq!(hit.eligible_for_sufficiency, None);
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
        assert_eq!(structural.eligible_for_sufficiency, None);

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
        assert_eq!(exact.eligible_for_sufficiency, None);

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
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
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
                unindexed_lexical_artifacts_only: false,
            })
        };
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
        let queries = vec![("handler".to_string(), 5)];

        let empty = build_sidecar_packet_batch_outcome(
            &controller,
            &queries,
            vec![query_result(None)],
            1,
            &HashSet::new(),
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
                &HashSet::new(),
                empty_resolution,
            )
            .expect("deadline result");
            assert_eq!(deadline.retryable_queries, ["handler"]);
            assert!(deadline.results[0].1.is_empty());
        }

        for reason in ["deadline", "stage_deadline"] {
            let mut retained = query_result(Some(reason));
            retained.query = "retained".into();
            let mut fresh = query_result(Some(reason));
            fresh.query = "fresh".into();
            let mixed = build_sidecar_packet_batch_outcome(
                &controller,
                &[("retained".into(), 5), ("fresh".into(), 5)],
                vec![retained, fresh],
                1,
                &HashSet::from(["retained".into()]),
                empty_resolution,
            )
            .expect("classify retained and freshly executed deadlines independently");
            assert_eq!(mixed.retained_deadline_queries, ["retained"]);
            assert_eq!(mixed.retryable_queries, ["fresh"]);
            assert_eq!(mixed.diagnostics.len(), 2);
            assert!(
                mixed
                    .diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.completion
                        == PacketQueryCompletionDto::Cancelled {
                            reason: reason.into()
                        })
            );
        }

        let cancelled = build_sidecar_packet_batch_outcome(
            &controller,
            &queries,
            vec![query_result(Some("cancelled"))],
            1,
            &HashSet::new(),
            empty_resolution,
        )
        .expect_err("public cancellation must not become an empty successful batch");
        assert_eq!(cancelled.code, "cancelled");
        let retained_cancelled = build_sidecar_packet_batch_outcome(
            &controller,
            &queries,
            vec![query_result(Some("cancelled"))],
            1,
            &HashSet::from(["handler".into()]),
            empty_resolution,
        )
        .expect_err("retained cancellation must not be converted to a deadline omission");
        assert_eq!(retained_cancelled.code, "cancelled");
    }

    #[test]
    fn query_rejection_recovery_uses_trace_cause_without_prescribing_index_repair() {
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        let project = tempfile::tempdir().expect("project root");
        let defaults = codestory_retrieval::SidecarProcessDefaults::new(
            process_cache.path().to_path_buf(),
            codestory_retrieval::SidecarRuntimeDefaults::from_process_env(),
        );
        let config = SidecarRuntimeConfig::for_project_profile_with_process_defaults(
            Some(project.path()),
            SidecarProfile::Agent,
            Some("query-recovery-fixture"),
            &defaults,
            &codestory_retrieval::SidecarRuntimeOverrides::default(),
        );
        let controller = AppController::new_with_config(config);
        controller
            .bind_project_paths_for_refresh(
                project.path().to_path_buf(),
                process_cache.path().join("codestory.db"),
            )
            .expect("bind project without opening retrieval");

        for (mode, stop, query_stop) in [
            ("full", Some("deadline"), true),
            ("full", Some("stage_deadline"), true),
            ("full", Some("cancelled"), true),
            ("unavailable", Some("stage_deadline"), false),
            ("unavailable", None, false),
            ("no_semantic", Some("deadline"), false),
            ("full", None, false),
            ("full", Some("unrecognized_stop"), false),
        ] {
            let trace = QueryTrace {
                retrieval_mode: mode.into(),
                degraded_reason: (mode != "full").then(|| "retrieval_manifest_missing".into()),
                total_budget_ms: 1_500,
                elapsed_ms: 1_412,
                cancel_reason: stop.map(str::to_string),
                cache_hit: false,
                stages: Vec::new(),
            };
            // Diagnostic prose must not decide whether the publication needs repair.
            let diagnostic = if query_stop {
                "opaque lookup diagnostic with retrieval_manifest_missing as a label"
            } else {
                "opaque stage_deadline diagnostic"
            };
            let error = sidecar_query_rejection_error(
                &controller,
                &trace.retrieval_mode,
                trace.cancel_reason.as_deref(),
                diagnostic,
            );
            assert_eq!(error.code, "retrieval_unavailable");
            assert!(
                error.message.contains(diagnostic),
                "retain bounded diagnostic"
            );
            let details = error.details.as_ref().expect("typed recovery details");
            assert_eq!(
                details.project.as_deref(),
                Some(project.path().to_str().expect("fixture project path"))
            );
            if query_stop {
                assert!(
                    details.next_commands.is_empty()
                        && details.minimum_next.is_empty()
                        && details.full_repair.is_empty(),
                    "{stop:?} in full mode must not prescribe index repair: {error:?}"
                );
                assert_eq!(details.cause_code.as_deref(), stop);
                assert_eq!(details.failed_layer.as_deref(), Some("retrieval_query"));
                let message = error.message.to_ascii_lowercase();
                if stop == Some("cancelled") {
                    assert!(message.contains("cancelled"));
                    assert!(
                        !message.contains("retry"),
                        "cancellation must not request retry"
                    );
                } else {
                    assert!(
                        message.contains("retry") && message.contains("narrow"),
                        "a query deadline needs a useful query action: {error:?}"
                    );
                }
            } else {
                assert_eq!(details.failed_layer.as_deref(), Some("retrieval_engine"));
                assert!(details.cause_code.is_none());
                assert_eq!(details.next_commands.len(), 3);
                assert!(details.next_commands[0].starts_with("codestory-cli retrieval index "));
                assert!(details.next_commands[1].starts_with("codestory-cli retrieval status "));
                assert!(details.next_commands[2].starts_with("codestory-cli doctor "));
                for command in &details.next_commands[..2] {
                    assert!(command.contains("--run-id query-recovery-fixture"));
                }
                assert_eq!(details.minimum_next, details.next_commands[..1]);
                assert_eq!(details.full_repair, details.next_commands);
            }
        }
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
        manifest.artifact_scope = Some(codestory_store::RetrievalArtifactScope::Local);
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
                artifact_scope: None,
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
        let resolution = resolve_sidecar_candidates_in_read(&controller, &pinned, &[candidate], 1)
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

    fn assert_published_file_packet_available(
        fixture: &PinnedOperationFixture,
        request: &AgentPacketRequestDto,
        packet: &AgentPacketDto,
    ) {
        use codestory_contracts::packet_projection_v3::{GapKindV3Dto, PacketProjectionV3Dto};
        use std::sync::atomic::AtomicBool;
        assert_ne!(
            packet.disposition.kind,
            PacketDispositionKindDto::Unavailable,
            "authenticated whole-file evidence must not imply unavailable retrieval: {:?}",
            packet.disposition
        );
        let service = crate::PublicOperationService::new(fixture.controller.clone());
        let _latency = crate::enter_packet_latency_scope(request.latency_budget_ms);
        let projected = service
            .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                crate::project_packet_v3(&service, "codestory-cli", request, packet, |p| {
                    serde_json::to_vec(p).map(|v| v.len()).map_err(|_| ())
                })
            })
            .unwrap()
            .value;
        let gaps = match &projected.projection {
            PacketProjectionV3Dto::Complete { gaps, .. }
            | PacketProjectionV3Dto::BudgetExceeded { gaps, .. } => gaps,
        };
        assert!(
            !gaps
                .as_slice()
                .iter()
                .any(|g| g.kind == GapKindV3Dto::RetrievalUnavailable),
            "authenticated lexical evidence must not emit packet-evidence-unavailable: {:?}",
            projected.projection
        );
    }

    #[test]
    fn browser_packet_delivers_small_published_lexical_file_without_core_symbols() {
        assert_browser_packet_delivers_small_published_lexical_files(Vec::new());
    }

    #[test]
    fn browser_packet_delivers_small_published_lexical_file_with_exact_probe_and_matching_query() {
        assert_browser_packet_delivers_small_published_lexical_files(vec![
            codestory_contracts::api::PacketProbeDto::ExactPath {
                path: "settings.ini".into(),
            },
        ]);
    }

    fn assert_browser_packet_delivers_small_published_lexical_files(
        probes: Vec<codestory_contracts::api::PacketProbeDto>,
    ) {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};
        use codestory_contracts::api::SupportUnitKindDto;
        use sha2::{Digest, Sha256};

        let files = [
            (
                "settings.ini",
                "[settings]\nretry_limit = 3\nstop_on_error = true\n",
            ),
            (
                "meson.build",
                "project('configuration', 'c')\nadd_project_arguments('-DSTOP_ON_ERROR=1', language: 'c')\n",
            ),
        ];
        let expected_sources = [
            "```text\n>    1 | [settings]\n     2 | retry_limit = 3\n     3 | stop_on_error = true\n```",
            "```text\n>    1 | project('configuration', 'c')\n     2 | add_project_arguments('-DSTOP_ON_ERROR=1', language: 'c')\n```",
        ];
        let question = "Where are retry_limit and stop_on_error configured and how does the build define STOP_ON_ERROR?";
        let fixture = public_packet_fixture_with_lexical_files(&files);
        let project_root = fixture._project.path();
        let core = Store::open_read_only(&fixture.storage_path).expect("open pinned core");
        for (path, _) in files {
            assert!(!codestory_workspace::has_supported_source_route(Path::new(
                path
            )));
            assert!(
                super::super::packet_compiler::find_admitted_file(&core, project_root, path)
                    .expect("look up exact Core file identity")
                    .is_none(),
                "lexical-only fixture must not seed a Core file row: {path}"
            );
        }
        drop(core);

        let pinned = Rc::new(
            PinnedRetrievalRead::begin_packet_descriptor(&fixture.controller)
                .expect("begin descriptor publication pin"),
        );
        let session = Rc::new(PacketProofSession::new());
        with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
            let _guard = install_packet_proof_session(Rc::clone(&session));
            preadmit_packet_descriptor_queries_with_policy(
                &fixture.controller,
                &[PacketRetrievalQuery {
                    query: question.to_string(),
                    lexical_policy: LexicalAdmissionPolicy::BroadQuestionV1,
                }],
                None,
            )
            .expect("retrieve and seal ordinary file descriptors");
            let retained = session
                .descriptor_result_with_policy(
                    question,
                    LexicalAdmissionPolicy::BroadQuestionV1,
                    true,
                    pinned.session.publication_identity(),
                )
                .expect("retain the same-publication descriptor result");
            assert!(session.retrieval_admission_is_sealed());
            let bare_path = PacketProofSession::new();
            assert_eq!(
                bare_path.admit_exact_selector("path:settings.ini", 512, 0),
                PacketAdmissionDecision::Admitted
            );
            bare_path.seal_retrieval_admission();
            assert!(
                !bare_path.authorizes_published_file(
                    &bare_path.receipts()[0],
                    pinned.session.publication_identity()
                ),
                "a caller path alone cannot authorize published source bytes"
            );
            for (path, _) in files {
                let identity = format!("path:{path}");
                assert!(
                    retained.hits.iter().any(|candidate| {
                        candidate.file_path == path
                            && candidate.source == CandidateSource::Lexical
                            && candidate.node_id.is_none()
                            && candidate
                                .packet_descriptor()
                                .is_some_and(|descriptor| descriptor.stable_identity == identity)
                    }),
                    "normal retrieval must retain an eligible file descriptor: {path}"
                );
                assert!(
                    session.is_admitted_identity(&identity),
                    "the file must cross sealed admission before source delivery: {path}"
                );
                let receipt = session
                    .receipts()
                    .into_iter()
                    .find(|receipt| receipt.stable_identity == identity)
                    .unwrap();
                assert!(
                    session
                        .authorizes_published_file(&receipt, pinned.session.publication_identity())
                );
                let mut forged = receipt.clone();
                forged.reserved_source_bytes += 1;
                assert!(
                    !session
                        .authorizes_published_file(&forged, pinned.session.publication_identity())
                );
                forged = receipt.clone();
                forged.packet_ordinal += 1;
                assert!(
                    !session
                        .authorizes_published_file(&forged, pinned.session.publication_identity())
                );
                assert!(
                    !PacketProofSession::new()
                        .authorizes_published_file(&receipt, pinned.session.publication_identity())
                );
                let mut foreign = pinned.session.publication_identity().clone();
                foreign.core_generation_id.push_str("-foreign");
                assert!(!session.authorizes_published_file(&receipt, &foreign));
            }
        });
        drop(pinned);

        let request = AgentPacketRequestDto {
            question: question.into(),
            budget: PacketBudgetModeDto::Standard,
            probes,
            latency_budget_ms: Some(30_000),
            parent_packet_id: None,
            option_ids: Vec::new(),
            core_generation_id: None,
            retrieval_generation: None,
        };
        let packet = fixture
            .controller
            .browser_service()
            .packet(request.clone())
            .expect("ordinary packet over the real Core and lexical publication");

        for ((path, content), expected) in files.into_iter().zip(expected_sources) {
            let source = packet
                .support
                .iter()
                .find(|row| {
                    row.kind == SupportUnitKindDto::SourceRange
                        && row.path.as_deref() == Some(path)
                })
                .unwrap_or_else(|| {
                    panic!(
                        "small admitted published file must reach physical source support: {path}; disposition={:?}",
                        packet.disposition
                    )
                });
            assert!(
                source.symbol_id.is_none(),
                "a file must not invent a graph ID"
            );
            let line_count = content.lines().count() as u32;
            assert_eq!(
                (source.start_line, source.end_line),
                (Some(1), Some(line_count))
            );
            let snippet = source.snippet.as_deref().expect("physical source bytes");
            assert_eq!(
                snippet, expected,
                "every original physical line must be delivered"
            );
            let delivery = source
                .source_delivery
                .as_ref()
                .expect("published file must carry an authenticated source receipt");
            assert_eq!(
                delivery.source_sha256,
                format!("{:x}", Sha256::digest(content))
            );
            assert_eq!(delivery.requested_range.start_line, 1);
            assert_eq!(delivery.requested_range.end_line, line_count);
            assert_eq!(delivery.remaining_source_bytes, 0);
            assert!(delivery.next_start_line.is_none());
            assert!(
                packet
                    .answer
                    .source_coverage
                    .iter()
                    .any(|coverage| coverage.path == path
                        && coverage.status
                            == codestory_contracts::api::SourceCoverageStatusDto::NotEstablished
                        && coverage.not_established_cause
                            == Some(
                                codestory_contracts::api::SourceCoverageNotEstablishedCauseDto::LookupUnavailable
                            )),
                "published source must preserve honest missing Core coverage: {path}"
            );
        }
        assert_published_file_packet_available(&fixture, &request, &packet);
    }

    #[test]
    fn mixed_published_file_packet_issuance_preserves_loaded_source_floor() {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};
        use crate::agent::packet_served_sources;
        use codestory_contracts::api::{PacketProbeDto, SupportUnitKindDto};
        use codestory_contracts::packet_projection_v3::{EvidenceKindV3Dto, PacketProjectionV3Dto};
        use std::sync::atomic::AtomicBool;
        let content = "ini_flag = true\n";
        assert_eq!(
            crate::retrieval_file_role_from_path("tests/support.rs"),
            crate::RetrievalFileRole::Test
        );
        let fixture = public_packet_fixture_with_lexical_files(&[
            ("settings.ini", content),
            ("tests/support.rs", "mod protected_context {}\n"),
        ]);
        let core = Store::open_read_only(&fixture.storage_path).unwrap();
        let node = core
            .get_nodes()
            .unwrap()
            .into_iter()
            .find(|n| n.kind == NodeKind::MODULE && n.serialized_name.contains("protected_context"))
            .expect("real parsed module must provide a numeric source identity");
        let indexed_file = core
            .get_file_by_id(node.file_node_id.expect("parsed symbol file").0)
            .unwrap()
            .unwrap();
        assert!(indexed_file.path.ends_with("tests/support.rs"));
        assert!(indexed_file.indexed && indexed_file.complete);
        assert_eq!((node.start_line, node.end_line), (Some(1), Some(1)));
        drop(core);
        let service = crate::PublicOperationService::new(fixture.controller.clone());
        let browser =
            crate::ReadOnlyBrowserService::new(fixture.controller.clone(), service.clone());
        let mut retained_request = None;
        let mut completed_branches = Vec::new();
        let mut executed_returned_patches = 0;
        for retain_file in [false, true] {
            let request = AgentPacketRequestDto {
                question: "ini_flag".into(),
                budget: PacketBudgetModeDto::Standard,
                probes: vec![PacketProbeDto::SymbolId {
                    id: node.id.0.to_string(),
                }],
                latency_budget_ms: Some(30_000),
                parent_packet_id: None,
                option_ids: Vec::new(),
                core_generation_id: None,
                retrieval_generation: None,
            };
            let _latency = crate::enter_packet_latency_scope(request.latency_budget_ms);
            let follow_up = service.run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                let represented = browser.packet_for_representation_v3(request.clone(), "codestory-cli", &mut |projection| {
                    let has_file = match projection {
                        PacketProjectionV3Dto::Complete { evidence, .. } => evidence.as_slice().iter().any(|r| r.kind == EvidenceKindV3Dto::ExactSource && r.path.as_ref().is_some_and(|p| p.as_str() == "settings.ini")),
                        _ => false,
                    };
                    if !retain_file && has_file { Ok(16_385) } else { serde_json::to_vec(projection).map(|v| v.len()).map_err(|_| ()) }
                })?;
                assert_ne!(represented.packet.disposition.kind, PacketDispositionKindDto::Unavailable, "mixed retained/omitted lexical file cannot mark numeric evidence unavailable: {:?}", represented.packet.disposition);
                let gaps = match &represented.product.projection {
                    PacketProjectionV3Dto::Complete { gaps, .. } | PacketProjectionV3Dto::BudgetExceeded { gaps, .. } => gaps,
                };
                assert!(!gaps.as_slice().iter().any(|g| g.kind == codestory_contracts::packet_projection_v3::GapKindV3Dto::RetrievalUnavailable), "mixed representation cannot emit packet-evidence-unavailable: {:?}", represented.product.projection);
                assert!(represented.packet.packet_id.starts_with("psr1:"), "mixed prepared input must retain issued source identity even when a file is omitted");
                if let Some(drill) = &represented.packet.disposition.drill {
                    for option in &drill.options {
                        if option.symbol_id.is_none() {
                            assert!(option.source_range.is_none(), "whole-file paths must never advertise a numeric source cursor");
                            assert!(!option.id.starts_with("range:path:"));
                        }
                    }
                }
                let path_option_offered = represented.packet.disposition.drill.as_ref().is_some_and(|drill| drill.options.iter().any(|o| o.path.as_deref() == Some("settings.ini") && o.symbol_id.is_none()));
                if !retain_file {
                    assert!(path_option_offered, "the selected omitted small file must expose the existing whole-file action for execution proof");
                }
                let offered_patch = if !retain_file {
                    match &represented.product.projection {
                        PacketProjectionV3Dto::Complete { continuation: Some(continuation), .. } => Some(continuation.packet_request_patch.clone().expect("public whole-file option must carry its executable request patch")),
                        _ => panic!("offered whole-file action requires complete public continuation"),
                    }
                } else { None };
                let originals = represented.packet.support.iter().filter(|r| r.kind == SupportUnitKindDto::SourceRange).cloned().collect::<Vec<_>>();
                assert!(originals.iter().any(|r| r.symbol_id.as_deref() == Some(node.id.0.to_string().as_str())));
                assert_eq!(originals.iter().any(|r| r.path.as_deref() == Some("settings.ini")), retain_file);
                packet_served_sources::commit(&fixture.controller, "codestory-cli", &represented.product)?;
                let mut continuation = request.clone();
                continuation.parent_packet_id = Some(represented.packet.packet_id.clone());
                let record = packet_served_sources::load(&fixture.controller, "codestory-cli", &continuation)?.expect("actual issued record must reload");
                assert_eq!(record.witnesses.iter().any(|w| w.node_id.is_none()), retain_file);
                let session = Rc::new(PacketProofSession::new());
                session.protect_served_sources(record.clone())?;
                session.seal_retrieval_admission();
                let _guard = install_packet_proof_session(Rc::clone(&session));
                let pinned = active_pinned_retrieval_read(&fixture.controller).expect("current public packet pin");
                let prepare = || super::super::packet_compiler::prepare_packet_compilation(
                    &fixture.controller, pinned.session.project_id(), &[], None, &session, None,
                );
                let prepared = prepare()?;
                assert_eq!(session.benchmark_retrieval_proof().descriptor_query_count, 0, "issued path authority must not depend on new ranked hits");
                let frozen = prepared.select_for_representation(|_, product| Ok(Some(serde_json::to_vec(&product.support).unwrap().len())))?;
                for old in &originals {
                    assert!(frozen.product.support.iter().any(|r| r.kind == SupportUnitKindDto::SourceRange && r.symbol_id == old.symbol_id && r.path == old.path && r.start_line == old.start_line && r.end_line == old.end_line && r.snippet == old.snippet && r.source_delivery == old.source_delivery), "loaded issued sources must remain exact protected context");
                }
                if retain_file {
                    retained_request = Some(continuation.clone());
                }
                Ok(offered_patch.map(|patch| {
                    let mut next = request.clone();
                    next.probes = patch.probes.as_slice().to_vec();
                    next.parent_packet_id = patch.parent_packet_id.as_ref().map(|v| v.as_str().to_owned());
                    next.option_ids = patch.option_ids.as_slice().iter().map(|v| v.as_str().to_owned()).collect();
                    next.core_generation_id = Some(patch.core_generation_id.as_str().to_owned());
                    next.retrieval_generation = patch.retrieval_generation.as_ref().map(|v| v.as_str().to_owned());
                    (next, originals)
                }))
            }).unwrap_or_else(|error| panic!("mixed retain_file={retain_file} stage=successful-issuance-reload-floor: {error:?}")).value;
            completed_branches.push(retain_file);
            assert_eq!(
                follow_up.is_some(),
                !retain_file,
                "mixed retain_file={retain_file} stage=returned-whole-file-patch"
            );
            if let Some((next, originals)) = follow_up {
                service.run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                    let represented = browser.packet_for_representation_v3(next.clone(), "codestory-cli", &mut |p| serde_json::to_vec(p).map(|v| v.len()).map_err(|_| ()))?;
                    assert_ne!(represented.packet.disposition.kind, PacketDispositionKindDto::Unavailable);
                    assert!(represented.packet.support.iter().any(|r| r.kind == SupportUnitKindDto::SourceRange && r.path.as_deref() == Some("settings.ini")), "ordinary whole-file follow-up must execute without the ranged-node refusal");
                    for old in &originals {
                        assert!(represented.packet.support.iter().any(|r| r.kind == SupportUnitKindDto::SourceRange && r.symbol_id == old.symbol_id && r.path == old.path && r.snippet == old.snippet && r.source_delivery == old.source_delivery), "executed returned patch preserves the issued numeric context");
                    }
                    Ok(())
                }).unwrap_or_else(|error| panic!("mixed retain_file={retain_file} stage=execute-returned-whole-file-patch: {error:?}"));
                executed_returned_patches += 1;
            }
        }
        assert_eq!(completed_branches, vec![false, true]);
        assert_eq!(executed_returned_patches, 1);

        // Authenticate issued history under the actual owner, then mutate.
        // Public retrieval freshness rejects the changed input before source
        // measurement; this does not assert the later history-digest boundary.
        // Restore only after the operation has returned.
        let retained_request = retained_request.expect("retained branch issued path history");
        let _hostile_latency =
            crate::enter_packet_latency_scope(retained_request.latency_budget_ms);
        let path = fixture._project.path().join("settings.ini");
        let mut public_refusal = None;
        let mut represented_product_returned = false;
        let mut measurements = 0;
        let hostile = service.run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
            let record = packet_served_sources::load(
                &fixture.controller,
                "codestory-cli",
                &retained_request,
            )?
            .expect("hostile operation loads authentic path history");
            assert!(
                record
                    .witnesses
                    .iter()
                    .any(|w| w.node_id.is_none() && w.path == "settings.ini")
            );
            std::fs::write(&path, content.replace("true", "fake")).unwrap();
            match browser.packet_for_representation_v3(
                retained_request.clone(),
                "codestory-cli",
                &mut |projection| {
                    measurements += 1;
                    serde_json::to_vec(projection)
                        .map(|bytes| bytes.len())
                        .map_err(|_| ())
                },
            ) {
                Err(error) => {
                    public_refusal = Some((error.code.clone(), error.message.clone()));
                    Err(error)
                }
                Ok(_) => {
                    represented_product_returned = true;
                    Ok(())
                }
            }
        });
        std::fs::write(&path, content).expect("restore hostile fixture outside the operation");
        assert!(
            !represented_product_returned,
            "stage=hostile-public-representation must not return evidence for changed signed source"
        );
        assert_eq!(
            measurements, 0,
            "stage=hostile-public-representation must refuse before source projection measurement"
        );
        let (code, message) = public_refusal
            .expect("stage=hostile-public-representation must refuse changed signed path bytes");
        assert_eq!(
            code, "retrieval_unavailable",
            "stage=hostile-public-representation actual code={code}, message={message}"
        );
        assert!(
            message.contains("retrieval_manifest_stale")
                && message.contains("sidecar_input_hash_changed"),
            "stage=hostile-public-representation: {message}"
        );
        let propagated = match hostile {
            Err(error) => error,
            Ok(_) => panic!(
                "stage=hostile-public-operation must propagate the public representation refusal"
            ),
        };
        assert_eq!((propagated.code, propagated.message), (code, message));
    }

    #[test]
    fn browser_packet_refuses_published_file_changed_after_selection() {
        use codestory_contracts::packet_projection_v3::{
            EvidenceKindV3Dto, GapKindV3Dto, PacketProjectionV3Dto,
        };
        use std::sync::atomic::AtomicBool;
        let original = "stop_on_error = true\n";
        let changed = "stop_on_error = fake\n";
        assert_eq!(original.len(), changed.len());
        let fixture = public_packet_fixture_with_lexical_files(&[("settings.ini", original)]);
        let service = crate::PublicOperationService::new(fixture.controller.clone());
        let browser =
            crate::ReadOnlyBrowserService::new(fixture.controller.clone(), service.clone());
        let request = AgentPacketRequestDto {
            question: "stop_on_error".into(),
            budget: PacketBudgetModeDto::Standard,
            probes: Vec::new(),
            latency_budget_ms: Some(30_000),
            parent_packet_id: None,
            option_ids: Vec::new(),
            core_generation_id: None,
            retrieval_generation: None,
        };
        let _latency = crate::enter_packet_latency_scope(request.latency_budget_ms);
        let baseline = service
            .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                browser.packet_for_representation_v3(
                    request.clone(),
                    "codestory-cli",
                    &mut |projection| {
                        serde_json::to_vec(projection)
                            .map(|bytes| bytes.len())
                            .map_err(|_| ())
                    },
                )
            })
            .expect("unchanged positive public representation control")
            .value;
        let expected_body = "```text\n>1 | stop_on_error = true\n```";
        match &baseline.product.projection {
            PacketProjectionV3Dto::Complete { evidence, .. } => {
                assert!(evidence.as_slice().iter().any(|row| {
                    row.kind == EvidenceKindV3Dto::ExactSource
                        && row
                            .path
                            .as_ref()
                            .is_some_and(|p| p.as_str() == "settings.ini")
                        && row
                            .summary
                            .as_ref()
                            .is_some_and(|s| s.as_str() == expected_body)
                }))
            }
            other => panic!("positive whole-file control must be complete: {other:?}"),
        }

        let path = fixture._project.path().join("settings.ini");
        let mut cutpoint_hits = 0;
        let outcome = service.run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
            browser.packet_for_representation_v3(request.clone(), "codestory-cli", &mut |projection| {
                if cutpoint_hits == 0 && matches!(projection, PacketProjectionV3Dto::Complete { evidence, .. }
                    if evidence.as_slice().iter().any(|row| row.kind == EvidenceKindV3Dto::ExactSource
                        && row.path.as_ref().is_some_and(|p| p.as_str() == "settings.ini")
                        && row.summary.as_ref().is_some_and(|s| s.as_str() == expected_body))) {
                    std::fs::write(&path, changed).unwrap();
                    cutpoint_hits += 1;
                }
                serde_json::to_vec(projection).map(|bytes| bytes.len()).map_err(|_| ())
            })
        });
        std::fs::write(&path, original)
            .expect("restore selected source after the operation returns");
        assert_eq!(
            cutpoint_hits, 1,
            "hostile cutpoint must see the actual complete exact file body before mutation"
        );
        match outcome {
            Err(error) => assert!(
                matches!(
                    error.code.as_str(),
                    "publication_changed" | "source_stale" | "source_unavailable"
                ),
                "expected a typed source/publication refusal after selection, got {error:?}"
            ),
            Ok(operation) => {
                let projection = &operation.value.product.projection;
                let gaps = match projection {
                    PacketProjectionV3Dto::Complete { evidence, gaps, .. } => {
                        assert!(
                            !evidence.as_slice().iter().any(|row| row.kind
                                == EvidenceKindV3Dto::ExactSource
                                && row
                                    .path
                                    .as_ref()
                                    .is_some_and(|p| p.as_str() == "settings.ini")),
                            "a completed packet served stale exact source after the selected file changed: {projection:?}"
                        );
                        gaps
                    }
                    PacketProjectionV3Dto::BudgetExceeded { gaps, .. } => gaps,
                };
                assert!(
                    gaps.as_slice()
                        .iter()
                        .any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable
                            && gap.message.as_ref().is_some_and(|message| {
                                let text = message.as_str();
                                text.contains("settings.ini")
                                    && [
                                        "changed",
                                        "stale",
                                        "omitted",
                                        "unavailable",
                                        "not delivered",
                                    ]
                                    .iter()
                                    .any(|reason| text.contains(reason))
                            })),
                    "a non-source result must explicitly retain the stale/omitted-file gap: {projection:?}"
                );
            }
        }
    }

    #[test]
    fn published_file_hydration_checks_freshness_before_budget_and_preserves_admission() {
        use crate::agent::packet_candidate::{PacketProofSession, install_packet_proof_session};
        use codestory_contracts::compilation::{
            PacketAdmissionGapKindV1, PacketParserCompletenessV1,
        };
        let files = [
            ("short.ini", "short_setting = 1\n".to_string()),
            ("raw.ini", format!("raw_setting = {}\n", "x".repeat(512))),
            (
                "lines.ini",
                format!("line_setting = 1\n{}", "\n".repeat(80)),
            ),
            (
                "stale.ini",
                format!("stale_setting = {}\n", "s".repeat(512)),
            ),
        ];
        let inputs = files
            .iter()
            .map(|(path, content)| (*path, content.as_str()))
            .collect::<Vec<_>>();
        let fixture = public_packet_fixture_with_lexical_files(&inputs);
        let pinned =
            Rc::new(PinnedRetrievalRead::begin_packet_descriptor(&fixture.controller).unwrap());
        let session = Rc::new(PacketProofSession::new());
        with_active_pinned_retrieval_read(&fixture.controller, Rc::clone(&pinned), || {
            let _guard = install_packet_proof_session(Rc::clone(&session));
            preadmit_packet_descriptor_queries_with_policy(
                &fixture.controller,
                &[PacketRetrievalQuery {
                    query: "Explain short_setting raw_setting line_setting stale_setting".into(),
                    lexical_policy: LexicalAdmissionPolicy::BroadQuestionV1,
                }],
                None,
            )
            .unwrap();
            for (path, _) in &files {
                assert!(session.is_admitted_identity(&format!("path:{path}")));
            }
            pinned.session.validate_full_readiness().unwrap();
            let receipts = session.receipts();
            let queries = session.benchmark_retrieval_proof().descriptor_query_count;
            let mut forged = receipts
                .iter()
                .find(|r| r.stable_identity == "path:short.ini")
                .unwrap()
                .clone();
            forged.reserved_source_bytes += 1;
            assert!(read_admitted_published_file(&fixture.controller, &forged).is_err());
            let stale = fixture._project.path().join("stale.ini");
            let modified = std::fs::metadata(&stale).unwrap().modified().unwrap();
            std::fs::write(&stale, files[3].1.replace('s', "t")).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&stale)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
            let prepared = super::super::packet_compiler::prepare_packet_compilation(
                &fixture.controller,
                pinned.session.project_id(),
                &[],
                None,
                &session,
                None,
            )
            .unwrap();
            let input = prepared.replay_input();
            let short = input
                .sources
                .iter()
                .find(|source| source.stable_identity == "path:short.ini")
                .expect("short whole file");
            assert!(short.callable_body.is_none());
            assert_eq!(
                short.parser_completeness,
                PacketParserCompletenessV1::Unknown
            );
            assert!(
                input.relations.is_empty(),
                "published paths must not become Core graph endpoints"
            );
            for path in ["raw.ini", "lines.ini"] {
                let identity = format!("path:{path}");
                assert!(
                    input
                        .admissions
                        .iter()
                        .any(|r| r.stable_identity == identity)
                );
                assert!(
                    !input
                        .sources
                        .iter()
                        .any(|source| source.stable_identity == identity)
                );
                assert!(
                    input
                        .admission_gaps
                        .iter()
                        .any(
                            |gap| gap.stable_identity.as_deref() == Some(identity.as_str())
                                && gap.kind == PacketAdmissionGapKindV1::SourceBudgetExceeded
                        )
                );
            }
            assert!(
                !input
                    .admissions
                    .iter()
                    .any(|r| r.stable_identity == "path:stale.ini")
            );
            assert!(
                input
                    .admission_gaps
                    .iter()
                    .any(
                        |gap| gap.stable_identity.as_deref() == Some("path:stale.ini")
                            && gap.kind == PacketAdmissionGapKindV1::SourceUnavailable
                    )
            );
            assert!(
                !input
                    .admission_gaps
                    .iter()
                    .any(
                        |gap| gap.stable_identity.as_deref() == Some("path:stale.ini")
                            && gap.kind == PacketAdmissionGapKindV1::SourceBudgetExceeded
                    )
            );
            assert_eq!(
                session.receipts(),
                receipts,
                "failed source must not refund or grow sealed reservations"
            );
            assert_eq!(
                session.benchmark_retrieval_proof().descriptor_query_count,
                queries,
                "source hydration must not issue another query"
            );
        });
    }

    #[test]
    fn published_lexical_file_packet_refuses_changed_discovery_exclusion() {
        let fixture = public_packet_fixture_with_lexical_files(&[
            (".ignore", "/.ignore\n"),
            ("settings.ini", "policy_setting = true\n"),
        ]);
        let request = AgentPacketRequestDto {
            question: "policy_setting".into(),
            budget: PacketBudgetModeDto::Standard,
            probes: Vec::new(),
            latency_budget_ms: Some(30_000),
            parent_packet_id: None,
            option_ids: Vec::new(),
            core_generation_id: None,
            retrieval_generation: None,
        };
        let before = fixture
            .controller
            .browser_service()
            .packet(request.clone())
            .unwrap();
        assert!(
            before
                .support
                .iter()
                .any(|r| r.path.as_deref() == Some("settings.ini") && r.snippet.is_some())
        );
        std::fs::write(
            fixture._project.path().join(".ignore"),
            "/.ignore\n/settings.ini\n",
        )
        .unwrap();
        let discovered =
            codestory_workspace::WorkspaceManifest::open(fixture._project.path().to_path_buf())
                .unwrap()
                .source_files()
                .unwrap();
        assert!(
            !discovered
                .iter()
                .any(|p| p.file_name().is_some_and(|n| n == "settings.ini"))
        );
        assert!(
            discovered
                .iter()
                .any(|p| p.file_name().is_some_and(|n| n == "metadata.rs"))
        );
        fixture.controller.browser_service().packet(request).expect_err("changed discovery policy must refuse the fresh packet before serving the old published file");
    }

    #[test]
    fn browser_packet_serves_published_lexical_artifact_without_core_symbols() {
        let question =
            "Which role is assumed by the control plane, and which is used by worker nodes?";
        let fixture = public_packet_fixture_with_unindexed_lexical_artifact();
        let project_root = fixture._project.path();
        assert!(project_root.join("LICENSE").is_file());
        assert!(!codestory_workspace::has_supported_source_route(Path::new(
            "LICENSE"
        )));
        let core = Store::open_read_only(&fixture.storage_path).expect("open pinned core");
        assert!(
            candidate_lookup_paths(project_root, "LICENSE")
                .into_iter()
                .all(|path| matches!(core.get_file_by_path(&path), Ok(None))),
            "LICENSE must be lexical-only and absent from every core lookup spelling"
        );
        drop(core);
        let browser = fixture.controller.browser_service();

        let request = AgentPacketRequestDto {
            question: question.into(),
            budget: PacketBudgetModeDto::Compact,
            probes: Vec::new(),
            latency_budget_ms: Some(30_000),
            parent_packet_id: None,
            option_ids: Vec::new(),
            core_generation_id: None,
            retrieval_generation: None,
        };
        let packet = browser.packet(request.clone())
            .unwrap_or_else(|error| {
                panic!(
                    "public packet must deliver a real published lexical file while retaining diagnostic authority limits, got {}: {}",
                    error.code, error.message
                )
            });

        assert_published_file_packet_available(&fixture, &request, &packet);
        let source = packet
            .support
            .iter()
            .find(|row| row.path.as_deref() == Some("LICENSE") && row.snippet.is_some())
            .expect("small published LICENSE remains ordinary lexical file evidence");
        assert!(source.symbol_id.is_none());
        assert_eq!((source.start_line, source.end_line), (Some(1), Some(1)));
        assert!(source.snippet.as_deref().unwrap().contains("The control plane assumes the control plane role, and worker nodes use the worker node role."));
        assert!(
            source
                .source_delivery
                .as_ref()
                .is_some_and(|delivery| delivery.remaining_source_bytes == 0)
        );
        assert!(packet.answer.citations.is_empty());
        assert!(
            packet
                .answer
                .retrieval_trace
                .retrieval_publication
                .is_some()
        );
        assert_ne!(
            packet.disposition.kind,
            PacketDispositionKindDto::NotEstablished
        );
        assert_ne!(
            packet.disposition.reason.as_deref(),
            Some("no bounded repository evidence was retained")
        );
        assert!(packet.disposition.drill.is_none());
        assert!(packet.answer.source_coverage.iter().any(|coverage| coverage.path == "LICENSE"
            && coverage.status == codestory_contracts::api::SourceCoverageStatusDto::NotEstablished
            && coverage.not_established_cause == Some(codestory_contracts::api::SourceCoverageNotEstablishedCauseDto::LookupUnavailable)));
        assert_eq!(
            packet.disposition.omission_receipts,
            vec!["source coverage (lookup_unavailable) is unproven for LICENSE."],
            "published source does not claim parser/Core coverage"
        );
        let shadow = packet
            .answer
            .retrieval_trace
            .retrieval_shadow
            .expect("public packet must retain aggregate sidecar diagnostics");
        assert_eq!(shadow.retrieval_mode, "full");
        assert_eq!(shadow.candidate_count, 1);
        assert_eq!(shadow.resolved_hit_count, 0);
        assert_eq!(shadow.unresolved_candidate_count, 1);
        assert!(shadow.diagnostic_only);
        assert!(
            shadow.candidates.is_empty(),
            "compact packet budgeting may omit verbose candidate rows while preserving aggregate diagnostics"
        );
    }

    #[test]
    fn sidecar_primary_serves_only_unindexed_lexical_artifacts_outside_core_routes() {
        use codestory_retrieval::{CandidateSource, classify_query};

        let fixture = pinned_operation_fixture();
        let project_root = fixture._project.path();
        std::fs::create_dir_all(project_root.join("src")).expect("create source directory");
        std::fs::write(project_root.join("LICENSE"), "license text\n")
            .expect("write extensionless artifact");
        std::fs::write(project_root.join("src/missing.rs"), "pub fn missing() {}\n")
            .expect("write parser-routed source");
        let controller = &fixture.controller;

        let full_result = |query: &str, candidate: CandidateHit| QueryResult {
            publication_identity: None,
            query: query.to_string(),
            features: classify_query(query),
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
        };
        let assert_rejected = |result: QueryResult, expected_resolutions: &[&str]| {
            match sidecar_primary_search_outcome_from_query_result(controller, result, 5) {
                SidecarPrimarySearchOutcome::Rejected { shadow, .. } => assert_eq!(
                    shadow
                        .candidates
                        .iter()
                        .map(|candidate| candidate.resolution.as_deref().unwrap_or("unlabeled"))
                        .collect::<Vec<_>>(),
                    expected_resolutions
                ),
                _ => panic!("candidate matrix row must stay rejected"),
            }
        };

        let artifact_candidate =
            CandidateHit::with_source("LICENSE", None, 0.8, CandidateSource::Lexical);
        assert!(artifact_candidate.target.is_none());
        let artifact_result = full_result("license", artifact_candidate);
        let artifact_resolution =
            resolve_sidecar_candidates_for_test(controller, &artifact_result.hits, 5)
                .expect("resolve lexical artifact");
        let artifact_diagnostic =
            packet_sidecar_query_diagnostic(&artifact_result, &artifact_resolution, 1, 1, 2);
        let artifact_outcome =
            sidecar_primary_search_outcome_from_query_result(controller, artifact_result, 5);
        let supported_source_result = full_result(
            "missing",
            CandidateHit::with_source("src/missing.rs", None, 0.8, CandidateSource::Lexical),
        );
        let supported_source_resolution =
            resolve_sidecar_candidates_for_test(controller, &supported_source_result.hits, 5)
                .expect("resolve parser-routed source");
        let supported_source_diagnostic = packet_sidecar_query_diagnostic(
            &supported_source_result,
            &supported_source_resolution,
            1,
            1,
            2,
        );
        assert_eq!(supported_source_diagnostic.unresolved_candidate_count, 1);
        assert_eq!(
            supported_source_diagnostic.blocking_unresolved_candidate_count,
            1
        );
        assert_rejected(supported_source_result, &["node_unresolved"]);

        let semantic_result = full_result(
            "handler",
            CandidateHit::with_source(
                "handler",
                Some("handler".into()),
                0.8,
                CandidateSource::Semantic,
            ),
        );
        let semantic_resolution =
            resolve_sidecar_candidates_for_test(controller, &semantic_result.hits, 5)
                .expect("resolve bare semantic anchor");
        let semantic_diagnostic =
            packet_sidecar_query_diagnostic(&semantic_result, &semantic_resolution, 1, 1, 2);
        assert_eq!(semantic_diagnostic.unresolved_candidate_count, 1);
        assert_eq!(semantic_diagnostic.blocking_unresolved_candidate_count, 0);
        assert_rejected(semantic_result, &["path_unresolvable"]);

        assert_rejected(
            full_result(
                "license symbol",
                CandidateHit::with_source(
                    "LICENSE",
                    Some("license".into()),
                    0.8,
                    CandidateSource::Lexical,
                ),
            ),
            &["node_unresolved"],
        );

        let mut targeted_artifact =
            CandidateHit::with_source("LICENSE", None, 0.8, CandidateSource::Lexical);
        targeted_artifact.target = Some(SearchTargetDto::FileRange {
            file_path: "LICENSE".into(),
            start_byte: 0,
            end_byte: 7,
        });
        assert_rejected(
            full_result("license target", targeted_artifact),
            &["node_unresolved"],
        );

        assert_rejected(
            full_result(
                "missing artifact",
                CandidateHit::with_source("NOTICE", None, 0.8, CandidateSource::Lexical),
            ),
            &["path_unresolvable"],
        );

        let foreign = tempfile::tempdir().expect("foreign project");
        let foreign_artifact = foreign.path().join("LICENSE");
        std::fs::write(&foreign_artifact, "foreign license\n").expect("write foreign artifact");
        assert_rejected(
            full_result(
                "foreign artifact",
                CandidateHit::with_source(
                    foreign_artifact.to_string_lossy(),
                    None,
                    0.8,
                    CandidateSource::Lexical,
                ),
            ),
            &["path_unresolvable"],
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            symlink(&foreign_artifact, project_root.join("ESCAPING_LICENSE"))
                .expect("create escaping symlink");
            assert_rejected(
                full_result(
                    "escaping artifact",
                    CandidateHit::with_source(
                        "ESCAPING_LICENSE",
                        None,
                        0.8,
                        CandidateSource::Lexical,
                    ),
                ),
                &["path_unresolvable"],
            );
        }

        let mut mixed_result = full_result(
            "mixed artifacts",
            CandidateHit::with_source("LICENSE", None, 0.8, CandidateSource::Lexical),
        );
        mixed_result.hits.push(CandidateHit::with_source(
            "src/missing.rs",
            None,
            0.7,
            CandidateSource::Lexical,
        ));
        assert_rejected(mixed_result, &["node_unresolved", "node_unresolved"]);

        let mut unavailable_mode = full_result(
            "license unavailable",
            CandidateHit::with_source("LICENSE", None, 0.8, CandidateSource::Lexical),
        );
        unavailable_mode.trace.retrieval_mode = "no_semantic".into();
        unavailable_mode.trace.degraded_reason = Some("fixture unavailable".into());
        assert_rejected(unavailable_mode, &["node_unresolved"]);

        let mut cancelled = full_result(
            "license cancelled",
            CandidateHit::with_source("LICENSE", None, 0.8, CandidateSource::Lexical),
        );
        cancelled.trace.cancel_reason = Some("deadline".into());
        assert_rejected(cancelled, &["node_unresolved"]);

        assert_eq!(artifact_diagnostic.resolved_hit_count, 0);
        assert_eq!(artifact_diagnostic.unresolved_candidate_count, 1);
        assert_eq!(artifact_diagnostic.blocking_unresolved_candidate_count, 0);

        match artifact_outcome {
            SidecarPrimarySearchOutcome::Served {
                hits,
                packet_hits,
                scored_hits,
                shadow,
            } => {
                assert!(
                    hits.is_empty(),
                    "artifact must not fabricate search evidence"
                );
                assert!(
                    packet_hits.is_empty(),
                    "artifact must not fabricate packet evidence"
                );
                assert!(
                    scored_hits.is_empty(),
                    "artifact must not fabricate scored evidence"
                );
                assert_eq!(shadow.candidate_count, 1);
                assert_eq!(shadow.resolved_hit_count, 0);
                assert_eq!(shadow.unresolved_candidate_count, 1);
                assert!(shadow.diagnostic_only);
                assert_eq!(shadow.candidates.len(), 1);
                assert_eq!(shadow.candidates[0].file_path, "LICENSE");
                assert_eq!(shadow.candidates[0].symbol_name, None);
                assert_eq!(
                    shadow.candidates[0].resolution.as_deref(),
                    Some("node_unresolved")
                );
                assert_eq!(
                    shadow.candidates[0].loss_reason.as_deref(),
                    Some("node_unresolved")
                );
            }
            SidecarPrimarySearchOutcome::Rejected { reason, shadow } => panic!(
                "an in-root lexical artifact outside core routing should serve no useful evidence; reason={reason}; shadow={shadow:?}"
            ),
            SidecarPrimarySearchOutcome::Unavailable { reason } => {
                panic!("artifact resolution should remain available: {reason}")
            }
            SidecarPrimarySearchOutcome::Retryable { error } => {
                panic!("artifact resolution should not retry: {error:?}")
            }
        }
    }

    #[test]
    fn sidecar_primary_serves_empty_full_result_without_artifact_diagnostic() {
        use codestory_retrieval::classify_query;

        let fixture = pinned_operation_fixture();
        let outcome = sidecar_primary_search_outcome_from_query_result(
            &fixture.controller,
            QueryResult {
                publication_identity: None,
                query: "unmatched".into(),
                features: classify_query("unmatched"),
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
            },
            5,
        );

        match outcome {
            SidecarPrimarySearchOutcome::Served {
                hits,
                packet_hits,
                shadow,
                ..
            } => {
                assert!(hits.is_empty());
                assert!(packet_hits.is_empty());
                assert_eq!(shadow.candidate_count, 0);
                assert_eq!(shadow.unresolved_candidate_count, 0);
                assert!(!shadow.diagnostic_only);
            }
            _ => panic!("an ordinary empty full result must remain served"),
        }
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
            unindexed_lexical_artifacts_only: false,
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
            unindexed_lexical_artifacts_only: false,
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
            unindexed_lexical_artifacts_only: false,
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
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
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

        let controller = AppController::new_with_owned_cache_root(process_cache.path());
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
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        use codestory_retrieval::CandidateSource;

        let temp = tempfile::tempdir().expect("tempdir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("storage parent"))
            .expect("create storage parent");
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
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
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        use codestory_retrieval::classify_query;

        let temp = tempfile::tempdir().expect("tempdir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("storage parent"))
            .expect("create storage parent");
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
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
    fn free_query_batch_checks_later_policy_mismatch_before_any_hydration() {
        let fixture = pinned_operation_fixture();
        let first = query_result_with_stages(Vec::new());
        let mut second = first.clone();
        second.features.lexical_policy = LexicalAdmissionPolicy::BroadQuestionV1;
        let query = first.query.clone();
        let result = build_sidecar_packet_batch_outcome(
            &fixture.controller,
            &[(query.clone(), 4), (query, 4)],
            vec![first, second],
            0,
            &HashSet::new(),
            |_, _| {
                panic!("no candidate may hydrate before all query/policy positions authenticate")
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn packet_batch_rejects_candidate_resolution_errors() {
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        use codestory_retrieval::CandidateSource;

        let temp = tempfile::tempdir().expect("tempdir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
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
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        use codestory_retrieval::CandidateSource;

        let temp = tempfile::tempdir().expect("tempdir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
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
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
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

        let controller = AppController::new_with_owned_cache_root(process_cache.path());
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
        let fixture =
            pinned_operation_fixture_with_profile(codestory_retrieval::SidecarProfile::Agent);
        let pinned = Rc::new(
            PinnedRetrievalRead::begin(&fixture.controller)
                .expect("begin a validated Agent pin before opt-out"),
        );
        // SAFETY: test-only env mutation; no concurrent tests rely on this variable.
        unsafe {
            std::env::set_var(RETRIEVAL_ENV, "0");
        }
        assert_eq!(retrieval_env_override(), Some(false));
        let primary_enabled =
            with_active_pinned_retrieval_read(&fixture.controller, pinned, || {
                sidecar_retrieval_primary_enabled(&fixture.controller)
            });
        // SAFETY: test-only env cleanup.
        unsafe {
            std::env::remove_var(RETRIEVAL_ENV);
        }
        assert!(
            !primary_enabled,
            "explicit retrieval opt-out wins over an otherwise validated active pin"
        );
        assert_eq!(retrieval_env_override(), None);
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
