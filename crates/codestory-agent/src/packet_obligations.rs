//! Versioned packet obligations planned before retrieval and finalized from carried evidence.

use super::packet_evidence::citation_sufficiency_eligible;
use super::packet_evidence_roles::{PacketEvidenceRole, packet_evidence_role};
use super::packet_flow_requirements::ordinary_incident_call_receipt_is_valid;
use super::packet_required_probes::{
    packet_prompt_exact_symbol_probe_queries, packet_prompt_explicit_source_path_queries,
    packet_sufficiency_required_probe_queries_from_terms,
};
use super::packet_scoring::{normalize_identifier, packet_display_path};
use super::packet_terms::packet_probe_terms;
use crate::packet_execution_graphs::packet_execution_graphs;
use crate::text::{exact_symbol_query_terms, looks_like_standalone_symbol_query};
use crate::trail::is_speculative_trail_edge;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto,
    EdgeId, EdgeKind, GraphArtifactDto, GraphEdgeDto, GraphResponse, NodeId, NodeKind,
    PACKET_OBLIGATION_PLAN_VERSION, PacketBudgetDto, PacketClaimDto, PacketClaimObligationDto,
    PacketClaimObligationKindDto, PacketObligationCarrierEdgeProofDto, PacketObligationPlanDto,
    PacketObligationProofStatusDto, PacketPlanQueryDto, PacketProbeDto,
    PacketProbeRejectionCodeDto, PacketProbeResolutionDto, PacketProbeResolutionStatusDto,
    PacketProofStatusDto, PacketQueryCompletionDto, PacketQueryObligationDto,
    PacketQueryObligationKindDto, PacketTaskClassDto,
};
use std::collections::{BTreeSet, HashMap, HashSet};

const PACKET_OBLIGATION_BINDING_TERM_LIMIT: usize = 8;
const PACKET_OBLIGATION_BINDING_TERM_CHAR_LIMIT: usize = 160;
const PACKET_SUPPLEMENTAL_QUERY_OBLIGATION_LIMIT: usize = 16;
const PACKET_SUPPLEMENTAL_QUERY_CHAR_LIMIT: usize = 240;
const REQUESTED_CLAIM_OVERFLOW_ID: &str = "requested_claim_overflow";
const PACKET_BUDGET_TRUNCATED_REASON: &str = "packet_budget_truncated";
pub const PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE: &str = "obligation receipt";

pub fn build_packet_obligation_plan(
    question: &str,
    task_class: PacketTaskClassDto,
    planned_queries: &[PacketPlanQueryDto],
) -> PacketObligationPlanDto {
    let terms = packet_probe_terms(question);
    let requires_complete_discovery =
        packet_question_requires_complete_discovery(question, task_class);
    let exact_symbol_queries =
        packet_prioritized_exact_symbol_queries(question, &terms, task_class);
    let (binding_terms, omitted_binding_term_count) =
        requested_claim_binding_terms(&exact_symbol_queries);
    let mut claim_obligations = default_profile_requested_claim_obligations(
        &binding_terms,
        task_class,
        requires_complete_discovery,
    );
    if omitted_binding_term_count > 0 {
        claim_obligations.push(requested_claim_overflow_obligation(
            omitted_binding_term_count,
            task_class,
            requires_complete_discovery,
        ));
    }
    let needs_material_fallback = !claim_obligations.iter().any(|obligation| {
        obligation.material && obligation.kind != PacketClaimObligationKindDto::ExactProbe
    });
    claim_obligations.extend(default_profile_guards(
        task_class,
        requires_complete_discovery,
        needs_material_fallback,
    ));

    let mut query_obligations = Vec::new();
    let mut required_queries = HashSet::new();
    for query in packet_prompt_explicit_source_path_queries(question) {
        if required_queries.insert(query.clone()) {
            push_query_obligation(
                &mut query_obligations,
                PacketQueryObligationKindDto::RequiredPath,
                &query,
                true,
            );
        }
    }
    for query in exact_symbol_queries
        .iter()
        .take(PACKET_OBLIGATION_BINDING_TERM_LIMIT)
    {
        if required_queries.insert(query.clone()) {
            push_query_obligation(
                &mut query_obligations,
                PacketQueryObligationKindDto::RequiredProbe,
                query,
                true,
            );
        }
    }
    for query in packet_sufficiency_required_probe_queries_from_terms(&terms, task_class) {
        if required_queries.insert(query.clone()) {
            push_query_obligation(
                &mut query_obligations,
                PacketQueryObligationKindDto::RequiredProbe,
                &query,
                true,
            );
        }
    }
    let mut supplemental_queries = 0;
    for query in planned_queries {
        if supplemental_queries < PACKET_SUPPLEMENTAL_QUERY_OBLIGATION_LIMIT
            && query.query.chars().count() <= PACKET_SUPPLEMENTAL_QUERY_CHAR_LIMIT
            && required_queries.insert(query.query.clone())
        {
            push_query_obligation(
                &mut query_obligations,
                PacketQueryObligationKindDto::Supplemental,
                &query.query,
                false,
            );
            supplemental_queries += 1;
        }
    }

    PacketObligationPlanDto {
        version: PACKET_OBLIGATION_PLAN_VERSION,
        binding_terms,
        claim_obligations,
        query_obligations,
    }
}

pub fn append_packet_probe_obligations(
    plan: &mut PacketObligationPlanDto,
    resolutions: &[PacketProbeResolutionDto],
    question: &str,
    task_class: PacketTaskClassDto,
) {
    scope_generic_fallback_obligations_to_exact_paths(plan, resolutions, question, task_class);
    plan.claim_obligations
        .retain(|obligation| obligation.probe_binding.is_none());
    plan.claim_obligations.extend(
        resolutions
            .iter()
            .filter(|resolution| packet_probe_is_exact_typed(&resolution.probe))
            .map(packet_probe_obligation),
    );
}

