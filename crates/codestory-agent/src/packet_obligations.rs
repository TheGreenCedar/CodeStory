//! Versioned packet obligations planned before retrieval and finalized from carried evidence.

use super::packet_evidence::citation_sufficiency_eligible;
use super::packet_evidence_roles::{PacketEvidenceRole, packet_evidence_role};
use super::packet_flow_requirements::{
    CoverageMode, EvidencePredicate, FlowRequirement, FlowRole,
    flow_requirement_call_receipt_is_valid, ordinary_incident_call_receipt_is_valid,
    packet_flow_requirements_for_terms,
};
use super::packet_required_probes::{
    packet_prompt_exact_symbol_probe_queries, packet_sufficiency_required_probe_queries_from_terms,
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
    let requirements = packet_flow_requirements_for_terms(&terms, task_class);
    let requires_complete_discovery =
        packet_question_requires_complete_discovery(question, task_class);
    let exact_symbol_queries =
        packet_prioritized_exact_symbol_queries(question, &terms, task_class);
    let (binding_terms, omitted_binding_term_count) =
        requested_claim_binding_terms(&exact_symbol_queries);
    let mut claim_obligations = requirements
        .iter()
        .map(|requirement| claim_obligation(requirement, requires_complete_discovery))
        .collect::<Vec<_>>();
    claim_obligations.extend(default_profile_requested_claim_obligations(
        &binding_terms,
        task_class,
        requires_complete_discovery,
    ));
    if omitted_binding_term_count > 0 {
        claim_obligations.push(requested_claim_overflow_obligation(
            omitted_binding_term_count,
            task_class,
            requires_complete_discovery,
        ));
    }
    if requirements.is_empty() {
        let needs_material_fallback = !claim_obligations.iter().any(|obligation| {
            obligation.material && obligation.kind != PacketClaimObligationKindDto::ExactProbe
        }) && task_class != PacketTaskClassDto::SymbolOwnership;
        claim_obligations.extend(default_profile_guards(
            task_class,
            requires_complete_discovery,
            needs_material_fallback,
        ));
    }

    let mut query_obligations = Vec::new();
    let mut required_queries = HashSet::new();
    let diagnostic_queries = requirements
        .iter()
        .filter(|requirement| !flow_requirement_is_material(requirement))
        .flat_map(|requirement| requirement.query_seeds.iter().copied())
        .collect::<HashSet<_>>();
    for requirement in &requirements {
        if !flow_requirement_is_material(requirement) {
            continue;
        }
        for query in requirement.query_seeds {
            if required_queries.insert((*query).to_string()) {
                push_query_obligation(
                    &mut query_obligations,
                    PacketQueryObligationKindDto::RequiredFlow,
                    query,
                    true,
                );
            }
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
            let material = !diagnostic_queries.contains(query.as_str());
            push_query_obligation(
                &mut query_obligations,
                PacketQueryObligationKindDto::RequiredProbe,
                &query,
                material,
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
    let has_recognized_flow = !packet_flow_requirements_for_terms(&terms, task_class).is_empty();
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

fn claim_obligation(
    requirement: &FlowRequirement,
    requires_complete_discovery: bool,
) -> PacketClaimObligationDto {
    let material = flow_requirement_is_material(requirement);
    let mut open_next_candidates = Vec::new();
    for query in requirement.query_seeds {
        if !open_next_candidates
            .iter()
            .any(|candidate| candidate == query)
        {
            open_next_candidates.push((*query).to_string());
        }
    }
    PacketClaimObligationDto {
        id: requirement.id.to_string(),
        kind: claim_obligation_kind(requirement.role),
        binding_terms: Vec::new(),
        probe_binding: None,
        material,
        // Role predicates declare their lawful node kinds; carrier predicates own the check and
        // use an empty list. This keeps generic callable-only policy out of SQL, HTML/CSS, form,
        // shell, interceptor, and other explicitly structural contracts.
        allowed_node_kinds: requirement.evidence.allowed_node_kinds().to_vec(),
        // Resolved role-based behavioral flows require an incident CALL receipt. Source-range,
        // lexical, diagnostic, and explicit carrier predicates are already proven by their own
        // evidence contract and must not inherit that unrelated graph requirement.
        required_edge_kind: flow_requirement_requires_call_edge(requirement)
            .then_some(EdgeKind::CALL),
        requires_complete_discovery,
        proof_status: PacketObligationProofStatusDto::Planned,
        reason: None,
        carrier_node_ids: Vec::new(),
        carrier_paths: Vec::new(),
        carrier_edge_proofs: Vec::new(),
        open_next_candidates,
    }
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
    task_class: PacketTaskClassDto,
    requires_complete_discovery: bool,
    needs_material_fallback: bool,
) -> Vec<PacketClaimObligationDto> {
    let selected_kind = default_profile_obligation_kind(task_class);
    let mut kinds = vec![selected_kind];
    for kind in [
        PacketClaimObligationKindDto::Entrypoint,
        PacketClaimObligationKindDto::Dispatch,
        PacketClaimObligationKindDto::Orchestration,
        PacketClaimObligationKindDto::StateWrite,
        PacketClaimObligationKindDto::ExternalIo,
    ] {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
        .into_iter()
        .map(|kind| PacketClaimObligationDto {
            id: if kind == selected_kind {
                default_profile_obligation_id(task_class).to_string()
            } else {
                format!("guard_{}", claim_obligation_kind_id(kind))
            },
            kind,
            binding_terms: Vec::new(),
            probe_binding: None,
            // These guards prevent a name/path lead from becoming proof. They do not require an
            // unrelated packet to discover all five behavioral roles. Absence claims are the one
            // exception: their selected profile remains material until complete discovery exists.
            material: (requires_complete_discovery || needs_material_fallback)
                && kind == selected_kind,
            allowed_node_kinds: allowed_node_kinds_for_obligation(kind),
            required_edge_kind: Some(EdgeKind::CALL),
            requires_complete_discovery,
            proof_status: PacketObligationProofStatusDto::Planned,
            reason: None,
            carrier_node_ids: Vec::new(),
            carrier_paths: Vec::new(),
            carrier_edge_proofs: Vec::new(),
            open_next_candidates: Vec::new(),
        })
        .collect()
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

fn claim_obligation_kind_id(kind: PacketClaimObligationKindDto) -> &'static str {
    match kind {
        PacketClaimObligationKindDto::Entrypoint => "entrypoint",
        PacketClaimObligationKindDto::Dispatch => "dispatch",
        PacketClaimObligationKindDto::Orchestration => "orchestration",
        PacketClaimObligationKindDto::StateWrite => "state_write",
        PacketClaimObligationKindDto::ExternalIo => "external_io",
        PacketClaimObligationKindDto::ExactProbe => "exact_probe",
    }
}

fn allowed_node_kinds_for_obligation(_kind: PacketClaimObligationKindDto) -> Vec<NodeKind> {
    // Every claim obligation in this schema is behavioral. Structural files, types, fields, and
    // variables may report a relevant name, but cannot prove execution behavior.
    vec![NodeKind::FUNCTION, NodeKind::METHOD, NodeKind::MACRO]
}

fn default_profile_obligation_kind(task_class: PacketTaskClassDto) -> PacketClaimObligationKindDto {
    match task_class {
        PacketTaskClassDto::BugLocalization | PacketTaskClassDto::RouteTracing => {
            PacketClaimObligationKindDto::Dispatch
        }
        PacketTaskClassDto::DataFlow => PacketClaimObligationKindDto::StateWrite,
        PacketTaskClassDto::ArchitectureExplanation
        | PacketTaskClassDto::ChangeImpact
        | PacketTaskClassDto::SymbolOwnership
        | PacketTaskClassDto::EditPlanning => PacketClaimObligationKindDto::Orchestration,
    }
}

fn default_profile_obligation_id(task_class: PacketTaskClassDto) -> &'static str {
    match task_class {
        PacketTaskClassDto::ArchitectureExplanation => "profile_architecture_behavior",
        PacketTaskClassDto::BugLocalization => "profile_bug_dispatch",
        PacketTaskClassDto::ChangeImpact => "profile_change_impact_behavior",
        PacketTaskClassDto::RouteTracing => "profile_route_dispatch",
        PacketTaskClassDto::SymbolOwnership => "profile_symbol_ownership_behavior",
        PacketTaskClassDto::DataFlow => "profile_data_flow_state_write",
        PacketTaskClassDto::EditPlanning => "profile_edit_plan_behavior",
    }
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

fn claim_obligation_kind(role: FlowRole) -> PacketClaimObligationKindDto {
    match role {
        FlowRole::Entrypoint => PacketClaimObligationKindDto::Entrypoint,
        FlowRole::Dispatch => PacketClaimObligationKindDto::Dispatch,
        FlowRole::StateOrStorage => PacketClaimObligationKindDto::StateWrite,
        FlowRole::TerminalBoundary => PacketClaimObligationKindDto::ExternalIo,
        FlowRole::Registration
        | FlowRole::Configuration
        | FlowRole::TransformOrValidate
        | FlowRole::ErrorOrFallback => PacketClaimObligationKindDto::Orchestration,
    }
}

fn flow_requirement_is_material(requirement: &FlowRequirement) -> bool {
    !matches!(requirement.coverage_mode, CoverageMode::DiagnosticOnly)
}

fn flow_requirement_requires_call_edge(requirement: &FlowRequirement) -> bool {
    matches!(
        requirement.coverage_mode,
        CoverageMode::RequiresResolvedSourceOrGraph
    ) && matches!(
        requirement.evidence,
        EvidencePredicate::CitedRoles { .. }
            | EvidencePredicate::CitedRolesOrCallBoundary { .. }
            | EvidencePredicate::CitedRolesOrOrderedCallBoundary { .. }
    )
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
        let carrier_node_id = obligation.carrier_node_ids.first().cloned().or_else(|| {
            obligation
                .carrier_edge_proofs
                .first()
                .map(|proof| proof.carrier_node_id.clone())
        });
        if let Some(carrier_node_id) = carrier_node_id {
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
    let requirements =
        packet_flow_requirements_for_terms(&packet_probe_terms(question), task_class)
            .into_iter()
            .map(|requirement| (requirement.id, requirement))
            .collect::<HashMap<_, _>>();
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
        let Some(requirement) = requirements.get(obligation.id.as_str()) else {
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
            continue;
        };
        finalize_claim_obligation(obligation, requirement, answer, evidence_view);
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
                    && citation.evidence_producer.as_deref() != Some("packet_exact_path_probe")
                    && citation.coverage_role.as_deref() != Some("explicit exact probe")
                    && packet_evidence_role(citation).is_some_and(|role| {
                        !matches!(
                            role,
                            PacketEvidenceRole::SourceEvidence
                                | PacketEvidenceRole::TestsAndRegressionCoverage
                        )
                    })
            } else {
                citation.coverage_role.as_deref() == Some("explicit exact probe")
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

fn finalize_claim_obligation(
    obligation: &mut PacketClaimObligationDto,
    requirement: &FlowRequirement,
    answer: &AgentAnswerDto,
    evidence_view: PacketObligationEvidenceView,
) {
    let evidence_removed_by_budget =
        obligation.reason.as_deref() == Some(PACKET_BUDGET_TRUNCATED_REASON);
    let matching_citations = answer
        .citations
        .iter()
        .filter(|citation| requirement.evidence.citation_proves(citation))
        .collect::<Vec<_>>();
    let reported_citations = answer
        .citations
        .iter()
        .filter(|citation| {
            requirement.evidence.citation_proves(citation)
                || citation_plausibly_reports_obligation(citation, obligation.kind)
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
        return;
    }
    if matching_citations.is_empty() && reported_citations.is_empty() {
        if evidence_removed_by_budget {
            obligation.proof_status = PacketObligationProofStatusDto::Reported;
            obligation.reason = Some(PACKET_BUDGET_TRUNCATED_REASON.to_string());
        } else {
            obligation.proof_status = PacketObligationProofStatusDto::Unsupported;
            obligation.reason = Some("required_carrier_missing".to_string());
        }
        return;
    }
    if matching_citations.is_empty() {
        obligation.proof_status = PacketObligationProofStatusDto::Reported;
        obligation.reason = Some(
            if evidence_removed_by_budget {
                PACKET_BUDGET_TRUNCATED_REASON
            } else {
                "carrier_does_not_satisfy_role_contract"
            }
            .to_string(),
        );
        return;
    }
    let allowed_citations = matching_citations
        .iter()
        .copied()
        .filter(|citation| {
            citation_sufficiency_eligible(citation)
                && (obligation.allowed_node_kinds.is_empty()
                    || obligation.allowed_node_kinds.contains(&citation.kind))
        })
        .collect::<Vec<_>>();
    if allowed_citations.is_empty() {
        obligation.proof_status = PacketObligationProofStatusDto::Reported;
        obligation.reason = Some(
            if matching_citations
                .iter()
                .all(|citation| !citation_sufficiency_eligible(citation))
            {
                "carrier_not_sufficiency_eligible"
            } else {
                "carrier_node_kind_not_allowed"
            }
            .to_string(),
        );
        return;
    }
    let proven_citations = allowed_citations
        .iter()
        .copied()
        .filter(|citation| {
            obligation
                .required_edge_kind
                .is_none_or(|required_edge_kind| {
                    citation_edge_proof_for_flow_requirement(
                        citation,
                        required_edge_kind,
                        requirement,
                        answer,
                    )
                    .is_some()
                })
        })
        .collect::<Vec<_>>();
    if proven_citations.is_empty() {
        obligation.proof_status = PacketObligationProofStatusDto::Reported;
        obligation.reason = Some(
            if evidence_removed_by_budget {
                PACKET_BUDGET_TRUNCATED_REASON
            } else {
                "required_evidence_edge_missing"
            }
            .to_string(),
        );
        return;
    }
    record_obligation_carriers(
        obligation,
        proven_citations.iter().copied(),
        evidence_view.max_carriers,
    );
    if let Some(required_edge_kind) = obligation.required_edge_kind {
        record_obligation_edge_proofs_for_flow_requirement(
            obligation,
            &proven_citations,
            required_edge_kind,
            requirement,
            answer,
            evidence_view.max_carriers,
        );
    }
    obligation.proof_status = PacketObligationProofStatusDto::Proven;
    obligation.reason = None;
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
            ) && (has_requested_identity
                || citation_plausibly_reports_obligation(citation, obligation.kind))
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
                exact_binding_terms.iter().any(|exact| exact == term),
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
    let display_segments = symbol_identity_segments(display_name);
    let requested_segments = symbol_identity_segments(requested);
    !requested_segments.is_empty()
        && display_segments.len() >= requested_segments.len()
        && display_segments[display_segments.len() - requested_segments.len()..]
            == requested_segments
}

fn citation_display_matches_exact_requested_identity(display_name: &str, requested: &str) -> bool {
    let display_segments = exact_symbol_identity_segments(display_name);
    let requested_segments = exact_symbol_identity_segments(requested);
    !requested_segments.is_empty()
        && display_segments.len() >= requested_segments.len()
        && display_segments[display_segments.len() - requested_segments.len()..]
            == requested_segments
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
    let citations = citations
        .into_iter()
        .take(max_carriers.max(1))
        .collect::<Vec<_>>();
    obligation.carrier_node_ids = citations
        .iter()
        .map(|citation| citation.node_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    obligation.carrier_paths = citations
        .iter()
        .filter_map(|citation| citation.file_path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
}

fn citation_plausibly_reports_obligation(
    citation: &AgentCitationDto,
    kind: PacketClaimObligationKindDto,
) -> bool {
    let display = citation.display_name.to_ascii_lowercase();
    let path = citation
        .file_path
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let contains_any = |needles: &[&str]| {
        needles
            .iter()
            .any(|needle| display.contains(needle) || path.contains(needle))
    };
    match kind {
        PacketClaimObligationKindDto::Entrypoint => {
            display == "main"
                || display.starts_with("cli")
                || contains_any(&["entrypoint", "/main."])
        }
        PacketClaimObligationKindDto::Dispatch => contains_any(&["dispatch", "handler", "router"]),
        PacketClaimObligationKindDto::Orchestration => {
            contains_any(&["runtime", "service", "orchestrat"])
        }
        PacketClaimObligationKindDto::StateWrite => {
            contains_any(&["store", "storage", "persist", "database", "snapshot"])
        }
        PacketClaimObligationKindDto::ExternalIo => contains_any(&[
            "network",
            "client",
            "transport",
            "socket",
            "send",
            "write",
            "terminal",
        ]),
        PacketClaimObligationKindDto::ExactProbe => false,
    }
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

fn citation_edge_proof_for_flow_requirement(
    citation: &AgentCitationDto,
    required_edge_kind: EdgeKind,
    requirement: &FlowRequirement,
    answer: &AgentAnswerDto,
) -> Option<PacketObligationCarrierEdgeProofDto> {
    if required_edge_kind != EdgeKind::CALL {
        return citation_edge_proof(citation, required_edge_kind, answer);
    }
    let graphs = packet_execution_graphs(answer);
    let cited_edge_ids = citation.evidence_edge_ids.iter().collect::<HashSet<_>>();
    graphs
        .iter()
        .flat_map(|graph| graph.edges.iter().map(move |edge| (*graph, edge)))
        .filter(|(_, edge)| {
            edge.kind == EdgeKind::CALL
                && cited_edge_ids.contains(&edge.id)
                && (edge.source == citation.node_id || edge.target == citation.node_id)
        })
        .filter(|(graph, edge)| {
            receipt_neighbor(graph, answer, citation, edge).is_some_and(|(label, kind)| {
                flow_requirement_call_receipt_is_valid(requirement, citation, edge, label, kind)
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

fn record_obligation_edge_proofs_for_flow_requirement(
    obligation: &mut PacketClaimObligationDto,
    citations: &[&AgentCitationDto],
    required_edge_kind: EdgeKind,
    requirement: &FlowRequirement,
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
        .filter_map(|citation| {
            citation_edge_proof_for_flow_requirement(
                citation,
                required_edge_kind,
                requirement,
                answer,
            )
        })
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
                && citation.evidence_producer.as_deref() != Some("packet_exact_path_probe")
                && citation.coverage_role.as_deref() != Some("explicit exact probe")
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
    task_class: PacketTaskClassDto,
    obligation: &PacketClaimObligationDto,
    citations: &[AgentCitationDto],
) -> String {
    if obligation.proof_status == PacketObligationProofStatusDto::Proven && !citations.is_empty() {
        if let Some(receipt) =
            proven_server_flow_receipt_text(answer, task_class, obligation, citations)
        {
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

/// Render a proven material obligation as a concrete typed relation.
///
/// This used to serve three hardcoded server obligation ids, so every other flow family —
/// site build, client request, SQL schema, form validation, shell install, buffered IO,
/// log handler, mapper, formatting, string predicate, and the rest — fell through to
/// "Material obligation `x` has independently cited carrier evidence at …", a pointer at
/// evidence rather than an explanation of it. The data needed for the real sentence was
/// already resolved for all of them; only the lookup was narrow.
fn proven_server_flow_receipt_text(
    answer: &AgentAnswerDto,
    task_class: PacketTaskClassDto,
    obligation: &PacketClaimObligationDto,
    citations: &[AgentCitationDto],
) -> Option<String> {
    let requirement = flow_requirement_for_obligation(answer, task_class, obligation)?;
    obligation.carrier_edge_proofs.iter().find_map(|proof| {
        if proof.edge_kind != EdgeKind::CALL {
            return None;
        }
        // The carrier predicate check that used to sit here was a second, narrower copy of
        // work `flow_requirement_call_receipt_is_valid` already does below: it runs the
        // requirement's own `EvidencePredicate`, which is the authority on whether this
        // citation may carry this requirement.
        let citation = citations.iter().find(|citation| {
            citation.node_id == proof.carrier_node_id
                && citation.evidence_edge_ids.contains(&proof.edge_id)
        })?;
        packet_execution_graphs(answer).iter().find_map(|graph| {
            let edge = graph.edges.iter().find(|edge| edge.id == proof.edge_id)?;
            let (target_label, target_kind) = receipt_neighbor(graph, answer, citation, edge)?;
            if !flow_requirement_call_receipt_is_valid(
                &requirement,
                citation,
                edge,
                target_label,
                target_kind,
            ) {
                return None;
            }
            let target = server_receipt_target(edge, target_label, target_kind);
            Some(flow_relation_receipt(
                requirement.role,
                proof.edge_kind,
                &citation.display_name,
                &target,
            ))
        })
    })
}

/// Recover the flow requirement that minted this obligation.
///
/// `claim_obligation` stamps `id` from the requirement and copies its `query_seeds` into
/// `open_next_candidates`, so replaying the same lookup `finalize_packet_claim_obligations`
/// performs is exact identity recovery, not inference. Kind plus full seed containment keeps
/// families that deliberately share an obligation id (server and client request flows) from
/// borrowing each other's semantics.
fn flow_requirement_for_obligation(
    answer: &AgentAnswerDto,
    task_class: PacketTaskClassDto,
    obligation: &PacketClaimObligationDto,
) -> Option<FlowRequirement> {
    packet_flow_requirements_for_terms(&packet_probe_terms(&answer.prompt), task_class)
        .into_iter()
        .find(|requirement| {
            requirement.id == obligation.id
                && obligation.kind == claim_obligation_kind(requirement.role)
                && requirement.query_seeds.iter().all(|seed| {
                    obligation
                        .open_next_candidates
                        .iter()
                        .any(|candidate| candidate == seed)
                })
        })
}

/// One sentence naming what this carrier does in the flow and the typed edge that proves it.
///
/// The verb comes from the requirement's declared `FlowRole` and the relation from the
/// `EdgeKind`; the carrier, target, and their spelling come from the graph. Nothing here
/// knows a repository, a language, or a framework noun — the previous version keyed English
/// words off the carrier's terminal segment (`use` → "middleware"), which is why it could
/// only ever describe a handful of shapes.
fn flow_relation_receipt(
    role: FlowRole,
    edge_kind: EdgeKind,
    carrier: &str,
    target: &str,
) -> String {
    let action = match role {
        FlowRole::Entrypoint => "enters this flow",
        FlowRole::Registration => "registers this flow's handlers",
        FlowRole::Configuration => "configures this flow",
        FlowRole::StateOrStorage => "reaches this flow's state",
        FlowRole::Dispatch => "delegates this flow",
        FlowRole::TransformOrValidate => "transforms this flow's data",
        FlowRole::TerminalBoundary => "completes this flow",
        FlowRole::ErrorOrFallback => "handles this flow's failure",
    };
    format!("`{carrier}` {action} through the retained {edge_kind:?} to `{target}`.")
}

fn server_receipt_target(edge: &GraphEdgeDto, target_label: &str, target_kind: NodeKind) -> String {
    if target_kind != NodeKind::UNKNOWN {
        return target_label.to_string();
    }
    let leaf = target_label
        .rsplit(['.', ':', '#'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(target_label);
    edge.callsite_identity
        .as_deref()
        .and_then(|identity| {
            identity.split('|').find_map(|segment| {
                segment
                    .strip_prefix("receiver-owner:")
                    .map(str::trim)
                    .filter(|owner| !owner.is_empty())
            })
        })
        .map_or_else(
            || target_label.to_string(),
            |owner| format!("{owner}.{leaf}"),
        )
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
    question: &str,
    _task_class: PacketTaskClassDto,
) -> bool {
    let tokens = packet_discovery_intent_tokens(question);
    if tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "absence"
                | "absent"
                | "nonexistent"
                | "orphan"
                | "orphaned"
                | "unreachable"
                | "unreferenced"
                | "unused"
        )
    }) {
        return true;
    }

    tokens.iter().enumerate().any(|(subject_index, token)| {
        packet_discovery_subject(token).is_some()
            && packet_discovery_subject_has_negator(&tokens, subject_index)
    })
}

fn packet_discovery_intent_tokens(question: &str) -> Vec<String> {
    let raw = question
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let mut tokens = Vec::with_capacity(raw.len());
    let mut index = 0usize;
    while index < raw.len() {
        if raw.get(index + 1).is_some_and(|next| next == "t")
            && matches!(
                raw[index].as_str(),
                "aren"
                    | "can"
                    | "couldn"
                    | "didn"
                    | "doesn"
                    | "don"
                    | "hadn"
                    | "hasn"
                    | "haven"
                    | "isn"
                    | "mustn"
                    | "shouldn"
                    | "wasn"
                    | "weren"
                    | "won"
                    | "wouldn"
            )
        {
            tokens.push("not".to_string());
            index += 2;
            continue;
        }
        tokens.push(raw[index].clone());
        index += 1;
    }
    tokens
}

fn packet_discovery_subject(token: &str) -> Option<&'static str> {
    match token {
        "call" | "called" | "caller" | "callers" | "calling" | "calls" => Some("call"),
        "exist" | "existed" | "exists" | "existing" => Some("existence"),
        "handler" | "handlers" | "handling" => Some("handler"),
        "implementation" | "implementations" | "implemented" | "implementing" => {
            Some("implementation")
        }
        "reference" | "referenced" | "references" | "referencing" => Some("reference"),
        "route" | "routed" | "routes" | "routing" => Some("route"),
        "usage" | "usages" | "use" | "used" | "uses" | "using" => Some("usage"),
        _ => None,
    }
}

fn packet_discovery_subject_has_negator(tokens: &[String], subject_index: usize) -> bool {
    let start = subject_index.saturating_sub(4);
    for negator_index in (start..subject_index).rev() {
        let negator = tokens[negator_index].as_str();
        if !matches!(
            negator,
            "missing" | "never" | "no" | "none" | "not" | "without" | "zero"
        ) {
            continue;
        }
        let bridge = &tokens[negator_index + 1..subject_index];
        let bridge_is_semantic_modifier = bridge.iter().all(|token| {
            matches!(
                token.as_str(),
                "any" | "direct" | "known" | "of" | "other" | "remaining" | "the"
            )
        });
        if bridge_is_semantic_modifier {
            return true;
        }
    }
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
        PacketSidecarQueryDiagnosticDto, SearchHitOrigin,
    };

    const INDEXING_QUESTION: &str = "Explain the indexing runtime, persistence, and snapshot flow.";

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

    fn query_diagnostic(
        query: &str,
        completion: PacketQueryCompletionDto,
    ) -> PacketSidecarQueryDiagnosticDto {
        PacketSidecarQueryDiagnosticDto {
            query: query.to_string(),
            completion,
            retrieval_mode: "full".to_string(),
            sidecar_query_ms: Some(1),
            candidate_resolution_ms: Some(0),
            total_elapsed_ms: Some(1),
            sidecar_stage_count: 1,
            sidecar_stage_total_ms: Some(1),
            batch_query_wall_ms: Some(1),
            candidate_count: 0,
            resolved_hit_count: 0,
            unresolved_candidate_count: 0,
            blocking_unresolved_candidate_count: 0,
            semantic_stage_timeout_zero_hits: false,
            semantic_abstained: false,
            diagnostic: None,
        }
    }

    fn lexical_citation(name: &str, path: &str, kind: NodeKind) -> AgentCitationDto {
        let mut citation = citation(name, path, kind);
        citation.evidence_tier = Some(PacketEvidenceTierDto::LexicalSource);
        citation.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);
        citation
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

    #[derive(Clone, Copy)]
    struct FlowBoundaryCase {
        label: &'static str,
        question: &'static str,
        task_class: PacketTaskClassDto,
        obligation_id: &'static str,
        carrier_name: &'static str,
        carrier_path: &'static str,
        carrier_kind: NodeKind,
        lawful_target: &'static str,
        role_only_name: &'static str,
        role_only_path: &'static str,
        role_only_kind: NodeKind,
    }

    fn evaluate_flow_boundary(
        case: FlowBoundaryCase,
        target_name: &str,
        outgoing: bool,
    ) -> (
        PacketObligationProofStatusDto,
        Option<String>,
        Vec<NodeId>,
        Vec<EdgeId>,
    ) {
        let edge_id = EdgeId(format!("{}-call", case.obligation_id));
        let mut carrier = citation(case.carrier_name, case.carrier_path, case.carrier_kind);
        carrier.evidence_edge_ids = vec![edge_id.clone()];
        let target = citation(target_name, "src/boundary_target.rs", NodeKind::METHOD);
        let (source, destination) = if outgoing {
            (carrier.node_id.clone(), target.node_id.clone())
        } else {
            (target.node_id.clone(), carrier.node_id.clone())
        };
        let mut carried_answer = answer(vec![carrier, target]);
        carried_answer.prompt = case.question.to_string();
        carried_answer.graphs.push(GraphArtifactDto::Uml {
            id: format!("{}-flow", case.obligation_id),
            title: format!("{} flow", case.label),
            graph: GraphResponse {
                center_id: source.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: edge_id,
                    source,
                    target: destination,
                    kind: EdgeKind::CALL,
                    confidence: Some(1.0),
                    certainty: Some("certain".to_string()),
                    callsite_identity: Some(format!("test:{}", case.obligation_id)),
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        });
        let mut plan = build_packet_obligation_plan(case.question, case.task_class, &[]);
        plan.claim_obligations
            .retain(|obligation| obligation.id == case.obligation_id);
        plan.query_obligations.clear();
        assert_eq!(
            plan.claim_obligations.len(),
            1,
            "{}: question did not select exactly one {} obligation",
            case.label,
            case.obligation_id
        );

        let snapshot = capture_packet_obligation_edge_proofs_before_budget(
            case.question,
            case.task_class,
            &plan,
            &carried_answer,
        );
        let protected_carriers = protected_packet_obligation_carrier_node_ids(&snapshot).to_vec();
        let protected_edges = protected_packet_obligation_edge_ids(&snapshot).to_vec();
        finalize_packet_obligation_plan(
            case.question,
            case.task_class,
            &mut plan,
            &carried_answer,
            &budget(),
        );
        let obligation = &plan.claim_obligations[0];
        (
            obligation.proof_status,
            obligation.reason.clone(),
            protected_carriers,
            protected_edges,
        )
    }

    fn raw_server_dispatch_answer(
        target_label: &str,
        target_kind: NodeKind,
        certainty: Option<&str>,
        confidence: Option<f32>,
        callsite_identity: Option<&str>,
        outgoing: bool,
    ) -> AgentAnswerDto {
        let mut carrier = citation("app.handle", "lib/application.js", NodeKind::METHOD);
        carrier.evidence_edge_ids = vec![EdgeId("dispatch-call".to_string())];
        let target_id = NodeId("dispatch-target".to_string());
        let (source, target) = if outgoing {
            (carrier.node_id.clone(), target_id.clone())
        } else {
            (target_id.clone(), carrier.node_id.clone())
        };
        let mut answer = answer(vec![carrier]);
        answer.prompt = "Trace how an HTTP server routes an incoming request through route registration, request handler dispatch, and response finalization.".to_string();
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "dispatch-flow".to_string(),
            title: "Dispatch flow".to_string(),
            graph: GraphResponse {
                center_id: source.clone(),
                nodes: vec![GraphNodeDto {
                    id: target_id,
                    label: target_label.to_string(),
                    kind: target_kind,
                    depth: 1,
                    label_policy: None,
                    badge_visible_members: None,
                    badge_total_members: None,
                    merged_symbol_examples: Vec::new(),
                    file_path: Some("lib/tiny.js".to_string()),
                    qualified_name: None,
                    member_access: None,
                }],
                edges: vec![GraphEdgeDto {
                    id: EdgeId("dispatch-call".to_string()),
                    source,
                    target,
                    kind: EdgeKind::CALL,
                    confidence,
                    certainty: certainty.map(str::to_string),
                    callsite_identity: callsite_identity.map(str::to_string),
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        });
        answer
    }

    fn raw_server_receipt_answer(
        carrier_name: &str,
        target_label: &str,
        receiver_owner: &str,
        edge_id: &str,
    ) -> AgentAnswerDto {
        let mut carrier = citation(carrier_name, "lib/server.js", NodeKind::METHOD);
        carrier.evidence_edge_ids = vec![EdgeId(edge_id.to_string())];
        let target_id = NodeId(format!("{edge_id}-target"));
        let mut answer = answer(vec![carrier.clone()]);
        answer.prompt =
            "Trace how a server routes an incoming request through a handler and sends the response."
                .to_string();
        answer.graphs.push(GraphArtifactDto::Uml {
            id: format!("{edge_id}-graph"),
            title: "Server flow".to_string(),
            graph: GraphResponse {
                center_id: carrier.node_id.clone(),
                nodes: vec![GraphNodeDto {
                    id: target_id.clone(),
                    label: target_label.to_string(),
                    kind: NodeKind::UNKNOWN,
                    depth: 1,
                    label_policy: None,
                    badge_visible_members: None,
                    badge_total_members: None,
                    merged_symbol_examples: Vec::new(),
                    file_path: Some("lib/server.js".to_string()),
                    qualified_name: None,
                    member_access: None,
                }],
                edges: vec![GraphEdgeDto {
                    id: EdgeId(edge_id.to_string()),
                    source: carrier.node_id,
                    target: target_id,
                    kind: EdgeKind::CALL,
                    confidence: None,
                    certainty: None,
                    callsite_identity: Some(format!(
                        "lib/server.js:1|syntax:js-member-call|receiver-owner:{receiver_owner}"
                    )),
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        });
        answer
    }

    fn finalize_server_dispatch_answer(
        answer: &AgentAnswerDto,
    ) -> (
        PacketObligationProofStatusDto,
        Option<String>,
        Vec<NodeId>,
        Vec<EdgeId>,
    ) {
        let question = answer.prompt.as_str();
        let mut plan =
            build_packet_obligation_plan(question, PacketTaskClassDto::RouteTracing, &[]);
        plan.claim_obligations
            .retain(|obligation| obligation.id == "request_dispatch");
        plan.query_obligations.clear();
        let snapshot = capture_packet_obligation_edge_proofs_before_budget(
            question,
            PacketTaskClassDto::RouteTracing,
            &plan,
            answer,
        );
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::RouteTracing,
            &mut plan,
            answer,
            &budget(),
        );
        let obligation = &plan.claim_obligations[0];
        (
            obligation.proof_status,
            obligation.reason.clone(),
            protected_packet_obligation_carrier_node_ids(&snapshot).to_vec(),
            protected_packet_obligation_edge_ids(&snapshot).to_vec(),
        )
    }

    fn finalized_server_receipt_claim(
        answer: &AgentAnswerDto,
        obligation_id: &str,
    ) -> (PacketObligationPlanDto, PacketClaimDto) {
        let mut plan =
            build_packet_obligation_plan(&answer.prompt, PacketTaskClassDto::RouteTracing, &[]);
        plan.claim_obligations
            .retain(|obligation| obligation.id == obligation_id);
        assert_eq!(plan.claim_obligations.len(), 1, "missing {obligation_id}");
        plan.query_obligations.clear();
        finalize_packet_obligation_plan(
            &answer.prompt,
            PacketTaskClassDto::RouteTracing,
            &mut plan,
            answer,
            &budget(),
        );
        let claims = packet_claims_with_obligation_receipts(answer, &plan, (Vec::new(), ()));
        assert_eq!(claims.len(), 1);
        (plan, claims.into_iter().next().expect("receipt claim"))
    }

    fn indexing_entrypoint_plan() -> PacketObligationPlanDto {
        let mut plan = build_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        plan.claim_obligations
            .retain(|obligation| obligation.id == "indexing_entrypoint");
        plan.query_obligations.clear();
        plan
    }

    fn indexing_obligation_plan(id: &str) -> PacketObligationPlanDto {
        let mut plan = build_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        plan.claim_obligations
            .retain(|obligation| obligation.id == id);
        plan.query_obligations.clear();
        plan
    }

    #[test]
    fn required_queries_survive_when_the_budgeted_plan_drops_them() {
        let plan = build_packet_obligation_plan(
            "Explain the indexing runtime, persistence, and snapshot flow.",
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );

        assert!(!plan.claim_obligations.is_empty());
        assert!(plan.query_obligations.iter().any(|obligation| {
            obligation.material
                && matches!(
                    obligation.kind,
                    PacketQueryObligationKindDto::RequiredFlow
                        | PacketQueryObligationKindDto::RequiredProbe
                )
        }));
        assert_eq!(
            plan.query_obligations
                .iter()
                .map(|obligation| obligation.query.as_str())
                .collect::<HashSet<_>>()
                .len(),
            plan.query_obligations.len(),
            "one retrieval query must not create duplicate obligation receipts"
        );
    }

    #[test]
    fn recognized_flow_keeps_concrete_requested_claim_and_exact_probe_material() {
        let plan = build_packet_obligation_plan(
            "Explain the indexing runtime and MissingWidget.",
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );

        assert!(
            plan.claim_obligations
                .iter()
                .any(|obligation| obligation.id == "indexing_entrypoint")
        );
        assert!(
            plan.claim_obligations
                .iter()
                .any(|obligation| obligation.id == "indexing_storage")
        );
        assert!(plan.claim_obligations.iter().any(|obligation| {
            obligation.material && obligation.binding_terms == ["MissingWidget"]
        }));
        assert!(plan.query_obligations.iter().any(|obligation| {
            obligation.material
                && obligation.kind == PacketQueryObligationKindDto::RequiredProbe
                && obligation.query == "MissingWidget"
        }));
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
    fn server_route_prompt_uses_flow_obligations_instead_of_prose_tokens() {
        let plan = build_packet_obligation_plan(
            "Trace how an Express application registers middleware and routes, then dispatches an incoming request through router layers to a route handler.",
            PacketTaskClassDto::RouteTracing,
            &[],
        );
        let material_ids = plan
            .claim_obligations
            .iter()
            .filter(|obligation| obligation.material)
            .map(|obligation| obligation.id.as_str())
            .collect::<Vec<_>>();

        assert!(plan.binding_terms.is_empty(), "{plan:#?}");
        assert_eq!(material_ids, ["request_entrypoint", "request_dispatch"]);
        assert!(
            plan.claim_obligations
                .iter()
                .all(|obligation| { !obligation.id.starts_with("requested_claim:") })
        );
    }

    #[test]
    fn filtered_generic_request_gets_one_material_fallback_guard() {
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
        assert_eq!(material.len(), 1);
        assert_eq!(material[0].id, "profile_architecture_behavior");
        assert!(material[0].binding_terms.is_empty());
    }

    #[test]
    fn exact_identity_does_not_hide_a_behavioral_fallback_or_pollute_pure_ownership() {
        let architecture = build_packet_obligation_plan(
            "Explain how RuntimeService::run participates in the architecture.",
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        assert!(architecture.claim_obligations.iter().any(|obligation| {
            obligation.id == "profile_architecture_behavior" && obligation.material
        }));
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
    fn natural_language_search_flow_does_not_mint_prose_claims() {
        let question = "Find the production packet/search path that turns ranked search results into packet evidence and agent handoff.";
        let plan = build_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        let material_ids = plan
            .claim_obligations
            .iter()
            .filter(|obligation| obligation.material)
            .map(|obligation| obligation.id.as_str())
            .collect::<Vec<_>>();

        assert!(plan.binding_terms.is_empty(), "{plan:#?}");
        assert_eq!(
            material_ids,
            [
                "search_entrypoint",
                "search_dispatch",
                "search_evidence_classification",
                "search_evidence_output",
            ]
        );
        assert!(
            plan.claim_obligations
                .iter()
                .all(|obligation| { !obligation.id.starts_with("requested_claim:") })
        );
        assert!(
            plan.query_obligations
                .iter()
                .all(|obligation| { obligation.query != "packet/search" || !obligation.material })
        );
    }

    #[test]
    fn exact_search_symbol_is_separate_from_typed_search_flow() {
        let question = "Explain how LiveSidecarSearch::semantic_search participates in the live sidecar search path and why packet/search evidence cannot be promoted when retrieval sidecars are unavailable or stale.";
        let plan = build_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        let requested = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.binding_terms == ["LiveSidecarSearch::semantic_search"])
            .expect("exact symbol identity obligation");

        assert_eq!(requested.kind, PacketClaimObligationKindDto::ExactProbe);
        assert_eq!(requested.required_edge_kind, None);
        assert!(requested.material);
        assert!(
            plan.claim_obligations
                .iter()
                .any(|obligation| { obligation.id == "search_entrypoint" && obligation.material })
        );
        assert!(
            plan.claim_obligations
                .iter()
                .any(|obligation| { obligation.id == "search_dispatch" && obligation.material })
        );
        assert!(
            plan.claim_obligations
                .iter()
                .all(|obligation| { obligation.binding_terms != ["packet/search"] })
        );
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
    fn diagnostic_flow_probe_stays_nonmaterial_when_required_probes_are_added() {
        let plan = build_packet_obligation_plan(
            "Explain shell installer function dispatch and completion.",
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        let completion = plan
            .query_obligations
            .iter()
            .find(|obligation| obligation.query == "shell completion")
            .expect("diagnostic completion query remains visible");
        assert_eq!(completion.kind, PacketQueryObligationKindDto::RequiredProbe);
        assert!(!completion.material);
        assert!(plan.query_obligations.iter().any(|obligation| {
            obligation.query == "shell installer bootstrap" && obligation.material
        }));
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
        carrier.coverage_role = Some("explicit exact probe".to_string());

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
        diagnostic.coverage_role = Some("explicit exact probe".to_string());
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
        synthetic_carrier.coverage_role = Some("explicit exact probe".to_string());
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
        let mut claims =
            packet_claims_with_obligation_receipts(&carried_answer, &plan, (vec![role_claim], ()));
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
        assert!(plan.claim_obligations.iter().any(|obligation| {
            obligation.id == "profile_architecture_behavior" && obligation.material
        }));
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
        let mut absence_plan = build_packet_obligation_plan(
            absence_question,
            PacketTaskClassDto::SymbolOwnership,
            &[],
        );
        let discovery_required_ids = absence_plan
            .claim_obligations
            .iter()
            .filter(|obligation| obligation.requires_complete_discovery && obligation.material)
            .map(|obligation| obligation.id.clone())
            .collect::<Vec<_>>();
        assert!(!discovery_required_ids.is_empty(), "{absence_plan:#?}");
        let absence_resolution = PacketProbeResolutionDto {
            input_index: 3,
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
        append_packet_probe_obligations(
            &mut absence_plan,
            &[absence_resolution],
            absence_question,
            PacketTaskClassDto::SymbolOwnership,
        );
        for id in discovery_required_ids {
            assert!(absence_plan.claim_obligations.iter().any(|obligation| {
                obligation.id == id && obligation.material && obligation.requires_complete_discovery
            }));
        }

        let fallback_question = "?";
        let mut fallback_plan = build_packet_obligation_plan(
            fallback_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        assert!(
            fallback_plan.claim_obligations.iter().any(|obligation| {
                obligation.id
                    == default_profile_obligation_id(PacketTaskClassDto::ArchitectureExplanation)
                    && obligation.material
            }),
            "{fallback_plan:#?}"
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
        assert!(fallback_plan.claim_obligations.iter().any(|obligation| {
            obligation.id
                == default_profile_obligation_id(PacketTaskClassDto::ArchitectureExplanation)
                && !obligation.material
        }));
    }

    #[test]
    fn exact_path_scope_preserves_flow_and_explicit_symbol_obligations() {
        assert!(packet_exact_symbol_query_is_explicit(
            "Explain RuntimeService() behavior.",
            "RuntimeService"
        ));
        assert!(!packet_exact_symbol_query_is_explicit(
            "Explain RuntimeService behavior.",
            "RuntimeService"
        ));
        let question = "Explain the indexing runtime and RuntimeService::run.";
        let task_class = PacketTaskClassDto::ArchitectureExplanation;
        let mut plan = build_packet_obligation_plan(question, task_class, &[]);
        let material_before = plan
            .claim_obligations
            .iter()
            .filter(|obligation| obligation.material)
            .map(|obligation| obligation.id.clone())
            .collect::<Vec<_>>();
        assert!(material_before.iter().any(|id| id.starts_with("indexing_")));
        assert!(material_before.iter().any(|id| {
            id.starts_with("requested_claim:") && id.contains("RuntimeService::run")
        }));
        let resolution = PacketProbeResolutionDto {
            input_index: 4,
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

        for id in material_before {
            let obligation = plan
                .claim_obligations
                .iter()
                .find(|obligation| obligation.id == id)
                .expect("pre-existing obligation");
            assert!(obligation.material, "{id} must remain material");
        }

        let mixed_question = "Explain RuntimeService::run alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo.";
        let mixed_task_class = PacketTaskClassDto::SymbolOwnership;
        let mut mixed_plan = build_packet_obligation_plan(mixed_question, mixed_task_class, &[]);
        assert!(
            mixed_plan
                .claim_obligations
                .iter()
                .all(|obligation| { obligation.id != REQUESTED_CLAIM_OVERFLOW_ID })
        );
        let mixed_resolution = PacketProbeResolutionDto {
            input_index: 5,
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
        append_packet_probe_obligations(
            &mut mixed_plan,
            &[mixed_resolution],
            mixed_question,
            mixed_task_class,
        );
        assert!(mixed_plan.claim_obligations.iter().any(|obligation| {
            obligation.id.starts_with("requested_claim:")
                && obligation.id.contains("RuntimeService::run")
                && obligation.material
        }));
        assert!(
            mixed_plan
                .claim_obligations
                .iter()
                .all(|obligation| { obligation.id != REQUESTED_CLAIM_OVERFLOW_ID })
        );

        let long_owner = "A".repeat(PACKET_OBLIGATION_BINDING_TERM_CHAR_LIMIT + 24);
        let long_question = format!("Explain {long_owner}::run.");
        let mut long_plan = build_packet_obligation_plan(&long_question, mixed_task_class, &[]);
        assert!(long_plan.claim_obligations.iter().any(|obligation| {
            obligation.id.starts_with("requested_claim:") && obligation.material
        }));
        let long_resolution = PacketProbeResolutionDto {
            input_index: 6,
            probe: PacketProbeDto::ExactPath {
                path: "src/long.rs".to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some("src/long.rs".to_string()),
            path: Some("src/long.rs".to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };
        append_packet_probe_obligations(
            &mut long_plan,
            &[long_resolution],
            &long_question,
            mixed_task_class,
        );
        assert!(long_plan.claim_obligations.iter().any(|obligation| {
            obligation.id.starts_with("requested_claim:") && obligation.material
        }));

        let bare_question = "Explain RuntimeService. System behavior follows.";
        let mut bare_plan = build_packet_obligation_plan(
            bare_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        let bare_resolution = PacketProbeResolutionDto {
            input_index: 7,
            probe: PacketProbeDto::ExactPath {
                path: "src/service.rs".to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some("src/service.rs".to_string()),
            path: Some("src/service.rs".to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };
        append_packet_probe_obligations(
            &mut bare_plan,
            &[bare_resolution],
            bare_question,
            PacketTaskClassDto::ArchitectureExplanation,
        );
        assert!(bare_plan.claim_obligations.iter().any(|obligation| {
            obligation.id.starts_with("requested_claim:")
                && obligation.id.contains("RuntimeService")
                && !obligation.material
        }));

        let standalone_question = "RuntimeService";
        let mut standalone_plan = build_packet_obligation_plan(
            standalone_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        let standalone_resolution = PacketProbeResolutionDto {
            input_index: 8,
            probe: PacketProbeDto::ExactPath {
                path: "src/standalone.rs".to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some("src/standalone.rs".to_string()),
            path: Some("src/standalone.rs".to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };
        append_packet_probe_obligations(
            &mut standalone_plan,
            &[standalone_resolution],
            standalone_question,
            PacketTaskClassDto::ArchitectureExplanation,
        );
        assert!(standalone_plan.claim_obligations.iter().any(|obligation| {
            obligation.id.starts_with("requested_claim:")
                && obligation.id.contains("RuntimeService")
                && obligation.material
        }));

        let quoted_question = "Explain the `RuntimeService()` system.";
        let mut quoted_plan = build_packet_obligation_plan(
            quoted_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        let quoted_resolution = PacketProbeResolutionDto {
            input_index: 8,
            probe: PacketProbeDto::ExactPath {
                path: "src/quoted.rs".to_string(),
            },
            status: PacketProbeResolutionStatusDto::ExactPath,
            normalized_query: Some("src/quoted.rs".to_string()),
            path: Some("src/quoted.rs".to_string()),
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };
        append_packet_probe_obligations(
            &mut quoted_plan,
            &[quoted_resolution],
            quoted_question,
            PacketTaskClassDto::ArchitectureExplanation,
        );
        assert!(quoted_plan.claim_obligations.iter().any(|obligation| {
            obligation.id.starts_with("requested_claim:")
                && obligation.id.contains("RuntimeService")
                && obligation.material
        }));
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
        let mut claims = packet_claims_with_obligation_receipts(&carried_answer, &plan, supported);
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
    fn flow_predicates_admit_lawful_structural_carriers_without_call_edges() {
        let cases = [
            (
                "Explain SQL schema tables and foreign-key relationships.",
                PacketTaskClassDto::ArchitectureExplanation,
                "sql_tables",
                lexical_citation("CREATE TABLE users", "schema/app.sql", NodeKind::FILE),
                lexical_citation("random token", "schema/app.sql", NodeKind::FILE),
            ),
            (
                "Explain SQL schema tables and foreign-key relationships.",
                PacketTaskClassDto::ArchitectureExplanation,
                "sql_relationships",
                lexical_citation("FOREIGN KEY users", "schema/app.sql", NodeKind::ANNOTATION),
                lexical_citation("CHECK price", "schema/app.sql", NodeKind::ANNOTATION),
            ),
            (
                "Explain the HTML and CSS template structure.",
                PacketTaskClassDto::ArchitectureExplanation,
                "html_app_shell",
                lexical_citation("app shell", "web/index.html", NodeKind::FILE),
                lexical_citation("footer", "web/index.html", NodeKind::FILE),
            ),
            (
                "Explain how form validation combines native constraints, custom validation, and submit guards.",
                PacketTaskClassDto::ArchitectureExplanation,
                "form_native_constraints",
                lexical_citation(
                    "required",
                    "examples/form-validation/index.html",
                    NodeKind::ANNOTATION,
                ),
                lexical_citation(
                    "title",
                    "examples/form-validation/index.html",
                    NodeKind::ANNOTATION,
                ),
            ),
            (
                "Trace request dispatch through an interceptor and transport adapter.",
                PacketTaskClassDto::RouteTracing,
                "request_interceptor_management",
                lexical_citation(
                    "InterceptorManager",
                    "src/interceptors.rs",
                    NodeKind::STRUCT,
                ),
                lexical_citation("TelemetryManager", "src/interceptors.rs", NodeKind::STRUCT),
            ),
        ];

        for (question, task_class, obligation_id, lawful, false_carrier) in cases {
            let lawful_kind = lawful.kind;
            let lawful_id = lawful.node_id.clone();
            let false_id = false_carrier.node_id.clone();
            let mut answer = answer(vec![lawful, false_carrier]);
            answer.prompt = question.to_string();
            let mut plan = build_packet_obligation_plan(question, task_class, &[]);
            finalize_packet_obligation_plan(question, task_class, &mut plan, &answer, &budget());
            let obligation = plan
                .claim_obligations
                .iter()
                .find(|obligation| obligation.id == obligation_id)
                .unwrap_or_else(|| panic!("missing {obligation_id} for {question}"));

            assert_eq!(
                obligation.proof_status,
                PacketObligationProofStatusDto::Proven,
                "{obligation_id}: {question}"
            );
            assert!(obligation.carrier_node_ids.contains(&lawful_id));
            assert!(!obligation.carrier_node_ids.contains(&false_id));
            assert!(
                obligation.allowed_node_kinds.is_empty()
                    || obligation.allowed_node_kinds.contains(&lawful_kind)
            );
            assert_eq!(obligation.required_edge_kind, None);
        }
    }

    #[test]
    fn transport_adapter_type_alone_is_not_adapter_selection_proof() {
        let question = "Trace request dispatch through an interceptor and transport adapter.";
        let adapter = lexical_citation("HttpTransportAdapter", "src/transport.rs", NodeKind::CLASS);
        let adapter_id = adapter.node_id.clone();
        let mut answer = answer(vec![adapter]);
        answer.prompt = question.to_string();
        let mut plan =
            build_packet_obligation_plan(question, PacketTaskClassDto::RouteTracing, &[]);
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::RouteTracing,
            &mut plan,
            &answer,
            &budget(),
        );
        let obligation = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == "request_terminal")
            .expect("request terminal obligation");

        assert_eq!(
            obligation.proof_status,
            PacketObligationProofStatusDto::Unsupported
        );
        assert!(!obligation.carrier_node_ids.contains(&adapter_id));
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
            let expected_material = if task_class == PacketTaskClassDto::SymbolOwnership {
                1
            } else {
                2
            };
            assert_eq!(requested.len(), expected_material, "{task_class:?}");
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
                usize::from(task_class != PacketTaskClassDto::SymbolOwnership),
                "non-ownership tasks retain one behavioral fallback: {task_class:?}"
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
                5,
                "ordinary packets retain five non-forcing category guards: {task_class:?}"
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
                6,
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

        assert!(
            plan.claim_obligations
                .iter()
                .filter(|obligation| obligation.material)
                .all(|obligation| obligation.requires_complete_discovery)
        );
        assert!(plan.claim_obligations.iter().any(|obligation| {
            obligation.material && obligation.binding_terms == ["Widget::run"]
        }));
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
    fn registered_source_paths_do_not_become_symbol_obligations() {
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
                plan.query_obligations
                    .iter()
                    .all(|obligation| { !(obligation.material && obligation.query == path) }),
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
    fn proven_entrypoint_cannot_promote_a_distinct_reported_storage_citation() {
        let mut entrypoint = citation(
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        entrypoint.evidence_edge_ids = vec![EdgeId("call-index".to_string())];
        let mut storage = citation(
            "SnapshotRefresh::refresh",
            "crates/example/src/snapshot.rs",
            NodeKind::METHOD,
        );
        storage.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
        storage.resolution_status = Some(PacketEvidenceResolutionDto::DiagnosticOnly);
        storage.eligible_for_sufficiency = Some(false);
        let target = citation(
            "Worker::execute",
            "crates/example/src/worker.rs",
            NodeKind::METHOD,
        );
        let mut answer = answer(vec![entrypoint.clone(), storage.clone(), target.clone()]);
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "indexing-flow".to_string(),
            title: "Indexing flow".to_string(),
            graph: GraphResponse {
                center_id: entrypoint.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("call-index".to_string()),
                    source: entrypoint.node_id.clone(),
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
        let mut plan = build_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget(),
        );
        assert!(plan.claim_obligations.iter().any(|obligation| {
            obligation.id == "indexing_entrypoint"
                && obligation.proof_status == PacketObligationProofStatusDto::Proven
        }));
        let storage_obligation = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == "indexing_storage")
            .expect("indexing storage obligation");
        assert_eq!(
            storage_obligation.proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert_eq!(
            storage_obligation.reason.as_deref(),
            Some("carrier_not_sufficiency_eligible")
        );

        let mut claims = vec![PacketClaimDto {
            claim: "Indexing entrypoint and storage evidence describe the refresh flow."
                .to_string(),
            required_obligation_ids: vec![
                "indexing_entrypoint".to_string(),
                "indexing_storage".to_string(),
            ],
            required_obligation_kinds: Vec::new(),
            proof_status: Some(PacketProofStatusDto::Proven),
            required_evidence_role: None,
            citations: vec![entrypoint, storage],
            coverage_role: Some("flow template".to_string()),
            eligible_for_sufficiency: Some(true),
        }];
        bind_claims_to_packet_obligations(&plan, &mut claims);

        assert_eq!(claims[0].proof_status, Some(PacketProofStatusDto::Reported));
        assert_eq!(claims[0].eligible_for_sufficiency, Some(false));
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
    fn real_call_edge_proves_required_behavioral_obligation() {
        let mut entrypoint = citation(
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        entrypoint.evidence_edge_ids = vec![EdgeId("call-index".to_string())];
        let target = citation(
            "Indexer::build",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        let mut answer = answer(vec![entrypoint.clone(), target.clone()]);
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "indexing-flow".to_string(),
            title: "Indexing flow".to_string(),
            graph: GraphResponse {
                center_id: entrypoint.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("call-index".to_string()),
                    source: entrypoint.node_id,
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
        let mut plan = indexing_entrypoint_plan();
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget(),
        );

        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Proven
        );
        assert_eq!(plan.claim_obligations[0].reason, None);
    }

    #[test]
    fn pre_budget_edge_receipt_cannot_prove_an_edge_missing_from_the_packet() {
        let mut answer = answer_with_call_edge(
            INDEXING_QUESTION,
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
        );
        let mut plan = indexing_entrypoint_plan();
        let snapshot = capture_packet_obligation_edge_proofs_before_budget(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &plan,
            &answer,
        );
        let receipt = snapshot
            .entries
            .iter()
            .map(|entry| entry.proof.clone())
            .collect::<Vec<_>>();
        assert_eq!(receipt.len(), 1);
        assert_eq!(
            receipt[0].carrier_node_id,
            NodeId("BuildIndex::run".to_string())
        );
        assert_eq!(receipt[0].edge_id, EdgeId("requested-call".to_string()));
        assert_eq!(receipt[0].edge_kind, EdgeKind::CALL);
        assert_eq!(
            protected_packet_obligation_carrier_node_ids(&snapshot),
            &[NodeId("BuildIndex::run".to_string())]
        );
        assert_eq!(
            protected_packet_obligation_edge_ids(&snapshot),
            &[EdgeId("requested-call".to_string())]
        );

        // A pre-cap snapshot is reservation input, not a substitute for serialized proof.
        answer.citations[0].evidence_edge_ids.clear();
        let GraphArtifactDto::Uml { graph, .. } = &mut answer.graphs[0] else {
            panic!("fixture must carry a UML graph");
        };
        graph.edges.clear();
        graph.truncated = true;
        graph.omitted_edge_count = 1;
        let mut truncated_budget = budget();
        truncated_budget.truncated = true;
        truncated_budget.omitted_sections = vec!["trail_edges".to_string()];

        install_retained_packet_obligation_edge_proofs(
            &mut plan,
            &answer,
            &truncated_budget,
            &snapshot,
            16,
        );
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &truncated_budget,
        );
        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert!(plan.claim_obligations[0].carrier_edge_proofs.is_empty());
        assert_eq!(
            plan.claim_obligations[0].reason.as_deref(),
            Some(PACKET_BUDGET_TRUNCATED_REASON)
        );

        // Rebuilding cannot resurrect the discarded receipt.
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &truncated_budget,
        );
        assert!(plan.claim_obligations[0].carrier_edge_proofs.is_empty());
    }

    #[test]
    fn hybrid_role_carriers_cannot_bypass_any_declared_target() {
        const SERVER_QUESTION: &str = "Trace how an HTTP server routes an incoming request through route registration, request handler dispatch, and response finalization.";
        const CLIENT_QUESTION: &str = "Explain how an HTTP client session accepts a request, dispatches it through the session, selects a transport adapter, and calls the adapter send boundary.";
        const FULL_CLIENT_QUESTION: &str = "Explain how a top-level request call becomes a prepared request and sends it through a session adapter.";
        let cases = [
            FlowBoundaryCase {
                label: "server request entrypoint",
                question: SERVER_QUESTION,
                task_class: PacketTaskClassDto::RouteTracing,
                obligation_id: "request_entrypoint",
                carrier_name: "Router.use",
                carrier_path: "src/router.js",
                carrier_kind: NodeKind::METHOD,
                lawful_target: "Router.route",
                role_only_name: "Router.map",
                role_only_path: "src/router.js",
                role_only_kind: NodeKind::METHOD,
            },
            FlowBoundaryCase {
                label: "server request dispatch",
                question: SERVER_QUESTION,
                task_class: PacketTaskClassDto::RouteTracing,
                obligation_id: "request_dispatch",
                carrier_name: "Router.dispatch",
                carrier_path: "src/router.js",
                carrier_kind: NodeKind::METHOD,
                lawful_target: "finalhandler",
                role_only_name: "RequestDispatcher.execute",
                role_only_path: "src/dispatcher.js",
                role_only_kind: NodeKind::METHOD,
            },
            FlowBoundaryCase {
                label: "server response terminal",
                question: SERVER_QUESTION,
                task_class: PacketTaskClassDto::RouteTracing,
                obligation_id: "request_terminal",
                carrier_name: "Response.writeBuffer",
                carrier_path: "src/response.js",
                carrier_kind: NodeKind::METHOD,
                lawful_target: "Socket.end",
                role_only_name: "ResponseBuffer.read",
                role_only_path: "src/response.js",
                role_only_kind: NodeKind::METHOD,
            },
            FlowBoundaryCase {
                label: "client request entrypoint",
                question: CLIENT_QUESTION,
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                obligation_id: "request_entrypoint",
                carrier_name: "HttpClientFactory.request",
                carrier_path: "src/client.rs",
                carrier_kind: NodeKind::METHOD,
                lawful_target: "PreparedRequest.build",
                role_only_name: "createClient",
                role_only_path: "src/client.rs",
                role_only_kind: NodeKind::FUNCTION,
            },
            FlowBoundaryCase {
                label: "client public facade",
                question: FULL_CLIENT_QUESTION,
                task_class: PacketTaskClassDto::DataFlow,
                obligation_id: "client_public_facade",
                carrier_name: "createClient.request",
                carrier_path: "src/requests/api.py",
                carrier_kind: NodeKind::FUNCTION,
                lawful_target: "Session.request",
                role_only_name: "createClient",
                role_only_path: "src/requests/api.py",
                role_only_kind: NodeKind::FUNCTION,
            },
        ];

        for case in cases {
            let (status, reason, protected_carriers, protected_edges) =
                evaluate_flow_boundary(case, "Metrics.record", true);
            assert_eq!(
                status,
                PacketObligationProofStatusDto::Reported,
                "{}: a resolved, certain wrong-target CALL must not prove the boundary",
                case.label
            );
            assert_eq!(
                reason.as_deref(),
                Some("required_evidence_edge_missing"),
                "{}",
                case.label
            );
            assert!(
                protected_carriers.is_empty() && protected_edges.is_empty(),
                "{}: prebudget protection admitted a wrong-target carrier",
                case.label
            );

            let (status, reason, protected_carriers, protected_edges) =
                evaluate_flow_boundary(case, case.lawful_target, true);
            assert_eq!(
                status,
                PacketObligationProofStatusDto::Proven,
                "{}: lawful exact target rejected",
                case.label
            );
            assert_eq!(reason, None, "{}", case.label);
            assert_eq!(
                protected_carriers,
                [NodeId(case.carrier_name.to_string())],
                "{}: exact carrier was not reserved",
                case.label
            );
            assert_eq!(
                protected_edges,
                [EdgeId(format!("{}-call", case.obligation_id))],
                "{}: exact edge was not reserved",
                case.label
            );

            let role_only = FlowBoundaryCase {
                carrier_name: case.role_only_name,
                carrier_path: case.role_only_path,
                carrier_kind: case.role_only_kind,
                ..case
            };
            let (status, reason, protected_carriers, protected_edges) =
                evaluate_flow_boundary(role_only, "Worker.run", true);
            assert_eq!(
                status,
                PacketObligationProofStatusDto::Proven,
                "{}: role-only witness lost the ordinary cited-CALL contract",
                case.label
            );
            assert_eq!(reason, None, "{}", case.label);
            assert_eq!(
                protected_carriers,
                [NodeId(case.role_only_name.to_string())],
                "{}: role-only carrier was not reserved",
                case.label
            );
            assert_eq!(
                protected_edges,
                [EdgeId(format!("{}-call", case.obligation_id))],
                "{}: role-only edge was not reserved",
                case.label
            );
        }
    }

    #[test]
    fn requests_public_facade_requires_the_exact_outgoing_session_call() {
        let case = FlowBoundaryCase {
            label: "Requests public facade",
            question: "Explain how a top-level request call becomes a prepared request and sends it through a session adapter.",
            task_class: PacketTaskClassDto::DataFlow,
            obligation_id: "client_public_facade",
            carrier_name: "request",
            carrier_path: "src/requests/api.py",
            carrier_kind: NodeKind::FUNCTION,
            lawful_target: "Session.request",
            role_only_name: "createClient",
            role_only_path: "src/requests/api.py",
            role_only_kind: NodeKind::FUNCTION,
        };

        let (status, reason, protected_carriers, protected_edges) =
            evaluate_flow_boundary(case, case.lawful_target, true);
        assert_eq!(status, PacketObligationProofStatusDto::Proven);
        assert_eq!(reason, None);
        assert_eq!(protected_carriers, [NodeId("request".to_string())]);
        assert_eq!(
            protected_edges,
            [EdgeId("client_public_facade-call".to_string())]
        );

        for (target, outgoing) in [("Metrics.request", true), ("Session.request", false)] {
            let (status, reason, protected_carriers, protected_edges) =
                evaluate_flow_boundary(case, target, outgoing);
            assert_eq!(
                status,
                PacketObligationProofStatusDto::Reported,
                "target={target} outgoing={outgoing}"
            );
            assert_eq!(reason.as_deref(), Some("required_evidence_edge_missing"));
            assert!(protected_carriers.is_empty());
            assert!(protected_edges.is_empty());
        }
    }

    #[test]
    fn express_server_carriers_require_the_exact_outgoing_boundary() {
        const QUESTION: &str = "Trace how an HTTP server routes an incoming request through route registration, request handler dispatch, and response finalization.";
        let cases = [
            FlowBoundaryCase {
                label: "app.use",
                question: QUESTION,
                task_class: PacketTaskClassDto::RouteTracing,
                obligation_id: "request_entrypoint",
                carrier_name: "app.use",
                carrier_path: "lib/application.js",
                carrier_kind: NodeKind::METHOD,
                lawful_target: "Router.use",
                role_only_name: "Router.map",
                role_only_path: "lib/router.js",
                role_only_kind: NodeKind::METHOD,
            },
            FlowBoundaryCase {
                label: "app.handle",
                question: QUESTION,
                task_class: PacketTaskClassDto::RouteTracing,
                obligation_id: "request_dispatch",
                carrier_name: "app.handle",
                carrier_path: "lib/application.js",
                carrier_kind: NodeKind::METHOD,
                lawful_target: "finalhandler",
                role_only_name: "RequestDispatcher.execute",
                role_only_path: "lib/router.js",
                role_only_kind: NodeKind::METHOD,
            },
            FlowBoundaryCase {
                label: "res.send",
                question: QUESTION,
                task_class: PacketTaskClassDto::RouteTracing,
                obligation_id: "request_terminal",
                carrier_name: "res.send",
                carrier_path: "lib/response.js",
                carrier_kind: NodeKind::METHOD,
                lawful_target: "res.end",
                role_only_name: "ResponseBuffer.read",
                role_only_path: "lib/response.js",
                role_only_kind: NodeKind::METHOD,
            },
        ];

        for case in cases {
            let (status, reason, protected_carriers, protected_edges) =
                evaluate_flow_boundary(case, case.lawful_target, true);
            assert_eq!(
                status,
                PacketObligationProofStatusDto::Proven,
                "{}",
                case.label
            );
            assert_eq!(reason, None, "{}", case.label);
            assert_eq!(protected_carriers, [NodeId(case.carrier_name.to_string())]);
            assert_eq!(
                protected_edges,
                [EdgeId(format!("{}-call", case.obligation_id))]
            );

            for (target, outgoing) in [("Metrics.record", true), (case.lawful_target, false)] {
                let (status, reason, protected_carriers, protected_edges) =
                    evaluate_flow_boundary(case, target, outgoing);
                assert_eq!(
                    status,
                    PacketObligationProofStatusDto::Reported,
                    "{} target={target} outgoing={outgoing}",
                    case.label
                );
                assert_eq!(
                    reason.as_deref(),
                    Some("required_evidence_edge_missing"),
                    "{}",
                    case.label
                );
                assert!(protected_carriers.is_empty(), "{}", case.label);
                assert!(protected_edges.is_empty(), "{}", case.label);
            }
        }
    }

    #[test]
    fn asymmetric_owner_pairs_prove_only_their_declared_boundaries() {
        const SERVER_QUESTION: &str = "Trace how an HTTP server routes an incoming request through route registration, request handler dispatch, and response finalization.";
        const CLIENT_QUESTION: &str = "Explain how an HTTP client session accepts a request, dispatches it through the session, selects a transport adapter, and calls the adapter send boundary.";
        let cases = [
            FlowBoundaryCase {
                label: "app.listen to Server.listen",
                question: SERVER_QUESTION,
                task_class: PacketTaskClassDto::RouteTracing,
                obligation_id: "request_entrypoint",
                carrier_name: "app.listen",
                carrier_path: "lib/application.js",
                carrier_kind: NodeKind::METHOD,
                lawful_target: "Server.listen",
                role_only_name: "Router.map",
                role_only_path: "lib/router.js",
                role_only_kind: NodeKind::METHOD,
            },
            FlowBoundaryCase {
                label: "Controller.dispatch to Controller.handle",
                question: SERVER_QUESTION,
                task_class: PacketTaskClassDto::RouteTracing,
                obligation_id: "request_dispatch",
                carrier_name: "Controller.dispatch",
                carrier_path: "lib/controller.js",
                carrier_kind: NodeKind::METHOD,
                lawful_target: "Controller.handle",
                role_only_name: "RequestDispatcher.execute",
                role_only_path: "lib/router.js",
                role_only_kind: NodeKind::METHOD,
            },
            FlowBoundaryCase {
                label: "ResponseSender.send to ResponseSender.finish",
                question: SERVER_QUESTION,
                task_class: PacketTaskClassDto::RouteTracing,
                obligation_id: "request_terminal",
                carrier_name: "ResponseSender.send",
                carrier_path: "lib/response.js",
                carrier_kind: NodeKind::METHOD,
                lawful_target: "ResponseSender.finish",
                role_only_name: "ResponseBuffer.read",
                role_only_path: "lib/response.js",
                role_only_kind: NodeKind::METHOD,
            },
            FlowBoundaryCase {
                label: "HttpClient.request to PreparedRequest.build",
                question: CLIENT_QUESTION,
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                obligation_id: "request_entrypoint",
                carrier_name: "HttpClient.request",
                carrier_path: "src/client.rs",
                carrier_kind: NodeKind::METHOD,
                lawful_target: "PreparedRequest.build",
                role_only_name: "createClient",
                role_only_path: "src/client.rs",
                role_only_kind: NodeKind::FUNCTION,
            },
        ];

        for case in cases {
            let (status, reason, protected_carriers, protected_edges) =
                evaluate_flow_boundary(case, case.lawful_target, true);
            assert_eq!(
                status,
                PacketObligationProofStatusDto::Proven,
                "{}",
                case.label
            );
            assert_eq!(reason, None, "{}", case.label);
            assert_eq!(protected_carriers, [NodeId(case.carrier_name.to_string())]);
            assert_eq!(
                protected_edges,
                [EdgeId(format!("{}-call", case.obligation_id))]
            );
        }

        let client_case = cases[3];
        let (status, reason, protected_carriers, protected_edges) =
            evaluate_flow_boundary(client_case, "Telemetry.prepareRequest", true);
        assert_eq!(status, PacketObligationProofStatusDto::Reported);
        assert_eq!(reason.as_deref(), Some("required_evidence_edge_missing"));
        assert!(protected_carriers.is_empty());
        assert!(protected_edges.is_empty());
    }

    #[test]
    fn tiny_unknown_dispatch_receipts_require_parser_receiver_proof() {
        let cases = [
            (
                "ownerless early return",
                raw_server_dispatch_answer("handle", NodeKind::UNKNOWN, None, None, None, true),
                false,
            ),
            (
                "missing callsite receiver",
                raw_server_dispatch_answer(
                    "handle",
                    NodeKind::UNKNOWN,
                    None,
                    None,
                    Some("lib/tiny.js:1|syntax:js-member-call"),
                    true,
                ),
                false,
            ),
            (
                "matching parser receiver",
                raw_server_dispatch_answer(
                    "handle",
                    NodeKind::UNKNOWN,
                    None,
                    None,
                    Some("lib/tiny.js:1|syntax:js-member-call|receiver-owner:app"),
                    true,
                ),
                true,
            ),
            (
                "certain but unresolved target",
                raw_server_dispatch_answer(
                    "handle",
                    NodeKind::UNKNOWN,
                    Some("certain"),
                    None,
                    Some("lib/tiny.js:1|syntax:js-member-call|receiver-owner:app"),
                    true,
                ),
                false,
            ),
            (
                "confidence-only wrong receiver",
                raw_server_dispatch_answer(
                    "handle",
                    NodeKind::UNKNOWN,
                    None,
                    Some(1.0),
                    Some("lib/tiny.js:1|syntax:js-member-call|receiver-owner:telemetry"),
                    true,
                ),
                false,
            ),
            (
                "incoming syntax call",
                raw_server_dispatch_answer(
                    "handle",
                    NodeKind::UNKNOWN,
                    None,
                    None,
                    Some("lib/tiny.js:1|syntax:js-member-call|receiver-owner:app"),
                    false,
                ),
                false,
            ),
        ];

        for (label, answer, expected_proven) in cases {
            let (status, reason, protected_carriers, protected_edges) =
                finalize_server_dispatch_answer(&answer);
            assert_eq!(
                status,
                if expected_proven {
                    PacketObligationProofStatusDto::Proven
                } else {
                    PacketObligationProofStatusDto::Reported
                },
                "{label}"
            );
            assert_eq!(reason.is_none(), expected_proven, "{label}");
            assert_eq!(
                !protected_carriers.is_empty(),
                expected_proven,
                "{label}: prebudget carrier reservation diverged"
            );
            assert_eq!(
                !protected_edges.is_empty(),
                expected_proven,
                "{label}: prebudget edge reservation diverged"
            );
        }
    }

    #[test]
    fn server_receipt_prose_comes_only_from_its_exact_retained_call() {
        let answer = raw_server_receipt_answer("res.send", "end", "res", "z-terminal");
        let (plan, claim) = finalized_server_receipt_claim(&answer, "request_terminal");
        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Proven
        );
        assert_eq!(
            claim.claim,
            "`res.send` sends output through the retained `res.end` call, completing the response body."
        );

        for (label, mutate) in [
            (
                "citation edge removed",
                Box::new(|changed: &mut AgentAnswerDto| {
                    changed.citations[0].evidence_edge_ids.clear();
                }) as Box<dyn Fn(&mut AgentAnswerDto)>,
            ),
            (
                "receiver proof removed",
                Box::new(|changed: &mut AgentAnswerDto| {
                    let GraphArtifactDto::Uml { graph, .. } = &mut changed.graphs[0] else {
                        panic!("expected UML graph");
                    };
                    graph.edges[0].callsite_identity =
                        Some("lib/server.js:1|syntax:js-member-call".to_string());
                }),
            ),
            (
                "unknown edge gained confidence",
                Box::new(|changed: &mut AgentAnswerDto| {
                    let GraphArtifactDto::Uml { graph, .. } = &mut changed.graphs[0] else {
                        panic!("expected UML graph");
                    };
                    graph.edges[0].confidence = Some(1.0);
                }),
            ),
        ] {
            let mut changed = answer.clone();
            mutate(&mut changed);
            let semantic = proven_server_flow_receipt_text(
                &changed,
                &plan.claim_obligations[0],
                &changed.citations,
            );
            assert_eq!(semantic, None, "{label}");
            let (_, changed_claim) = finalized_server_receipt_claim(&changed, "request_terminal");
            assert!(
                !changed_claim.claim.contains("completing the response body"),
                "{label}: {}",
                changed_claim.claim
            );
        }

        let write = raw_server_receipt_answer("res.send", "write", "res", "write-terminal");
        let (_, write_claim) = finalized_server_receipt_claim(&write, "request_terminal");
        assert_eq!(
            write_claim.claim,
            "`res.send` writes response output through the retained `res.write` call."
        );
        assert!(!write_claim.claim.contains("completing"));
    }

    #[test]
    fn semantic_server_receipts_skip_earlier_role_only_proofs() {
        let mut answer =
            raw_server_receipt_answer("app.handle", "handle", "app.router", "z-dispatch");
        let role = citation(
            "RequestDispatcher.execute",
            "lib/dispatch.js",
            NodeKind::METHOD,
        );
        answer.citations.insert(0, role.clone());
        let (mut plan, _) = finalized_server_receipt_claim(&answer, "request_dispatch");
        let obligation = &mut plan.claim_obligations[0];
        obligation.carrier_node_ids.insert(0, role.node_id.clone());
        obligation.carrier_edge_proofs.insert(
            0,
            PacketObligationCarrierEdgeProofDto {
                carrier_node_id: role.node_id,
                edge_id: EdgeId("a-role-proof".to_string()),
                edge_kind: EdgeKind::CALL,
            },
        );

        let receipt = proven_server_flow_receipt_text(&answer, obligation, &answer.citations)
            .expect("later exact structural proof should produce semantic receipt");
        assert_eq!(
            receipt,
            "`app.handle` delegates request handling through the retained `app.router.handle` call."
        );
    }

    #[test]
    fn semantic_server_receipts_require_server_obligation_provenance() {
        const CLIENT_QUESTION: &str = "Explain how an HTTP client session accepts a request, dispatches it through the session, selects a transport adapter, and calls the adapter send boundary.";
        let mut answer =
            raw_server_receipt_answer("app.listen", "listen", "app.router", "client-overlap");
        answer.prompt = CLIENT_QUESTION.to_string();
        let mut plan = build_packet_obligation_plan(
            CLIENT_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        plan.claim_obligations
            .retain(|obligation| obligation.id == "request_entrypoint");
        let obligation = &mut plan.claim_obligations[0];
        obligation.proof_status = PacketObligationProofStatusDto::Proven;
        obligation.carrier_node_ids = vec![answer.citations[0].node_id.clone()];
        obligation.carrier_edge_proofs = vec![PacketObligationCarrierEdgeProofDto {
            carrier_node_id: answer.citations[0].node_id.clone(),
            edge_id: answer.citations[0].evidence_edge_ids[0].clone(),
            edge_kind: EdgeKind::CALL,
        }];

        assert_eq!(
            proven_server_flow_receipt_text(&answer, obligation, &answer.citations),
            None
        );
        let receipt = packet_obligation_receipt_text(&answer, obligation, &answer.citations);
        assert_eq!(
            receipt,
            "Material obligation `request_entrypoint` has independently cited carrier evidence at `app.listen`."
        );
    }

    #[test]
    fn semantic_server_receipts_prefer_resolved_targets_and_describe_listeners_truthfully() {
        let mut dispatch = raw_server_receipt_answer(
            "app.handle",
            "Router.handle",
            "stale.receiver",
            "resolved-dispatch",
        );
        let GraphArtifactDto::Uml { graph, .. } = &mut dispatch.graphs[0] else {
            panic!("expected UML graph");
        };
        graph.nodes[0].kind = NodeKind::METHOD;
        let (_, dispatch_claim) = finalized_server_receipt_claim(&dispatch, "request_dispatch");
        assert_eq!(
            dispatch_claim.claim,
            "`app.handle` delegates request handling through the retained `Router.handle` call."
        );
        assert!(!dispatch_claim.claim.contains("stale.receiver"));

        let mut listener = raw_server_receipt_answer(
            "app.listen()",
            "Server.listen",
            "stale.receiver",
            "resolved-listen",
        );
        let GraphArtifactDto::Uml { graph, .. } = &mut listener.graphs[0] else {
            panic!("expected UML graph");
        };
        graph.nodes[0].kind = NodeKind::METHOD;
        let (_, listener_claim) = finalized_server_receipt_claim(&listener, "request_entrypoint");
        assert_eq!(
            listener_claim.claim,
            "`app.listen()` starts the server listener through the retained `Server.listen` call."
        );
    }

    #[test]
    fn strict_dispatch_receipts_reject_uncertain_and_wrong_owner_targets() {
        let cases = [
            (
                "resolved certain exact target",
                raw_server_dispatch_answer(
                    "app.handle",
                    NodeKind::METHOD,
                    Some("certain"),
                    None,
                    Some("lib/tiny.js:1"),
                    true,
                ),
                true,
            ),
            (
                "concrete target without certainty",
                raw_server_dispatch_answer("app.handle", NodeKind::METHOD, None, None, None, true),
                true,
            ),
            (
                "probable concrete target",
                raw_server_dispatch_answer(
                    "app.handle",
                    NodeKind::METHOD,
                    Some("probable"),
                    None,
                    Some("lib/tiny.js:1"),
                    true,
                ),
                false,
            ),
            (
                "resolved same-action wrong owner",
                raw_server_dispatch_answer(
                    "Telemetry.handle",
                    NodeKind::METHOD,
                    Some("certain"),
                    Some(1.0),
                    Some("lib/tiny.js:1"),
                    true,
                ),
                false,
            ),
            (
                "resolved incoming exact target",
                raw_server_dispatch_answer(
                    "app.handle",
                    NodeKind::METHOD,
                    Some("certain"),
                    Some(1.0),
                    Some("lib/tiny.js:1"),
                    false,
                ),
                false,
            ),
        ];
        for (label, answer, expected_proven) in cases {
            let (status, reason, protected_carriers, protected_edges) =
                finalize_server_dispatch_answer(&answer);
            assert_eq!(
                status,
                if expected_proven {
                    PacketObligationProofStatusDto::Proven
                } else {
                    PacketObligationProofStatusDto::Reported
                },
                "{label}"
            );
            assert_eq!(reason.is_none(), expected_proven, "{label}");
            assert_eq!(!protected_carriers.is_empty(), expected_proven, "{label}");
            assert_eq!(!protected_edges.is_empty(), expected_proven, "{label}");
        }
    }

    #[test]
    fn role_only_incident_call_requires_resolved_metadata() {
        let terms = packet_probe_terms("Trace HTTP server request handler dispatch.");
        let requirement =
            packet_flow_requirements_for_terms(&terms, PacketTaskClassDto::RouteTracing)
                .into_iter()
                .find(|requirement| requirement.id == "request_dispatch")
                .expect("server dispatch requirement");
        let role_only = citation(
            "RequestDispatcher.execute",
            "src/dispatcher.js",
            NodeKind::METHOD,
        );
        assert!(
            requirement
                .evidence
                .citation_proves_without_call_boundary(&role_only)
        );
        for (label, neighbor_kind, certainty, expected) in [
            ("resolved certain", NodeKind::METHOD, Some("certain"), true),
            ("resolved implicit", NodeKind::METHOD, None, true),
            ("unknown", NodeKind::UNKNOWN, None, false),
            ("certain unknown", NodeKind::UNKNOWN, Some("certain"), false),
            ("probable", NodeKind::METHOD, Some("probable"), false),
        ] {
            let edge = GraphEdgeDto {
                id: EdgeId(label.to_string()),
                source: role_only.node_id.clone(),
                target: NodeId("Worker.run".to_string()),
                kind: EdgeKind::CALL,
                confidence: None,
                certainty: certainty.map(str::to_string),
                callsite_identity: Some("src/dispatcher.js:1".to_string()),
                candidate_targets: Vec::new(),
            };
            assert_eq!(
                flow_requirement_call_receipt_is_valid(
                    &requirement,
                    &role_only,
                    &edge,
                    "Worker.run",
                    neighbor_kind,
                ),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn ordered_boundary_rejects_probable_receipts() {
        let carrier = citation("HttpClient.send", "src/client.rs", NodeKind::METHOD);
        let terms = packet_probe_terms(
            "Explain how an HTTP client session accepts a request and dispatches it through an adapter.",
        );
        let requirement = packet_flow_requirements_for_terms(&terms, PacketTaskClassDto::DataFlow)
            .into_iter()
            .find(|requirement| {
                requirement.id == "request_dispatch"
                    && requirement
                        .evidence
                        .ordered_call_boundary(&carrier)
                        .is_some()
            })
            .expect("ordered client dispatch requirement");
        let edge = GraphEdgeDto {
            id: EdgeId("ordered-probable".to_string()),
            source: carrier.node_id.clone(),
            target: NodeId("Session.get_adapter".to_string()),
            kind: EdgeKind::CALL,
            confidence: None,
            certainty: Some("probable".to_string()),
            callsite_identity: Some("src/client.rs:1".to_string()),
            candidate_targets: Vec::new(),
        };
        assert!(
            requirement
                .evidence
                .ordered_call_boundary(&carrier)
                .is_some()
        );
        assert!(!flow_requirement_call_receipt_is_valid(
            &requirement,
            &carrier,
            &edge,
            "Session.get_adapter",
            NodeKind::METHOD,
        ));
    }

    #[test]
    fn client_request_obligations_protect_distinct_lawful_carriers_before_capping() {
        let question = "Explain how an HTTP client session accepts a request, dispatches it through the session, selects a transport adapter, and calls the adapter send boundary.";
        let mut request = citation("HttpClient.request", "src/client.rs", NodeKind::METHOD);
        request.evidence_edge_ids = vec![EdgeId("request-call".to_string())];
        let mut send = citation("HttpClient.send", "src/client.rs", NodeKind::METHOD);
        send.evidence_edge_ids = vec![EdgeId("send-call".to_string())];
        let selector = citation(
            "HttpClient.selectAdapter",
            "src/client.rs",
            NodeKind::METHOD,
        );
        let transport = citation(
            "HttpTransportAdapter.send",
            "src/transport.rs",
            NodeKind::METHOD,
        );
        let prepared = citation("PreparedRequest.build", "src/model.rs", NodeKind::METHOD);
        let mut carried_answer = answer(vec![
            request.clone(),
            send.clone(),
            selector.clone(),
            transport.clone(),
            prepared.clone(),
        ]);
        carried_answer.prompt = question.to_string();
        carried_answer.graphs.push(GraphArtifactDto::Uml {
            id: "client-request-flow".to_string(),
            title: "Client request flow".to_string(),
            graph: GraphResponse {
                center_id: request.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![
                    GraphEdgeDto {
                        id: EdgeId("request-call".to_string()),
                        source: request.node_id.clone(),
                        target: prepared.node_id,
                        kind: EdgeKind::CALL,
                        confidence: Some(1.0),
                        certainty: Some("certain".to_string()),
                        callsite_identity: Some("test:request".to_string()),
                        candidate_targets: Vec::new(),
                    },
                    GraphEdgeDto {
                        id: EdgeId("send-call".to_string()),
                        source: send.node_id.clone(),
                        target: selector.node_id.clone(),
                        kind: EdgeKind::CALL,
                        confidence: Some(1.0),
                        certainty: Some("certain".to_string()),
                        callsite_identity: Some("test:send".to_string()),
                        candidate_targets: Vec::new(),
                    },
                ],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        });
        let mut plan = build_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        plan.claim_obligations.retain(|obligation| {
            matches!(
                obligation.id.as_str(),
                "request_entrypoint"
                    | "request_dispatch"
                    | "request_terminal"
                    | "client_transport_send"
            )
        });
        plan.query_obligations.clear();
        let material_ids = plan
            .claim_obligations
            .iter()
            .filter(|obligation| obligation.material)
            .map(|obligation| obligation.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            material_ids,
            [
                "request_entrypoint",
                "request_dispatch",
                "request_terminal",
                "client_transport_send"
            ]
        );

        let snapshot = capture_packet_obligation_edge_proofs_before_budget(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &plan,
            &carried_answer,
        );
        assert_eq!(
            protected_packet_obligation_carrier_node_ids(&snapshot),
            &[
                request.node_id,
                send.node_id,
                selector.node_id,
                transport.node_id,
            ]
        );
        assert_eq!(
            protected_packet_obligation_edge_ids(&snapshot),
            &[
                EdgeId("request-call".to_string()),
                EdgeId("send-call".to_string()),
            ]
        );

        let mut wrong_target_answer = carried_answer.clone();
        wrong_target_answer
            .citations
            .iter_mut()
            .find(|citation| citation.display_name == "PreparedRequest.build")
            .expect("request edge target")
            .display_name = "PluginRegistry.registerPlugin".to_string();
        let wrong_target_snapshot = capture_packet_obligation_edge_proofs_before_budget(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &plan,
            &wrong_target_answer,
        );
        assert!(
            !protected_packet_obligation_edge_ids(&wrong_target_snapshot)
                .contains(&EdgeId("request-call".to_string())),
            "matching carrier morphology cannot consume an unrelated outgoing call"
        );
    }

    #[test]
    fn client_request_dispatch_rejects_an_unrelated_incident_call() {
        let question = "Explain how an HTTP client session accepts a request, dispatches it through the session, selects a transport adapter, and calls the adapter send boundary.";
        let mut send = citation("HttpClient.send", "src/client.rs", NodeKind::METHOD);
        send.evidence_edge_ids = vec![
            EdgeId("logging-call".to_string()),
            EdgeId("metrics-call".to_string()),
            EdgeId("adapter-metrics-call".to_string()),
        ];
        let logger = citation("Logger.record", "src/logging.rs", NodeKind::METHOD);
        let metrics = citation(
            "ClientRequestMetrics.record",
            "src/metrics.rs",
            NodeKind::METHOD,
        );
        let adapter_metrics = citation(
            "AdapterResolveMetrics.record",
            "src/metrics.rs",
            NodeKind::METHOD,
        );
        let mut carried_answer = answer(vec![
            send.clone(),
            logger.clone(),
            metrics.clone(),
            adapter_metrics.clone(),
        ]);
        carried_answer.prompt = question.to_string();
        carried_answer.graphs.push(GraphArtifactDto::Uml {
            id: "client-dispatch-logging".to_string(),
            title: "Unrelated incident call".to_string(),
            graph: GraphResponse {
                center_id: send.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![
                    GraphEdgeDto {
                        id: EdgeId("logging-call".to_string()),
                        source: send.node_id.clone(),
                        target: logger.node_id.clone(),
                        kind: EdgeKind::CALL,
                        confidence: Some(1.0),
                        certainty: Some("certain".to_string()),
                        callsite_identity: Some("test:logging".to_string()),
                        candidate_targets: Vec::new(),
                    },
                    GraphEdgeDto {
                        id: EdgeId("metrics-call".to_string()),
                        source: metrics.node_id,
                        target: send.node_id.clone(),
                        kind: EdgeKind::CALL,
                        confidence: Some(1.0),
                        certainty: Some("certain".to_string()),
                        callsite_identity: Some("test:metrics".to_string()),
                        candidate_targets: Vec::new(),
                    },
                    GraphEdgeDto {
                        id: EdgeId("adapter-metrics-call".to_string()),
                        source: send.node_id.clone(),
                        target: adapter_metrics.node_id,
                        kind: EdgeKind::CALL,
                        confidence: Some(1.0),
                        certainty: Some("certain".to_string()),
                        callsite_identity: Some("test:adapter-metrics".to_string()),
                        candidate_targets: Vec::new(),
                    },
                ],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        });
        let mut plan = build_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        plan.claim_obligations
            .retain(|obligation| obligation.id == "request_dispatch");
        plan.query_obligations.clear();

        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &carried_answer,
            &budget(),
        );

        assert_eq!(plan.claim_obligations.len(), 1);
        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert_eq!(
            plan.claim_obligations[0].reason.as_deref(),
            Some("required_evidence_edge_missing")
        );
        assert!(plan.claim_obligations[0].carrier_edge_proofs.is_empty());

        let request = citation("HttpClient.request", "src/client.rs", NodeKind::METHOD);
        let mut incoming_send = send;
        incoming_send.evidence_edge_ids = vec![EdgeId("request-to-send".to_string())];
        let mut incoming_answer = answer(vec![request.clone(), incoming_send.clone()]);
        incoming_answer.prompt = question.to_string();
        incoming_answer.graphs.push(GraphArtifactDto::Uml {
            id: "client-dispatch-incoming".to_string(),
            title: "Preceding request call".to_string(),
            graph: GraphResponse {
                center_id: incoming_send.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("request-to-send".to_string()),
                    source: request.node_id,
                    target: incoming_send.node_id,
                    kind: EdgeKind::CALL,
                    confidence: Some(1.0),
                    certainty: Some("certain".to_string()),
                    callsite_identity: Some("test:request-to-send".to_string()),
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        });
        let mut incoming_plan = build_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        incoming_plan
            .claim_obligations
            .retain(|obligation| obligation.id == "request_dispatch");
        incoming_plan.query_obligations.clear();
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut incoming_plan,
            &incoming_answer,
            &budget(),
        );
        assert_eq!(
            incoming_plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Proven
        );
        assert_eq!(
            incoming_plan.claim_obligations[0]
                .carrier_edge_proofs
                .iter()
                .map(|proof| proof.edge_id.clone())
                .collect::<Vec<_>>(),
            [EdgeId("request-to-send".to_string())]
        );
    }

    #[test]
    fn post_cap_receipts_require_the_actual_bounded_edge() {
        let mut answer = answer_with_call_edge(
            INDEXING_QUESTION,
            "ZIndex::run",
            "crates/example/src/z_index.rs",
        );
        let z_carrier = answer.citations[0].clone();
        let mut a_carrier = citation(
            "AIndex::run",
            "crates/example/src/a_index.rs",
            NodeKind::METHOD,
        );
        a_carrier.evidence_edge_ids = vec![EdgeId("a-call".to_string())];
        answer.citations = vec![z_carrier.clone(), z_carrier.clone(), a_carrier.clone()];
        let GraphArtifactDto::Uml { graph, .. } = &mut answer.graphs[0] else {
            panic!("fixture must carry a UML graph");
        };
        graph.edges.push(GraphEdgeDto {
            id: EdgeId("a-call".to_string()),
            source: a_carrier.node_id.clone(),
            target: NodeId("Worker::run".to_string()),
            kind: EdgeKind::CALL,
            confidence: Some(1.0),
            certainty: Some("certain".to_string()),
            callsite_identity: Some("test:2".to_string()),
            candidate_targets: Vec::new(),
        });
        let mut plan = indexing_entrypoint_plan();
        let snapshot = capture_packet_obligation_edge_proofs_before_budget(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &plan,
            &answer,
        );
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| { entry.proof.carrier_node_id == NodeId("ZIndex::run".to_string()) })
        );
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| { entry.proof.carrier_node_id == NodeId("AIndex::run".to_string()) })
        );

        // Model the actual post-citation-cap selection: duplicates of Z are dropped and A is the
        // only retained lawful carrier. The graph cap then omits A's exact pre-cap CALL edge.
        answer.citations = vec![a_carrier];
        let GraphArtifactDto::Uml { graph, .. } = &mut answer.graphs[0] else {
            panic!("fixture must carry a UML graph");
        };
        graph.edges.clear();
        graph.truncated = true;
        graph.omitted_edge_count = 2;
        let mut truncated_budget = budget();
        truncated_budget.truncated = true;
        truncated_budget.omitted_sections = vec!["trail_edges".to_string()];
        install_retained_packet_obligation_edge_proofs(
            &mut plan,
            &answer,
            &truncated_budget,
            &snapshot,
            2,
        );
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &truncated_budget,
        );
        assert_eq!(
            plan.claim_obligations[0].carrier_node_ids,
            vec![NodeId("AIndex::run".to_string())]
        );
        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert!(plan.claim_obligations[0].carrier_edge_proofs.is_empty());
    }

    #[test]
    fn omitted_unrelated_edges_do_not_mint_an_entrypoint_proof() {
        let entrypoint = citation(
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        let mut unrelated = citation(
            "Unrelated::start",
            "crates/example/src/unrelated.rs",
            NodeKind::METHOD,
        );
        unrelated.evidence_edge_ids = vec![EdgeId("unrelated-call".to_string())];
        let target = citation(
            "Unrelated::finish",
            "crates/example/src/unrelated.rs",
            NodeKind::METHOD,
        );
        let mut answer = answer(vec![entrypoint, unrelated.clone(), target.clone()]);
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "unrelated-flow".to_string(),
            title: "Unrelated flow".to_string(),
            graph: GraphResponse {
                center_id: unrelated.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("unrelated-call".to_string()),
                    source: unrelated.node_id,
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
        let mut plan = indexing_entrypoint_plan();
        let snapshot = capture_packet_obligation_edge_proofs_before_budget(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &plan,
            &answer,
        );
        assert!(snapshot.entries.is_empty());

        let GraphArtifactDto::Uml { graph, .. } = &mut answer.graphs[0] else {
            panic!("fixture must carry a UML graph");
        };
        graph.edges.clear();
        graph.truncated = true;
        graph.omitted_edge_count = 1;
        let mut truncated_budget = budget();
        truncated_budget.truncated = true;
        truncated_budget.omitted_sections = vec!["trail_edges".to_string()];
        install_retained_packet_obligation_edge_proofs(
            &mut plan,
            &answer,
            &truncated_budget,
            &snapshot,
            16,
        );
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &truncated_budget,
        );

        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert!(plan.claim_obligations[0].carrier_edge_proofs.is_empty());
    }

    #[test]
    fn prior_edge_receipt_rejects_missing_wrong_kind_or_changed_carrier() {
        let answer = answer_with_call_edge(
            INDEXING_QUESTION,
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
        );
        let base_plan = indexing_entrypoint_plan();
        let snapshot = capture_packet_obligation_edge_proofs_before_budget(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &base_plan,
            &answer,
        );
        assert_eq!(snapshot.entries.len(), 1);

        let mut truncated_budget = budget();
        truncated_budget.truncated = true;
        truncated_budget.omitted_sections =
            vec!["citations".to_string(), "trail_edges".to_string()];

        let mut missing_carrier_answer = answer.clone();
        missing_carrier_answer.citations.clear();
        let GraphArtifactDto::Uml { graph, .. } = &mut missing_carrier_answer.graphs[0] else {
            panic!("fixture must carry a UML graph");
        };
        graph.edges.clear();
        let mut missing_carrier_plan = base_plan.clone();
        install_retained_packet_obligation_edge_proofs(
            &mut missing_carrier_plan,
            &missing_carrier_answer,
            &truncated_budget,
            &snapshot,
            16,
        );
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut missing_carrier_plan,
            &missing_carrier_answer,
            &truncated_budget,
        );
        assert_eq!(
            missing_carrier_plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert!(
            missing_carrier_plan.claim_obligations[0]
                .carrier_edge_proofs
                .is_empty()
        );

        let mut wrong_kind_answer = answer.clone();
        wrong_kind_answer.citations[0].kind = NodeKind::STRUCT;
        let GraphArtifactDto::Uml { graph, .. } = &mut wrong_kind_answer.graphs[0] else {
            panic!("fixture must carry a UML graph");
        };
        graph.edges.clear();
        let mut wrong_kind_plan = base_plan.clone();
        install_retained_packet_obligation_edge_proofs(
            &mut wrong_kind_plan,
            &wrong_kind_answer,
            &truncated_budget,
            &snapshot,
            16,
        );
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut wrong_kind_plan,
            &wrong_kind_answer,
            &truncated_budget,
        );
        assert_eq!(
            wrong_kind_plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert!(
            wrong_kind_plan.claim_obligations[0]
                .carrier_edge_proofs
                .is_empty()
        );

        let mut changed_edge_answer = answer.clone();
        let GraphArtifactDto::Uml { graph, .. } = &mut changed_edge_answer.graphs[0] else {
            panic!("fixture must carry a UML graph");
        };
        graph.edges[0].kind = EdgeKind::INHERITANCE;
        let mut changed_edge_plan = base_plan.clone();
        install_retained_packet_obligation_edge_proofs(
            &mut changed_edge_plan,
            &changed_edge_answer,
            &truncated_budget,
            &snapshot,
            16,
        );
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut changed_edge_plan,
            &changed_edge_answer,
            &truncated_budget,
        );
        assert_eq!(
            changed_edge_plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert!(
            changed_edge_plan.claim_obligations[0]
                .carrier_edge_proofs
                .is_empty()
        );

        let mut speculative_edge_answer = answer;
        let GraphArtifactDto::Uml { graph, .. } = &mut speculative_edge_answer.graphs[0] else {
            panic!("fixture must carry a UML graph");
        };
        graph.edges[0].certainty = Some("speculative".to_string());
        let mut speculative_edge_plan = base_plan;
        install_retained_packet_obligation_edge_proofs(
            &mut speculative_edge_plan,
            &speculative_edge_answer,
            &truncated_budget,
            &snapshot,
            16,
        );
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut speculative_edge_plan,
            &speculative_edge_answer,
            &truncated_budget,
        );
        assert_eq!(
            speculative_edge_plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert!(
            speculative_edge_plan.claim_obligations[0]
                .carrier_edge_proofs
                .is_empty()
        );
    }

    #[test]
    fn incident_call_edge_without_explicit_citation_edge_id_is_reported() {
        let entrypoint = citation(
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        let target = citation(
            "Indexer::build",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        let mut answer = answer(vec![entrypoint.clone(), target.clone()]);
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "indexing-flow".to_string(),
            title: "Indexing flow".to_string(),
            graph: GraphResponse {
                center_id: entrypoint.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("uncited-call".to_string()),
                    source: entrypoint.node_id,
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
        let mut plan = indexing_entrypoint_plan();
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget(),
        );

        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert_eq!(
            plan.claim_obligations[0].reason.as_deref(),
            Some("required_evidence_edge_missing")
        );
    }

    #[test]
    fn unrelated_explicit_call_edge_cannot_prove_a_carrier() {
        let mut entrypoint = citation(
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        entrypoint.evidence_edge_ids = vec![EdgeId("unrelated-call".to_string())];
        let unrelated_source = citation(
            "Unrelated::start",
            "crates/example/src/unrelated.rs",
            NodeKind::METHOD,
        );
        let unrelated_target = citation(
            "Unrelated::finish",
            "crates/example/src/unrelated.rs",
            NodeKind::METHOD,
        );
        let mut answer = answer(vec![
            entrypoint,
            unrelated_source.clone(),
            unrelated_target.clone(),
        ]);
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "unrelated-flow".to_string(),
            title: "Unrelated flow".to_string(),
            graph: GraphResponse {
                center_id: unrelated_source.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("unrelated-call".to_string()),
                    source: unrelated_source.node_id,
                    target: unrelated_target.node_id,
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
        let mut plan = indexing_entrypoint_plan();
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget(),
        );

        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert_eq!(
            plan.claim_obligations[0].reason.as_deref(),
            Some("required_evidence_edge_missing")
        );
    }

    #[test]
    fn proven_obligation_excludes_false_carrier_paths_from_avoid_opening() {
        let mut entrypoint = citation(
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        entrypoint.evidence_edge_ids = vec![EdgeId("call-index".to_string())];
        let target = citation(
            "Indexer::build",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        let false_carrier = citation(
            "CliErrorBody",
            "crates/example/src/cli/errors.rs",
            NodeKind::STRUCT,
        );
        let mut answer = answer(vec![entrypoint.clone(), target.clone(), false_carrier]);
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "indexing-flow".to_string(),
            title: "Indexing flow".to_string(),
            graph: GraphResponse {
                center_id: entrypoint.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("call-index".to_string()),
                    source: entrypoint.node_id,
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
        let mut plan = indexing_entrypoint_plan();
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget(),
        );

        assert_eq!(
            packet_proven_obligation_carrier_paths(&plan),
            vec!["crates/example/src/indexer.rs".to_string()]
        );
    }

    #[test]
    fn absent_query_receipt_is_cancelled_and_blocks_material_completion() {
        let mut plan = build_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer(Vec::new()),
            &budget(),
        );

        assert!(plan.query_obligations.iter().any(|obligation| {
            obligation.material
                && matches!(
                    obligation.completion,
                    Some(PacketQueryCompletionDto::Cancelled { ref reason })
                        if reason == "not_dispatched"
                )
        }));
        assert!(!material_packet_obligations_are_proven(&plan));
    }

    #[test]
    fn cancelled_required_query_receipt_survives_with_its_typed_reason() {
        let mut plan = build_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        let query = plan
            .query_obligations
            .iter()
            .find(|obligation| obligation.material)
            .expect("material query obligation")
            .query
            .clone();
        let mut answer = answer(Vec::new());
        answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .push(query_diagnostic(
                &query,
                PacketQueryCompletionDto::Cancelled {
                    reason: "stage_deadline".to_string(),
                },
            ));

        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget(),
        );

        let obligation = plan
            .query_obligations
            .iter()
            .find(|obligation| obligation.query == query)
            .expect("cancelled obligation remains in the result");
        assert_eq!(
            obligation.completion,
            Some(PacketQueryCompletionDto::Cancelled {
                reason: "stage_deadline".to_string()
            })
        );
        answer.retrieval_trace.packet_sidecar_diagnostics.clear();
        let mut truncated_budget = budget();
        truncated_budget.truncated = true;
        truncated_budget.omitted_sections = vec!["packet_payload".to_string()];
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &truncated_budget,
        );
        assert_eq!(
            plan.query_obligations
                .iter()
                .find(|obligation| obligation.query == query)
                .and_then(|obligation| obligation.completion.clone()),
            Some(PacketQueryCompletionDto::Cancelled {
                reason: "stage_deadline".to_string()
            }),
            "budget compaction must preserve the actual cancellation cause"
        );
        assert!(!material_packet_obligations_are_proven(&plan));
    }

    #[test]
    fn truncated_packet_retains_and_demotes_an_otherwise_proven_claim_obligation() {
        let mut entrypoint = citation(
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        entrypoint.evidence_edge_ids = vec![EdgeId("call-index".to_string())];
        let target = citation(
            "Indexer::build",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        let mut answer = answer(vec![entrypoint.clone(), target.clone()]);
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "indexing-flow".to_string(),
            title: "Indexing flow".to_string(),
            graph: GraphResponse {
                center_id: entrypoint.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("call-index".to_string()),
                    source: entrypoint.node_id,
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
        let mut plan = indexing_entrypoint_plan();
        let snapshot = capture_packet_obligation_edge_proofs_before_budget(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &plan,
            &answer,
        );
        answer.citations.clear();
        let mut truncated_budget = budget();
        truncated_budget.truncated = true;
        truncated_budget.omitted_sections = vec!["citations".to_string()];
        install_retained_packet_obligation_edge_proofs(
            &mut plan,
            &answer,
            &truncated_budget,
            &snapshot,
            16,
        );

        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &truncated_budget,
        );

        assert_eq!(plan.claim_obligations.len(), 1);
        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert_eq!(
            plan.claim_obligations[0].reason.as_deref(),
            Some(PACKET_BUDGET_TRUNCATED_REASON)
        );
        assert!(!material_packet_obligations_are_proven(&plan));
    }

    #[test]
    fn global_omission_flags_preserve_surviving_carrier_failure_reasons() {
        let mut truncated_budget = budget();
        truncated_budget.truncated = true;
        truncated_budget.omitted_sections =
            vec!["citations".to_string(), "trail_edges".to_string()];

        let mut missing_edge_plan = indexing_entrypoint_plan();
        let missing_edge_answer = answer(vec![citation(
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        )]);
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut missing_edge_plan,
            &missing_edge_answer,
            &truncated_budget,
        );
        assert_eq!(
            missing_edge_plan.claim_obligations[0].reason.as_deref(),
            Some("required_evidence_edge_missing"),
            "an unrelated global edge omission cannot relabel a surviving carrier"
        );

        let request_question = "Explain how an HTTP client session accepts a request, dispatches it through the session, selects a transport adapter, and calls the adapter send boundary.";
        let mut wrong_role_plan = build_packet_obligation_plan(
            request_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        wrong_role_plan
            .claim_obligations
            .retain(|obligation| obligation.id == "request_dispatch");
        wrong_role_plan.query_obligations.clear();
        let mut wrong_role_answer = answer(vec![citation(
            "dispatch_hook",
            "src/hooks.rs",
            NodeKind::METHOD,
        )]);
        wrong_role_answer.prompt = request_question.to_string();
        finalize_packet_obligation_plan(
            request_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut wrong_role_plan,
            &wrong_role_answer,
            &truncated_budget,
        );
        assert_eq!(
            wrong_role_plan.claim_obligations[0].reason.as_deref(),
            Some("carrier_does_not_satisfy_role_contract"),
            "a survived close carrier must retain its actual role failure"
        );
    }

    #[test]
    fn repeated_finalization_marks_the_exact_removed_edge_only() {
        let mut answer = answer_with_call_edge(
            INDEXING_QUESTION,
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
        );
        let mut plan = indexing_entrypoint_plan();
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget(),
        );
        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Proven
        );

        let GraphArtifactDto::Uml { graph, .. } = &mut answer.graphs[0] else {
            panic!("fixture must carry a UML graph");
        };
        graph.edges.clear();
        let mut truncated_budget = budget();
        truncated_budget.truncated = true;
        truncated_budget.omitted_sections = vec!["trail_edges".to_string()];
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &truncated_budget,
        );
        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert_eq!(
            plan.claim_obligations[0].reason.as_deref(),
            Some(PACKET_BUDGET_TRUNCATED_REASON)
        );
    }

    #[test]
    fn exact_budget_loss_survives_weaker_reported_citations() {
        let mut truncated_budget = budget();
        truncated_budget.truncated = true;
        truncated_budget.omitted_sections =
            vec!["citations".to_string(), "trail_edges".to_string()];

        let request_question = "Explain how an HTTP client session accepts a request, dispatches it through the session, selects a transport adapter, and calls the adapter send boundary.";
        let mut request_answer =
            answer_with_call_edge(request_question, "HttpClient.send", "src/client.rs");
        let selector = citation("HttpClient.get_adapter", "src/client.rs", NodeKind::METHOD);
        request_answer.citations[1] = selector.clone();
        let GraphArtifactDto::Uml { graph, .. } = &mut request_answer.graphs[0] else {
            panic!("fixture must carry a UML graph");
        };
        graph.edges[0].target = selector.node_id;
        let mut request_plan = build_packet_obligation_plan(
            request_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        request_plan
            .claim_obligations
            .retain(|obligation| obligation.id == "request_dispatch");
        request_plan.query_obligations.clear();
        let request_snapshot = capture_packet_obligation_edge_proofs_before_budget(
            request_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &request_plan,
            &request_answer,
        );
        assert!(!request_snapshot.entries.is_empty());
        request_answer.citations =
            vec![citation("dispatch_hook", "src/hooks.rs", NodeKind::METHOD)];
        request_answer.graphs.clear();
        install_retained_packet_obligation_edge_proofs(
            &mut request_plan,
            &request_answer,
            &truncated_budget,
            &request_snapshot,
            16,
        );
        finalize_packet_obligation_plan(
            request_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut request_plan,
            &request_answer,
            &truncated_budget,
        );
        assert_eq!(
            request_plan.claim_obligations[0].reason.as_deref(),
            Some(PACKET_BUDGET_TRUNCATED_REASON),
            "a wrong-role survivor cannot overwrite the exact removed flow carrier"
        );

        let profile_question = "Describe the component behavior.";
        let mut profile_answer = answer_with_call_edge(
            profile_question,
            "RequestDispatcher::dispatch",
            "src/dispatcher.rs",
        );
        let mut profile_plan = PacketObligationPlanDto {
            version: PACKET_OBLIGATION_PLAN_VERSION,
            binding_terms: Vec::new(),
            claim_obligations: vec![PacketClaimObligationDto {
                id: "fixture_default_dispatch".to_string(),
                kind: PacketClaimObligationKindDto::Dispatch,
                binding_terms: Vec::new(),
                probe_binding: None,
                material: true,
                allowed_node_kinds: vec![NodeKind::FUNCTION, NodeKind::METHOD, NodeKind::MACRO],
                required_edge_kind: Some(EdgeKind::CALL),
                requires_complete_discovery: false,
                proof_status: PacketObligationProofStatusDto::Planned,
                reason: None,
                carrier_node_ids: Vec::new(),
                carrier_paths: Vec::new(),
                carrier_edge_proofs: Vec::new(),
                open_next_candidates: Vec::new(),
            }],
            query_obligations: Vec::new(),
        };
        let profile_snapshot = capture_packet_obligation_edge_proofs_before_budget(
            profile_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &profile_plan,
            &profile_answer,
        );
        assert!(!profile_snapshot.entries.is_empty());
        profile_answer.citations =
            vec![citation("dispatch_hook", "src/hooks.rs", NodeKind::METHOD)];
        profile_answer.graphs.clear();
        install_retained_packet_obligation_edge_proofs(
            &mut profile_plan,
            &profile_answer,
            &truncated_budget,
            &profile_snapshot,
            16,
        );
        finalize_packet_obligation_plan(
            profile_question,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut profile_plan,
            &profile_answer,
            &truncated_budget,
        );
        assert_eq!(
            profile_plan.claim_obligations[0].reason.as_deref(),
            Some(PACKET_BUDGET_TRUNCATED_REASON),
            "a weaker default-profile survivor cannot overwrite the exact removed typed proof"
        );
    }

    #[test]
    fn unrelated_markdown_truncation_keeps_retained_typed_proof() {
        let mut entrypoint = citation(
            "BuildIndex::run",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        entrypoint.evidence_edge_ids = vec![EdgeId("call-index".to_string())];
        let target = citation(
            "Indexer::build",
            "crates/example/src/indexer.rs",
            NodeKind::METHOD,
        );
        let mut answer = answer(vec![entrypoint.clone(), target.clone()]);
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "indexing-flow".to_string(),
            title: "Indexing flow".to_string(),
            graph: GraphResponse {
                center_id: entrypoint.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("call-index".to_string()),
                    source: entrypoint.node_id,
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
        let mut truncated_budget = budget();
        truncated_budget.truncated = true;
        truncated_budget.omitted_sections = vec!["markdown_blocks".to_string()];
        let mut plan = indexing_entrypoint_plan();

        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &truncated_budget,
        );

        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Proven
        );
        assert_eq!(plan.claim_obligations[0].reason, None);
    }

    #[test]
    fn tokenized_absence_profile_requires_complete_discovery() {
        let plan = build_packet_obligation_plan(
            "Find unused callers for Widget::run.",
            PacketTaskClassDto::SymbolOwnership,
            &[],
        );
        assert!(
            plan.claim_obligations
                .iter()
                .all(|obligation| obligation.requires_complete_discovery)
        );
    }

    #[test]
    fn common_negative_paraphrases_require_complete_discovery() {
        for question in [
            "Find zero references to Widget::run.",
            "Show where Widget::run is not referenced.",
            "Find Widget::run with no usages.",
            "Show where Widget::run is not used.",
            "Confirm Widget::run isn't called.",
            "Confirm Widget::run does not exist.",
            "Find none of the handlers for Widget::run.",
            "Find missing implementations of Widget::run.",
            "Confirm zero known direct callers of Widget::run.",
            "Is Widget::run unreachable?",
        ] {
            let plan =
                build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
            assert!(
                plan.claim_obligations
                    .iter()
                    .filter(|obligation| obligation.material)
                    .all(|obligation| obligation.requires_complete_discovery),
                "{question}"
            );
            assert!(
                plan.claim_obligations
                    .iter()
                    .any(|obligation| obligation.material),
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

    #[test]
    fn unmet_material_obligations_emit_at_most_one_deduplicated_query_each() {
        let mut plan = indexing_entrypoint_plan();
        plan.claim_obligations[0].proof_status = PacketObligationProofStatusDto::Reported;
        plan.claim_obligations[0].open_next_candidates = vec![
            "generic indexing entrypoint".to_string(),
            "ignored second candidate".to_string(),
        ];
        plan.query_obligations = vec![
            PacketQueryObligationDto {
                id: "query:indexer".to_string(),
                kind: PacketQueryObligationKindDto::Supplemental,
                query: "src/indexer.rs".to_string(),
                material: false,
                completion: Some(PacketQueryCompletionDto::Cancelled {
                    reason: "not_dispatched".to_string(),
                }),
            },
            PacketQueryObligationDto {
                id: "query:snapshot".to_string(),
                kind: PacketQueryObligationKindDto::RequiredFlow,
                query: "snapshot publication".to_string(),
                material: true,
                completion: Some(PacketQueryCompletionDto::Cancelled {
                    reason: "stage_deadline".to_string(),
                }),
            },
        ];

        assert_eq!(
            packet_unmet_material_follow_up_queries(&plan),
            vec!["src/indexer.rs", "snapshot publication"]
        );
    }

    #[test]
    fn typed_contradiction_survives_rebuild_and_budget_refresh() {
        let mut plan = indexing_entrypoint_plan();
        plan.claim_obligations[0].proof_status = PacketObligationProofStatusDto::Contradicted;
        plan.claim_obligations[0].reason = Some("source_contradiction".to_string());
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer(Vec::new()),
            &budget(),
        );

        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Contradicted
        );
        assert_eq!(
            plan.claim_obligations[0].reason.as_deref(),
            Some("source_contradiction")
        );
    }

    #[test]
    fn indexing_source_range_flow_still_rejects_unlawful_structural_carriers() {
        let false_carriers = [
            ("CompilationDatabase", NodeKind::CLASS),
            ("store_path", NodeKind::VARIABLE),
            ("snapshot_field", NodeKind::FIELD),
        ];
        for (name, kind) in false_carriers {
            let candidate = citation(name, "src/indexer/store.rs", kind);
            let answer = answer(vec![candidate]);
            let mut plan = indexing_obligation_plan("indexing_storage");
            finalize_packet_obligation_plan(
                INDEXING_QUESTION,
                PacketTaskClassDto::ArchitectureExplanation,
                &mut plan,
                &answer,
                &budget(),
            );
            assert_ne!(
                plan.claim_obligations[0].proof_status,
                PacketObligationProofStatusDto::Proven,
                "{name}"
            );
        }
    }
}
