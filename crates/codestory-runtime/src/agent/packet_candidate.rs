//! Runtime-only packet candidates that keep graph proof beside public search hits.

#[cfg(test)]
use codestory_contracts::api::EdgeKind;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, EdgeId, GraphArtifactDto, GraphResponse, SearchHit,
};
#[cfg(test)]
use codestory_contracts::compilation::PACKET_RETRIEVAL_SCORE_VERSION_V1;
use codestory_contracts::compilation::{
    INTERIM_MAX_ADMITTED_CANDIDATES, INTERIM_MAX_ADMITTED_SOURCE_BYTES,
    INTERIM_SOURCE_CONTINUATION_UPPER_BOUND, PacketAdmissionGapKindV1, PacketAdmissionGapV1,
    PacketAdmissionOriginV1, PacketAdmissionReceiptV1, PacketCandidateDescriptorV1,
};
use codestory_contracts::graph::NodeId as CoreNodeId;
use codestory_retrieval::LexicalAdmissionPolicy;
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
    pub(crate) issued_source_record:
        RefCell<Option<super::packet_served_sources::ServedSourceRecord>>,
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
    admitted_call_annotations:
        RefCell<HashMap<String, codestory_contracts::compilation::PacketPublishedCallsV1>>,
    include_dense_semantic: bool,
    replay_ranked_descriptors: RefCell<Option<Vec<PacketCandidateDescriptorV1>>>,
    #[cfg(any(test, feature = "benchmark-support"))]
    descriptor_query_count: Cell<u32>,
    #[cfg(any(test, feature = "benchmark-support"))]
    descriptor_cache_hit_count: Cell<u32>,
    #[cfg(any(test, feature = "benchmark-support"))]
    descriptor_stage_invocations: RefCell<BTreeMap<String, u32>>,
    #[cfg(any(test, feature = "benchmark-support"))]
    descriptor_stage_candidates: RefCell<BTreeMap<String, u64>>,
}

/// Origin is retained before query text enters execution or result lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacketRetrievalQuery {
    pub query: String,
    pub lexical_policy: LexicalAdmissionPolicy,
}

impl PacketRetrievalQuery {
    #[cfg(test)]
    pub(crate) fn strict(query: &str) -> Self {
        Self {
            query: query.trim().to_string(),
            lexical_policy: LexicalAdmissionPolicy::StrictLiteralV1,
        }
    }
}

pub(crate) fn packet_retrieval_queries(
    plan: &codestory_contracts::api::PacketPlanDto,
) -> Vec<PacketRetrievalQuery> {
    plan.queries
        .iter()
        .map(|query| PacketRetrievalQuery {
            query: query.query.clone(),
            lexical_policy: if codestory_agent::packet_plan::packet_plan_query_is_typed_free_query(
                query,
            ) {
                LexicalAdmissionPolicy::StrictLiteralV1
            } else {
                LexicalAdmissionPolicy::BroadQuestionV1
            },
        })
        .collect()
}

#[derive(Debug, Clone)]
struct RetainedPacketDescriptorResult {
    lexical_policy: LexicalAdmissionPolicy,
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
    pub(crate) fn enable_replay_metadata(&self) {
        *self.replay_ranked_descriptors.borrow_mut() = Some(Vec::new());
    }

    pub(crate) fn retain_replay_ranked_descriptors(
        &self,
        descriptors: &[PacketCandidateDescriptorV1],
    ) {
        let mut retained = self.replay_ranked_descriptors.borrow_mut();
        if let Some(rows) = retained.as_mut() {
            let mut seen = HashSet::new();
            rows.extend(
                descriptors
                    .iter()
                    .filter(|row| seen.insert(row.stable_identity.clone()))
                    .cloned(),
            );
        }
    }