fn scope_generic_fallback_obligations_to_exact_paths(
    plan: &mut PacketObligationPlanDto,
    resolutions: &[PacketProbeResolutionDto],
    question: &str,
    task_class: PacketTaskClassDto,
) {
    let has_resolved_exact_path = resolutions.iter().any(|resolution| {
        matches!(&resolution.probe, PacketProbeDto::ExactPath { .. })
            && matches!(
                resolution.status,
                PacketProbeResolutionStatusDto::ExactPath
                    | PacketProbeResolutionStatusDto::ValidUncoveredPath
            )
    });
    if !has_resolved_exact_path {
        return;
    }
    if packet_question_requires_complete_discovery(question, task_class) {
        return;
    }

    let terms = packet_probe_terms(question);
    let has_recognized_flow = false;
    let detected_exact_symbol_queries =
        packet_prioritized_exact_symbol_queries(question, &terms, task_class);
    // A resolved exact-path set is the explicit evidence scope. Preserve an independent symbol
    // requirement only when the prompt carries unambiguous symbol syntax; a bare PascalCase word
    // embedded in prose is also a common product or project name and cannot safely add a row.
    let exact_symbol_queries = detected_exact_symbol_queries
        .iter()
        .filter(|query| packet_exact_symbol_query_is_explicit(question, query))
        .cloned()
        .collect::<Vec<_>>();
    let exact_symbol_binding_terms = exact_symbol_queries
        .iter()
        .map(|query| {
            query
                .chars()
                .take(PACKET_OBLIGATION_BINDING_TERM_CHAR_LIMIT)
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let exact_symbol_bounded_identity_loss = exact_symbol_queries
        .iter()
        .any(|query| query.chars().count() > PACKET_OBLIGATION_BINDING_TERM_CHAR_LIMIT)
        || exact_symbol_binding_terms
            .iter()
            .collect::<HashSet<_>>()
            .len()
            < exact_symbol_queries.len();
    if !has_recognized_flow {
        for obligation in &mut plan.query_obligations {
            if obligation.kind == PacketQueryObligationKindDto::RequiredProbe
                && detected_exact_symbol_queries
                    .iter()
                    .any(|query| query == &obligation.query)
                && !exact_symbol_queries
                    .iter()
                    .any(|query| query == &obligation.query)
            {
                obligation.material = false;
            }
        }
    }
    for obligation in &mut plan.claim_obligations {
        if !has_recognized_flow
            && exact_symbol_queries.is_empty()
            && obligation.id == default_profile_obligation_id(task_class)
        {
            obligation.material = false;
            continue;
        }
        if obligation.id == REQUESTED_CLAIM_OVERFLOW_ID {
            obligation.material = exact_symbol_bounded_identity_loss
                || exact_symbol_queries.len() > PACKET_OBLIGATION_BINDING_TERM_LIMIT;
            continue;
        }
        if !obligation.id.starts_with("requested_claim:") {
            continue;
        }
        let preserves_explicit_symbol = obligation.binding_terms.iter().any(|binding_term| {
            exact_symbol_binding_terms
                .iter()
                .any(|query| query.eq_ignore_ascii_case(binding_term))
        });
        if !preserves_explicit_symbol {
            obligation.material = false;
        }
    }
}

fn packet_prioritized_exact_symbol_queries(
    question: &str,
    terms: &[String],
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    let mut queries = packet_prompt_exact_symbol_probe_queries(question, terms, task_class);
    // Strong syntax is unambiguous and must survive the bounded requested-claim/query ledger even
    // when earlier prose contains many ambiguous PascalCase names.
    queries.sort_by_key(|query| !packet_exact_symbol_query_is_explicit(question, query));
    queries
}

fn packet_exact_symbol_query_is_explicit(question: &str, query: &str) -> bool {
    packet_question_has_backticked_exact_symbol(question, query)
        || packet_question_has_invoked_exact_symbol(question, query)
        || (looks_like_standalone_symbol_query(question)
            && exact_symbol_query_terms(question)
                .iter()
                .any(|candidate| candidate == query))
        || query.contains("::")
        || query.contains('/')
        || query.contains('.')
        || query.contains('_')
        || query.contains('$')
}

fn packet_question_has_invoked_exact_symbol(question: &str, query: &str) -> bool {
    question.match_indices(query).any(|(start, _)| {
        let before_is_symbol = question[..start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_');
        let after = &question[start + query.len()..];
        !before_is_symbol
            && after
                .chars()
                .next()
                .is_some_and(|character| matches!(character, '(' | '<' | '['))
    })
}

fn packet_question_has_backticked_exact_symbol(question: &str, query: &str) -> bool {
    question
        .split('`')
        .skip(1)
        .step_by(2)
        .flat_map(exact_symbol_query_terms)
        .any(|candidate| candidate == query)
}

fn packet_probe_is_exact_typed(probe: &PacketProbeDto) -> bool {
    matches!(
        probe,
        PacketProbeDto::ExactPath { .. }
            | PacketProbeDto::SymbolId { .. }
            | PacketProbeDto::FileSymbol { .. }
            | PacketProbeDto::Continuation {
                symbol_id: Some(_),
                ..
            }
    )
}

fn packet_probe_obligation(resolution: &PacketProbeResolutionDto) -> PacketClaimObligationDto {
    let (proof_status, reason) = match resolution.status {
        PacketProbeResolutionStatusDto::Rejected => (
            PacketObligationProofStatusDto::Unsupported,
            Some(format!(
                "exact_probe_rejected:{}",
                resolution
                    .rejection
                    .as_ref()
                    .map(|rejection| packet_probe_rejection_code_id(rejection.code))
                    .unwrap_or("reason_unavailable")
            )),
        ),
        PacketProbeResolutionStatusDto::Ambiguous => (
            PacketObligationProofStatusDto::Unsupported,
            Some(format!(
                "exact_probe_ambiguous:candidates={}",
                resolution.candidates.len()
            )),
        ),
        _ => (PacketObligationProofStatusDto::Planned, None),
    };
    PacketClaimObligationDto {
        id: format!("exact_probe:{}", resolution.input_index),
        kind: PacketClaimObligationKindDto::ExactProbe,
        binding_terms: resolution.normalized_query.iter().cloned().collect(),
        probe_binding: Some(resolution.clone()),
        material: resolution.status != PacketProbeResolutionStatusDto::Rejected,
        allowed_node_kinds: Vec::new(),
        required_edge_kind: None,
        requires_complete_discovery: false,
        proof_status,
        reason,
        carrier_node_ids: Vec::new(),
        carrier_paths: Vec::new(),
        carrier_edge_proofs: Vec::new(),
        open_next_candidates: packet_probe_open_next_candidates(resolution),
    }
}

fn packet_probe_rejection_code_id(code: PacketProbeRejectionCodeDto) -> &'static str {
    match code {
        PacketProbeRejectionCodeDto::MalformedProbe => "malformed_probe",
        PacketProbeRejectionCodeDto::MissingTarget => "missing_target",
        PacketProbeRejectionCodeDto::OutOfProject => "out_of_project",
        PacketProbeRejectionCodeDto::StaleSymbolId => "stale_symbol_id",
        PacketProbeRejectionCodeDto::StaleContinuation => "stale_continuation",
        PacketProbeRejectionCodeDto::IncompatibleContinuation => "incompatible_continuation",
    }
}

fn packet_probe_open_next_candidates(resolution: &PacketProbeResolutionDto) -> Vec<String> {
    let candidate = resolution
        .normalized_query
        .clone()
        .or_else(|| resolution.path.clone())
        .unwrap_or_else(|| match &resolution.probe {
            PacketProbeDto::ExactPath { path } => path.clone(),
            PacketProbeDto::SymbolId { id } => id.clone(),
            PacketProbeDto::FileSymbol { path, symbol } => format!("{path}::{symbol}"),
            PacketProbeDto::Continuation {
                symbol_id, query, ..
            } => symbol_id.clone().unwrap_or_else(|| query.clone()),
            PacketProbeDto::FreeQuery { query } => query.clone(),
        });
    (!candidate.trim().is_empty())
        .then(|| candidate.trim().to_string())
        .into_iter()
        .collect()
}

fn default_profile_requested_claim_obligations(
    binding_terms: &[String],
    _task_class: PacketTaskClassDto,
    requires_complete_discovery: bool,
) -> Vec<PacketClaimObligationDto> {
    // Requested identities prove that the named callable was actually carried. Flow predicates
    // independently prove how it participates in behavior. Giving every requested identity the
    // task's default behavioral role fabricated orchestration/dispatch claims and forced a CALL
    // edge even for an ordinary exact lookup.
    let kind = PacketClaimObligationKindDto::ExactProbe;
    binding_terms
        .iter()
        .enumerate()
        .map(|(index, binding_term)| PacketClaimObligationDto {
            id: format!("requested_claim:{index}:{binding_term}"),
            kind,
            binding_terms: vec![binding_term.clone()],
            probe_binding: None,
            material: true,
            allowed_node_kinds: allowed_node_kinds_for_obligation(kind),
            required_edge_kind: None,
            requires_complete_discovery,
            proof_status: PacketObligationProofStatusDto::Planned,
            reason: None,
            carrier_node_ids: Vec::new(),
            carrier_paths: Vec::new(),
            carrier_edge_proofs: Vec::new(),
            open_next_candidates: vec![binding_term.clone()],
        })
        .collect()
}

fn default_profile_guards(
    _task_class: PacketTaskClassDto,
    _requires_complete_discovery: bool,
    _needs_material_fallback: bool,
) -> Vec<PacketClaimObligationDto> {
    // Horizon A: ordinary wording reaches generic retrieval only. Task-class
    // answer-shape guards (Orchestration/CALL) must not steer selection.
    Vec::new()
}

fn requested_claim_binding_terms(exact_symbol_queries: &[String]) -> (Vec<String>, usize) {
    let mut candidates = Vec::new();
    let mut omitted_exact_symbol_count = 0;
    for term in exact_symbol_queries {
        let bounded_identity_loss =
            term.chars().count() > PACKET_OBLIGATION_BINDING_TERM_CHAR_LIMIT;
        let inserted = push_exact_requested_claim_binding_candidate(&mut candidates, term);
        if bounded_identity_loss || !inserted {
            // Distinct exact identities can share the same bounded receipt key. Keep that loss
            // visible so an identity beyond the query cap cannot silently disappear.
            omitted_exact_symbol_count += 1;
        }
    }

    // Natural-language terms describe retrieval intent; they are not independent code claims.
    // Only the exact-symbol parser may mint a requested-identity obligation. This prevents prose
    // such as "participates", "evidence", or "path" from consuming the bounded material ledger.
    let omitted = omitted_exact_symbol_count
        + candidates
            .len()
            .saturating_sub(PACKET_OBLIGATION_BINDING_TERM_LIMIT);
    candidates.truncate(PACKET_OBLIGATION_BINDING_TERM_LIMIT);
    (candidates, omitted)
}

fn push_exact_requested_claim_binding_candidate(candidates: &mut Vec<String>, term: &str) -> bool {
    let bounded = term
        .chars()
        .take(PACKET_OBLIGATION_BINDING_TERM_CHAR_LIMIT)
        .collect::<String>();
    if !bounded.is_empty() && !candidates.iter().any(|candidate| candidate == &bounded) {
        candidates.push(bounded);
        true
    } else {
        false
    }
}

fn requested_claim_overflow_obligation(
    omitted_count: usize,
    task_class: PacketTaskClassDto,
    requires_complete_discovery: bool,
) -> PacketClaimObligationDto {
    PacketClaimObligationDto {
        id: REQUESTED_CLAIM_OVERFLOW_ID.to_string(),
        kind: default_profile_obligation_kind(task_class),
        binding_terms: Vec::new(),
        probe_binding: None,
        material: true,
        allowed_node_kinds: Vec::new(),
        required_edge_kind: None,
        requires_complete_discovery,
        proof_status: PacketObligationProofStatusDto::Planned,
        reason: Some(format!(
            "requested_claim_binding_limit_exceeded:{omitted_count}"
        )),
        carrier_node_ids: Vec::new(),
        carrier_paths: Vec::new(),
        carrier_edge_proofs: Vec::new(),
        open_next_candidates: Vec::new(),
    }
}

fn allowed_node_kinds_for_obligation(_kind: PacketClaimObligationKindDto) -> Vec<NodeKind> {
    // Every claim obligation in this schema is behavioral. Structural files, types, fields, and
    // variables may report a relevant name, but cannot prove execution behavior.
    vec![NodeKind::FUNCTION, NodeKind::METHOD, NodeKind::MACRO]
}

fn default_profile_obligation_kind(
    _task_class: PacketTaskClassDto,
) -> PacketClaimObligationKindDto {
    PacketClaimObligationKindDto::ExactProbe
}

fn default_profile_obligation_id(_task_class: PacketTaskClassDto) -> &'static str {
    "profile_generic_behavior"
}

fn packet_obligation_candidate_is_path(candidate: &str) -> bool {
    let candidate = candidate.trim();
    candidate.contains(['/', '\\'])
        || [
            ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java", ".kt", ".swift", ".cs",
            ".cpp", ".c", ".h", ".sql", ".json", ".toml", ".yaml", ".yml",
        ]
        .iter()
        .any(|extension| candidate.ends_with(extension))
}

fn push_query_obligation(
    obligations: &mut Vec<PacketQueryObligationDto>,
    kind: PacketQueryObligationKindDto,
    query: &str,
    material: bool,
) {
    let id = format!("query:{}", obligations.len());
    obligations.push(PacketQueryObligationDto {
        id,
        kind,
        query: query.to_string(),
        material,
        completion: None,
    });
}

pub fn finalize_packet_obligation_plan(
    question: &str,
    task_class: PacketTaskClassDto,
    plan: &mut PacketObligationPlanDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
) {
    finalize_packet_claim_obligations(
        question,
        task_class,
        plan,
        answer,
        PacketObligationEvidenceView::from_budget(budget),
    );
    finalize_query_obligations(plan, answer, budget);
}

pub fn refinalize_packet_obligation_plan_after_rebuild(
    question: &str,
    task_class: PacketTaskClassDto,
    plan: &mut PacketObligationPlanDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
) {
    finalize_packet_claim_obligations(
        question,
        task_class,
        plan,
        answer,
        PacketObligationEvidenceView::from_budget(budget),
    );
    finalize_query_obligations(plan, answer, budget);
}

/// Evaluate the current uncapped answer against the planned proof ledger so retrieval can spend
/// its next query on evidence that is still missing. This preview never changes the public plan:
/// finalization still runs after citation, graph, and byte caps have selected the carried proof.
pub fn preview_packet_obligation_plan_before_budget(
    question: &str,
    task_class: PacketTaskClassDto,
    plan: &PacketObligationPlanDto,
    answer: &AgentAnswerDto,
) -> PacketObligationPlanDto {
    let mut preview = plan.clone();
    finalize_packet_claim_obligations(
        question,
        task_class,
        &mut preview,
        answer,
        PacketObligationEvidenceView::complete(answer.citations.len()),
    );
    preview
}

#[derive(Clone, Debug, Default)]
pub struct PacketObligationEdgeProofSnapshot {
    entries: Vec<PacketObligationEdgeProofSnapshotEntry>,
    carriers: Vec<PacketObligationCarrierSnapshotEntry>,
    protected_carrier_node_ids: Vec<NodeId>,
    protected_edge_ids: Vec<EdgeId>,
}

#[derive(Clone, Debug)]
struct PacketObligationEdgeProofSnapshotEntry {
    obligation_id: String,
    obligation_kind: PacketClaimObligationKindDto,
    proof: PacketObligationCarrierEdgeProofDto,
}

#[derive(Clone, Debug)]
struct PacketObligationCarrierSnapshotEntry {
    obligation_id: String,
    obligation_kind: PacketClaimObligationKindDto,
    carrier_node_id: NodeId,
}

/// Capture exact carrier-edge candidates while the full graph is still present. The snapshot is
/// local to packet construction; only candidates whose carrier survives the real citation cap can
/// become serialized obligation receipts.
pub fn capture_packet_obligation_edge_proofs_before_budget(
    question: &str,
    task_class: PacketTaskClassDto,
    plan: &PacketObligationPlanDto,
    answer: &AgentAnswerDto,
) -> PacketObligationEdgeProofSnapshot {
    let mut proof_plan = plan.clone();
    finalize_packet_claim_obligations(
        question,
        task_class,
        &mut proof_plan,
        answer,
        PacketObligationEvidenceView::complete(answer.citations.len()),
    );
    let mut protected_carrier_node_ids = Vec::new();
    let mut protected_carrier_node_id_set = HashSet::new();
    let mut protected_edge_ids = Vec::new();
    let mut protected_edge_id_set = HashSet::new();
    for obligation in &proof_plan.claim_obligations {
        if !obligation.material || obligation.proof_status != PacketObligationProofStatusDto::Proven
        {
            continue;
        }
        let primary_carrier_node_id = obligation.carrier_node_ids.first().cloned().or_else(|| {
            obligation
                .carrier_edge_proofs
                .first()
                .map(|proof| proof.carrier_node_id.clone())
        });
        let Some(primary_carrier_node_id) = primary_carrier_node_id else {
            continue;
        };
        let secondary_carrier_node_id =
            first_context_connected_proven_carrier(answer, obligation, &primary_carrier_node_id);
        for carrier_node_id in
            std::iter::once(primary_carrier_node_id).chain(secondary_carrier_node_id)
        {
            if protected_carrier_node_id_set.insert(carrier_node_id.clone()) {
                protected_carrier_node_ids.push(carrier_node_id.clone());
            }
            if let Some(proof) = obligation
                .carrier_edge_proofs
                .iter()
                .find(|proof| proof.carrier_node_id == carrier_node_id)
                && protected_edge_id_set.insert(proof.edge_id.clone())
            {
                protected_edge_ids.push(proof.edge_id.clone());
            }
        }
    }
    let mut entries = proof_plan
        .claim_obligations
        .iter()
        .filter(|obligation| obligation.proof_status == PacketObligationProofStatusDto::Proven)
        .flat_map(|obligation| {
            obligation.carrier_edge_proofs.iter().cloned().map(|proof| {
                PacketObligationEdgeProofSnapshotEntry {
                    obligation_id: obligation.id.clone(),
                    obligation_kind: obligation.kind,
                    proof,
                }
            })
        })
        .collect::<Vec<_>>();
    let mut carriers = proof_plan
        .claim_obligations
        .iter()
        .filter(|obligation| obligation.proof_status == PacketObligationProofStatusDto::Proven)
        .flat_map(|obligation| {
            obligation
                .carrier_node_ids
                .iter()
                .cloned()
                .map(|carrier_node_id| PacketObligationCarrierSnapshotEntry {
                    obligation_id: obligation.id.clone(),
                    obligation_kind: obligation.kind,
                    carrier_node_id,
                })
        })
        .collect::<Vec<_>>();
    sort_and_dedup_edge_proof_snapshot_entries(&mut entries);
    carriers.sort_by(|left, right| {
        left.obligation_id
            .cmp(&right.obligation_id)
            .then_with(|| left.carrier_node_id.0.cmp(&right.carrier_node_id.0))
    });
    carriers.dedup_by(|left, right| {
        left.obligation_id == right.obligation_id
            && left.obligation_kind == right.obligation_kind
            && left.carrier_node_id == right.carrier_node_id
    });
    PacketObligationEdgeProofSnapshot {
        entries,
        carriers,
        protected_carrier_node_ids,
        protected_edge_ids,
    }
}

fn first_context_connected_proven_carrier(
    answer: &AgentAnswerDto,
    obligation: &PacketClaimObligationDto,
    primary_carrier_node_id: &NodeId,
) -> Option<NodeId> {
    obligation
        .carrier_node_ids
        .iter()
        .filter(|candidate_node_id| *candidate_node_id != primary_carrier_node_id)
        .filter(|candidate_node_id| {
            obligation
                .carrier_edge_proofs
                .iter()
                .any(|proof| &proof.carrier_node_id == *candidate_node_id)
        })
        .find(|candidate_node_id| {
            let candidate_node_id = *candidate_node_id;
            answer.graphs.iter().any(|artifact| {
                let GraphArtifactDto::Uml { graph, .. } = artifact else {
                    return false;
                };
                graph.edges.iter().any(|edge| {
                    packet_call_is_usable_selection_context(edge)
                        && ((&edge.source == primary_carrier_node_id
                            && &edge.target == candidate_node_id)
                            || (&edge.target == primary_carrier_node_id
                                && &edge.source == candidate_node_id))
                })
            })
        })
        .cloned()
}

fn packet_call_is_usable_selection_context(edge: &GraphEdgeDto) -> bool {
    if edge.kind != EdgeKind::CALL || edge.source == edge.target {
        return false;
    }
    match edge.certainty.as_deref() {
        Some(certainty) if certainty.eq_ignore_ascii_case("certain") => true,
        Some(certainty) if certainty.eq_ignore_ascii_case("probable") => true,
        Some(_) => false,
        None => edge.confidence.is_none_or(|confidence| {
            confidence >= codestory_contracts::graph::ResolutionCertainty::CERTAIN_MIN
        }),
    }
}

/// Exact lawful carriers selected while the complete answer and graph are still available.
/// Packet capping uses this ordered set before spending citation slots on general relevance.
pub fn protected_packet_obligation_carrier_node_ids(
    snapshot: &PacketObligationEdgeProofSnapshot,
) -> &[NodeId] {
    &snapshot.protected_carrier_node_ids
}

/// Exact typed edges paired with the ordered material carriers above. Graph capping spends these
/// slots before unrelated trail context so the public packet keeps the proof it claims to carry.
pub fn protected_packet_obligation_edge_ids(
    snapshot: &PacketObligationEdgeProofSnapshot,
) -> &[EdgeId] {
    &snapshot.protected_edge_ids
}

/// Bind pre-cap proof candidates to the carriers that survived the actual citation and graph
/// caps. Normal finalization still rechecks role, eligibility, node kind, and the retained edge.
pub fn install_retained_packet_obligation_edge_proofs(
    plan: &mut PacketObligationPlanDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    snapshot: &PacketObligationEdgeProofSnapshot,
    max_carriers: usize,
) {
    let citations_omitted = packet_budget_omitted_obligation_evidence(budget, "citations");
    let trail_edges_omitted = packet_budget_omitted_obligation_evidence(budget, "trail_edges");
    let retained_order = answer.citations.iter().enumerate().fold(
        HashMap::<NodeId, usize>::new(),
        |mut order, (index, citation)| {
            order.entry(citation.node_id.clone()).or_insert(index);
            order
        },
    );
    let retained_citation_edges = answer
        .citations
        .iter()
        .map(|citation| {
            (
                citation.node_id.clone(),
                citation
                    .evidence_edge_ids
                    .iter()
                    .cloned()
                    .collect::<HashSet<_>>(),
            )
        })
        .collect::<HashMap<_, _>>();
    let retained_graph_edges = answer
        .graphs
        .iter()
        .filter_map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
            GraphArtifactDto::Mermaid { .. } => None,
        })
        .flatten()
        .map(|edge| (edge.id.clone(), edge))
        .collect::<HashMap<_, _>>();
    for obligation in &mut plan.claim_obligations {
        if obligation.proof_status == PacketObligationProofStatusDto::Contradicted
            || obligation.requires_complete_discovery
        {
            continue;
        }
        let snapshot_carriers = snapshot
            .carriers
            .iter()
            .filter(|entry| {
                entry.obligation_id == obligation.id && entry.obligation_kind == obligation.kind
            })
            .map(|entry| &entry.carrier_node_id)
            .collect::<Vec<_>>();
        let snapshot_proofs = snapshot
            .entries
            .iter()
            .filter(|entry| {
                entry.obligation_id == obligation.id
                    && entry.obligation_kind == obligation.kind
                    && obligation
                        .required_edge_kind
                        .is_none_or(|required_edge_kind| {
                            entry.proof.edge_kind == required_edge_kind
                        })
            })
            .collect::<Vec<_>>();
        let mut proofs = snapshot
            .entries
            .iter()
            .filter(|entry| {
                entry.obligation_id == obligation.id
                    && entry.obligation_kind == obligation.kind
                    && obligation
                        .required_edge_kind
                        .is_some_and(|required_edge_kind| {
                            entry.proof.edge_kind == required_edge_kind
                        })
                    && retained_order.contains_key(&entry.proof.carrier_node_id)
                    && retained_citation_edges
                        .get(&entry.proof.carrier_node_id)
                        .is_some_and(|edge_ids| edge_ids.contains(&entry.proof.edge_id))
                    && retained_graph_edges
                        .get(&entry.proof.edge_id)
                        .is_some_and(|edge| {
                            edge.kind == entry.proof.edge_kind
                                && (edge.source == entry.proof.carrier_node_id
                                    || edge.target == entry.proof.carrier_node_id)
                                && !is_speculative_trail_edge(edge)
                        })
            })
            .map(|entry| entry.proof.clone())
            .collect::<Vec<_>>();
        proofs.sort_by(|left, right| {
            retained_order[&left.carrier_node_id]
                .cmp(&retained_order[&right.carrier_node_id])
                .then_with(|| left.carrier_node_id.0.cmp(&right.carrier_node_id.0))
                .then_with(|| left.edge_id.0.cmp(&right.edge_id.0))
        });
        proofs.dedup_by(|left, right| {
            left.carrier_node_id == right.carrier_node_id
                && left.edge_id == right.edge_id
                && left.edge_kind == right.edge_kind
        });
        proofs.truncate(max_carriers.max(1));
        let retained_snapshot_carrier = snapshot_carriers
            .iter()
            .any(|carrier_node_id| retained_order.contains_key(*carrier_node_id));
        let removed_carrier_proof = snapshot_proofs
            .iter()
            .any(|entry| !retained_order.contains_key(&entry.proof.carrier_node_id));
        let retained_proof_edge_removed = snapshot_proofs.iter().any(|entry| {
            retained_order.contains_key(&entry.proof.carrier_node_id)
                && !proofs.iter().any(|proof| proof == &entry.proof)
        });
        let exact_evidence_removed = if obligation.required_edge_kind.is_some() {
            !snapshot_proofs.is_empty()
                && proofs.is_empty()
                && ((citations_omitted && removed_carrier_proof)
                    || (trail_edges_omitted && retained_proof_edge_removed))
        } else {
            !snapshot_carriers.is_empty() && !retained_snapshot_carrier && citations_omitted
        };
        if exact_evidence_removed {
            obligation.reason = Some(PACKET_BUDGET_TRUNCATED_REASON.to_string());
        } else if obligation.reason.as_deref() == Some(PACKET_BUDGET_TRUNCATED_REASON) {
            obligation.reason = None;
        }
        if !proofs.is_empty() {
            obligation.proof_status = PacketObligationProofStatusDto::Proven;
            obligation.carrier_edge_proofs = proofs;
        }
    }
}

