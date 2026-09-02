//! Runtime-only packet candidates that keep graph proof beside public search hits.

#[cfg(test)]
use codestory_contracts::api::EdgeKind;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, EdgeId, GraphArtifactDto, GraphResponse, SearchHit,
};
#[cfg(test)]
use codestory_contracts::compilation::PACKET_RETRIEVAL_SCORE_VERSION_V1;
use codestory_contracts::compilation::{
    INTERIM_MAX_ADMITTED_CANDIDATES, INTERIM_MAX_ADMITTED_SOURCE_BYTES, PacketAdmissionGapKindV1,
    PacketAdmissionGapV1, PacketAdmissionOriginV1, PacketAdmissionReceiptV1,
    PacketCandidateDescriptorV1,
};
use codestory_contracts::graph::NodeId as CoreNodeId;
use codestory_retrieval::{QueryResult, RetrievalPublicationIdentity};
use sha2::{Digest, Sha256};
#[cfg(any(test, feature = "benchmark-support"))]
use std::cell::Cell;
use std::cell::RefCell;
#[cfg(any(test, feature = "benchmark-support"))]
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::rc::Rc;

const PACKET_CANDIDATE_SELECTION_VIEW_ID: &str = "packet-search-provenance";
const PACKET_CANDIDATE_SELECTION_VIEW_ID_PREFIX: &str = "packet-search-provenance-";
const PACKET_CANDIDATE_GRAPH_EDGE_LIMIT: usize = 20;
const PACKET_CITATION_EDGE_LIMIT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PacketGraphDirection {
    Outgoing,
    Incoming,
}

/// Thread-scoped Horizon A admission counters for one packet operation.
///
/// Formula hydration, trail-scan ledgers, and promotion need-sets no longer
/// live here. The session only bounds how many candidates may hydrate and how
/// many source bytes they may charge before exact hydration.
#[derive(Debug)]
pub(crate) struct PacketProofSession {
    /// Packet-scoped count of candidates selected for source/graph hydration.
    /// Horizon A admits at most [`INTERIM_MAX_ADMITTED_CANDIDATES`] across the
    /// whole packet before hydration, not per subquery.
    pub(crate) hydrated_admissions: RefCell<usize>,
    /// Conservative source bytes charged before exact hydration.
    pub(crate) admitted_source_bytes: RefCell<usize>,
    admitted_identities: RefCell<HashMap<String, usize>>,
    receipts: RefCell<Vec<PacketAdmissionReceiptV1>>,
    gaps: RefCell<Vec<PacketAdmissionGapV1>>,
    retrieval_admission_sealed: RefCell<bool>,
    descriptor_results: RefCell<Vec<RetainedPacketDescriptorResult>>,
    include_dense_semantic: bool,
    #[cfg(any(test, feature = "benchmark-support"))]
    descriptor_query_count: Cell<u32>,
    #[cfg(any(test, feature = "benchmark-support"))]
    descriptor_cache_hit_count: Cell<u32>,
    #[cfg(any(test, feature = "benchmark-support"))]
    descriptor_stage_invocations: RefCell<BTreeMap<String, u32>>,
    #[cfg(any(test, feature = "benchmark-support"))]
    descriptor_stage_candidates: RefCell<BTreeMap<String, u64>>,
}

#[derive(Debug, Clone)]
struct RetainedPacketDescriptorResult {
    query: String,
    include_dense_semantic: bool,
    publication: RetrievalPublicationIdentity,
    result: QueryResult,
}