    pub(crate) fn replay_descriptor_metadata(&self) -> serde_json::Value {
        let lane = |evidence: Option<&codestory_retrieval::CandidateLaneEvidence>| {
            evidence.map(|evidence| {
                serde_json::json!({
                    "rank": evidence.rank,
                    "raw_score": evidence.raw_score,
                    "raw_score_bits": evidence.raw_score.to_bits(),
                })
            })
        };
        let queries = self
            .descriptor_results
            .borrow()
            .iter()
            .enumerate()
            .map(|(query_index, entry)| {
                let eligible_descriptors = entry
                    .result
                    .hits
                    .iter()
                    .enumerate()
                    .filter(|(_, hit)| !codestory_retrieval::is_phantom_sidecar_hit(hit))
                    .filter_map(|(returned_index, hit)| {
                        let descriptor = hit.packet_descriptor()?;
                        Some(serde_json::json!({
                            "returned_index": returned_index,
                            "stable_identity": descriptor.stable_identity,
                            "retrieval_lane": descriptor.retrieval_lane,
                            "retrieval_score": {
                                "version": descriptor.retrieval_score.version,
                                "value": descriptor.retrieval_score.value,
                                "bits": descriptor.retrieval_score.value.to_bits(),
                            },
                            "source_bytes_upper_bound": descriptor.source_bytes_upper_bound,
                            "lane_scores": {
                                "lexical": lane(hit.lane_scores.lexical.as_ref()),
                                "semantic": lane(hit.lane_scores.semantic.as_ref()),
                                "graph": lane(hit.lane_scores.graph.as_ref()),
                            },
                        }))
                    })
                    .collect::<Vec<_>>();
                serde_json::json!({
                    "query_index": query_index,
                    "query_sha256": format!("{:x}", Sha256::digest(entry.query.as_bytes())),
                    "query": entry.query,
                    "lexical_policy": format!("{:?}", entry.lexical_policy),
                    "include_dense_semantic": entry.include_dense_semantic,
                    "publication": entry.publication,
                    "candidate_count": entry.result.hits.len(),
                    "shape": entry.result.features.shape,
                    "trace": entry.result.trace,
                    "eligible_descriptors": eligible_descriptors,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "ranked_eligible_descriptors":*self.replay_ranked_descriptors.borrow(),
            "queries": queries,
            "unavailable_evidence": ["rank_features", "native_query_window_details"],
        })
    }

    pub(crate) fn new() -> Self {
        Self {
            hydrated_admissions: RefCell::new(0),
            issued_source_record: RefCell::new(None),
            admitted_source_bytes: RefCell::new(0),
            admitted_identities: RefCell::new(HashMap::new()),
            receipts: RefCell::new(Vec::new()),
            gaps: RefCell::new(Vec::new()),
            retrieval_admission_sealed: RefCell::new(false),
            descriptor_results: RefCell::new(Vec::new()),
            admitted_call_annotations: RefCell::new(HashMap::new()),
            include_dense_semantic: true,
            replay_ranked_descriptors: RefCell::new(None),
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

    pub(crate) fn retain_admitted_call_annotation(&self, descriptor: &PacketCandidateDescriptorV1) {
        let mut annotations = self.admitted_call_annotations.borrow_mut();
        if descriptor.published_calls_conflicted {
            annotations.remove(&descriptor.stable_identity);
            return;
        }
        if let Some(annotation) = &descriptor.published_calls {
            // Caller has already validated publication/owner through pure
            // admission ordering. Store only a currently admitted owner.
            if self
                .admitted_identities
                .borrow()
                .contains_key(&descriptor.stable_identity)
                && annotation
                    .neighborhood
                    .is_valid_for(&descriptor.stable_identity)
            {
                annotations.insert(descriptor.stable_identity.clone(), annotation.clone());
            }
        }
    }

    pub(crate) fn admitted_call_navigation(
        &self,
    ) -> Vec<codestory_contracts::compilation::PacketDirectedRelationV1> {
        use codestory_contracts::compilation::*;
        let admitted = self.admitted_identities.borrow();
        let mut links = Vec::new();
        for (source, annotation) in self.admitted_call_annotations.borrow().iter() {
            if !admitted.contains_key(source) {
                continue;
            }
            for target in &annotation.neighborhood.targets {
                if source == &target.stable_identity
                    || !admitted.contains_key(&target.stable_identity)
                {
                    continue;
                }
                links.push(PacketDirectedRelationV1 {
                    relation_id: format!("navigation:{}:{}", source, target.representative_edge_id),
                    from_identity: source.clone(),
                    to_identity: target.stable_identity.clone(),
                    relation_kind: PacketRelationKindV1::Call,
                    certainty: target.certainty,
                });
            }
        }
        links.sort_by(|a, b| {
            (&a.from_identity, &a.to_identity, &a.relation_id).cmp(&(
                &b.from_identity,
                &b.to_identity,
                &b.relation_id,
            ))
        });
        links.dedup_by(|a, b| a.from_identity == b.from_identity && a.to_identity == b.to_identity);
        links
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
    #[cfg(test)]
    pub(crate) fn retain_descriptor_result(
        &self,
        result: &QueryResult,
        include_dense_semantic: bool,
        publication: &RetrievalPublicationIdentity,
    ) -> Result<(), &'static str> {
        self.retain_descriptor_result_with_policy(
            result,
            LexicalAdmissionPolicy::StrictLiteralV1,
            include_dense_semantic,
            publication,
        )
    }

    pub(crate) fn retain_descriptor_result_with_policy(
        &self,
        result: &QueryResult,
        expected_policy: LexicalAdmissionPolicy,
        include_dense_semantic: bool,
        publication: &RetrievalPublicationIdentity,
    ) -> Result<(), &'static str> {
        if result.features.lexical_policy != expected_policy {
            return Err("descriptor execution policy does not match requested policy");
        }
        if result.publication_identity.as_ref() != Some(publication) {
            return Err("descriptor result publication does not match the packet pin");
        }
        let mut retained = self.descriptor_results.borrow_mut();
        if retained.iter().any(|entry| {
            entry.query == result.query
                && entry.lexical_policy == expected_policy
                && entry.include_dense_semantic == include_dense_semantic
                && entry.publication == *publication
        }) {
            return Err("descriptor result was retained twice for one packet query");
        }
        retained.push(RetainedPacketDescriptorResult {
            lexical_policy: expected_policy,
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
        self.descriptor_result_with_policy(
            query,
            LexicalAdmissionPolicy::StrictLiteralV1,
            include_dense_semantic,
            publication,
        )
    }

    pub(crate) fn descriptor_result_with_policy(
        &self,
        query: &str,
        lexical_policy: LexicalAdmissionPolicy,
        include_dense_semantic: bool,
        publication: &RetrievalPublicationIdentity,
    ) -> Option<QueryResult> {
        self.descriptor_results
            .borrow()
            .iter()
            .find(|entry| {
                entry.query == query
                    && entry.lexical_policy == lexical_policy
                    && entry.result.features.lexical_policy == lexical_policy
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

    // Verified metadata consumes the existing whole-packet limits before
    // optional descriptor retrieval; loading the record itself reads no body.
    pub(crate) fn protect_served_sources(
        &self,
        record: super::packet_served_sources::ServedSourceRecord,
    ) -> Result<(), codestory_contracts::api::ApiError> {
        for (ordinal, witness) in record.witnesses.iter().enumerate() {
            if self.admit_exact_selector(
                &witness.stable_identity(),
                witness.rendered_bytes,
                ordinal as u32,
            ) != PacketAdmissionDecision::Admitted
            {
                return Err(codestory_contracts::api::ApiError::invalid_argument(
                    "Issued source inventory exceeds remaining packet admission.",
                ));
            }
        }
        *self.issued_source_record.borrow_mut() = Some(record);
        Ok(())
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

    pub(crate) fn retrieval_admission_is_sealed(&self) -> bool {
        *self.retrieval_admission_sealed.borrow()
    }

    /// Authorize only an exact issued receipt backed by this pin's retained
    /// lexical metadata. Query text never crosses into source compilation.
    pub(crate) fn authorizes_published_file(
        &self,
        receipt: &PacketAdmissionReceiptV1,
        publication: &RetrievalPublicationIdentity,
    ) -> bool {
        if !self.retrieval_admission_is_sealed()
            || !receipt.stable_identity.starts_with("path:")
            || !self
                .receipts
                .borrow()
                .iter()
                .any(|issued| issued == receipt)
        {
            return false;
        }
        (receipt.origin == PacketAdmissionOriginV1::ExactTypedSelector
            && self
                .issued_source_record
                .borrow()
                .as_ref()
                .is_some_and(|record| {
                    record.authorizes_published_file(&receipt.stable_identity, publication)
                }))
            || self.descriptor_results.borrow().iter().any(|entry| {
                &entry.publication == publication
                    && entry.result.publication_identity.as_ref() == Some(publication)
                    && entry.result.hits.iter().any(|candidate| {
                        candidate.source == codestory_retrieval::CandidateSource::Lexical
                            && candidate.node_id.is_none()
                            && candidate.symbol_name.is_none()
                            && candidate.qualified_name.is_none()
                            && candidate.target.is_none()
                            && candidate.packet_descriptor().is_some_and(|descriptor| {
                                descriptor.stable_identity == receipt.stable_identity
                            })
                    })
            })
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

    /// Grow only a retained identity's source reservation after descriptor
    /// admission is sealed. The caller authenticates a typed source cursor
    /// before invoking this and hydrates its next page afterward.
    ///
    /// Existing reservations, identity count, and ordering remain intact.
    /// Partial capacity is charged conservatively; repeated requests cannot
    /// replenish the shared packet budget or reclaim a failed reservation.
    #[cfg(test)]
    pub(crate) fn reserve_continuation_source(&self, stable_identity: &str) -> Option<usize> {
        if !self.retrieval_admission_is_sealed() {
            return None;
        }
        let mut receipts = self.receipts.borrow_mut();
        let receipt = receipts
            .iter_mut()
            .find(|receipt| receipt.stable_identity == stable_identity)?;
        let current = receipt.reserved_source_bytes as usize;
        let extra = INTERIM_SOURCE_CONTINUATION_UPPER_BOUND
            .saturating_sub(current)
            .min(self.remaining_source_bytes());
        let mut identities = self.admitted_identities.borrow_mut();
        let reserved = identities.get_mut(stable_identity)?;
        debug_assert_eq!(*reserved, current);
        *reserved += extra;
        *self.admitted_source_bytes.borrow_mut() += extra;
        receipt.reserved_source_bytes = u32::try_from(current + extra).unwrap_or(u32::MAX);
        Some(current + extra)
    }

    /// Reserve spare capacity for authenticated requested source ranges first,
    /// fairly within that group, then share the remainder across context nodes.
    /// Existing reservations stay charged even when later hydration fails.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn reserve_source_pages(&self, requested: &HashSet<String>) {
        self.reserve_source_pages_with_demands(requested, &HashMap::new());
    }

    /// Authenticated rendered demand limits growth, never refunds an earlier
    /// reservation. Unknown demand retains the conservative page upper bound.
    pub(crate) fn reserve_source_pages_with_demands(
        &self,
        requested: &HashSet<String>,
        demands: &HashMap<String, usize>,
    ) {
        if !self.retrieval_admission_is_sealed() {
            return;
        }
        let mut receipts = self.receipts.borrow_mut();
        let mut identities = self.admitted_identities.borrow_mut();
        let mut used = self.admitted_source_bytes.borrow_mut();
        for requested_group in [true, false] {
            while *used < INTERIM_MAX_ADMITTED_SOURCE_BYTES {
                let mut progressed = false;
                for receipt in receipts.iter_mut() {
                    let current = receipt.reserved_source_bytes as usize;
                    if *used == INTERIM_MAX_ADMITTED_SOURCE_BYTES {
                        break;
                    }
                    if requested.contains(&receipt.stable_identity) != requested_group
                        || !receipt.stable_identity.starts_with("node:")
                        || current
                            >= demands
                                .get(&receipt.stable_identity)
                                .copied()
                                .unwrap_or(INTERIM_SOURCE_CONTINUATION_UPPER_BOUND)
                                .min(INTERIM_SOURCE_CONTINUATION_UPPER_BOUND)
                    {
                        continue;
                    }
                    let Some(reserved) = identities.get_mut(&receipt.stable_identity) else {
                        continue;
                    };
                    debug_assert_eq!(*reserved, current);
                    *reserved += 1;
                    *used += 1;
                    receipt.reserved_source_bytes += 1;
                    progressed = true;
                }
                if !progressed {
                    break;
                }
            }
        }
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

    #[test]
    fn replay_metadata_preserves_all_query_contributions_without_source_payloads() {
        use codestory_retrieval::{
            CandidateHit, CandidateLaneEvidence, CandidateLaneScores, CandidateSource, QueryTrace,
            RetrievalStageKind, StageCompletionStatus, StageTrace, classify_query,
        };
        use serde_json::json;

        let session = PacketProofSession::new();
        session.enable_replay_metadata();
        assert_eq!(
            session.admit("node:9000", 1),
            PacketAdmissionDecision::Admitted
        );
        let publication = RetrievalPublicationIdentity {
            artifact_scope: None,
            core_generation_id: "core".into(),
            core_run_id: "run".into(),
            sidecar_generation: "sidecar".into(),
            sidecar_input_hash: "input".into(),
            semantic_generation: "semantic".into(),
        };
        let hit = |id: usize, score| {
            let mut hit = CandidateHit::with_source(
                "src/public.rs",
                Some(format!("entry_{id}")),
                score,
                CandidateSource::Lexical,
            );
            hit.node_id = Some(id.to_string());
            hit.source_bytes_upper_bound = Some(512);
            hit.source_excerpt = Some("EXCERPT_MUST_NOT_BE_EXPORTED".into());
            hit.lane_scores = CandidateLaneScores {
                lexical: Some(CandidateLaneEvidence {
                    rank: 1,
                    raw_score: 0.5,
                    provenance: vec!["LANE_PROVENANCE_MUST_NOT_BE_EXPORTED".into()],
                }),
                ..Default::default()
            };
            hit
        };
        for query_index in 0..5 {
            let query = format!("  literal query {query_index} é\r\n");
            let mut features = classify_query(&query);
            features.lexical_policy = LexicalAdmissionPolicy::BroadQuestionV1;
            let mut phantom = hit(8000, 1.0);
            phantom.file_path = "lexical:synthetic".into();
            let mut unbounded = hit(8001, 1.0);
            unbounded.source_bytes_upper_bound = None;
            let mut hits = vec![phantom, unbounded];
            for row in 0..18 {
                let id = if query_index == 4 && row == 0 {
                    0
                } else {
                    query_index * 100 + row
                };
                let score = if query_index == 4 && row == 0 {
                    f32::from_bits(0x3f7fffff)
                } else if query_index == 0 && row == 0 {
                    f32::from_bits(0x3eaaaaab)
                } else {
                    0.5
                };
                let mut candidate = hit(id, score);
                candidate.lane_scores.lexical.as_mut().unwrap().rank = row as u32 + 1;
                if row == 0 {
                    candidate.lane_scores.semantic = Some(CandidateLaneEvidence {
                        rank: 19,
                        raw_score: -0.0,
                        provenance: Vec::new(),
                    });
                }
                if row == 17 {
                    candidate.lane_scores.graph = Some(CandidateLaneEvidence {
                        rank: 2,
                        raw_score: f32::from_bits(0x3eaaaaab),
                        provenance: Vec::new(),
                    });
                }
                hits.push(candidate);
            }
            let stage = |stage, completion_status| StageTrace {
                stage,
                budget_ms: 200,
                elapsed_ms: 4,
                admission_wait_ms: 0,
                queue_wait_ms: Some(0),
                execution_ms: Some(4),
                candidates_added: 18,
                marginal_gain: 1.0,
                cancel_reason: None,
                cache_hit: false,
                degraded: false,
                stub_reason: None,
                completion_status,
            };
            let result = QueryResult {
                publication_identity: Some(publication.clone()),
                query,
                features,
                hits,
                trace: QueryTrace {
                    retrieval_mode: "full".into(),
                    degraded_reason: None,
                    total_budget_ms: 400,
                    elapsed_ms: 8,
                    cancel_reason: None,
                    cache_hit: false,
                    stages: vec![
                        stage(
                            RetrievalStageKind::Stage1Lexical,
                            StageCompletionStatus::Completed,
                        ),
                        stage(
                            RetrievalStageKind::Stage1bSemantic,
                            if query_index == 4 {
                                StageCompletionStatus::Skipped
                            } else {
                                StageCompletionStatus::Completed
                            },
                        ),
                    ],
                },
            };
            session
                .retain_descriptor_result_with_policy(
                    &result,
                    LexicalAdmissionPolicy::BroadQuestionV1,
                    true,
                    &publication,
                )
                .unwrap();
        }
        let before = session.descriptor_results.borrow().clone();
        let admissions = serde_json::to_value(session.receipts()).unwrap();
        let metadata = session.replay_descriptor_metadata();
        let queries = metadata["queries"].as_array().unwrap();
        assert_eq!(queries.len(), 5);
        assert_eq!(
            queries[0]["query_sha256"],
            "ada9b88eb22ab770078f5c3b1c97c42fe9d353eaa8f09df6958c0d8d541ae16a"
        );
        assert_eq!(
            metadata["unavailable_evidence"],
            json!(["rank_features", "native_query_window_details"])
        );
        for (query_index, query) in queries.iter().enumerate() {
            assert_eq!(query["query_index"], query_index);
            assert_eq!(query["lexical_policy"], "BroadQuestionV1");
            assert_eq!(query["include_dense_semantic"], true);
            assert_eq!(
                query["query"],
                format!("  literal query {query_index} é\r\n")
            );
            assert_eq!(query["candidate_count"], 20);
            assert_eq!(
                query["publication"],
                serde_json::to_value(&publication).unwrap()
            );
            assert_eq!(
                query["trace"],
                serde_json::to_value(&before[query_index].result.trace).unwrap()
            );
            assert_eq!(
                query["shape"],
                serde_json::to_value(before[query_index].result.features.shape).unwrap()
            );
            let rows = query["eligible_descriptors"].as_array().unwrap();
            assert_eq!(rows.len(), 18);
            for (row, descriptor) in rows.iter().enumerate() {
                let id = if query_index == 4 && row == 0 {
                    0
                } else {
                    query_index * 100 + row
                };
                assert_eq!(descriptor["stable_identity"], format!("node:{id}"));
                assert_eq!(descriptor["returned_index"], row + 2);
                assert_eq!(descriptor["retrieval_lane"], "lexical");
                assert_eq!(descriptor["source_bytes_upper_bound"], 512);
                let expected_bits = if query_index == 4 && row == 0 {
                    0x3f7fffffu32
                } else if query_index == 0 && row == 0 {
                    0x3eaaaaabu32
                } else {
                    0x3f000000u32
                };
                assert_eq!(descriptor["retrieval_score"]["bits"], expected_bits);
                assert_eq!(
                    descriptor["retrieval_score"]["value"],
                    json!(f32::from_bits(expected_bits))
                );
                assert!(
                    descriptor["retrieval_score"]["version"]
                        .as_str()
                        .unwrap()
                        .starts_with(PACKET_RETRIEVAL_SCORE_VERSION_V1)
                );
                assert_eq!(
                    descriptor["lane_scores"]["lexical"],
                    json!({"rank":row+1,"raw_score":0.5,"raw_score_bits":0x3f000000u32})
                );
            }
            assert_eq!(
                rows[0]["lane_scores"]["semantic"],
                json!({"rank":19,"raw_score":-0.0,"raw_score_bits":0x80000000u32})
            );
            assert!(rows[1]["lane_scores"]["semantic"].is_null());
            assert!(rows[1]["lane_scores"]["graph"].is_null());
            assert_eq!(
                rows[17]["lane_scores"]["graph"]["raw_score_bits"],
                0x3eaaaaabu32
            );
            assert_eq!(rows[1]["retrieval_score"]["bits"], 0x3f000000u32);
            assert_eq!(rows[2]["retrieval_score"]["bits"], 0x3f000000u32);
        }
        assert_eq!(
            queries[0]["eligible_descriptors"][0]["retrieval_score"]["bits"],
            0x3eaaaaabu32
        );
        assert_eq!(
            queries[4]["eligible_descriptors"][0]["retrieval_score"]["bits"],
            0x3f7fffffu32
        );
        assert_eq!(
            queries[4]["trace"]["stages"][1]["completion_status"],
            "skipped"
        );
        let serialized = metadata.to_string();
        for forbidden in [
            "EXCERPT_MUST_NOT_BE_EXPORTED",
            "LANE_PROVENANCE_MUST_NOT_BE_EXPORTED",
            "source_excerpt",
            "structural_kind",
        ] {
            assert!(!serialized.contains(forbidden));
        }
        assert_eq!(session.descriptor_results.borrow().len(), before.len());
        for (expected, actual) in before
            .iter()
            .zip(session.descriptor_results.borrow().iter())
        {
            assert_eq!(expected.query, actual.query);
            assert_eq!(expected.result.hits, actual.result.hits);
            assert_eq!(expected.result.features, actual.result.features);
            assert_eq!(
                expected.result.publication_identity,
                actual.result.publication_identity
            );
            assert_eq!(
                serde_json::to_value(&expected.result.trace).unwrap(),
                serde_json::to_value(&actual.result.trace).unwrap()
            );
        }
        assert_eq!(
            serde_json::to_value(session.receipts()).unwrap(),
            admissions
        );
        assert_eq!(*session.hydrated_admissions.borrow(), 1);
        assert_eq!(*session.admitted_source_bytes.borrow(), 1);
    }

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
                published_calls: None,
                published_calls_conflicted: false,
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
            published_calls: None,
            published_calls_conflicted: false,
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
    fn demand_capped_reservations_keep_context_without_refunding_or_widening() {
        let make = || {
            let session = PacketProofSession::new();
            for id in 0..16 {
                session.admit(&format!("node:{id}"), 512);
            }
            session.seal_retrieval_admission();
            session
        };
        let old = make();
        old.reserve_source_pages(&HashSet::from(["node:0".into()]));
        assert_eq!(old.receipts()[0].reserved_source_bytes, 4096);
        assert!(old.receipts()[1].reserved_source_bytes < 1024);
        let session = make();
        let before = session.receipts();
        let mut demands = (0..16)
            .map(|id| (format!("node:{id}"), 512))
            .collect::<HashMap<_, _>>();
        demands.insert("node:0".into(), 2048);
        demands.insert("node:1".into(), 1024);
        let selected = HashSet::from(["node:0".into()]);
        session.reserve_source_pages_with_demands(&selected, &demands);
        let after = session.receipts();
        assert_eq!(after[0].reserved_source_bytes, 2048);
        assert_eq!(after[1].reserved_source_bytes, 1024);
        assert!(
            after
                .iter()
                .zip(&before)
                .all(|(a, b)| a.reserved_source_bytes >= b.reserved_source_bytes)
        );
        assert!(after.iter().all(|a| a.reserved_source_bytes <= 4096));
        assert!(
            after
                .iter()
                .map(|a| a.reserved_source_bytes as usize)
                .sum::<usize>()
                <= INTERIM_MAX_ADMITTED_SOURCE_BYTES
        );
        session.reserve_source_pages_with_demands(&selected, &demands);
        assert_eq!(session.receipts(), after);
        demands.insert("node:0".into(), 1);
        session.reserve_source_pages_with_demands(&selected, &demands);
        assert_eq!(
            session.receipts(),
            after,
            "a smaller observation cannot refund a charge"
        );
    }

    #[test]
    fn requested_source_pages_share_priority_without_refunds_or_new_identities() {
        for requested_count in [1, 2, 4, 16] {
            let session = PacketProofSession::new();
            for id in 0..16 {
                session.admit(&format!("node:{id}"), 512);
            }
            let mut requested = (0..requested_count)
                .map(|id| format!("node:{id}"))
                .collect::<HashSet<_>>();
            requested.insert("node:unadmitted".into());
            let initial = session.receipts();
            session.reserve_source_pages(&requested);
            assert_eq!(
                session.receipts(),
                initial,
                "unsealed admission cannot allocate pages"
            );
            session.seal_retrieval_admission();
            session.reserve_source_pages(&requested);
            let pages = session.receipts();
            let priority = &pages[..requested_count];
            let least = priority
                .iter()
                .map(|page| page.reserved_source_bytes)
                .min()
                .unwrap();
            let most = priority
                .iter()
                .map(|page| page.reserved_source_bytes)
                .max()
                .unwrap();
            assert!(most - least <= 1, "requested sources share capacity fairly");
            assert_eq!(least as usize, (512 + 8192 / requested_count).min(4096));
            assert!(
                pages
                    .iter()
                    .all(|page| (512..=4096).contains(&page.reserved_source_bytes))
            );
            assert_eq!(pages.len(), 16, "unadmitted selectors add no identity");
            let charged = *session.admitted_source_bytes.borrow();
            assert_eq!(charged, 16384);
            assert_eq!(
                pages
                    .iter()
                    .map(|page| page.reserved_source_bytes as usize)
                    .sum::<usize>(),
                charged
            );
            session.reserve_source_pages(&requested);
            assert_eq!(session.receipts(), pages);
            session.consume_unresolved_reservation("node:0");
            session.reserve_source_pages(&requested);
            assert_eq!(
                *session.admitted_source_bytes.borrow(),
                charged,
                "failed hydration never refunds capacity"
            );
            assert_eq!(session.receipts(), pages[1..]);
        }
    }

    #[test]
    fn source_pages_share_sealed_capacity_without_refunds_or_extra_identities() {
        for count in [1, 3, 8, 16] {
            let session = PacketProofSession::new();
            for index in 0..count {
                assert_eq!(
                    session.admit(&format!("node:{index}"), 512),
                    PacketAdmissionDecision::Admitted
                );
            }
            let original = session.receipts();
            session.reserve_source_pages(&HashSet::new());
            assert_eq!(session.receipts(), original);
            session.seal_retrieval_admission();
            session.reserve_source_pages(&HashSet::new());
            let pages = session.receipts();
            assert_eq!(pages.len(), count);
            let minimum = pages
                .iter()
                .map(|page| page.reserved_source_bytes)
                .min()
                .unwrap();
            let maximum = pages
                .iter()
                .map(|page| page.reserved_source_bytes)
                .max()
                .unwrap();
            assert!(maximum - minimum <= 1);
            assert_eq!(
                minimum as usize,
                (INTERIM_MAX_ADMITTED_SOURCE_BYTES / count)
                    .min(INTERIM_SOURCE_CONTINUATION_UPPER_BOUND)
            );
            assert!(
                pages
                    .iter()
                    .map(|page| page.reserved_source_bytes as usize)
                    .sum::<usize>()
                    <= INTERIM_MAX_ADMITTED_SOURCE_BYTES
            );
            session.reserve_source_pages(&HashSet::new());
            assert_eq!(pages, session.receipts());
        }
    }

    #[test]
    fn source_continuations_use_only_remaining_sealed_packet_capacity() {
        let session = PacketProofSession::new();
        for index in 0..INTERIM_MAX_ADMITTED_CANDIDATES {
            assert_eq!(
                session.admit(&format!("node:{index}"), 512),
                PacketAdmissionDecision::Admitted
            );
        }
        let original = session.receipts();
        assert_eq!(session.reserve_continuation_source("node:0"), None);
        assert_eq!(session.receipts(), original);
        session.seal_retrieval_admission();
        assert_eq!(session.reserve_continuation_source("node:missing"), None);
        assert_eq!(session.reserve_continuation_source("node:0"), Some(4096));
        assert_eq!(session.reserve_continuation_source("node:0"), Some(4096));
        assert_eq!(session.reserve_continuation_source("node:1"), Some(4096));
        assert_eq!(session.reserve_continuation_source("node:2"), Some(1536));
        assert_eq!(session.reserve_continuation_source("node:3"), Some(512));
        assert_eq!(session.remaining_source_bytes(), 0);
        assert_eq!(
            *session.admitted_source_bytes.borrow(),
            INTERIM_MAX_ADMITTED_SOURCE_BYTES
        );
        assert_eq!(
            *session.hydrated_admissions.borrow(),
            INTERIM_MAX_ADMITTED_CANDIDATES
        );
        let receipts = session.receipts();
        for (before, after) in original.iter().zip(&receipts) {
            assert_eq!(before.stable_identity, after.stable_identity);
            assert_eq!(before.packet_ordinal, after.packet_ordinal);
            assert_eq!(before.origin, after.origin);
            assert_eq!(before.score_version, after.score_version);
            assert_eq!(
                session.admitted_identities.borrow()[&after.stable_identity],
                after.reserved_source_bytes as usize
            );
        }
        assert_eq!(
            receipts
                .iter()
                .map(|receipt| receipt.reserved_source_bytes as usize)
                .sum::<usize>(),
            INTERIM_MAX_ADMITTED_SOURCE_BYTES
        );
    }

    #[test]
    fn source_continuation_reservation_follows_the_canonical_identity() {
        let session = PacketProofSession::new();
        assert_eq!(
            session.admit_exact_selector("selector:qualified_symbol:shared", 512, 0),
            PacketAdmissionDecision::Admitted
        );
        session.canonicalize_identity("selector:qualified_symbol:shared", "node:2");
        session.seal_retrieval_admission();
        assert_eq!(
            session.reserve_continuation_source("selector:qualified_symbol:shared"),
            None
        );
        assert_eq!(session.reserve_continuation_source("node:2"), Some(4096));
        assert_eq!(session.reserve_continuation_source("node:2"), Some(4096));
        assert_eq!(session.receipts().len(), 1);
        assert_eq!(session.receipts()[0].stable_identity, "node:2");
        assert_eq!(session.receipts()[0].reserved_source_bytes, 4096);
        assert_eq!(*session.admitted_source_bytes.borrow(), 4096);
        assert_eq!(*session.hydrated_admissions.borrow(), 1);
    }

    #[test]
    fn source_continuations_do_not_refund_failed_selector_reservations() {
        let session = PacketProofSession::new();
        assert_eq!(
            session.admit_exact_selector("node:failed", 512, 0),
            PacketAdmissionDecision::Admitted
        );
        session.consume_unresolved_reservation("node:failed");
        session.seal_retrieval_admission();
        assert_eq!(session.reserve_continuation_source("node:failed"), None);
        assert!(session.receipts().is_empty());
        assert_eq!(*session.admitted_source_bytes.borrow(), 512);
        assert_eq!(*session.hydrated_admissions.borrow(), 1);
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
    fn paragraph_queries_preserve_generic_and_explicit_free_query_origin() {
        let seed = codestory_contracts::compilation::RetrievalSeedPlanV1 {
            contract_version: 1,
            generic_query: "alpha flow\n\nbeta ordering".into(),
            exact_selectors: Vec::new(),
            free_queries: vec!["beta ordering".into()],
        };
        let plan = codestory_agent::packet_plan::build_packet_plan_from_seed_plan(
            &seed,
            codestory_contracts::api::PacketBudgetModeDto::Standard,
        );
        let queries = packet_retrieval_queries(&plan);
        assert_eq!(queries.len(), 4);
        assert!(
            queries[..3]
                .iter()
                .all(|query| query.lexical_policy == LexicalAdmissionPolicy::BroadQuestionV1)
        );
        assert_eq!(queries[2].query, queries[3].query);
        assert_eq!(
            queries[3].lexical_policy,
            LexicalAdmissionPolicy::StrictLiteralV1
        );
        assert_eq!(
            plan.queries
                .iter()
                .map(|query| &query.query)
                .collect::<Vec<_>>(),
            queries.iter().map(|query| &query.query).collect::<Vec<_>>()
        );
    }

    #[test]
    fn identical_generic_and_free_query_keep_distinct_origin() {
        let seed = codestory_contracts::compilation::RetrievalSeedPlanV1 {
            contract_version: 1,
            generic_query: "text empty `absent`".into(),
            exact_selectors: Vec::new(),
            free_queries: vec!["text empty `absent`".into(), "text empty `absent`".into()],
        };
        let plan = codestory_agent::packet_plan::build_packet_plan_from_seed_plan(
            &seed,
            codestory_contracts::api::PacketBudgetModeDto::Standard,
        );
        let queries = packet_retrieval_queries(&plan);
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].query, seed.generic_query);
        assert_eq!(queries[1].query, seed.generic_query);
        assert_eq!(
            queries[0].lexical_policy,
            LexicalAdmissionPolicy::BroadQuestionV1
        );
        assert_eq!(
            queries[1].lexical_policy,
            LexicalAdmissionPolicy::StrictLiteralV1
        );
    }

    #[test]
    fn sealed_retrieval_admission_rejects_late_descriptors() {
        let session = PacketProofSession::new();
        session.seal_retrieval_admission();
        let descriptor = PacketCandidateDescriptorV1 {
            published_calls: None,
            published_calls_conflicted: false,
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
            published_calls: None,
            published_calls_conflicted: false,
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
            focused_source: None,
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
        for hostile_eligibility in [Some(true), Some(false)] {
            let mut hit = packet_hit("edge-1");
            hit.hit.eligible_for_sufficiency = hostile_eligibility;
            let citation = hit.citation(true);
            assert_eq!(
                citation.eligible_for_sufficiency, None,
                "packet citations carry retrieval provenance, never answer-sufficiency authority"
            );
        }
        let hit = packet_hit("edge-1");
        let citation = hit.citation(true);
        assert_eq!(citation.evidence_edge_ids, [EdgeId("edge-1".into())]);
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