fn sort_and_dedup_edge_proof_snapshot_entries(
    entries: &mut Vec<PacketObligationEdgeProofSnapshotEntry>,
) {
    entries.sort_by(|left, right| {
        left.obligation_id
            .cmp(&right.obligation_id)
            .then_with(|| {
                left.proof
                    .carrier_node_id
                    .0
                    .cmp(&right.proof.carrier_node_id.0)
            })
            .then_with(|| left.proof.edge_id.0.cmp(&right.proof.edge_id.0))
    });
    entries.dedup_by(|left, right| {
        left.obligation_id == right.obligation_id
            && left.obligation_kind == right.obligation_kind
            && left.proof == right.proof
    });
}

#[derive(Clone, Copy)]
struct PacketObligationEvidenceView {
    max_carriers: usize,
    citations_omitted: bool,
    trail_edges_omitted: bool,
}

impl PacketObligationEvidenceView {
    fn complete(max_carriers: usize) -> Self {
        Self {
            max_carriers: max_carriers.max(1),
            citations_omitted: false,
            trail_edges_omitted: false,
        }
    }

    fn from_budget(budget: &PacketBudgetDto) -> Self {
        Self {
            max_carriers: (budget.limits.max_anchors as usize).max(1),
            citations_omitted: packet_budget_omitted_obligation_evidence(budget, "citations"),
            trail_edges_omitted: packet_budget_omitted_obligation_evidence(budget, "trail_edges"),
        }
    }
}

fn obligation_evidence_was_removed_by_budget(
    obligation: &PacketClaimObligationDto,
    answer: &AgentAnswerDto,
    evidence_view: PacketObligationEvidenceView,
) -> bool {
    if obligation.reason.as_deref() == Some(PACKET_BUDGET_TRUNCATED_REASON) {
        return true;
    }
    if obligation.carrier_edge_proofs.is_empty() {
        return evidence_view.citations_omitted
            && !obligation.carrier_node_ids.is_empty()
            && obligation.carrier_node_ids.iter().all(|carrier_node_id| {
                !answer
                    .citations
                    .iter()
                    .any(|citation| citation.node_id == *carrier_node_id)
            });
    }

    let retained_graph_edges = answer
        .graphs
        .iter()
        .filter_map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
            GraphArtifactDto::Mermaid { .. } => None,
        })
        .flatten()
        .map(|edge| (&edge.id, edge))
        .collect::<HashMap<_, _>>();
    let proof_survives = |proof: &PacketObligationCarrierEdgeProofDto| {
        answer
            .citations
            .iter()
            .find(|citation| citation.node_id == proof.carrier_node_id)
            .is_some_and(|citation| citation.evidence_edge_ids.contains(&proof.edge_id))
            && retained_graph_edges
                .get(&proof.edge_id)
                .is_some_and(|edge| {
                    edge.kind == proof.edge_kind
                        && (edge.source == proof.carrier_node_id
                            || edge.target == proof.carrier_node_id)
                        && !is_speculative_trail_edge(edge)
                })
    };
    if obligation.carrier_edge_proofs.iter().any(proof_survives) {
        return false;
    }
    obligation.carrier_edge_proofs.iter().any(|proof| {
        let carrier_retained = answer
            .citations
            .iter()
            .any(|citation| citation.node_id == proof.carrier_node_id);
        (!carrier_retained && evidence_view.citations_omitted)
            || (carrier_retained && evidence_view.trail_edges_omitted)
    })
}

fn finalize_packet_claim_obligations(
    question: &str,
    task_class: PacketTaskClassDto,
    plan: &mut PacketObligationPlanDto,
    answer: &AgentAnswerDto,
    evidence_view: PacketObligationEvidenceView,
) {
    let binding_terms = plan.binding_terms.clone();
    let requested_paths = packet_obligation_requested_paths(plan);
    let exact_binding_terms = packet_prompt_exact_symbol_probe_queries(
        question,
        &packet_probe_terms(question),
        task_class,
    );
    for obligation in &mut plan.claim_obligations {
        if obligation.proof_status == PacketObligationProofStatusDto::Contradicted {
            continue;
        }
        if obligation_evidence_was_removed_by_budget(obligation, answer, evidence_view) {
            obligation.reason = Some(PACKET_BUDGET_TRUNCATED_REASON.to_string());
        }
        if obligation.probe_binding.is_some() {
            finalize_exact_probe_obligation(obligation, answer, evidence_view);
            continue;
        }
        obligation.carrier_node_ids.clear();
        obligation.carrier_paths.clear();
        obligation.carrier_edge_proofs.clear();
        if obligation.id == REQUESTED_CLAIM_OVERFLOW_ID {
            obligation.proof_status = PacketObligationProofStatusDto::Unsupported;
            obligation
                .reason
                .get_or_insert_with(|| "requested_claim_binding_limit_exceeded".to_string());
            continue;
        }
        let obligation_binding_terms = if obligation.binding_terms.is_empty() {
            binding_terms.clone()
        } else {
            obligation.binding_terms.clone()
        };
        finalize_default_profile_obligation(
            obligation,
            &obligation_binding_terms,
            &exact_binding_terms,
            &requested_paths,
            answer,
            evidence_view,
        );
    }
}

fn finalize_exact_probe_obligation(
    obligation: &mut PacketClaimObligationDto,
    answer: &AgentAnswerDto,
    evidence_view: PacketObligationEvidenceView,
) {
    let evidence_removed_by_budget =
        obligation.reason.as_deref() == Some(PACKET_BUDGET_TRUNCATED_REASON);
    obligation.carrier_node_ids.clear();
    obligation.carrier_paths.clear();
    obligation.carrier_edge_proofs.clear();
    let Some(binding) = obligation.probe_binding.clone() else {
        return;
    };
    match binding.status {
        PacketProbeResolutionStatusDto::Rejected => {
            obligation.proof_status = PacketObligationProofStatusDto::Unsupported;
            obligation.reason = Some(format!(
                "exact_probe_rejected:{}",
                binding
                    .rejection
                    .as_ref()
                    .map(|rejection| packet_probe_rejection_code_id(rejection.code))
                    .unwrap_or("reason_unavailable")
            ));
            return;
        }
        PacketProbeResolutionStatusDto::Ambiguous => {
            obligation.proof_status = PacketObligationProofStatusDto::Unsupported;
            obligation.reason = Some(format!(
                "exact_probe_ambiguous:candidates={}",
                binding.candidates.len()
            ));
            return;
        }
        PacketProbeResolutionStatusDto::FreeQuery => {
            obligation.proof_status = PacketObligationProofStatusDto::Unsupported;
            obligation.reason = Some("exact_probe_binding_is_not_exact".to_string());
            return;
        }
        PacketProbeResolutionStatusDto::ExactPath
        | PacketProbeResolutionStatusDto::ValidUncoveredPath
        | PacketProbeResolutionStatusDto::IndexedSymbol
        | PacketProbeResolutionStatusDto::FileScopedSymbol
        | PacketProbeResolutionStatusDto::TextHit
        | PacketProbeResolutionStatusDto::Continuation => {}
    }
    if !packet_probe_resolution_binding_is_complete(&binding) {
        obligation.proof_status = PacketObligationProofStatusDto::Unsupported;
        obligation.reason = Some("exact_probe_resolution_binding_missing".to_string());
        return;
    }
    let exact_path_binding = matches!(&binding.probe, PacketProbeDto::ExactPath { .. });
    let matching_citations = answer
        .citations
        .iter()
        .filter(|citation| {
            let admissible_carrier = if exact_path_binding {
                citation_sufficiency_eligible(citation)
                    && matches!(
                        citation.kind,
                        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
                    )
                    && !citation_is_exact_probe_diagnostic(citation)
                    && packet_evidence_role(citation).is_some_and(|role| {
                        !matches!(
                            role,
                            PacketEvidenceRole::SourceEvidence
                                | PacketEvidenceRole::TestsAndRegressionCoverage
                        )
                    })
            } else {
                citation.evidence_producer.as_deref() == Some("packet_exact_symbol_probe")
            };
            admissible_carrier && citation_matches_exact_probe_binding(citation, &binding)
        })
        .collect::<Vec<_>>();
    if matching_citations.is_empty() {
        obligation.proof_status = if evidence_removed_by_budget {
            PacketObligationProofStatusDto::Reported
        } else {
            PacketObligationProofStatusDto::Unsupported
        };
        obligation.reason = Some(
            if evidence_removed_by_budget {
                PACKET_BUDGET_TRUNCATED_REASON
            } else {
                "exact_probe_carrier_missing"
            }
            .to_string(),
        );
        return;
    }
    record_obligation_carriers(obligation, matching_citations, evidence_view.max_carriers);
    obligation.proof_status = PacketObligationProofStatusDto::Proven;
    obligation.reason = None;
}

fn packet_probe_resolution_binding_is_complete(binding: &PacketProbeResolutionDto) -> bool {
    match &binding.probe {
        PacketProbeDto::ExactPath { .. } => binding.path.is_some(),
        PacketProbeDto::SymbolId { id } => binding
            .symbol_id
            .as_deref()
            .is_some_and(|resolved| resolved == id.trim()),
        PacketProbeDto::FileSymbol { .. } => binding.path.is_some() && binding.symbol_id.is_some(),
        PacketProbeDto::Continuation {
            symbol_id: Some(requested),
            ..
        } => binding
            .symbol_id
            .as_deref()
            .is_some_and(|resolved| resolved == requested.trim()),
        PacketProbeDto::FreeQuery { .. }
        | PacketProbeDto::Continuation {
            symbol_id: None, ..
        } => false,
    }
}

fn citation_is_exact_probe_diagnostic(citation: &AgentCitationDto) -> bool {
    matches!(
        citation.evidence_producer.as_deref(),
        Some("packet_exact_path_probe" | "packet_exact_symbol_probe")
    )
}

fn citation_matches_exact_probe_binding(
    citation: &AgentCitationDto,
    binding: &PacketProbeResolutionDto,
) -> bool {
    let path_matches = |expected: &str| {
        citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .is_some_and(|actual| actual == packet_display_path(expected))
    };
    match &binding.probe {
        PacketProbeDto::ExactPath { .. } => binding.path.as_deref().is_some_and(path_matches),
        PacketProbeDto::SymbolId { .. }
        | PacketProbeDto::Continuation {
            symbol_id: Some(_), ..
        } => {
            binding
                .symbol_id
                .as_deref()
                .is_some_and(|symbol_id| citation.node_id.0 == symbol_id)
                && binding.path.as_deref().is_none_or(path_matches)
        }
        PacketProbeDto::FileSymbol { symbol, .. } => {
            binding
                .symbol_id
                .as_deref()
                .is_some_and(|symbol_id| citation.node_id.0 == symbol_id)
                && binding.path.as_deref().is_some_and(path_matches)
                && citation_display_matches_exact_requested_identity(
                    &citation.display_name,
                    symbol.trim(),
                )
        }
        PacketProbeDto::FreeQuery { .. }
        | PacketProbeDto::Continuation {
            symbol_id: None, ..
        } => false,
    }
}

fn finalize_default_profile_obligation(
    obligation: &mut PacketClaimObligationDto,
    binding_terms: &[String],
    exact_binding_terms: &[String],
    requested_paths: &[String],
    answer: &AgentAnswerDto,
    evidence_view: PacketObligationEvidenceView,
) {
    let evidence_removed_by_budget =
        obligation.reason.as_deref() == Some(PACKET_BUDGET_TRUNCATED_REASON);
    let has_requested_identity = !binding_terms.is_empty();
    let reported_citations = answer
        .citations
        .iter()
        .filter(|citation| {
            citation_matches_default_profile_binding(
                citation,
                binding_terms,
                exact_binding_terms,
                requested_paths,
            ) && has_requested_identity
        })
        .collect::<Vec<_>>();
    record_obligation_carriers(
        obligation,
        reported_citations.iter().copied(),
        evidence_view.max_carriers,
    );
    if obligation.requires_complete_discovery {
        obligation.proof_status = PacketObligationProofStatusDto::Reported;
        obligation.reason = Some("complete_discovery_and_collector_coverage_unproven".to_string());
    } else if reported_citations.is_empty() {
        if evidence_removed_by_budget {
            obligation.proof_status = PacketObligationProofStatusDto::Reported;
            obligation.reason = Some(PACKET_BUDGET_TRUNCATED_REASON.to_string());
        } else {
            obligation.proof_status = PacketObligationProofStatusDto::Unsupported;
            obligation.reason = Some("selected_claim_profile_carrier_missing".to_string());
        }
    } else {
        let proven_citations = reported_citations
            .iter()
            .copied()
            .filter(|citation| {
                citation_sufficiency_eligible(citation)
                    && obligation.allowed_node_kinds.contains(&citation.kind)
                    && obligation
                        .required_edge_kind
                        .is_none_or(|required_edge_kind| {
                            citation_edge_proof(citation, required_edge_kind, answer).is_some()
                        })
            })
            .collect::<Vec<_>>();
        if proven_citations.is_empty() {
            obligation.proof_status = PacketObligationProofStatusDto::Reported;
            obligation.reason = Some(
                if evidence_removed_by_budget {
                    PACKET_BUDGET_TRUNCATED_REASON
                } else {
                    "selected_claim_profile_requires_typed_flow"
                }
                .to_string(),
            );
        } else {
            record_obligation_carriers(
                obligation,
                proven_citations.iter().copied(),
                evidence_view.max_carriers,
            );
            if let Some(required_edge_kind) = obligation.required_edge_kind {
                record_obligation_edge_proofs(
                    obligation,
                    &proven_citations,
                    required_edge_kind,
                    answer,
                    evidence_view.max_carriers,
                );
            }
            obligation.proof_status = PacketObligationProofStatusDto::Proven;
            obligation.reason = None;
        }
    }
}