impl Default for PacketProofSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "benchmark-support"))]
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct BenchmarkPacketRetrievalProof {
    pub contract: &'static str,
    pub requested_policy: &'static str,
    pub descriptor_query_count: u32,
    pub descriptor_cache_hit_count: u32,
    pub descriptor_stage_invocations: BTreeMap<String, u32>,
    pub descriptor_stage_candidates: BTreeMap<String, u64>,
    pub dense_semantic_stage_invocations: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PacketAdmissionDecision {
    Admitted,
    AlreadyAdmitted,
    CountBudgetExceeded,
    SourceBudgetExceeded,
}

impl PacketProofSession {
    pub(crate) fn new() -> Self {
        Self {
            hydrated_admissions: RefCell::new(0),
            admitted_source_bytes: RefCell::new(0),
            admitted_identities: RefCell::new(HashMap::new()),
            receipts: RefCell::new(Vec::new()),
            gaps: RefCell::new(Vec::new()),
            retrieval_admission_sealed: RefCell::new(false),
            descriptor_results: RefCell::new(Vec::new()),
            include_dense_semantic: true,
            #[cfg(any(test, feature = "benchmark-support"))]
            descriptor_query_count: Cell::new(0),
            #[cfg(any(test, feature = "benchmark-support"))]
            descriptor_cache_hit_count: Cell::new(0),
            #[cfg(any(test, feature = "benchmark-support"))]
            descriptor_stage_invocations: RefCell::new(BTreeMap::new()),
            #[cfg(any(test, feature = "benchmark-support"))]
            descriptor_stage_candidates: RefCell::new(BTreeMap::new()),
        }
    }

    #[cfg(feature = "benchmark-support")]
    pub(crate) fn without_dense_semantic_for_benchmark() -> Self {
        Self {
            include_dense_semantic: false,
            ..Self::new()
        }
    }

    pub(crate) fn includes_dense_semantic(&self) -> bool {
        self.include_dense_semantic
    }

    #[cfg(any(test, feature = "benchmark-support"))]
    pub(crate) fn record_descriptor_trace(&self, trace: &codestory_retrieval::QueryTrace) {
        self.descriptor_query_count
            .set(self.descriptor_query_count.get().saturating_add(1));
        if trace.cache_hit {
            self.descriptor_cache_hit_count
                .set(self.descriptor_cache_hit_count.get().saturating_add(1));
        }
        let mut invocations = self.descriptor_stage_invocations.borrow_mut();
        let mut candidates = self.descriptor_stage_candidates.borrow_mut();
        for stage in &trace.stages {
            if stage.completion_status != codestory_retrieval::StageCompletionStatus::Completed {
                continue;
            }
            let label = stage.stage.label().to_string();
            let invocation_count = invocations.entry(label.clone()).or_default();
            *invocation_count = invocation_count.saturating_add(1);
            let candidate_count = candidates.entry(label).or_default();
            *candidate_count = candidate_count.saturating_add(stage.candidates_added as u64);
        }
    }

    #[cfg(any(test, feature = "benchmark-support"))]
    pub(crate) fn benchmark_retrieval_proof(&self) -> BenchmarkPacketRetrievalProof {
        let descriptor_stage_invocations = self.descriptor_stage_invocations.borrow().clone();
        BenchmarkPacketRetrievalProof {
            contract: "codestory.packet-dense-candidate-ablation-proof/v1",
            requested_policy: if self.include_dense_semantic {
                "repository_graph_lexical_dense_candidate_stage_enabled_v1"
            } else {
                "repository_graph_lexical_dense_candidate_stage_disabled_v1"
            },
            descriptor_query_count: self.descriptor_query_count.get(),
            descriptor_cache_hit_count: self.descriptor_cache_hit_count.get(),
            dense_semantic_stage_invocations: descriptor_stage_invocations
                .get("stage1b_semantic")
                .copied()
                .unwrap_or(0),
            descriptor_stage_invocations,
            descriptor_stage_candidates: self.descriptor_stage_candidates.borrow().clone(),
        }
    }

    /// Retain the exact descriptor result that authorized packet admission.
    ///
    /// The key includes the query, retrieval policy, and full immutable
    /// publication. A later hydration stage may reuse only an identical key;
    /// it cannot silently rerun under a shorter budget or cross a packet,
    /// policy, or publication boundary.
    pub(crate) fn retain_descriptor_result(
        &self,
        result: &QueryResult,
        include_dense_semantic: bool,
        publication: &RetrievalPublicationIdentity,
    ) -> Result<(), &'static str> {
        if result.publication_identity.as_ref() != Some(publication) {
            return Err("descriptor result publication does not match the packet pin");
        }
        let mut retained = self.descriptor_results.borrow_mut();
        if retained.iter().any(|entry| {
            entry.query == result.query
                && entry.include_dense_semantic == include_dense_semantic
                && entry.publication == *publication
        }) {
            return Err("descriptor result was retained twice for one packet query");
        }
        retained.push(RetainedPacketDescriptorResult {
            query: result.query.clone(),
            include_dense_semantic,
            publication: publication.clone(),
            result: result.clone(),
        });
        Ok(())
    }

    pub(crate) fn descriptor_result(
        &self,
        query: &str,
        include_dense_semantic: bool,
        publication: &RetrievalPublicationIdentity,
    ) -> Option<QueryResult> {
        self.descriptor_results
            .borrow()
            .iter()
            .find(|entry| {
                entry.query == query
                    && entry.include_dense_semantic == include_dense_semantic
                    && entry.publication == *publication
            })
            .map(|entry| entry.result.clone())
    }

    pub(crate) fn remaining_hydration_slots(&self) -> usize {
        INTERIM_MAX_ADMITTED_CANDIDATES.saturating_sub(*self.hydrated_admissions.borrow())
    }

    pub(crate) fn remaining_source_bytes(&self) -> usize {
        INTERIM_MAX_ADMITTED_SOURCE_BYTES.saturating_sub(*self.admitted_source_bytes.borrow())
    }

    #[cfg(test)]
    pub(crate) fn admit(
        &self,
        stable_identity: &str,
        source_bytes: usize,
    ) -> PacketAdmissionDecision {
        self.admit_with_receipt(
            stable_identity,
            source_bytes,
            PacketAdmissionOriginV1::Retrieval,
            PACKET_RETRIEVAL_SCORE_VERSION_V1,
            None,
        )
    }

    pub(crate) fn admit_exact_selector(
        &self,
        stable_identity: &str,
        source_bytes: usize,
        selector_ordinal: u32,
    ) -> PacketAdmissionDecision {
        self.admit_with_receipt(
            stable_identity,
            source_bytes,
            PacketAdmissionOriginV1::ExactTypedSelector,
            "exact-selector/v1",
            Some(selector_ordinal),
        )
    }

    pub(crate) fn admit_descriptor(
        &self,
        descriptor: &PacketCandidateDescriptorV1,
    ) -> PacketAdmissionDecision {
        let origin = if descriptor.exact_selector_ordinal.is_some() {
            PacketAdmissionOriginV1::ExactTypedSelector
        } else {
            PacketAdmissionOriginV1::Retrieval
        };
        if self
            .admitted_identities
            .borrow()
            .contains_key(&descriptor.stable_identity)
        {
            return PacketAdmissionDecision::AlreadyAdmitted;
        }
        if *self.retrieval_admission_sealed.borrow() {
            self.record_gap(
                PacketAdmissionGapKindV1::CandidateCountExceeded,
                unadmitted_gap_identity(origin, &descriptor.stable_identity),
                descriptor.exact_selector_ordinal,
            );
            return PacketAdmissionDecision::CountBudgetExceeded;
        }
        let Some(source_bytes) = descriptor.source_bytes_upper_bound else {
            self.record_gap(
                PacketAdmissionGapKindV1::SourceBoundMissing,
                unadmitted_gap_identity(origin, &descriptor.stable_identity),
                descriptor.exact_selector_ordinal,
            );
            return PacketAdmissionDecision::SourceBudgetExceeded;
        };
        self.admit_with_receipt(
            &descriptor.stable_identity,
            source_bytes as usize,
            origin,
            &descriptor.retrieval_score.version,
            descriptor.exact_selector_ordinal,
        )
    }

    pub(crate) fn seal_retrieval_admission(&self) {
        *self.retrieval_admission_sealed.borrow_mut() = true;
    }

    fn admit_with_receipt(
        &self,
        stable_identity: &str,
        source_bytes: usize,
        origin: PacketAdmissionOriginV1,
        score_version: &str,
        exact_selector_ordinal: Option<u32>,
    ) -> PacketAdmissionDecision {
        if self
            .admitted_identities
            .borrow()
            .contains_key(stable_identity)
        {
            return PacketAdmissionDecision::AlreadyAdmitted;
        }
        if self.remaining_hydration_slots() == 0 {
            self.record_gap(
                PacketAdmissionGapKindV1::CandidateCountExceeded,
                unadmitted_gap_identity(origin, stable_identity),
                exact_selector_ordinal,
            );
            return PacketAdmissionDecision::CountBudgetExceeded;
        }
        if source_bytes > self.remaining_source_bytes() {
            self.record_gap(
                PacketAdmissionGapKindV1::SourceBudgetExceeded,
                unadmitted_gap_identity(origin, stable_identity),
                exact_selector_ordinal,
            );
            return PacketAdmissionDecision::SourceBudgetExceeded;
        }

        self.admitted_identities
            .borrow_mut()
            .insert(stable_identity.to_owned(), source_bytes);
        *self.hydrated_admissions.borrow_mut() += 1;
        *self.admitted_source_bytes.borrow_mut() += source_bytes;
        let packet_ordinal = self.receipts.borrow().len() as u32;
        self.receipts.borrow_mut().push(PacketAdmissionReceiptV1 {
            packet_ordinal,
            stable_identity: stable_identity.to_string(),
            score_version: score_version.to_string(),
            reserved_source_bytes: u32::try_from(source_bytes).unwrap_or(u32::MAX),
            origin,
        });
        PacketAdmissionDecision::Admitted
    }

    pub(crate) fn record_ineligible_candidate(
        &self,
        kind: PacketAdmissionGapKindV1,
        stable_identity: Option<String>,
    ) {
        self.record_gap(kind, stable_identity, None);
    }

    fn record_gap(
        &self,
        kind: PacketAdmissionGapKindV1,
        stable_identity: Option<String>,
        exact_selector_ordinal: Option<u32>,
    ) {
        self.gaps.borrow_mut().push(PacketAdmissionGapV1 {
            kind,
            stable_identity,
            exact_selector_ordinal,
        });
    }

    pub(crate) fn receipts(&self) -> Vec<PacketAdmissionReceiptV1> {
        self.receipts.borrow().clone()
    }

    pub(crate) fn gaps(&self) -> Vec<PacketAdmissionGapV1> {
        self.gaps.borrow().clone()
    }

    pub(crate) fn is_admitted_identity(&self, stable_identity: &str) -> bool {
        self.admitted_identities
            .borrow()
            .contains_key(stable_identity)
    }

    pub(crate) fn is_admitted_node(&self, node_id: CoreNodeId) -> bool {
        self.is_admitted_identity(&format!("node:{}", node_id.0))
    }

    /// Keep a failed selector's reservation charged while hiding its synthetic
    /// pre-resolution identity from compiler input. Repository reads are never
    /// refunded into later hydration capacity.
    pub(crate) fn consume_unresolved_reservation(&self, identity: &str) {
        self.receipts
            .borrow_mut()
            .retain(|receipt| receipt.stable_identity != identity);
    }

    pub(crate) fn canonicalize_identity(&self, reserved: &str, stable: &str) {
        if reserved == stable {
            return;
        }
        let Some(source_bytes) = self.admitted_identities.borrow_mut().remove(reserved) else {
            return;
        };
        if self.admitted_identities.borrow().contains_key(stable) {
            let admitted_count = self.hydrated_admissions.borrow().saturating_sub(1);
            let admitted_bytes = self
                .admitted_source_bytes
                .borrow()
                .saturating_sub(source_bytes);
            *self.hydrated_admissions.borrow_mut() = admitted_count;
            *self.admitted_source_bytes.borrow_mut() = admitted_bytes;
            self.receipts
                .borrow_mut()
                .retain(|receipt| receipt.stable_identity != reserved);
        } else {
            self.admitted_identities
                .borrow_mut()
                .insert(stable.to_owned(), source_bytes);
            if let Some(receipt) = self
                .receipts
                .borrow_mut()
                .iter_mut()
                .find(|receipt| receipt.stable_identity == reserved)
            {
                receipt.stable_identity = stable.to_owned();
            }
        }
    }
}