fn packet_budget_omitted_obligation_evidence(budget: &PacketBudgetDto, section: &str) -> bool {
    budget.truncated
        && budget
            .omitted_sections
            .iter()
            .any(|omitted| omitted == section)
}

/// Whether a requested term is certainly a symbol rather than a capitalised English word.
///
/// Case-sensitive carrier matching is right when someone typed an identifier and wrong when
/// they wrote a product or language name, and the retrieval-side classifier that produces
/// `exact_binding_terms` cannot tell those apart: it treats any word with an internal
/// capital as an identifier, so ordinary mixed-case product, language, and acronym terms
/// all arrive here as exact terms. Under case-sensitive matching a repository whose spelling
/// convention differs from the question's then cannot satisfy its own obligation: a cited
/// lower-camel identifier and the equivalent acronym-prefixed term compare as different
/// strings, so the packet holds the carrier and rejects it.
///
/// Punctuation is the honest discriminator: prose does not put `_`, `::`, `.`, `/`, or `$`
/// inside a word. Everything else falls back to case-insensitive matching, which still
/// matches an exactly-spelled identifier -- it just also accepts the same identity under a
/// different casing convention. That predicate is a property of the two spellings, not of
/// any language or repository.
///
/// Deliberately scoped to obligation binding. The shared classifier also drives lexical
/// search, scoring, and query intent, and changing it there would move retrieval itself.
fn term_is_unambiguously_a_symbol(term: &str) -> bool {
    let term = term.trim();
    term.contains('_')
        || term.contains("::")
        || term.contains('.')
        || term.contains('/')
        || term.contains('\\')
        || term.contains('$')
        || term.contains('#')
        // Lower-initial with an internal capital is camelCase, which English is not.
        || (term
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
            && term.chars().skip(1).any(|ch| ch.is_ascii_uppercase()))
}

fn citation_matches_default_profile_binding(
    citation: &AgentCitationDto,
    binding_terms: &[String],
    exact_binding_terms: &[String],
    requested_paths: &[String],
) -> bool {
    let path = packet_display_path(citation.file_path.as_deref().unwrap_or_default());
    let identity_matches = binding_terms.is_empty()
        || binding_terms.iter().any(|term| {
            citation_display_matches_requested_identity_with_case(
                &citation.display_name,
                term,
                exact_binding_terms.iter().any(|exact| exact == term)
                    && term_is_unambiguously_a_symbol(term),
            )
        });
    let path_scope_matches = requested_paths.is_empty()
        || requested_paths.iter().any(|requested_path| {
            let requested_path = packet_display_path(requested_path);
            path.eq_ignore_ascii_case(&requested_path)
                || path
                    .to_ascii_lowercase()
                    .ends_with(&format!("/{}", requested_path.to_ascii_lowercase()))
        });
    identity_matches && path_scope_matches
}

#[cfg(test)]
fn citation_display_matches_requested_identity(display_name: &str, requested: &str) -> bool {
    let exact_case = exact_symbol_query_terms(requested)
        .iter()
        .any(|candidate| candidate == requested.trim());
    citation_display_matches_requested_identity_with_case(display_name, requested, exact_case)
}

fn citation_display_matches_requested_identity_with_case(
    display_name: &str,
    requested: &str,
    exact_case: bool,
) -> bool {
    if exact_case {
        return citation_display_matches_exact_requested_identity(display_name, requested);
    }
    identity_segments_cover(
        &symbol_identity_segments(display_name),
        &symbol_identity_segments(requested),
    )
}

fn citation_display_matches_exact_requested_identity(display_name: &str, requested: &str) -> bool {
    identity_segments_cover(
        &exact_symbol_identity_segments(display_name),
        &exact_symbol_identity_segments(requested),
    )
}

/// A requested identity matches as a suffix (`Type.method` covers `method`) or
/// as an owner prefix (`Type.method` covers `Type`). Suffix-only matching left
/// a cited method unable to carry the type the question named.
fn identity_segments_cover(display_segments: &[String], requested_segments: &[String]) -> bool {
    if requested_segments.is_empty() || display_segments.len() < requested_segments.len() {
        return false;
    }
    let suffix_start = display_segments.len() - requested_segments.len();
    display_segments[suffix_start..] == requested_segments[..]
        || display_segments[..requested_segments.len()] == requested_segments[..]
}

fn exact_symbol_identity_segments(value: &str) -> Vec<String> {
    value
        .split([':', '.', '#', '/', '\\'])
        .map(str::trim)
        // Display names may render a zero-argument callable with `()`. Strip only that known
        // presentation suffix: punctuation such as Ruby's `?` and `!` is part of the identity.
        .map(|segment| segment.strip_suffix("()").unwrap_or(segment))
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

fn symbol_identity_segments(value: &str) -> Vec<String> {
    value
        .split([':', '.', '#', '/', '\\'])
        .map(normalize_identifier)
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn record_obligation_carriers<'a>(
    obligation: &mut PacketClaimObligationDto,
    citations: impl IntoIterator<Item = &'a AgentCitationDto>,
    max_carriers: usize,
) {
    let mut carrier_node_ids = Vec::new();
    let mut seen_node_ids = HashSet::new();
    let mut carrier_paths = Vec::new();
    let mut seen_paths = HashSet::new();
    for citation in citations.into_iter().take(max_carriers.max(1)) {
        if seen_node_ids.insert(citation.node_id.clone()) {
            carrier_node_ids.push(citation.node_id.clone());
        }
        if let Some(path) = citation.file_path.clone()
            && seen_paths.insert(path.clone())
        {
            carrier_paths.push(path);
        }
    }
    // The caller relevance-ranks the input across every retrieval stage. Preserve that order: the
    // pre-budget snapshot protects the first carrier, so sorting opaque node ids here made the
    // retained proof depend on a hash rather than evidence quality.
    obligation.carrier_node_ids = carrier_node_ids;
    obligation.carrier_paths = carrier_paths;
}

fn citation_edge_proof(
    citation: &AgentCitationDto,
    required_edge_kind: EdgeKind,
    answer: &AgentAnswerDto,
) -> Option<PacketObligationCarrierEdgeProofDto> {
    let graphs = packet_execution_graphs(answer);
    let cited_edge_ids = citation.evidence_edge_ids.iter().collect::<HashSet<_>>();
    graphs
        .iter()
        .flat_map(|graph| graph.edges.iter().map(move |edge| (*graph, edge)))
        .filter(|(graph, edge)| {
            edge.kind == required_edge_kind
                && cited_edge_ids.contains(&edge.id)
                && (edge.source == citation.node_id || edge.target == citation.node_id)
                && receipt_neighbor(graph, answer, citation, edge).is_some_and(|(_, kind)| {
                    required_edge_kind != EdgeKind::CALL
                        || ordinary_incident_call_receipt_is_valid(citation, edge, kind)
                })
        })
        .min_by(|(_, left), (_, right)| left.id.0.cmp(&right.id.0))
        .map(|(_, edge)| PacketObligationCarrierEdgeProofDto {
            carrier_node_id: citation.node_id.clone(),
            edge_id: edge.id.clone(),
            edge_kind: edge.kind,
        })
}

fn receipt_neighbor<'a>(
    graph: &'a GraphResponse,
    answer: &'a AgentAnswerDto,
    citation: &AgentCitationDto,
    edge: &GraphEdgeDto,
) -> Option<(&'a str, NodeKind)> {
    let neighbor_id = if edge.source == citation.node_id {
        &edge.target
    } else if edge.target == citation.node_id {
        &edge.source
    } else {
        return None;
    };
    graph
        .nodes
        .iter()
        .find(|node| node.id == *neighbor_id)
        .map(|node| (node.label.as_str(), node.kind))
        .or_else(|| {
            answer
                .citations
                .iter()
                .find(|candidate| candidate.node_id == *neighbor_id)
                .map(|candidate| (candidate.display_name.as_str(), candidate.kind))
        })
}

fn record_obligation_edge_proofs(
    obligation: &mut PacketClaimObligationDto,
    citations: &[&AgentCitationDto],
    required_edge_kind: EdgeKind,
    answer: &AgentAnswerDto,
    max_proofs: usize,
) {
    let retained_carrier_node_ids = obligation
        .carrier_node_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let mut proofs = citations
        .iter()
        .filter_map(|citation| citation_edge_proof(citation, required_edge_kind, answer))
        .filter(|proof| retained_carrier_node_ids.contains(&proof.carrier_node_id))
        .collect::<Vec<_>>();
    proofs.sort_by(|left, right| {
        left.carrier_node_id
            .0
            .cmp(&right.carrier_node_id.0)
            .then_with(|| left.edge_id.0.cmp(&right.edge_id.0))
    });
    proofs.dedup_by(|left, right| {
        left.carrier_node_id == right.carrier_node_id
            && left.edge_id == right.edge_id
            && left.edge_kind == right.edge_kind
    });
    proofs.truncate(max_proofs.max(1));
    obligation.carrier_edge_proofs = proofs;
}

fn finalize_query_obligations(
    plan: &mut PacketObligationPlanDto,
    answer: &AgentAnswerDto,
    _budget: &PacketBudgetDto,
) {
    for obligation in &mut plan.query_obligations {
        if let Some(diagnostic) = answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .iter()
            .rev()
            .find(|diagnostic| diagnostic.query == obligation.query)
        {
            obligation.completion = Some(diagnostic.completion.clone());
            continue;
        }
        if let Some(step) = answer.retrieval_trace.steps.iter().rev().find(|step| {
            step.kind == AgentRetrievalStepKindDto::Search
                && step
                    .input
                    .iter()
                    .any(|field| field.key == "query" && field.value == obligation.query)
        }) {
            obligation.completion = Some(match step.status {
                AgentRetrievalStepStatusDto::Ok => PacketQueryCompletionDto::Completed,
                AgentRetrievalStepStatusDto::Error => PacketQueryCompletionDto::Cancelled {
                    reason: "retrieval_error".to_string(),
                },
                AgentRetrievalStepStatusDto::Skipped => PacketQueryCompletionDto::Cancelled {
                    reason: "retrieval_skipped".to_string(),
                },
                AgentRetrievalStepStatusDto::Truncated => PacketQueryCompletionDto::Cancelled {
                    reason: "retrieval_truncated".to_string(),
                },
            });
            continue;
        }
        if obligation.completion.is_none() {
            obligation.completion = Some(PacketQueryCompletionDto::Cancelled {
                reason: "not_dispatched".to_string(),
            });
        }
    }
}

pub fn bind_claims_to_packet_obligations(
    plan: &PacketObligationPlanDto,
    claims: &mut [PacketClaimDto],
) {
    for claim in claims.iter_mut().filter(|claim| {
        claim.eligible_for_sufficiency == Some(true) && claim.required_obligation_ids.is_empty()
    }) {
        // Category membership and citation roles are not claim identity. An eligible claim must
        // name the exact planned row whose semantics it asserts, even when the plan is empty.
        claim.proof_status = Some(PacketProofStatusDto::Reported);
        claim.eligible_for_sufficiency = Some(false);
    }
    if plan.claim_obligations.is_empty() {
        return;
    }
    for claim in claims {
        // Citation membership alone is not a semantic binding: a runtime node may prove an
        // entrypoint row while a claim made with that node asserts missing storage behavior. Each
        // claim therefore names the exact obligation IDs it asserts. Optional kind declarations
        // constrain those named rows; they never select sibling rows. Every binding must be Proven
        // *and* carried by one of this claim's own citations, and every citation must participate
        // in one of those required bindings.
        let required_obligations = plan
            .claim_obligations
            .iter()
            .filter(|obligation| {
                claim
                    .required_obligation_ids
                    .iter()
                    .any(|id| id == &obligation.id)
            })
            .collect::<Vec<_>>();
        let unique_required_ids = claim.required_obligation_ids.iter().collect::<HashSet<_>>();
        let has_declared_binding = !unique_required_ids.is_empty()
            && required_obligations.len() == unique_required_ids.len();
        let every_required_id_is_proven_and_carried =
            claim.required_obligation_ids.iter().all(|id| {
                required_obligations.iter().any(|obligation| {
                    obligation.id == *id
                        && obligation.proof_status == PacketObligationProofStatusDto::Proven
                        && claim
                            .citations
                            .iter()
                            .any(|citation| obligation.carrier_node_ids.contains(&citation.node_id))
                })
            });
        let every_required_kind_is_proven_and_carried =
            claim.required_obligation_kinds.iter().all(|kind| {
                required_obligations.iter().any(|obligation| {
                    obligation.kind == *kind
                        && obligation.proof_status == PacketObligationProofStatusDto::Proven
                        && claim
                            .citations
                            .iter()
                            .any(|citation| obligation.carrier_node_ids.contains(&citation.node_id))
                })
            });
        let exact_rows_match_declared_kinds = claim.required_obligation_kinds.is_empty()
            || required_obligations
                .iter()
                .all(|obligation| claim.required_obligation_kinds.contains(&obligation.kind));
        let every_citation_has_a_required_binding = claim.citations.iter().all(|citation| {
            required_obligations.iter().any(|obligation| {
                obligation.proof_status == PacketObligationProofStatusDto::Proven
                    && obligation.carrier_node_ids.contains(&citation.node_id)
            })
        });
        let proven = has_declared_binding
            && every_required_id_is_proven_and_carried
            && every_required_kind_is_proven_and_carried
            && exact_rows_match_declared_kinds
            && every_citation_has_a_required_binding;
        if proven {
            claim.proof_status = Some(PacketProofStatusDto::Proven);
        } else {
            claim.proof_status = Some(packet_unproven_claim_status(claim, &required_obligations));
            claim.eligible_for_sufficiency = Some(false);
        }
    }
}

fn packet_unproven_claim_status(
    claim: &PacketClaimDto,
    required_obligations: &[&PacketClaimObligationDto],
) -> PacketProofStatusDto {
    if claim.coverage_role.as_deref() == Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE)
        && required_obligations.len() == 1
        && matches!(
            required_obligations[0].proof_status,
            PacketObligationProofStatusDto::Planned
                | PacketObligationProofStatusDto::Unsupported
                | PacketObligationProofStatusDto::Contradicted
        )
    {
        PacketProofStatusDto::Unsupported
    } else {
        PacketProofStatusDto::Reported
    }
}

pub fn packet_claims_with_obligation_receipts<T>(
    answer: &AgentAnswerDto,
    task_class: PacketTaskClassDto,
    plan: &PacketObligationPlanDto,
    supported_claims_with_telemetry: (Vec<PacketClaimDto>, T),
) -> Vec<PacketClaimDto> {
    packet_claims_with_obligation_receipts_and_telemetry(
        answer,
        task_class,
        plan,
        supported_claims_with_telemetry,
    )
    .0
}

pub fn packet_claims_with_obligation_receipts_and_telemetry<T>(
    answer: &AgentAnswerDto,
    task_class: PacketTaskClassDto,
    plan: &PacketObligationPlanDto,
    (mut claims, telemetry): (Vec<PacketClaimDto>, T),
) -> (Vec<PacketClaimDto>, T) {
    bind_role_claims_to_exact_path_obligations(plan, &mut claims);
    append_packet_obligation_receipt_claims(answer, task_class, plan, &mut claims);
    (claims, telemetry)
}

fn bind_role_claims_to_exact_path_obligations(
    plan: &PacketObligationPlanDto,
    claims: &mut [PacketClaimDto],
) {
    for obligation in plan.claim_obligations.iter().filter(|obligation| {
        obligation.material
            && obligation.proof_status == PacketObligationProofStatusDto::Proven
            && obligation
                .probe_binding
                .as_ref()
                .is_some_and(|binding| matches!(&binding.probe, PacketProbeDto::ExactPath { .. }))
    }) {
        let Some(claim) = claims.iter_mut().find(|claim| {
            claim.required_obligation_ids.is_empty()
                && exact_path_role_claim_matches_obligation(claim, obligation)
        }) else {
            continue;
        };
        claim.required_obligation_ids = vec![obligation.id.clone()];
        claim.required_obligation_kinds = vec![PacketClaimObligationKindDto::ExactProbe];
        claim.eligible_for_sufficiency = Some(true);
    }
}

fn exact_path_role_claim_matches_obligation(
    claim: &PacketClaimDto,
    obligation: &PacketClaimObligationDto,
) -> bool {
    let Some(claim_role) = claim.coverage_role.as_deref() else {
        return false;
    };
    !claim.citations.is_empty()
        && claim.citations.iter().all(|citation| {
            obligation.carrier_node_ids.contains(&citation.node_id)
                && citation_sufficiency_eligible(citation)
                && matches!(
                    citation.kind,
                    NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
                )
                && !citation_is_exact_probe_diagnostic(citation)
                && packet_evidence_role(citation).is_some_and(|role| {
                    !matches!(
                        role,
                        PacketEvidenceRole::SourceEvidence
                            | PacketEvidenceRole::TestsAndRegressionCoverage
                    ) && role.as_str() == claim_role
                })
        })
}

fn append_packet_obligation_receipt_claims(
    answer: &AgentAnswerDto,
    task_class: PacketTaskClassDto,
    plan: &PacketObligationPlanDto,
    claims: &mut Vec<PacketClaimDto>,
) {
    let mut seen_ids = HashSet::new();
    for obligation in plan
        .claim_obligations
        .iter()
        .filter(|obligation| obligation.material)
    {
        if !seen_ids.insert(obligation.id.as_str()) {
            continue;
        }
        let mut seen_carriers = HashSet::new();
        let citations = answer
            .citations
            .iter()
            .filter(|citation| obligation.carrier_node_ids.contains(&citation.node_id))
            .filter(|citation| seen_carriers.insert(citation.node_id.0.clone()))
            .cloned()
            .collect::<Vec<_>>();
        let has_carried_citation = !citations.is_empty();
        let status = packet_obligation_receipt_proof_status(obligation, has_carried_citation);
        let exact_path_obligation = obligation
            .probe_binding
            .as_ref()
            .is_some_and(|binding| matches!(&binding.probe, PacketProbeDto::ExactPath { .. }));
        let eligible = !exact_path_obligation
            && obligation.proof_status == PacketObligationProofStatusDto::Proven
            && has_carried_citation;
        claims.push(PacketClaimDto {
            claim: packet_obligation_receipt_text(answer, task_class, obligation, &citations),
            required_obligation_ids: vec![obligation.id.clone()],
            required_obligation_kinds: vec![obligation.kind],
            proof_status: Some(status),
            required_evidence_role: None,
            citations,
            coverage_role: Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE.to_string()),
            eligible_for_sufficiency: Some(eligible),
        });
    }
}

fn packet_obligation_receipt_proof_status(
    obligation: &PacketClaimObligationDto,
    has_carried_citation: bool,
) -> PacketProofStatusDto {
    match obligation.proof_status {
        PacketObligationProofStatusDto::Proven if has_carried_citation => {
            PacketProofStatusDto::Proven
        }
        PacketObligationProofStatusDto::Reported => PacketProofStatusDto::Reported,
        PacketObligationProofStatusDto::Planned
        | PacketObligationProofStatusDto::Unsupported
        | PacketObligationProofStatusDto::Contradicted => PacketProofStatusDto::Unsupported,
        PacketObligationProofStatusDto::Proven => PacketProofStatusDto::Reported,
    }
}

fn packet_obligation_receipt_text(
    answer: &AgentAnswerDto,
    _task_class: PacketTaskClassDto,
    obligation: &PacketClaimObligationDto,
    citations: &[AgentCitationDto],
) -> String {
    if obligation.proof_status == PacketObligationProofStatusDto::Proven && !citations.is_empty() {
        if let Some(receipt) = cited_graph_relation_receipt(answer, citations) {
            return receipt;
        }
        let anchors = citations
            .iter()
            .map(|citation| format!("`{}`", citation.display_name))
            .collect::<Vec<_>>()
            .join(", ");
        return format!(
            "Material obligation `{}` has independently cited carrier evidence at {}.",
            obligation.id, anchors
        );
    }
    let status = match obligation.proof_status {
        PacketObligationProofStatusDto::Planned => "planned",
        PacketObligationProofStatusDto::Proven => "proven",
        PacketObligationProofStatusDto::Reported => "reported",
        PacketObligationProofStatusDto::Unsupported => "unsupported",
        PacketObligationProofStatusDto::Contradicted => "contradicted",
    };
    let reason = obligation.reason.as_deref().unwrap_or_else(|| {
        if obligation.proof_status == PacketObligationProofStatusDto::Proven {
            "carrier_citation_missing_from_packet"
        } else {
            "reason_unavailable"
        }
    });
    format!(
        "Material obligation `{}` is `{status}`: `{reason}`.",
        obligation.id
    )
}

/// Name a retained CALL/INHERITANCE among this obligation's carriers when the typed
/// flow-requirement receipt did not fire. The verb and the two spellings come from
/// the graph; Compact used to drop those edges, leaving only "has independently
/// cited carrier evidence".
fn cited_graph_relation_receipt(
    answer: &AgentAnswerDto,
    citations: &[AgentCitationDto],
) -> Option<String> {
    let cited = citations
        .iter()
        .map(|citation| citation.node_id.clone())
        .collect::<HashSet<_>>();
    let label = |graph: &GraphResponse, id: &NodeId| -> Option<String> {
        graph
            .nodes
            .iter()
            .find(|node| node.id == *id)
            .map(|node| node.label.clone())
            .or_else(|| {
                citations
                    .iter()
                    .find(|citation| citation.node_id == *id)
                    .map(|citation| citation.display_name.clone())
            })
    };
    packet_execution_graphs(answer).iter().find_map(|graph| {
        let mut both_endpoints = None;
        let mut one_endpoint = None;
        for edge in &graph.edges {
            if !matches!(edge.kind, EdgeKind::CALL | EdgeKind::INHERITANCE) {
                continue;
            }
            let source_cited = cited.contains(&edge.source);
            let target_cited = cited.contains(&edge.target);
            if !source_cited && !target_cited {
                continue;
            }
            let Some(from) = label(graph, &edge.source) else {
                continue;
            };
            let Some(to) = label(graph, &edge.target) else {
                continue;
            };
            let verb = match edge.kind {
                EdgeKind::CALL => "calls",
                EdgeKind::INHERITANCE => "extends",
                _ => "relates to",
            };
            let sentence = format!("`{from}` {verb} `{to}`.");
            if source_cited && target_cited {
                both_endpoints = Some(sentence);
                break;
            }
            if one_endpoint.is_none() {
                one_endpoint = Some(sentence);
            }
        }
        both_endpoints.or(one_endpoint)
    })
}

pub fn material_packet_obligations_are_proven(plan: &PacketObligationPlanDto) -> bool {
    plan.claim_obligations
        .iter()
        .filter(|obligation| obligation.material)
        .all(|obligation| obligation.proof_status == PacketObligationProofStatusDto::Proven)
        && plan
            .query_obligations
            .iter()
            .filter(|obligation| obligation.material)
            .all(|obligation| {
                matches!(
                    obligation.completion,
                    Some(PacketQueryCompletionDto::Completed)
                )
            })
}

pub fn packet_obligation_open_next_candidates(plan: &PacketObligationPlanDto) -> Vec<String> {
    let has_missing_material_claim = plan.claim_obligations.iter().any(|obligation| {
        obligation.material && obligation.proof_status != PacketObligationProofStatusDto::Proven
    });
    let mut candidates = plan
        .claim_obligations
        .iter()
        .filter(|obligation| {
            obligation.proof_status != PacketObligationProofStatusDto::Proven
                && (obligation.material || !obligation.carrier_paths.is_empty())
        })
        .flat_map(|obligation| {
            let mut candidates = obligation.carrier_paths.clone();
            if obligation.material {
                candidates.extend(obligation.open_next_candidates.iter().cloned());
            }
            candidates
        })
        .collect::<BTreeSet<_>>();
    if has_missing_material_claim {
        candidates.extend(packet_obligation_requested_paths(plan));
    }
    candidates.into_iter().collect()
}

/// Return at most one actionable query for each unmet material obligation.
///
/// Claim rows lead with the source or query the planner attached to that exact row. Query rows
/// retain their original query and cancellation cause in the ledger; this helper only projects a
/// bounded next action. Keeping the row-to-query mapping one-to-one prevents packet gaps from
/// multiplying into several equivalent adapter commands.
pub fn packet_unmet_material_follow_up_queries(plan: &PacketObligationPlanDto) -> Vec<String> {
    let mut queries = Vec::new();
    let mut requested_paths = packet_obligation_requested_paths(plan).into_iter();
    for obligation in plan.claim_obligations.iter().filter(|obligation| {
        obligation.material && obligation.proof_status != PacketObligationProofStatusDto::Proven
    }) {
        let candidate = obligation
            .carrier_paths
            .iter()
            .find(|candidate| !candidate.trim().is_empty())
            .cloned()
            .or_else(|| requested_paths.next())
            .or_else(|| {
                obligation
                    .open_next_candidates
                    .iter()
                    .chain(obligation.binding_terms.iter())
                    .find(|candidate| !candidate.trim().is_empty())
                    .cloned()
            })
            .unwrap_or_else(|| obligation.id.replace('_', " "));
        if !queries.iter().any(|existing| existing == &candidate) {
            queries.push(candidate);
        }
    }
    for obligation in plan.query_obligations.iter().filter(|obligation| {
        obligation.material
            && !obligation.query.trim().is_empty()
            && !matches!(
                obligation.completion.as_ref(),
                Some(PacketQueryCompletionDto::Completed)
            )
    }) {
        if !queries.iter().any(|existing| existing == &obligation.query) {
            queries.push(obligation.query.clone());
        }
    }
    queries
}

fn packet_obligation_requested_paths(plan: &PacketObligationPlanDto) -> Vec<String> {
    plan.query_obligations
        .iter()
        .map(|obligation| obligation.query.as_str())
        // Prompt-derived exact symbols are already carried as case-bearing claim bindings. A
        // slash may qualify a symbol (`Foo/run`); it does not turn that exact identity into a
        // requested source path.
        .filter(|query| !plan.binding_terms.iter().any(|term| term == query))
        .filter(|query| packet_obligation_candidate_is_path(query))
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(PACKET_SUPPLEMENTAL_QUERY_OBLIGATION_LIMIT)
        .collect()
}