fn unadmitted_gap_identity(
    origin: PacketAdmissionOriginV1,
    stable_identity: &str,
) -> Option<String> {
    (origin == PacketAdmissionOriginV1::ExactTypedSelector).then(|| stable_identity.to_string())
}

thread_local! {
    static ACTIVE_PACKET_PROOF_SESSION: RefCell<Option<Rc<PacketProofSession>>> =
        const { RefCell::new(None) };
}

pub(crate) fn active_packet_proof_session() -> Option<Rc<PacketProofSession>> {
    ACTIVE_PACKET_PROOF_SESSION.with(|active| active.borrow().clone())
}

pub(crate) struct PacketProofSessionGuard {
    previous: Option<Rc<PacketProofSession>>,
}

impl Drop for PacketProofSessionGuard {
    fn drop(&mut self) {
        ACTIVE_PACKET_PROOF_SESSION.with(|active| {
            active.replace(self.previous.take());
        });
    }
}

/// Installs the packet proof session for the current thread until the guard
/// drops (same scoped pattern as the pinned-retrieval read).
pub(crate) fn install_packet_proof_session(
    session: Rc<PacketProofSession>,
) -> PacketProofSessionGuard {
    let previous = ACTIVE_PACKET_PROOF_SESSION.with(|active| active.replace(Some(session)));
    PacketProofSessionGuard { previous }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacketGraphEdgeProvenance {
    pub(crate) edge_id: EdgeId,
    pub(crate) direction: PacketGraphDirection,
    pub(crate) hop: u32,
    pub(crate) producers: Vec<String>,
    pub(crate) certainty: Option<String>,
}

/// A packet-only search result. Public search DTOs stay unchanged while exact graph proof remains
/// attached until the packet citation and graph artifact are assembled.
#[derive(Debug, Clone)]
pub(crate) struct PacketSearchHit {
    pub(crate) hit: SearchHit,
    pub(crate) graph_provenance: Vec<PacketGraphEdgeProvenance>,
    pub(crate) graph: Option<GraphResponse>,
}

impl PacketSearchHit {
    #[cfg(test)]
    pub(crate) fn without_graph(hit: SearchHit) -> Self {
        Self {
            hit,
            graph_provenance: Vec::new(),
            graph: None,
        }
    }

    pub(crate) fn citation(&self, include_evidence: bool) -> AgentCitationDto {
        let mut citation = codestory_agent::citation::to_citation_from_hit(
            &self.hit,
            None,
            None,
            include_evidence,
        );
        // Search DTOs retain this legacy field for non-packet callers. The
        // packet compiler must never receive answer-sufficiency authority from
        // retrieval metadata.
        citation.eligible_for_sufficiency = None;
        if include_evidence && self.hit.resolvable {
            // Empty-requirement path: proof_edge_ids is empty, so the dense-only
            // upgrade never fires. Citation edges are every provenance edge
            // present in the graph (CALL filter disabled), truncated to 12.
            citation.evidence_edge_ids = self.selected_edge_ids(PACKET_CITATION_EDGE_LIMIT);
        }
        citation
    }

    #[cfg(test)]
    pub(crate) fn has_proof_call_provenance(&self) -> bool {
        let Some(graph) = self.graph.as_ref() else {
            return false;
        };
        let provenance_ids = self
            .graph_provenance
            .iter()
            .map(|provenance| &provenance.edge_id)
            .collect::<HashSet<_>>();
        graph.edges.iter().any(|edge| {
            provenance_ids.contains(&edge.id)
                && edge.kind == EdgeKind::CALL
                && (edge.certainty.as_deref() == Some("certain")
                    || (edge.certainty.is_none()
                        && edge.confidence.is_none()
                        && edge.callsite_identity.as_deref().is_some_and(|identity| {
                            identity.contains("|receiver-owner:")
                                && identity.split('|').any(|segment| {
                                    segment.starts_with("syntax:") && segment.ends_with("-call")
                                })
                        })))
        })
    }

    fn selected_edge_ids(&self, limit: usize) -> Vec<EdgeId> {
        let Some(graph) = self.graph.as_ref() else {
            return Vec::new();
        };
        let graph_edge_ids = graph
            .edges
            .iter()
            .map(|edge| &edge.id)
            .collect::<HashSet<_>>();
        let mut selected = self
            .graph_provenance
            .iter()
            .map(|provenance| provenance.edge_id.clone())
            .filter(|edge_id| graph_edge_ids.contains(edge_id))
            .collect::<Vec<_>>();
        selected.sort_by(|left, right| left.0.cmp(&right.0));
        selected.dedup();
        selected.truncate(limit);
        selected
    }

    fn graph_for_citation(&self) -> Option<GraphResponse> {
        let graph = self.graph.as_ref()?;
        let graph_edge_ids = graph
            .edges
            .iter()
            .map(|edge| &edge.id)
            .collect::<HashSet<_>>();
        let mut selected_edge_ids = self
            .graph_provenance
            .iter()
            .map(|provenance| provenance.edge_id.clone())
            .filter(|edge_id| graph_edge_ids.contains(edge_id))
            .collect::<Vec<_>>();
        selected_edge_ids.sort_by(|left, right| left.0.cmp(&right.0));
        selected_edge_ids.dedup();
        selected_edge_ids.truncate(PACKET_CANDIDATE_GRAPH_EDGE_LIMIT);
        let selected_order = selected_edge_ids
            .iter()
            .enumerate()
            .map(|(index, edge_id)| (edge_id, index))
            .collect::<HashMap<_, _>>();
        let mut edges = graph
            .edges
            .iter()
            .filter(|edge| selected_order.contains_key(&edge.id))
            .cloned()
            .collect::<Vec<_>>();
        edges.sort_by_key(|edge| selected_order[&edge.id]);
        if edges.is_empty() {
            return None;
        }

        let retained_node_ids = edges
            .iter()
            .flat_map(|edge| [edge.source.clone(), edge.target.clone()])
            .chain(std::iter::once(graph.center_id.clone()))
            .collect::<HashSet<_>>();
        let nodes = graph
            .nodes
            .iter()
            .filter(|node| retained_node_ids.contains(&node.id))
            .cloned()
            .collect::<Vec<_>>();
        let candidate_omitted = graph.edges.len().saturating_sub(edges.len());
        Some(GraphResponse {
            center_id: graph.center_id.clone(),
            nodes,
            edges,
            truncated: graph.truncated || candidate_omitted > 0,
            omitted_edge_count: graph
                .omitted_edge_count
                .saturating_add(u32::try_from(candidate_omitted).unwrap_or(u32::MAX)),
            canonical_layout: None,
        })
    }
}

impl Deref for PacketSearchHit {
    type Target = SearchHit;

    fn deref(&self) -> &Self::Target {
        &self.hit
    }
}

/// Preserve one capped candidate view as one graph artifact. `GraphResponse::omitted_edge_count`
/// is artifact-local: it describes only the bounded source view fingerprinted into this artifact
/// and must not be summed across candidate artifacts. The ID is stable lineage for that original
/// view; a later output cap may remove a known retained edge and increment the local count without
/// changing lineage. Keeping overlapping views separate avoids inventing union arithmetic for
/// opaque omissions whose edge identities are unavailable.
pub(crate) fn merge_packet_candidate_graph(answer: &mut AgentAnswerDto, hit: &PacketSearchHit) {
    let Some(candidate_graph) = hit.graph_for_citation() else {
        return;
    };
    let artifact_id = packet_candidate_selection_view_id(&candidate_graph);
    if !answer.graphs.iter().any(|artifact| match artifact {
        GraphArtifactDto::Uml { id, .. } | GraphArtifactDto::Mermaid { id, .. } => {
            id == &artifact_id
        }
    }) {
        answer.graphs.push(GraphArtifactDto::Uml {
            id: artifact_id.clone(),
            title: "Packet search graph provenance".to_string(),
            graph: candidate_graph,
        });
    }
    if !answer.subgraph_ids.contains(&artifact_id) {
        answer.subgraph_ids.push(artifact_id);
    }
}

/// Immutable identity of the original bounded selection view, computed before any downstream
/// presentation cap. This is lineage, not a checksum of the graph's current serialized rows: a
/// later output cap may remove known rows and increase that view's omission count while the ID
/// remains stable. Replaying the same candidate therefore finds the existing lineage and must not
/// restore optional rows deliberately removed to meet the packet budget.
fn packet_candidate_selection_view_id(graph: &GraphResponse) -> String {
    let mut edge_ids = graph
        .edges
        .iter()
        .map(|edge| edge.id.0.as_str())
        .collect::<Vec<_>>();
    edge_ids.sort_unstable();

    let mut digest = Sha256::new();
    hash_graph_id_component(&mut digest, "immutable-candidate-selection-view-v1");
    hash_graph_id_component(&mut digest, &graph.center_id.0);
    for edge_id in edge_ids {
        hash_graph_id_component(&mut digest, edge_id);
    }
    digest.update([u8::from(graph.truncated)]);
    digest.update(graph.omitted_edge_count.to_le_bytes());
    let fingerprint = digest.finalize();
    format!("{PACKET_CANDIDATE_SELECTION_VIEW_ID}-{fingerprint:x}")
}

pub(crate) fn is_packet_candidate_selection_view_id(id: &str) -> bool {
    id.strip_prefix(PACKET_CANDIDATE_SELECTION_VIEW_ID_PREFIX)
        .is_some_and(|fingerprint| {
            fingerprint.len() == 64 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn hash_graph_id_component(digest: &mut Sha256, value: &str) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_le_bytes());
    digest.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    use codestory_contracts::api::{
        AgentRetrievalTraceDto, GraphEdgeDto, GraphNodeDto, NodeId, NodeKind,
        PacketEvidenceResolutionDto, PacketEvidenceTierDto, SearchHitOrigin,
    };
    use codestory_contracts::compilation::{PacketRetrievalLaneV1, VersionedRetrievalScoreV1};

    #[cfg(feature = "benchmark-support")]
    #[test]
    fn benchmark_retrieval_proof_records_executed_descriptor_stages() {
        assert!(
            PacketProofSession::default().includes_dense_semantic(),
            "enabling benchmark support must not change the normal packet default"
        );
        let session = PacketProofSession::without_dense_semantic_for_benchmark();
        session.record_descriptor_trace(&codestory_retrieval::QueryTrace {
            retrieval_mode: "full".into(),
            degraded_reason: None,
            total_budget_ms: 800,
            elapsed_ms: 4,
            cancel_reason: None,
            cache_hit: false,
            stages: vec![
                codestory_retrieval::StageTrace {
                    stage: codestory_retrieval::RetrievalStageKind::Stage1Lexical,
                    budget_ms: 200,
                    elapsed_ms: 4,
                    admission_wait_ms: 0,
                    queue_wait_ms: Some(0),
                    execution_ms: Some(4),
                    candidates_added: 3,
                    marginal_gain: 1.0,
                    cancel_reason: None,
                    cache_hit: false,
                    degraded: false,
                    stub_reason: None,
                    completion_status: codestory_retrieval::StageCompletionStatus::Completed,
                },
                codestory_retrieval::StageTrace {
                    stage: codestory_retrieval::RetrievalStageKind::Stage1bSemantic,
                    budget_ms: 200,
                    elapsed_ms: 0,
                    admission_wait_ms: 0,
                    queue_wait_ms: None,
                    execution_ms: None,
                    candidates_added: 0,
                    marginal_gain: 0.0,
                    cancel_reason: Some("unique_exact_definition".into()),
                    cache_hit: false,
                    degraded: false,
                    stub_reason: None,
                    completion_status: codestory_retrieval::StageCompletionStatus::Skipped,
                },
            ],
        });

        let proof = session.benchmark_retrieval_proof();
        assert_eq!(proof.descriptor_query_count, 1);
        assert_eq!(proof.descriptor_cache_hit_count, 0);
        assert_eq!(proof.dense_semantic_stage_invocations, 0);
        assert!(
            !proof
                .descriptor_stage_invocations
                .contains_key("stage1b_semantic")
        );
        assert_eq!(proof.descriptor_stage_invocations["stage1_lexical"], 1);
        assert_eq!(proof.descriptor_stage_candidates["stage1_lexical"], 3);
        assert_eq!(
            proof.requested_policy,
            "repository_graph_lexical_dense_candidate_stage_disabled_v1"
        );
    }

    #[test]
    fn admission_is_packet_wide_identity_deduplicated_and_count_bounded() {
        let session = PacketProofSession::new();
        for index in 0..INTERIM_MAX_ADMITTED_CANDIDATES {
            assert_eq!(
                session.admit(&format!("node:{index}"), 1),
                PacketAdmissionDecision::Admitted
            );
        }
        assert_eq!(
            session.admit("node:0", 1),
            PacketAdmissionDecision::AlreadyAdmitted
        );
        assert_eq!(
            session.admit("node:17", 1),
            PacketAdmissionDecision::CountBudgetExceeded
        );
        assert_eq!(*session.hydrated_admissions.borrow(), 16);
    }

    #[test]
    fn exact_selectors_and_retrieval_share_one_sixteen_identity_session() {
        let session = PacketProofSession::new();
        for index in 0..8 {
            assert_eq!(
                session.admit_exact_selector(&format!("node:exact-{index}"), 1, index),
                PacketAdmissionDecision::Admitted
            );
        }
        for index in 0..8 {
            let descriptor = PacketCandidateDescriptorV1 {
                stable_identity: format!("node:retrieved-{index}"),
                path: format!("src/retrieved-{index}.rs"),
                symbol: Some(format!("retrieved_{index}")),
                retrieval_lane: PacketRetrievalLaneV1::Lexical,
                retrieval_score: VersionedRetrievalScoreV1 {
                    version: PACKET_RETRIEVAL_SCORE_VERSION_V1.to_string(),
                    value: 1.0 - index as f32 / 100.0,
                },
                source_bytes_upper_bound: Some(1),
                exact_selector_ordinal: None,
            };
            assert_eq!(
                session.admit_descriptor(&descriptor),
                PacketAdmissionDecision::Admitted
            );
        }

        let rejected = PacketCandidateDescriptorV1 {
            stable_identity: "node:seventeenth".into(),
            path: "src/seventeenth.rs".into(),
            symbol: Some("seventeenth".into()),
            retrieval_lane: PacketRetrievalLaneV1::Semantic,
            retrieval_score: VersionedRetrievalScoreV1 {
                version: PACKET_RETRIEVAL_SCORE_VERSION_V1.to_string(),
                value: 0.5,
            },
            source_bytes_upper_bound: Some(1),
            exact_selector_ordinal: None,
        };
        assert_eq!(
            session.admit_descriptor(&rejected),
            PacketAdmissionDecision::CountBudgetExceeded
        );
        assert_eq!(session.receipts().len(), 16);
        assert_eq!(session.gaps().len(), 1);
        assert_eq!(
            session.gaps()[0].kind,
            PacketAdmissionGapKindV1::CandidateCountExceeded
        );
    }

    #[test]
    fn admission_rejects_source_overflow_before_mutating_the_session() {
        let session = PacketProofSession::new();
        assert_eq!(
            session.admit("node:oversized", INTERIM_MAX_ADMITTED_SOURCE_BYTES + 1),
            PacketAdmissionDecision::SourceBudgetExceeded
        );
        assert_eq!(*session.hydrated_admissions.borrow(), 0);
        assert_eq!(*session.admitted_source_bytes.borrow(), 0);
    }

    #[test]
    fn sealed_retrieval_admission_rejects_late_descriptors() {
        let session = PacketProofSession::new();
        session.seal_retrieval_admission();
        let descriptor = PacketCandidateDescriptorV1 {
            stable_identity: "node:late".into(),
            path: "src/late.rs".into(),
            symbol: Some("late".into()),
            retrieval_lane: PacketRetrievalLaneV1::Lexical,
            retrieval_score: VersionedRetrievalScoreV1 {
                version: PACKET_RETRIEVAL_SCORE_VERSION_V1.to_string(),
                value: 1.0,
            },
            source_bytes_upper_bound: Some(1),
            exact_selector_ordinal: None,
        };
        assert_eq!(
            session.admit_descriptor(&descriptor),
            PacketAdmissionDecision::CountBudgetExceeded
        );
        assert!(session.receipts().is_empty());
    }

    #[test]
    fn retrieval_overflow_never_exposes_an_unauthenticated_identity() {
        let session = PacketProofSession::new();
        for index in 0..INTERIM_MAX_ADMITTED_CANDIDATES {
            assert_eq!(
                session.admit(&format!("node:{index}"), 1),
                PacketAdmissionDecision::Admitted
            );
        }
        let stale = PacketCandidateDescriptorV1 {
            stable_identity: "node:999999".into(),
            path: "src/stale.rs".into(),
            symbol: Some("stale".into()),
            retrieval_lane: PacketRetrievalLaneV1::Lexical,
            retrieval_score: VersionedRetrievalScoreV1 {
                version: PACKET_RETRIEVAL_SCORE_VERSION_V1.to_string(),
                value: 0.1,
            },
            source_bytes_upper_bound: Some(1),
            exact_selector_ordinal: None,
        };

        assert_eq!(
            session.admit_descriptor(&stale),
            PacketAdmissionDecision::CountBudgetExceeded
        );
        assert_eq!(session.gaps().len(), 1);
        assert_eq!(session.gaps()[0].stable_identity, None);
    }

    fn answer() -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "answer".into(),
            prompt: "prompt".into(),
            summary: "summary".into(),
            freshness: None,
            sections: Vec::new(),
            citations: Vec::new(),
            subgraph_ids: Vec::new(),
            retrieval_version: "sidecar".into(),
            graphs: Vec::new(),
            source_coverage: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "r".into(),
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
                source_freshness_telemetry: None,
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        }
    }

    fn packet_hit(edge_id: &str) -> PacketSearchHit {
        let node_id = NodeId("2".into());
        PacketSearchHit {
            hit: SearchHit {
                node_id: node_id.clone(),
                display_name: "Session.send".into(),
                kind: NodeKind::METHOD,
                file_path: Some("requests/sessions.py".into()),
                line: Some(1),
                score: 0.8,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                match_quality: None,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                eligible_for_sufficiency: None,
                source_excerpt: None,
                verification_targets: Vec::new(),
                score_breakdown: None,
            },
            graph_provenance: vec![PacketGraphEdgeProvenance {
                edge_id: EdgeId(edge_id.into()),
                direction: PacketGraphDirection::Incoming,
                hop: 1,
                producers: vec!["scip_graph_projection".into()],
                certainty: Some("certain".into()),
            }],
            graph: Some(GraphResponse {
                center_id: node_id.clone(),
                nodes: [("1", "Session.request"), ("2", "Session.send")]
                    .into_iter()
                    .map(|(id, label)| GraphNodeDto {
                        id: NodeId(id.into()),
                        label: label.into(),
                        kind: NodeKind::METHOD,
                        depth: u32::from(id != "2"),
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: None,
                        qualified_name: None,
                        member_access: None,
                    })
                    .collect(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId(edge_id.into()),
                    source: NodeId("1".into()),
                    target: node_id,
                    kind: EdgeKind::CALL,
                    confidence: Some(1.0),
                    certainty: Some("certain".into()),
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            }),
        }
    }

    fn overlapping_candidate_hit(
        center: &str,
        edge_specs: &[(&str, &str, &str)],
        omitted_edge_count: u32,
    ) -> PacketSearchHit {
        let center_id = NodeId(center.into());
        let mut node_ids = edge_specs
            .iter()
            .flat_map(|(_, source, target)| [*source, *target])
            .collect::<Vec<_>>();
        node_ids.sort_unstable();
        node_ids.dedup();
        let edges = edge_specs
            .iter()
            .map(|(id, source, target)| GraphEdgeDto {
                id: EdgeId((*id).into()),
                source: NodeId((*source).into()),
                target: NodeId((*target).into()),
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".into()),
                callsite_identity: None,
                candidate_targets: Vec::new(),
            })
            .collect::<Vec<_>>();
        PacketSearchHit {
            hit: SearchHit {
                node_id: center_id.clone(),
                display_name: center.into(),
                kind: NodeKind::METHOD,
                file_path: Some("src/overlap.js".into()),
                line: Some(1),
                score: 0.8,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                match_quality: None,
                evidence_tier: Some(PacketEvidenceTierDto::ResolvedGraph),
                evidence_producer: Some("core_incident_call".into()),
                resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
                loss_reason: None,
                eligible_for_sufficiency: Some(true),
                source_excerpt: None,
                verification_targets: Vec::new(),
                score_breakdown: None,
            },
            graph_provenance: edges
                .iter()
                .map(|edge| PacketGraphEdgeProvenance {
                    edge_id: edge.id.clone(),
                    direction: if edge.source == center_id {
                        PacketGraphDirection::Outgoing
                    } else {
                        PacketGraphDirection::Incoming
                    },
                    hop: 1,
                    producers: vec!["core_incident_call".into()],
                    certainty: edge.certainty.clone(),
                })
                .collect(),
            graph: Some(GraphResponse {
                center_id: center_id.clone(),
                nodes: node_ids
                    .into_iter()
                    .map(|id| GraphNodeDto {
                        id: NodeId(id.into()),
                        label: id.into(),
                        kind: NodeKind::METHOD,
                        depth: u32::from(id != center),
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("src/overlap.js".into()),
                        qualified_name: Some(id.into()),
                        member_access: None,
                    })
                    .collect(),
                edges,
                truncated: omitted_edge_count > 0,
                omitted_edge_count,
                canonical_layout: None,
            }),
        }
    }

    #[test]
    fn citation_and_graph_keep_exact_packet_candidate_provenance() {
        let hit = packet_hit("edge-1");
        let citation = hit.citation(true);
        assert_eq!(citation.evidence_edge_ids, [EdgeId("edge-1".into())]);
        assert_eq!(
            citation.eligible_for_sufficiency, None,
            "packet citations carry retrieval provenance, never answer-sufficiency authority"
        );
        assert!(hit.has_proof_call_provenance());

        let mut answer = answer();
        merge_packet_candidate_graph(&mut answer, &hit);
        merge_packet_candidate_graph(&mut answer, &hit);
        let GraphArtifactDto::Uml { id, graph, .. } = &answer.graphs[0] else {
            panic!("expected UML graph");
        };
        assert_eq!(answer.graphs.len(), 1, "exact replay must be idempotent");
        assert_eq!(graph.edges.len(), 1);
        assert!(id.starts_with(PACKET_CANDIDATE_SELECTION_VIEW_ID));
        assert_eq!(answer.subgraph_ids, std::slice::from_ref(id));
    }

    #[test]
    fn overlapping_candidate_omissions_remain_artifact_local_and_replay_is_idempotent() {
        // A retains {a,b} and omits {c}; B retains {b,c} and omits {a}. The retained union is
        // complete, but opaque counts cannot prove whether the hidden identities overlap. Keep
        // the two bounded views separate instead of publishing a false aggregate omission of 2.
        let first = overlapping_candidate_hit(
            "candidate-a",
            &[
                ("a", "caller-a", "candidate-a"),
                ("b", "candidate-a", "candidate-b"),
            ],
            1,
        );
        let second = overlapping_candidate_hit(
            "candidate-b",
            &[
                ("b", "candidate-a", "candidate-b"),
                ("c", "candidate-b", "target-c"),
            ],
            1,
        );
        let complete = overlapping_candidate_hit(
            "candidate-complete",
            &[("complete", "candidate-complete", "target-complete")],
            0,
        );

        let mut merged = answer();
        for hit in [&first, &second, &first, &second, &complete, &complete] {
            merge_packet_candidate_graph(&mut merged, hit);
        }

        assert_eq!(
            merged.graphs.len(),
            3,
            "exact replays must not add artifacts"
        );
        assert_eq!(merged.subgraph_ids.len(), 3);
        assert_eq!(merged.subgraph_ids.iter().collect::<HashSet<_>>().len(), 3);

        let mut overlapping_views = merged
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. }
                    if graph.center_id.0 == "candidate-a" || graph.center_id.0 == "candidate-b" =>
                {
                    let mut ids = graph
                        .edges
                        .iter()
                        .map(|edge| edge.id.0.as_str())
                        .collect::<Vec<_>>();
                    ids.sort_unstable();
                    Some((graph.center_id.0.as_str(), ids, graph))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        overlapping_views.sort_by_key(|(center, _, _)| *center);
        assert_eq!(overlapping_views.len(), 2);
        assert_eq!(overlapping_views[0].1, ["a", "b"]);
        assert_eq!(overlapping_views[1].1, ["b", "c"]);
        for (_, _, graph) in &overlapping_views {
            assert!(graph.truncated, "one edge remains omitted from this view");
            assert_eq!(graph.omitted_edge_count, 1);
        }
        let retained_union = overlapping_views
            .iter()
            .flat_map(|(_, ids, _)| ids.iter().copied())
            .collect::<HashSet<_>>();
        assert_eq!(retained_union, HashSet::from(["a", "b", "c"]));
        assert!(
            overlapping_views
                .iter()
                .all(|(_, _, graph)| graph.omitted_edge_count != 2),
            "no artifact may claim a synthetic aggregate omission"
        );

        let complete_graph = merged
            .graphs
            .iter()
            .find_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. }
                    if graph.center_id.0 == "candidate-complete" =>
                {
                    Some(graph)
                }
                _ => None,
            })
            .expect("complete candidate view");
        assert!(!complete_graph.truncated);
        assert_eq!(complete_graph.omitted_edge_count, 0);
    }
}