pub fn packet_proven_obligation_carrier_paths(plan: &PacketObligationPlanDto) -> Vec<String> {
    plan.claim_obligations
        .iter()
        .filter(|obligation| obligation.proof_status == PacketObligationProofStatusDto::Proven)
        .flat_map(|obligation| obligation.carrier_paths.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn packet_question_requires_complete_discovery(
    _question: &str,
    _task_class: PacketTaskClassDto,
) -> bool {
    // Absence is observation-receipt only. Prompt taxonomies must not force
    // complete-discovery obligations.
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalTraceDto, EdgeId,
        GraphArtifactDto, GraphEdgeDto, GraphNodeDto, GraphResponse, IndexFreshnessDto,
        IndexFreshnessStatusDto, NodeId, PACKET_PROBE_CONTRACT_VERSION, PacketBudgetLimitsDto,
        PacketBudgetModeDto, PacketBudgetUsageDto, PacketEvidenceResolutionDto,
        PacketEvidenceTierDto, PacketProbeAmbiguityCandidateDto, PacketProbeRejectionDto,
        SearchHitOrigin,
    };

    const INDEXING_QUESTION: &str = "Explain the indexing runtime, persistence, and snapshot flow.";

    /// Obligation planning must key on prompt structure, not on the particular nouns used.
    /// A renamed prompt has to yield the renamed plan and nothing else.
    #[test]
    fn obligation_plan_is_invariant_under_mechanical_renaming() {
        const RENAMES: &[(&str, &str)] = &[
            ("Alpha", "Quernal"),
            ("Beta", "Tovsk"),
            ("Gamma", "Fenwick"),
            ("dispatch", "wrangle"),
            ("receive", "gather"),
            ("load", "hoist"),
        ];

        fn rename(text: &str) -> String {
            let mut ordered = RENAMES.to_vec();
            ordered.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
            let mut renamed = text.to_string();
            for (from, to) in ordered {
                renamed = renamed.replace(from, to);
            }
            renamed
        }

        fn plan_queries(question: &str) -> Vec<String> {
            let plan = build_packet_obligation_plan(
                question,
                PacketTaskClassDto::DataFlow,
                &[PacketPlanQueryDto {
                    query: question.to_string(),
                    purpose: "original task phrasing".to_string(),
                }],
            );
            let mut queries = plan
                .claim_obligations
                .iter()
                .flat_map(|obligation| obligation.binding_terms.iter().cloned())
                .chain(
                    plan.query_obligations
                        .iter()
                        .map(|obligation| obligation.query.clone()),
                )
                .collect::<Vec<_>>();
            queries.sort();
            queries
        }

        let question = "Explain how Alpha.dispatch reaches Beta.receive through Gamma.load.";
        let original = plan_queries(question);
        assert!(
            !original.is_empty(),
            "fixture prompt must plan some obligations for the comparison to mean anything"
        );

        let mut expected = original
            .iter()
            .map(|query| rename(query))
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(
            plan_queries(&rename(question)),
            expected,
            "renaming every identifier must rename the plan and change nothing else"
        );
    }

    fn fresh_index_observation() -> IndexFreshnessDto {
        IndexFreshnessDto {
            status: IndexFreshnessStatusDto::Fresh,
            changed_file_count: 0,
            new_file_count: 0,
            removed_file_count: 0,
            checked_file_count: 8,
            indexed_file_count: 8,
            duration_ms: 1,
            reason: None,
            not_checked_cause: None,
            samples: Vec::new(),
        }
    }

    fn citation(name: &str, path: &str, kind: NodeKind) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(name.to_string()),
            display_name: name.to_string(),
            kind,
            file_path: Some(path.to_string()),
            line: Some(1),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: Some(PacketEvidenceTierDto::ResolvedGraph),
            evidence_producer: Some("test".to_string()),
            resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
            source_excerpt: None,
        }
    }

    fn answer(citations: Vec<AgentCitationDto>) -> AgentAnswerDto {
        AgentAnswerDto {
            source_coverage: Vec::new(),
            answer_id: "obligation-test".to_string(),
            prompt: INDEXING_QUESTION.to_string(),
            summary: "test".to_string(),
            freshness: Some(fresh_index_observation()),
            sections: Vec::new(),
            citations,
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "obligation-test".to_string(),
                retrieval_publication: None,
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
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

    fn budget() -> PacketBudgetDto {
        PacketBudgetDto {
            requested: PacketBudgetModeDto::Standard,
            limits: PacketBudgetLimitsDto {
                max_anchors: 16,
                max_files: 16,
                max_snippets: 16,
                max_trail_edges: 16,
                max_output_bytes: 64_000,
            },
            used: PacketBudgetUsageDto {
                anchors: 1,
                files: 1,
                snippets: 1,
                trail_edges: 0,
                output_bytes: 1_000,
            },
            truncated: false,
            omitted_sections: Vec::new(),
            next_deeper_command: None,
        }
    }

    #[test]
    fn obligation_carriers_preserve_ranked_input_order_instead_of_node_id_order() {
        let mut best = citation("best", "src/best.rs", NodeKind::FUNCTION);
        best.node_id = NodeId("z-ranked-first".to_string());
        let mut weaker = citation("weaker", "src/weaker.rs", NodeKind::FUNCTION);
        weaker.node_id = NodeId("a-ranked-second".to_string());
        let mut plan = build_packet_obligation_plan(
            "Find Widget::run.",
            PacketTaskClassDto::RouteTracing,
            &[],
        );
        let obligation = plan
            .claim_obligations
            .first_mut()
            .expect("an exact identity must create an obligation");

        record_obligation_carriers(obligation, [&best, &weaker], 2);

        assert_eq!(
            obligation.carrier_node_ids,
            [
                NodeId("z-ranked-first".to_string()),
                NodeId("a-ranked-second".to_string())
            ]
        );
        assert_eq!(
            obligation.carrier_paths,
            ["src/best.rs".to_string(), "src/weaker.rs".to_string()]
        );
    }

    fn answer_with_call_edge(
        question: &str,
        carrier_name: &str,
        carrier_path: &str,
    ) -> AgentAnswerDto {
        let mut carrier = citation(carrier_name, carrier_path, NodeKind::METHOD);
        carrier.evidence_edge_ids = vec![EdgeId("requested-call".to_string())];
        let target = citation("Worker::run", "src/worker.rs", NodeKind::METHOD);
        let mut answer = answer(vec![carrier.clone(), target.clone()]);
        answer.prompt = question.to_string();
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "requested-flow".to_string(),
            title: "Requested flow".to_string(),
            graph: GraphResponse {
                center_id: carrier.node_id.clone(),
                nodes: vec![GraphNodeDto {
                    id: target.node_id.clone(),
                    label: target.display_name.clone(),
                    kind: target.kind,
                    depth: 1,
                    label_policy: None,
                    badge_visible_members: None,
                    badge_total_members: None,
                    merged_symbol_examples: Vec::new(),
                    file_path: target.file_path.clone(),
                    qualified_name: None,
                    member_access: None,
                }],
                edges: vec![GraphEdgeDto {
                    id: EdgeId("requested-call".to_string()),
                    source: carrier.node_id,
                    target: target.node_id,
                    kind: EdgeKind::CALL,
                    confidence: Some(1.0),
                    certainty: Some("certain".to_string()),
                    callsite_identity: Some("test:1".to_string()),
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        });
        answer
    }

    #[test]
    fn explicitly_named_source_files_are_material_query_obligations() {
        let plan = build_packet_obligation_plan(
            "Explain how `source/animate.css` imports _vars.css and connects classes to keyframes.",
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );

        for expected in ["source/animate.css", "_vars.css"] {
            assert!(
                plan.query_obligations.iter().any(|obligation| {
                    obligation.query == expected
                        && obligation.kind == PacketQueryObligationKindDto::RequiredPath
                        && obligation.material
                }),
                "explicit file was not retained as material: {expected} {plan:#?}"
            );
        }
    }

    #[test]
    fn leading_task_verbs_do_not_become_material_requested_claims() {
        let identity = "RuntimeReferenceService::resolve";
        for (verb, task_class) in [
            ("Assess", PacketTaskClassDto::ChangeImpact),
            ("Locate", PacketTaskClassDto::SymbolOwnership),
            ("Plan", PacketTaskClassDto::EditPlanning),
            ("Trace", PacketTaskClassDto::BugLocalization),
            ("Explain", PacketTaskClassDto::ArchitectureExplanation),
            ("Find", PacketTaskClassDto::SymbolOwnership),
            ("Show", PacketTaskClassDto::SymbolOwnership),
        ] {
            let question = format!("{verb} {identity}.");
            let plan = build_packet_obligation_plan(&question, task_class, &[]);
            let material_binding_terms = plan
                .claim_obligations
                .iter()
                .filter(|obligation| obligation.material)
                .flat_map(|obligation| obligation.binding_terms.iter().map(String::as_str))
                .collect::<Vec<_>>();

            assert_eq!(plan.binding_terms, [identity], "{verb}: {plan:#?}");
            assert_eq!(material_binding_terms, [identity], "{verb}: {plan:#?}");
        }
    }

    #[test]
    fn filtered_generic_request_does_not_mint_a_behavioral_fallback() {
        let plan = build_packet_obligation_plan(
            "Explain architecture and behavior.",
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        let material = plan
            .claim_obligations
            .iter()
            .filter(|obligation| obligation.material)
            .collect::<Vec<_>>();

        assert!(plan.binding_terms.is_empty());
        assert!(material.is_empty(), "{material:?}");
    }

    #[test]
    fn exact_identity_does_not_hide_a_behavioral_fallback_or_pollute_pure_ownership() {
        let architecture = build_packet_obligation_plan(
            "Explain how RuntimeService::run participates in the architecture.",
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        assert!(
            architecture
                .claim_obligations
                .iter()
                .all(|obligation| { obligation.id != "profile_generic_behavior" })
        );
        assert!(architecture.claim_obligations.iter().any(|obligation| {
            obligation.kind == PacketClaimObligationKindDto::ExactProbe && obligation.material
        }));

        let ownership = build_packet_obligation_plan(
            "RuntimeService::run",
            PacketTaskClassDto::SymbolOwnership,
            &[],
        );
        assert!(ownership.claim_obligations.iter().any(|obligation| {
            obligation.kind == PacketClaimObligationKindDto::ExactProbe && obligation.material
        }));
        assert!(ownership.claim_obligations.iter().all(|obligation| {
            obligation.id != "profile_symbol_ownership_behavior" || !obligation.material
        }));
    }

    #[test]
    fn requested_claim_cap_records_the_ninth_symbol_as_a_material_overflow() {
        let eight = "Find SymbolOne SymbolTwo SymbolThree SymbolFour SymbolFive SymbolSix SymbolSeven SymbolEight.";
        let nine = "Find SymbolOne SymbolTwo SymbolThree SymbolFour SymbolFive SymbolSix SymbolSeven SymbolEight SymbolNine.";
        let eight_plan =
            build_packet_obligation_plan(eight, PacketTaskClassDto::SymbolOwnership, &[]);
        let mut nine_plan =
            build_packet_obligation_plan(nine, PacketTaskClassDto::SymbolOwnership, &[]);

        assert_eq!(eight_plan.binding_terms.len(), 8);
        assert!(
            !eight_plan
                .claim_obligations
                .iter()
                .any(|obligation| obligation.id == REQUESTED_CLAIM_OVERFLOW_ID)
        );
        assert_eq!(nine_plan.binding_terms.len(), 8);
        let overflow = nine_plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == REQUESTED_CLAIM_OVERFLOW_ID)
            .expect("ninth symbol must produce an explicit overflow receipt");
        assert!(overflow.material);
        assert_eq!(
            overflow.reason.as_deref(),
            Some("requested_claim_binding_limit_exceeded:1")
        );

        let mut answer = answer(Vec::new());
        answer.prompt = nine.to_string();
        finalize_packet_obligation_plan(
            nine,
            PacketTaskClassDto::SymbolOwnership,
            &mut nine_plan,
            &answer,
            &budget(),
        );
        let overflow = nine_plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == REQUESTED_CLAIM_OVERFLOW_ID)
            .unwrap();
        assert_eq!(
            overflow.proof_status,
            PacketObligationProofStatusDto::Unsupported
        );
        assert!(!material_packet_obligations_are_proven(&nine_plan));
    }

    #[test]
    fn exact_typed_probe_ledger_preserves_each_input_and_typed_failure() {
        let resolutions = vec![
            PacketProbeResolutionDto {
                input_index: 0,
                probe: PacketProbeDto::ExactPath {
                    path: "src/lib.rs".to_string(),
                },
                status: PacketProbeResolutionStatusDto::ExactPath,
                normalized_query: Some("src/lib.rs".to_string()),
                path: Some("src/lib.rs".to_string()),
                symbol_id: None,
                candidates: Vec::new(),
                rejection: None,
            },
            PacketProbeResolutionDto {
                input_index: 1,
                probe: PacketProbeDto::SymbolId {
                    id: "stale-node".to_string(),
                },
                status: PacketProbeResolutionStatusDto::Rejected,
                normalized_query: None,
                path: None,
                symbol_id: None,
                candidates: Vec::new(),
                rejection: Some(PacketProbeRejectionDto {
                    code: PacketProbeRejectionCodeDto::StaleSymbolId,
                    message: "stale symbol".to_string(),
                }),
            },
            PacketProbeResolutionDto {
                input_index: 2,
                probe: PacketProbeDto::FileSymbol {
                    path: "src/lib.rs".to_string(),
                    symbol: "App.run".to_string(),
                },
                status: PacketProbeResolutionStatusDto::Ambiguous,
                normalized_query: Some("src/lib.rs::App.run".to_string()),
                path: None,
                symbol_id: None,
                candidates: vec![PacketProbeAmbiguityCandidateDto {
                    symbol_id: "node-a".to_string(),
                    display_name: "App.run".to_string(),
                    path: Some("src/lib.rs".to_string()),
                    kind: NodeKind::METHOD,
                }],
                rejection: None,
            },
            PacketProbeResolutionDto {
                input_index: 3,
                probe: PacketProbeDto::Continuation {
                    contract_version: PACKET_PROBE_CONTRACT_VERSION,
                    project_id: "project".to_string(),
                    core_generation_id: "generation".to_string(),
                    retrieval_generation: None,
                    symbol_id: Some("node-c".to_string()),
                    query: "App.run".to_string(),
                },
                status: PacketProbeResolutionStatusDto::Continuation,
                normalized_query: Some("App.run".to_string()),
                path: Some("src/lib.rs".to_string()),
                symbol_id: Some("node-c".to_string()),
                candidates: Vec::new(),
                rejection: None,
            },
            PacketProbeResolutionDto {
                input_index: 4,
                probe: PacketProbeDto::Continuation {
                    contract_version: PACKET_PROBE_CONTRACT_VERSION,
                    project_id: "project".to_string(),
                    core_generation_id: "generation".to_string(),
                    retrieval_generation: None,
                    symbol_id: None,
                    query: "diagnostic query".to_string(),
                },
                status: PacketProbeResolutionStatusDto::Continuation,
                normalized_query: Some("diagnostic query".to_string()),
                path: None,
                symbol_id: None,
                candidates: Vec::new(),
                rejection: None,
            },
            PacketProbeResolutionDto {
                input_index: 5,
                probe: PacketProbeDto::FreeQuery {
                    query: "diagnostic query".to_string(),
                },
                status: PacketProbeResolutionStatusDto::FreeQuery,
                normalized_query: Some("diagnostic query".to_string()),
                path: None,
                symbol_id: None,
                candidates: Vec::new(),
                rejection: None,
            },
        ];
        let mut plan = PacketObligationPlanDto {
            version: PACKET_OBLIGATION_PLAN_VERSION,
            ..Default::default()
        };

        append_packet_probe_obligations(
            &mut plan,
            &resolutions,
            "Find the exact probes.",
            PacketTaskClassDto::SymbolOwnership,
        );

        assert_eq!(
            plan.claim_obligations
                .iter()
                .map(|obligation| obligation.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "exact_probe:0",
                "exact_probe:1",
                "exact_probe:2",
                "exact_probe:3"
            ]
        );
        assert!(plan.claim_obligations.iter().all(|obligation| {
            obligation.probe_binding.as_ref().is_some_and(|binding| {
                obligation.id == format!("exact_probe:{}", binding.input_index)
            })
        }));
        assert!(plan.claim_obligations[0].material);
        assert!(!plan.claim_obligations[1].material);
        assert!(plan.claim_obligations[2].material);
        assert!(plan.claim_obligations[3].material);
        assert_eq!(
            plan.claim_obligations[1].reason.as_deref(),
            Some("exact_probe_rejected:stale_symbol_id")
        );
        assert_eq!(
            plan.claim_obligations[2].reason.as_deref(),
            Some("exact_probe_ambiguous:candidates=1")
        );
        assert!(!material_packet_obligations_are_proven(&plan));
    }

    #[test]
    fn resolved_file_symbol_cannot_borrow_another_probe_carrier() {
        let resolutions = [
            PacketProbeResolutionDto {
                input_index: 7,
                probe: PacketProbeDto::FileSymbol {
                    path: "src/foo.rs".to_string(),
                    symbol: "Foo/run".to_string(),
                },
                status: PacketProbeResolutionStatusDto::FileScopedSymbol,
                normalized_query: Some("src/foo.rs::Foo/run".to_string()),
                path: Some("src/foo.rs".to_string()),
                symbol_id: Some("node-foo".to_string()),
                candidates: Vec::new(),
                rejection: None,
            },
            PacketProbeResolutionDto {
                input_index: 8,
                probe: PacketProbeDto::FileSymbol {
                    path: "src/bar.rs".to_string(),
                    symbol: "Foo/run".to_string(),
                },
                status: PacketProbeResolutionStatusDto::FileScopedSymbol,
                normalized_query: Some("src/bar.rs::Foo/run".to_string()),
                path: Some("src/bar.rs".to_string()),
                symbol_id: Some("node-bar".to_string()),
                candidates: Vec::new(),
                rejection: None,
            },
        ];
        let mut plan = PacketObligationPlanDto {
            version: PACKET_OBLIGATION_PLAN_VERSION,
            ..Default::default()
        };
        append_packet_probe_obligations(
            &mut plan,
            &resolutions,
            "Find the exact symbols.",
            PacketTaskClassDto::SymbolOwnership,
        );
        let mut carrier = citation("Foo/run", "src/bar.rs", NodeKind::METHOD);
        carrier.node_id = NodeId("node-bar".to_string());
        carrier.evidence_producer = Some("packet_exact_symbol_probe".to_string());

        finalize_packet_obligation_plan(
            "Find the exact symbols.",
            PacketTaskClassDto::SymbolOwnership,
            &mut plan,
            &answer(vec![carrier]),
            &budget(),
        );

        let foo = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == "exact_probe:7")
            .unwrap();
        let bar = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == "exact_probe:8")
            .unwrap();
        assert_eq!(
            foo.proof_status,
            PacketObligationProofStatusDto::Unsupported
        );
        assert_eq!(foo.reason.as_deref(), Some("exact_probe_carrier_missing"));
        assert!(foo.carrier_node_ids.is_empty());
        assert_eq!(bar.proof_status, PacketObligationProofStatusDto::Proven);
        assert_eq!(bar.carrier_node_ids, vec![NodeId("node-bar".to_string())]);
    }

    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn exact_path_obligation_requires_same_path_eligible_carrier() {
        let resolution = PacketProbeResolutionDto {
            input_index: 0,
            probe: PacketProbeDto::ExactPath {
                path: "src/lib.rs".to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some("src/lib.rs".to_string()),
            path: Some("src/lib.rs".to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };
        let mut plan = PacketObligationPlanDto {
            version: PACKET_OBLIGATION_PLAN_VERSION,
            ..Default::default()
        };
        append_packet_probe_obligations(
            &mut plan,
            std::slice::from_ref(&resolution),
            "Explain this exact path.",
            PacketTaskClassDto::ArchitectureExplanation,
        );
        let mut diagnostic = citation("src/lib.rs", "src/lib.rs", NodeKind::FILE);
        diagnostic.evidence_producer = Some("packet_exact_path_probe".to_string());
        diagnostic.eligible_for_sufficiency = Some(false);

        finalize_packet_obligation_plan(
            "Explain this exact path.",
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer(vec![diagnostic.clone()]),
            &budget(),
        );
        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Unsupported
        );
        assert_eq!(
            plan.claim_obligations[0].reason.as_deref(),
            Some("exact_probe_carrier_missing")
        );

        let mut synthetic_carrier = citation("indexed_target", "src/lib.rs", NodeKind::FUNCTION);
        synthetic_carrier.evidence_producer = Some("packet_exact_symbol_probe".to_string());
        synthetic_carrier.eligible_for_sufficiency = Some(true);
        finalize_packet_obligation_plan(
            "Explain this exact path.",
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer(vec![diagnostic.clone(), synthetic_carrier]),
            &budget(),
        );
        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Unsupported,
            "a synthetic exact-probe citation must not prove semantic coverage"
        );

        let mut non_behavioral = citation("HttpTransportAdapter", "src/lib.rs", NodeKind::CLASS);
        non_behavioral.evidence_producer = Some("symbol_doc".to_string());
        non_behavioral.coverage_role = Some("transport adapter".to_string());
        non_behavioral.eligible_for_sufficiency = Some(true);
        finalize_packet_obligation_plan(
            "Explain this exact path.",
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer(vec![diagnostic.clone(), non_behavioral]),
            &budget(),
        );
        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Unsupported,
            "a named type must not stand in for behavioral path evidence"
        );

        let mut carrier = citation("run_stdio_server", "src/lib.rs", NodeKind::FUNCTION);
        carrier.evidence_producer = Some("symbol_doc".to_string());
        carrier.coverage_role = Some("command entrypoint".to_string());
        carrier.eligible_for_sufficiency = Some(true);
        let carried_answer = answer(vec![diagnostic, carrier.clone()]);
        finalize_packet_obligation_plan(
            "Explain this exact path.",
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &carried_answer,
            &budget(),
        );
        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Proven
        );
        assert_eq!(
            plan.claim_obligations[0].carrier_node_ids,
            vec![carrier.node_id.clone()]
        );
        let role_claim = PacketClaimDto {
            claim: "The command entrypoint starts the stdio server.".to_string(),
            required_obligation_ids: Vec::new(),
            required_obligation_kinds: Vec::new(),
            proof_status: Some(PacketProofStatusDto::Likely),
            required_evidence_role: Some(PacketEvidenceTierDto::ResolvedGraph),
            citations: vec![carrier],
            coverage_role: Some("command entrypoint".to_string()),
            eligible_for_sufficiency: Some(false),
        };
        let mut claims = packet_claims_with_obligation_receipts(
            &carried_answer,
            PacketTaskClassDto::ArchitectureExplanation,
            &plan,
            (vec![role_claim], ()),
        );
        bind_claims_to_packet_obligations(&plan, &mut claims);
        assert_eq!(claims.len(), 2);
        assert_eq!(
            claims[0].required_obligation_ids,
            ["exact_probe:0".to_string()]
        );
        assert_eq!(claims[0].proof_status, Some(PacketProofStatusDto::Proven));
        assert_eq!(claims[0].eligible_for_sufficiency, Some(true));
        assert_eq!(
            claims[1].coverage_role.as_deref(),
            Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE)
        );
        assert_eq!(claims[1].proof_status, Some(PacketProofStatusDto::Proven));
        assert_eq!(claims[1].eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn resolved_exact_paths_scope_only_generic_fallback_claim_rows() {
        let question = "Explain alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo ownership.";
        let mut plan = build_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        assert!(
            plan.claim_obligations
                .iter()
                .all(|obligation| { !obligation.id.starts_with("requested_claim:") })
        );
        assert!(
            plan.claim_obligations
                .iter()
                .all(|obligation| { obligation.id != REQUESTED_CLAIM_OVERFLOW_ID })
        );
        assert!(
            plan.claim_obligations
                .iter()
                .all(|obligation| { obligation.id != "profile_generic_behavior" })
        );
        let resolution = PacketProbeResolutionDto {
            input_index: 0,
            probe: PacketProbeDto::ExactPath {
                path: "src/lib.rs".to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some("src/lib.rs".to_string()),
            path: Some("src/lib.rs".to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };

        append_packet_probe_obligations(
            &mut plan,
            &[resolution],
            question,
            PacketTaskClassDto::ArchitectureExplanation,
        );

        assert!(plan.claim_obligations.iter().all(|obligation| {
            !obligation.id.starts_with("requested_claim:") || !obligation.material
        }));
        assert!(
            plan.claim_obligations
                .iter()
                .all(|obligation| { obligation.id != REQUESTED_CLAIM_OVERFLOW_ID })
        );
        assert!(
            plan.claim_obligations
                .iter()
                .any(|obligation| { obligation.id == "exact_probe:0" && obligation.material })
        );

        let product_question = "Explain the ownership boundary from the packaged CodeStory plugin request through stdio transport, runtime grounding orchestration, retrieval, and evidence publication. Identify uncertainty or gaps.";
        let mut product_plan = build_packet_obligation_plan(
            product_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        assert!(product_plan.claim_obligations.iter().any(|obligation| {
            obligation.id == "requested_claim:0:CodeStory" && obligation.material
        }));
        assert!(
            product_plan
                .query_obligations
                .iter()
                .any(|obligation| { obligation.query == "CodeStory" && obligation.material })
        );
        let product_resolution = PacketProbeResolutionDto {
            input_index: 2,
            probe: PacketProbeDto::ExactPath {
                path: "src/transport.rs".to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some("src/transport.rs".to_string()),
            path: Some("src/transport.rs".to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };
        append_packet_probe_obligations(
            &mut product_plan,
            &[product_resolution],
            product_question,
            PacketTaskClassDto::ArchitectureExplanation,
        );
        assert!(product_plan.claim_obligations.iter().any(|obligation| {
            obligation.id == "requested_claim:0:CodeStory" && !obligation.material
        }));
        assert!(
            product_plan
                .query_obligations
                .iter()
                .any(|obligation| { obligation.query == "CodeStory" && !obligation.material })
        );

        let absence_question = "Is this runtime implementation unused?";
        let absence_plan = build_packet_obligation_plan(
            absence_question,
            PacketTaskClassDto::SymbolOwnership,
            &[],
        );
        assert!(
            absence_plan
                .claim_obligations
                .iter()
                .all(|obligation| !obligation.requires_complete_discovery),
            "prompt wording must not force complete-discovery: {absence_plan:#?}"
        );

        let fallback_question = "?";
        let mut fallback_plan = build_packet_obligation_plan(
            fallback_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        assert!(
            fallback_plan.claim_obligations.iter().all(|obligation| {
                obligation.id
                    != default_profile_obligation_id(PacketTaskClassDto::ArchitectureExplanation)
            }),
            "ordinary wording must not mint a task-class behavioral fallback: {fallback_plan:#?}"
        );
        let fallback_resolution = PacketProbeResolutionDto {
            input_index: 1,
            probe: PacketProbeDto::ExactPath {
                path: "src/entry.rs".to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some("src/entry.rs".to_string()),
            path: Some("src/entry.rs".to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };
        append_packet_probe_obligations(
            &mut fallback_plan,
            &[fallback_resolution],
            fallback_question,
            PacketTaskClassDto::ArchitectureExplanation,
        );
        assert!(fallback_plan.claim_obligations.iter().all(|obligation| {
            obligation.id
                != default_profile_obligation_id(PacketTaskClassDto::ArchitectureExplanation)
        }));
        assert!(
            fallback_plan
                .claim_obligations
                .iter()
                .any(|obligation| obligation.id == "exact_probe:1" && obligation.material)
        );
    }

    #[test]
    fn exact_symbol_syntax_precedes_ambiguous_names_at_the_obligation_cap() {
        let question = "Explain AlphaName BravoName CharlieName DeltaName EchoName FoxtrotName GolfName HotelName RuntimeService::run.";
        let task_class = PacketTaskClassDto::ArchitectureExplanation;
        let mut plan = build_packet_obligation_plan(question, task_class, &[]);

        let explicit_claim = plan
            .claim_obligations
            .iter()
            .find(|obligation| {
                obligation.id.starts_with("requested_claim:")
                    && obligation.id.contains("RuntimeService::run")
            })
            .expect("qualified symbol must survive the bounded claim ledger");
        assert!(explicit_claim.material, "{plan:#?}");
        assert!(
            plan.query_obligations.iter().any(|obligation| {
                obligation.query == "RuntimeService::run" && obligation.material
            }),
            "{plan:#?}"
        );

        let resolution = PacketProbeResolutionDto {
            input_index: 7,
            probe: PacketProbeDto::ExactPath {
                path: "src/runtime.rs".to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some("src/runtime.rs".to_string()),
            path: Some("src/runtime.rs".to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };
        append_packet_probe_obligations(&mut plan, &[resolution], question, task_class);

        assert!(plan.claim_obligations.iter().any(|obligation| {
            obligation.id.starts_with("requested_claim:")
                && obligation.id.contains("RuntimeService::run")
                && obligation.material
        }));
        for ambiguous in [
            "AlphaName",
            "BravoName",
            "CharlieName",
            "DeltaName",
            "EchoName",
            "FoxtrotName",
            "GolfName",
            "HotelName",
        ] {
            assert!(
                plan.claim_obligations.iter().all(|obligation| {
                    !obligation.id.starts_with("requested_claim:")
                        || !obligation.id.contains(ambiguous)
                        || !obligation.material
                }),
                "{ambiguous} remained material: {plan:#?}"
            );
        }
        assert!(plan.claim_obligations.iter().any(|obligation| {
            obligation.id == REQUESTED_CLAIM_OVERFLOW_ID && !obligation.material
        }));
        assert!(plan.query_obligations.iter().any(|obligation| {
            obligation.query == "RuntimeService::run" && obligation.material
        }));
    }

    #[test]
    fn bounded_exact_symbol_collisions_keep_an_overflow_guard() {
        let shared_prefix = "A".repeat(PACKET_OBLIGATION_BINDING_TERM_CHAR_LIMIT);
        let symbols = (0..9)
            .map(|index| format!("{shared_prefix}::run{index}"))
            .collect::<Vec<_>>();
        let question = format!("Explain {}.", symbols.join(" "));
        let task_class = PacketTaskClassDto::ArchitectureExplanation;
        let mut plan = build_packet_obligation_plan(&question, task_class, &[]);

        assert_eq!(
            plan.query_obligations
                .iter()
                .filter(|obligation| {
                    obligation.kind == PacketQueryObligationKindDto::RequiredProbe
                        && symbols.contains(&obligation.query)
                })
                .count(),
            PACKET_OBLIGATION_BINDING_TERM_LIMIT
        );
        assert!(plan.claim_obligations.iter().any(|obligation| {
            obligation.id == REQUESTED_CLAIM_OVERFLOW_ID && obligation.material
        }));

        let resolution = PacketProbeResolutionDto {
            input_index: 8,
            probe: PacketProbeDto::ExactPath {
                path: "src/runtime.rs".to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some("src/runtime.rs".to_string()),
            path: Some("src/runtime.rs".to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };
        append_packet_probe_obligations(&mut plan, &[resolution], &question, task_class);

        assert!(plan.claim_obligations.iter().any(|obligation| {
            obligation.id == REQUESTED_CLAIM_OVERFLOW_ID && obligation.material
        }));

        let pair_question = format!("Explain {} {}.", symbols[0], symbols[1]);
        let mut pair_plan = build_packet_obligation_plan(&pair_question, task_class, &[]);
        assert!(pair_plan.claim_obligations.iter().any(|obligation| {
            obligation.id == REQUESTED_CLAIM_OVERFLOW_ID && obligation.material
        }));
        let pair_resolution = PacketProbeResolutionDto {
            input_index: 9,
            probe: PacketProbeDto::ExactPath {
                path: "src/pair.rs".to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some("src/pair.rs".to_string()),
            path: Some("src/pair.rs".to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };
        append_packet_probe_obligations(
            &mut pair_plan,
            &[pair_resolution],
            &pair_question,
            task_class,
        );
        assert!(pair_plan.claim_obligations.iter().any(|obligation| {
            obligation.id == REQUESTED_CLAIM_OVERFLOW_ID && obligation.material
        }));
    }

    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn exact_path_scoped_packet_binds_three_distinct_semantic_claims() {
        let question = "Explain the ownership boundary from the packaged GraphForge plugin request through stdio transport, runtime orchestration, retrieval, and evidence publication.";
        let task_class = PacketTaskClassDto::ArchitectureExplanation;
        let paths = ["src/launch.rs", "src/stdio.rs", "src/runtime.rs"];
        let mut citations = [
            citation("spawn_runtime", paths[0], NodeKind::FUNCTION),
            citation("run_stdio_server", paths[1], NodeKind::FUNCTION),
            citation("coordinate_packet", paths[2], NodeKind::METHOD),
        ];
        for (citation, role) in citations.iter_mut().zip([
            "request dispatch",
            "command entrypoint",
            "runtime orchestration",
        ]) {
            citation.coverage_role = Some(role.to_string());
            citation.evidence_producer = Some("symbol_doc".to_string());
        }
        let resolutions = paths
            .iter()
            .enumerate()
            .map(|(input_index, path)| PacketProbeResolutionDto {
                input_index: input_index as u32,
                probe: PacketProbeDto::ExactPath {
                    path: (*path).to_string(),
                },
                status: PacketProbeResolutionStatusDto::ExactPath,
                normalized_query: Some((*path).to_string()),
                path: Some((*path).to_string()),
                symbol_id: None,
                candidates: Vec::new(),
                rejection: None,
            })
            .collect::<Vec<_>>();
        let mut plan = build_packet_obligation_plan(question, task_class, &[]);
        append_packet_probe_obligations(&mut plan, &resolutions, question, task_class);
        let mut carried_answer = answer(citations.to_vec());
        carried_answer.prompt = question.to_string();
        finalize_packet_obligation_plan(
            question,
            task_class,
            &mut plan,
            &carried_answer,
            &budget(),
        );
        let supported =
            crate::packet_claims::packet_supported_claims_with_telemetry(&carried_answer);
        let mut claims =
            packet_claims_with_obligation_receipts(&carried_answer, task_class, &plan, supported);
        bind_claims_to_packet_obligations(&plan, &mut claims);

        let bound = claims
            .iter()
            .filter(|claim| {
                claim.eligible_for_sufficiency == Some(true)
                    && claim.proof_status == Some(PacketProofStatusDto::Proven)
                    && claim.required_obligation_kinds == [PacketClaimObligationKindDto::ExactProbe]
            })
            .collect::<Vec<_>>();
        assert_eq!(bound.len(), 3, "{claims:#?}");
        assert_eq!(
            bound
                .iter()
                .flat_map(|claim| claim.required_obligation_ids.iter())
                .collect::<HashSet<_>>()
                .len(),
            3
        );
        assert!(
            claims
                .iter()
                .filter(|claim| {
                    claim.coverage_role.as_deref() == Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE)
                })
                .all(|claim| claim.eligible_for_sufficiency == Some(false))
        );
        assert!(material_packet_obligations_are_proven(&plan));
    }

    #[test]
    fn eligible_claim_without_an_exact_row_id_is_demoted_even_for_an_empty_plan() {
        let mut claims = vec![PacketClaimDto {
            claim: "A generic role claim.".to_string(),
            required_obligation_ids: Vec::new(),
            required_obligation_kinds: vec![PacketClaimObligationKindDto::Entrypoint],
            proof_status: Some(PacketProofStatusDto::Proven),
            required_evidence_role: None,
            citations: vec![citation("Cli::run", "src/cli.rs", NodeKind::METHOD)],
            coverage_role: Some("fixture".to_string()),
            eligible_for_sufficiency: Some(true),
        }];

        bind_claims_to_packet_obligations(&PacketObligationPlanDto::default(), &mut claims);

        assert_eq!(claims[0].proof_status, Some(PacketProofStatusDto::Reported));
        assert_eq!(claims[0].eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn every_task_profile_has_pre_retrieval_behavioral_guards() {
        for task_class in [
            PacketTaskClassDto::ArchitectureExplanation,
            PacketTaskClassDto::BugLocalization,
            PacketTaskClassDto::ChangeImpact,
            PacketTaskClassDto::RouteTracing,
            PacketTaskClassDto::SymbolOwnership,
            PacketTaskClassDto::DataFlow,
            PacketTaskClassDto::EditPlanning,
        ] {
            let plan = build_packet_obligation_plan("Find Widget::run.", task_class, &[]);
            let requested = plan
                .claim_obligations
                .iter()
                .filter(|obligation| obligation.material)
                .collect::<Vec<_>>();
            assert_eq!(requested.len(), 1, "{task_class:?}");
            let exact = requested
                .iter()
                .find(|obligation| obligation.kind == PacketClaimObligationKindDto::ExactProbe)
                .unwrap_or_else(|| panic!("missing exact identity for {task_class:?}"));
            assert_eq!(exact.binding_terms, vec!["Widget::run"], "{task_class:?}");
            assert_eq!(
                requested
                    .iter()
                    .filter(|obligation| obligation.binding_terms.is_empty())
                    .count(),
                0,
                "task class must not mint a behavioral fallback: {task_class:?}"
            );
            assert!(
                plan.claim_obligations.iter().all(|obligation| {
                    obligation.proof_status == PacketObligationProofStatusDto::Planned
                }),
                "ordinary packets begin with planned obligations: {task_class:?}"
            );
            assert_eq!(
                plan.claim_obligations
                    .iter()
                    .filter(|obligation| obligation.binding_terms.is_empty())
                    .count(),
                0,
                "ordinary packets must not retain category guards: {task_class:?}"
            );
            assert!(
                plan.claim_obligations.iter().all(|obligation| {
                    obligation.material
                        || (obligation.binding_terms.is_empty()
                            && obligation.proof_status == PacketObligationProofStatusDto::Planned)
                }),
                "only concrete requested claims are material: {task_class:?}"
            );
            assert_eq!(exact.kind, PacketClaimObligationKindDto::ExactProbe);
            assert_eq!(exact.required_edge_kind, None);
            assert_eq!(
                plan.claim_obligations
                    .iter()
                    .map(|obligation| obligation.kind)
                    .collect::<HashSet<_>>()
                    .len(),
                1,
                "{task_class:?}"
            );
        }
    }

    #[test]
    fn absence_profile_keeps_the_selected_guard_material() {
        let plan = build_packet_obligation_plan(
            "Find unused callers for Widget::run.",
            PacketTaskClassDto::SymbolOwnership,
            &[],
        );

        assert!(plan.claim_obligations.iter().any(|obligation| {
            obligation.material && obligation.binding_terms == ["Widget::run"]
        }));
        assert!(
            plan.claim_obligations
                .iter()
                .all(|obligation| !obligation.requires_complete_discovery)
        );
    }

    #[test]
    fn only_punctuated_or_camel_case_terms_earn_case_sensitive_binding() {
        // Punctuation is not something English puts inside a word, so these are certainly
        // identifiers and the repository's exact spelling is what was asked for.
        for symbol in [
            "RuntimeService::run",
            "pkg.Foo.run",
            "snake_case_name",
            "src/lib.rs",
            "$handler",
            "Widget#render",
            "urlSession",
        ] {
            assert!(
                term_is_unambiguously_a_symbol(symbol),
                "{symbol:?} is an identifier"
            );
        }
        // A capitalised bare word is a product, language, or type name that the retrieval
        // classifier cannot tell from an identifier, so it must not force exact casing.
        for prose in ["JavaScript", "APIs", "AutoMapper", "URLSession", "HTTP"] {
            assert!(
                !term_is_unambiguously_a_symbol(prose),
                "{prose:?} is prose-ambiguous and must fall back to case-insensitive binding"
            );
        }
    }

    #[test]
    fn qualified_binding_requires_the_requested_owner_and_member() {
        assert!(citation_display_matches_requested_identity(
            "RuntimeService::run",
            "RuntimeService::run"
        ));
        assert!(citation_display_matches_requested_identity(
            "crate::runtime::RuntimeService::run()",
            "RuntimeService::run"
        ));
        assert!(!citation_display_matches_requested_identity(
            "RuntimeService::stop",
            "RuntimeService::run"
        ));
        assert!(!citation_display_matches_requested_identity(
            "OtherRuntimeService::run",
            "RuntimeService::run"
        ));
        assert!(!citation_display_matches_requested_identity(
            "RuntimeService::run_extra",
            "RuntimeService::run"
        ));
        assert!(citation_display_matches_requested_identity(
            "pkg.Foo.run",
            "Foo.run"
        ));
        assert!(citation_display_matches_requested_identity(
            "TransportClient.send",
            "TransportClient"
        ));
        assert!(citation_display_matches_requested_identity(
            "RuntimeService::run",
            "RuntimeService"
        ));
        assert!(!citation_display_matches_requested_identity(
            "OtherRuntimeService::run",
            "RuntimeService"
        ));
        assert!(!citation_display_matches_requested_identity(
            "pkg.foo.run",
            "Foo.run"
        ));
        assert!(citation_display_matches_exact_requested_identity(
            "pkg/Foo/run",
            "Foo/run"
        ));
        assert!(!citation_display_matches_exact_requested_identity(
            "pkg/foo/run",
            "Foo/run"
        ));
        assert!(!citation_display_matches_exact_requested_identity(
            "Workflow::ready?",
            "Workflow::ready"
        ));
        assert!(!citation_display_matches_exact_requested_identity(
            "Workflow::save!",
            "Workflow::save"
        ));
        assert!(!citation_display_matches_exact_requested_identity(
            "Widget::~Widget",
            "Widget::Widget"
        ));
    }

    #[test]
    fn registered_source_paths_become_path_queries_but_not_symbol_obligations() {
        for extension in codestory_contracts::language_support::supported_extensions() {
            let path = format!("src/example.{extension}");
            let question = format!("Inspect {path}.");
            let plan =
                build_packet_obligation_plan(&question, PacketTaskClassDto::SymbolOwnership, &[]);

            assert!(
                plan.claim_obligations
                    .iter()
                    .all(|obligation| obligation.binding_terms != [path.as_str()]),
                "registered source path became a material symbol claim: {path}"
            );
            assert!(
                plan.query_obligations.iter().any(|obligation| {
                    obligation.material
                        && obligation.kind == PacketQueryObligationKindDto::RequiredPath
                        && obligation.query == path
                }),
                "registered source path was not retained as a path query: {path}"
            );
            assert!(
                plan.query_obligations.iter().all(|obligation| {
                    !(obligation.material
                        && obligation.kind == PacketQueryObligationKindDto::RequiredProbe
                        && obligation.query == path)
                }),
                "registered source path became a material symbol query: {path}"
            );
        }
    }

    #[test]
    fn wrong_member_cannot_prove_a_qualified_requested_claim() {
        let question = "Find RuntimeService::run.";
        let answer =
            answer_with_call_edge(question, "RuntimeService::stop", "src/runtime_service.rs");
        let mut plan =
            build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::SymbolOwnership,
            &mut plan,
            &answer,
            &budget(),
        );

        let requested = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.binding_terms == ["RuntimeService::run"])
            .unwrap();
        assert_eq!(
            requested.proof_status,
            PacketObligationProofStatusDto::Unsupported
        );
        assert!(requested.carrier_node_ids.is_empty());
    }

    #[test]
    fn a_requested_path_scopes_but_never_substitutes_for_symbol_identity() {
        let question = "Find RuntimeService::run and MissingWidget in src/runtime_service.rs.";
        let shared_path = "src/runtime_service.rs";
        let answer = answer_with_call_edge(question, "RuntimeService::run", shared_path);
        let planned_queries = vec![PacketPlanQueryDto {
            query: shared_path.to_string(),
            purpose: "path explicitly requested by the user".to_string(),
        }];
        let mut plan = build_packet_obligation_plan(
            question,
            PacketTaskClassDto::SymbolOwnership,
            &planned_queries,
        );
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::SymbolOwnership,
            &mut plan,
            &answer,
            &budget(),
        );

        let runtime = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.binding_terms == ["RuntimeService::run"])
            .unwrap();
        let missing = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.binding_terms == ["MissingWidget"])
            .unwrap();
        assert_eq!(runtime.proof_status, PacketObligationProofStatusDto::Proven);
        assert_eq!(
            missing.proof_status,
            PacketObligationProofStatusDto::Unsupported
        );
        assert!(missing.carrier_node_ids.is_empty());
    }

    #[test]
    fn exact_symbol_lookup_does_not_invent_a_behavioral_edge_requirement() {
        let question = "Find RuntimeService::run.";
        let mut answer = answer(vec![citation(
            "RuntimeService::run",
            "src/runtime_service.rs",
            NodeKind::METHOD,
        )]);
        answer.prompt = question.to_string();
        let mut plan =
            build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::SymbolOwnership,
            &mut plan,
            &answer,
            &budget(),
        );

        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Proven
        );
        assert_eq!(
            plan.claim_obligations[0].kind,
            PacketClaimObligationKindDto::ExactProbe
        );
        assert_eq!(plan.claim_obligations[0].required_edge_kind, None);
        assert_eq!(plan.claim_obligations[0].reason, None);
    }

    #[test]
    fn non_forcing_guards_demote_false_and_unmatched_claims_locally() {
        let question =
            "Explain CliErrorBody, runtime_path, CompilationDatabase, and Widget ownership.";
        let false_carriers = vec![
            citation("CliErrorBody", "src/cli/errors.rs", NodeKind::STRUCT),
            citation("runtime_path", "src/runtime/config.rs", NodeKind::VARIABLE),
            citation(
                "CompilationDatabase",
                "src/store/database.rs",
                NodeKind::CLASS,
            ),
        ];
        let structural = citation("Widget", "src/widget.rs", NodeKind::STRUCT);
        let mut answer = answer(
            false_carriers
                .iter()
                .cloned()
                .chain([structural.clone()])
                .collect(),
        );
        answer.prompt = question.to_string();
        let mut plan =
            build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::SymbolOwnership,
            &mut plan,
            &answer,
            &budget(),
        );
        let mut claims = false_carriers
            .iter()
            .cloned()
            .chain([structural.clone()])
            .map(|citation| PacketClaimDto {
                claim: format!("claim for {}", citation.display_name),
                required_obligation_ids: vec!["indexing_entrypoint".to_string()],
                required_obligation_kinds: Vec::new(),
                proof_status: Some(PacketProofStatusDto::Proven),
                required_evidence_role: None,
                citations: vec![citation],
                coverage_role: Some("fixture".to_string()),
                eligible_for_sufficiency: Some(true),
            })
            .collect::<Vec<_>>();
        bind_claims_to_packet_obligations(&plan, &mut claims);

        assert!(claims[..3].iter().all(|claim| {
            claim.proof_status == Some(PacketProofStatusDto::Reported)
                && claim.eligible_for_sufficiency == Some(false)
        }));
        assert_eq!(claims[3].proof_status, Some(PacketProofStatusDto::Reported));
        assert_eq!(claims[3].eligible_for_sufficiency, Some(false));
        assert!(!material_packet_obligations_are_proven(&plan));
        let open_next = packet_obligation_open_next_candidates(&plan);
        for carrier in false_carriers {
            assert!(
                open_next.contains(carrier.file_path.as_ref().expect("carrier path")),
                "{} must remain open-next",
                carrier.display_name
            );
        }
    }

    #[test]
    fn every_behavioral_obligation_rejects_type_field_and_variable_carriers() {
        for obligation_kind in [
            PacketClaimObligationKindDto::Entrypoint,
            PacketClaimObligationKindDto::Dispatch,
            PacketClaimObligationKindDto::Orchestration,
            PacketClaimObligationKindDto::StateWrite,
            PacketClaimObligationKindDto::ExternalIo,
        ] {
            let allowed = allowed_node_kinds_for_obligation(obligation_kind);
            for rejected in [
                NodeKind::STRUCT,
                NodeKind::CLASS,
                NodeKind::FIELD,
                NodeKind::VARIABLE,
            ] {
                assert!(
                    !allowed.contains(&rejected),
                    "{obligation_kind:?} {rejected:?}"
                );
            }
        }
    }

    #[test]
    fn tokenized_absence_profile_does_not_infer_complete_discovery() {
        let plan = build_packet_obligation_plan(
            "Find unused callers for Widget::run.",
            PacketTaskClassDto::SymbolOwnership,
            &[],
        );
        assert!(
            plan.claim_obligations
                .iter()
                .all(|obligation| !obligation.requires_complete_discovery)
        );
    }

    #[test]
    fn common_negative_paraphrases_do_not_force_complete_discovery() {
        for question in [
            "Find zero references to Widget::run.",
            "Show where Widget::run is not referenced.",
            "Find Widget::run with no usages.",
            "Show where Widget::run is not used.",
            "Confirm Widget::run isn't called.",
            "Confirm Widget::run does not exist.",
            "Find missing implementations of Widget::run.",
            "Confirm zero known direct callers of Widget::run.",
            "Is Widget::run unreachable?",
        ] {
            let plan =
                build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
            assert!(
                plan.claim_obligations
                    .iter()
                    .all(|obligation| !obligation.requires_complete_discovery),
                "{question}"
            );
        }
    }

    #[test]
    fn nearby_nonnegative_wording_does_not_request_complete_discovery() {
        for question in [
            "Show where Widget::run is referenced.",
            "Find Widget::run with usages.",
            "Explain how Widget::run is used.",
            "Trace Widget::run without changing callers.",
            "No need to change callers while editing Widget::run.",
            "Show Widget::run is not only called by tests.",
        ] {
            let plan =
                build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
            assert!(
                plan.claim_obligations
                    .iter()
                    .filter(|obligation| obligation.material)
                    .all(|obligation| !obligation.requires_complete_discovery),
                "{question}"
            );
        }
    }
}
