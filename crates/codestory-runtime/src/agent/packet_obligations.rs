//! Versioned packet obligations planned before retrieval and finalized from carried evidence.

use super::packet_claims::packet_supported_claims_with_telemetry;
use super::packet_evidence::citation_sufficiency_eligible;
use super::packet_flow_requirements::{
    CoverageMode, EvidencePredicate, FlowRequirement, FlowRole, packet_flow_requirements_for_terms,
};
use super::packet_profile_telemetry::PacketClaimTelemetry;
use super::packet_required_probes::{
    packet_prompt_exact_symbol_probe_queries, packet_sufficiency_required_probe_queries_from_terms,
};
use super::packet_scoring::{
    normalize_identifier, packet_adjacent_query_stop_term, packet_display_path,
    packet_query_stop_term,
};
use super::packet_sufficiency::packet_execution_graphs;
use super::packet_terms::packet_probe_terms;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto,
    EdgeKind, NodeKind, PACKET_OBLIGATION_PLAN_VERSION, PacketBudgetDto, PacketClaimDto,
    PacketClaimObligationDto, PacketClaimObligationKindDto, PacketObligationPlanDto,
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
pub(crate) const PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE: &str = "obligation receipt";

pub(crate) fn build_packet_obligation_plan(
    question: &str,
    task_class: PacketTaskClassDto,
    planned_queries: &[PacketPlanQueryDto],
) -> PacketObligationPlanDto {
    let terms = packet_probe_terms(question);
    let requirements = packet_flow_requirements_for_terms(&terms, task_class);
    let requires_complete_discovery =
        packet_question_requires_complete_discovery(question, task_class);
    let exact_symbol_queries =
        packet_prompt_exact_symbol_probe_queries(question, &terms, task_class);
    let (binding_terms, omitted_binding_term_count) =
        requested_claim_binding_terms(question, &exact_symbol_queries, !requirements.is_empty());
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
        let needs_material_fallback = !claim_obligations
            .iter()
            .any(|obligation| obligation.material);
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

pub(crate) fn append_packet_probe_obligations(
    plan: &mut PacketObligationPlanDto,
    resolutions: &[PacketProbeResolutionDto],
) {
    plan.claim_obligations
        .retain(|obligation| obligation.probe_binding.is_none());
    plan.claim_obligations.extend(
        resolutions
            .iter()
            .filter(|resolution| packet_probe_is_exact_typed(&resolution.probe))
            .map(packet_probe_obligation),
    );
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
        material: true,
        allowed_node_kinds: Vec::new(),
        required_edge_kind: None,
        requires_complete_discovery: false,
        proof_status,
        reason,
        carrier_node_ids: Vec::new(),
        carrier_paths: Vec::new(),
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
        open_next_candidates,
    }
}

fn default_profile_requested_claim_obligations(
    binding_terms: &[String],
    task_class: PacketTaskClassDto,
    requires_complete_discovery: bool,
) -> Vec<PacketClaimObligationDto> {
    let kind = default_profile_obligation_kind(task_class);
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
            required_edge_kind: Some(EdgeKind::CALL),
            requires_complete_discovery,
            proof_status: PacketObligationProofStatusDto::Planned,
            reason: None,
            carrier_node_ids: Vec::new(),
            carrier_paths: Vec::new(),
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
            open_next_candidates: Vec::new(),
        })
        .collect()
}

fn requested_claim_binding_terms(
    question: &str,
    exact_symbol_queries: &[String],
    has_recognized_flow: bool,
) -> (Vec<String>, usize) {
    let mut candidates = Vec::new();
    let mut exact_symbol_components = HashSet::new();
    for term in exact_symbol_queries {
        // Consume a qualified exact symbol atomically. The ordinary prompt tokenizer may retain a
        // CamelCase owner as one token while `symbol_query_tokens` splits it, so record both forms
        // and prevent either owner or member fragments from becoming extra material claim rows.
        for segment in symbol_identity_segments(term) {
            exact_symbol_components.insert(segment);
        }
        for component in crate::symbol_query_tokens(term) {
            exact_symbol_components.insert(normalize_identifier(&component));
        }
        push_exact_requested_claim_binding_candidate(&mut candidates, term);
    }

    // A recognized flow's ordinary nouns describe the flow profile; code-shaped exact symbols
    // remain independent requested claims. Without a recognized flow, retain the old natural-
    // language fallback for concrete identifiers while avoiding owner/member fragments already
    // represented by an exact qualified symbol.
    if !has_recognized_flow {
        for term in packet_probe_terms(question)
            .into_iter()
            .filter(|term| packet_obligation_binding_term_is_concrete(term))
            .filter(|term| !exact_symbol_components.contains(&normalize_identifier(term)))
        {
            push_requested_claim_binding_candidate(&mut candidates, &term);
        }
    }

    let omitted = candidates
        .len()
        .saturating_sub(PACKET_OBLIGATION_BINDING_TERM_LIMIT);
    candidates.truncate(PACKET_OBLIGATION_BINDING_TERM_LIMIT);
    (candidates, omitted)
}

fn push_requested_claim_binding_candidate(candidates: &mut Vec<String>, term: &str) {
    let bounded = term
        .chars()
        .take(PACKET_OBLIGATION_BINDING_TERM_CHAR_LIMIT)
        .collect::<String>();
    if !bounded.is_empty()
        && !candidates
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&bounded))
    {
        candidates.push(bounded);
    }
}

fn push_exact_requested_claim_binding_candidate(candidates: &mut Vec<String>, term: &str) {
    let bounded = term
        .chars()
        .take(PACKET_OBLIGATION_BINDING_TERM_CHAR_LIMIT)
        .collect::<String>();
    if !bounded.is_empty() && !candidates.iter().any(|candidate| candidate == &bounded) {
        candidates.push(bounded);
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
        open_next_candidates: Vec::new(),
    }
}

fn packet_obligation_binding_term_is_concrete(term: &str) -> bool {
    term.len() >= 4
        && !packet_query_stop_term(term)
        && !packet_adjacent_query_stop_term(term)
        && !matches!(
            term,
            "architecture"
                | "assess"
                | "behavior"
                | "behaviour"
                | "callers"
                | "change"
                | "dispatch"
                | "entrypoint"
                | "external"
                | "handler"
                | "handlers"
                | "locate"
                | "orchestration"
                | "ownership"
                | "plan"
                | "persistence"
                | "references"
                | "router"
                | "runtime"
                | "service"
                | "state"
                | "storage"
                | "trace"
                | "tracing"
        )
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
    ) && matches!(requirement.evidence, EvidencePredicate::CitedRoles { .. })
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

pub(crate) fn finalize_packet_obligation_plan(
    question: &str,
    task_class: PacketTaskClassDto,
    plan: &mut PacketObligationPlanDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
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
        if obligation.probe_binding.is_some() {
            finalize_exact_probe_obligation(obligation, answer, budget);
            continue;
        }
        obligation.carrier_node_ids.clear();
        obligation.carrier_paths.clear();
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
                budget,
            );
            continue;
        };
        finalize_claim_obligation(obligation, requirement, answer, budget);
    }
    finalize_query_obligations(plan, answer, budget);
}

fn finalize_exact_probe_obligation(
    obligation: &mut PacketClaimObligationDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
) {
    obligation.carrier_node_ids.clear();
    obligation.carrier_paths.clear();
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
    let matching_citations = answer
        .citations
        .iter()
        .filter(|citation| {
            citation.coverage_role.as_deref() == Some("explicit exact probe")
                && citation_matches_exact_probe_binding(citation, &binding)
        })
        .collect::<Vec<_>>();
    if matching_citations.is_empty() {
        obligation.proof_status = if packet_budget_omitted_obligation_evidence(budget, "citations")
        {
            PacketObligationProofStatusDto::Reported
        } else {
            PacketObligationProofStatusDto::Unsupported
        };
        obligation.reason = Some(
            if packet_budget_omitted_obligation_evidence(budget, "citations") {
                "packet_budget_truncated"
            } else {
                "exact_probe_carrier_missing"
            }
            .to_string(),
        );
        return;
    }
    record_obligation_carriers(
        obligation,
        matching_citations,
        budget.limits.max_anchors as usize,
    );
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
    budget: &PacketBudgetDto,
) {
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
        budget.limits.max_anchors as usize,
    );

    if obligation.requires_complete_discovery {
        obligation.proof_status = PacketObligationProofStatusDto::Reported;
        obligation.reason = Some("complete_discovery_and_collector_coverage_unproven".to_string());
        return;
    }
    if matching_citations.is_empty() && reported_citations.is_empty() {
        if packet_budget_omitted_obligation_evidence(budget, "citations") {
            obligation.proof_status = PacketObligationProofStatusDto::Reported;
            obligation.reason = Some("packet_budget_truncated".to_string());
        } else {
            obligation.proof_status = PacketObligationProofStatusDto::Unsupported;
            obligation.reason = Some("required_carrier_missing".to_string());
        }
        return;
    }
    if matching_citations.is_empty() {
        obligation.proof_status = PacketObligationProofStatusDto::Reported;
        obligation.reason = Some("carrier_does_not_satisfy_role_contract".to_string());
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
                    citation_satisfies_edge_requirement(citation, required_edge_kind, answer)
                })
        })
        .collect::<Vec<_>>();
    if proven_citations.is_empty() {
        obligation.proof_status = PacketObligationProofStatusDto::Reported;
        obligation.reason = Some(
            if packet_budget_omitted_obligation_evidence(budget, "trail_edges") {
                "packet_budget_truncated"
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
        budget.limits.max_anchors as usize,
    );
    obligation.proof_status = PacketObligationProofStatusDto::Proven;
    obligation.reason = None;
}

fn finalize_default_profile_obligation(
    obligation: &mut PacketClaimObligationDto,
    binding_terms: &[String],
    exact_binding_terms: &[String],
    requested_paths: &[String],
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
) {
    let reported_citations = answer
        .citations
        .iter()
        .filter(|citation| {
            citation_matches_default_profile_binding(
                citation,
                binding_terms,
                exact_binding_terms,
                requested_paths,
            ) && citation_plausibly_reports_obligation(citation, obligation.kind)
        })
        .collect::<Vec<_>>();
    record_obligation_carriers(
        obligation,
        reported_citations.iter().copied(),
        budget.limits.max_anchors as usize,
    );
    if obligation.requires_complete_discovery {
        obligation.proof_status = PacketObligationProofStatusDto::Reported;
        obligation.reason = Some("complete_discovery_and_collector_coverage_unproven".to_string());
    } else if reported_citations.is_empty() {
        if packet_budget_omitted_obligation_evidence(budget, "citations") {
            obligation.proof_status = PacketObligationProofStatusDto::Reported;
            obligation.reason = Some("packet_budget_truncated".to_string());
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
                            citation_satisfies_edge_requirement(
                                citation,
                                required_edge_kind,
                                answer,
                            )
                        })
            })
            .collect::<Vec<_>>();
        if proven_citations.is_empty() {
            obligation.proof_status = PacketObligationProofStatusDto::Reported;
            obligation.reason = Some(
                if packet_budget_omitted_obligation_evidence(budget, "trail_edges") {
                    "packet_budget_truncated"
                } else {
                    "selected_claim_profile_requires_typed_flow"
                }
                .to_string(),
            );
        } else {
            record_obligation_carriers(
                obligation,
                proven_citations.iter().copied(),
                budget.limits.max_anchors as usize,
            );
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
    let exact_case = crate::exact_symbol_query_terms(requested)
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

fn citation_satisfies_edge_requirement(
    citation: &AgentCitationDto,
    required_edge_kind: EdgeKind,
    answer: &AgentAnswerDto,
) -> bool {
    let graphs = packet_execution_graphs(answer);
    let cited_edge_ids = citation.evidence_edge_ids.iter().collect::<HashSet<_>>();
    !cited_edge_ids.is_empty()
        && graphs.iter().any(|graph| {
            graph.edges.iter().any(|edge| {
                edge.kind == required_edge_kind
                    && cited_edge_ids.contains(&edge.id)
                    && (edge.source == citation.node_id || edge.target == citation.node_id)
                    && !crate::graph_builders::is_speculative_trail_edge(edge)
            })
        })
}

fn finalize_query_obligations(
    plan: &mut PacketObligationPlanDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
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
        obligation.completion = Some(PacketQueryCompletionDto::Cancelled {
            reason: if budget.truncated {
                "packet_budget_truncated"
            } else {
                "not_dispatched"
            }
            .to_string(),
        });
    }
}

pub(crate) fn bind_claims_to_packet_obligations(
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

pub(crate) fn packet_claims_with_obligation_receipts(
    answer: &AgentAnswerDto,
    plan: &PacketObligationPlanDto,
) -> Vec<PacketClaimDto> {
    packet_claims_with_obligation_receipts_and_telemetry(answer, plan).0
}

pub(crate) fn packet_claims_with_obligation_receipts_and_telemetry(
    answer: &AgentAnswerDto,
    plan: &PacketObligationPlanDto,
) -> (Vec<PacketClaimDto>, PacketClaimTelemetry) {
    let (mut claims, telemetry) = packet_supported_claims_with_telemetry(answer);
    append_packet_obligation_receipt_claims(answer, plan, &mut claims);
    (claims, telemetry)
}

fn append_packet_obligation_receipt_claims(
    answer: &AgentAnswerDto,
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
        let eligible = obligation.proof_status == PacketObligationProofStatusDto::Proven
            && has_carried_citation;
        claims.push(PacketClaimDto {
            claim: packet_obligation_receipt_text(obligation, &citations),
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
    obligation: &PacketClaimObligationDto,
    citations: &[AgentCitationDto],
) -> String {
    if obligation.proof_status == PacketObligationProofStatusDto::Proven && !citations.is_empty() {
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

pub(crate) fn material_packet_obligations_are_proven(plan: &PacketObligationPlanDto) -> bool {
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

pub(crate) fn packet_obligation_open_next_candidates(
    plan: &PacketObligationPlanDto,
) -> Vec<String> {
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

pub(crate) fn packet_proven_obligation_carrier_paths(
    plan: &PacketObligationPlanDto,
) -> Vec<String> {
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
    use crate::agent::packet_sufficiency::build_packet_sufficiency_with_obligation_context;
    use codestory_contracts::api::{
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalTraceDto, EdgeId,
        GraphArtifactDto, GraphEdgeDto, GraphResponse, NodeId, PACKET_PROBE_CONTRACT_VERSION,
        PacketBudgetLimitsDto, PacketBudgetModeDto, PacketBudgetUsageDto,
        PacketEvidenceResolutionDto, PacketEvidenceTierDto, PacketProbeAmbiguityCandidateDto,
        PacketProbeRejectionDto, PacketSidecarQueryDiagnosticDto, PacketSufficiencyStatusDto,
        SearchHitOrigin,
    };
    use std::path::Path;

    const INDEXING_QUESTION: &str = "Explain the indexing runtime, persistence, and snapshot flow.";

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
            answer_id: "obligation-test".to_string(),
            prompt: INDEXING_QUESTION.to_string(),
            summary: "test".to_string(),
            freshness: Some(crate::agent::packet_freshness::fresh_index_observation()),
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

    fn finalized_obligation(
        id: &str,
        kind: PacketClaimObligationKindDto,
        proof_status: PacketObligationProofStatusDto,
        carrier: Option<&AgentCitationDto>,
        reason: Option<&str>,
    ) -> PacketClaimObligationDto {
        PacketClaimObligationDto {
            id: id.to_string(),
            kind,
            binding_terms: Vec::new(),
            probe_binding: None,
            material: true,
            allowed_node_kinds: Vec::new(),
            required_edge_kind: None,
            requires_complete_discovery: false,
            proof_status,
            reason: reason.map(str::to_string),
            carrier_node_ids: carrier
                .into_iter()
                .map(|citation| citation.node_id.clone())
                .collect(),
            carrier_paths: carrier
                .and_then(|citation| citation.file_path.clone())
                .into_iter()
                .collect(),
            open_next_candidates: Vec::new(),
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
                nodes: Vec::new(),
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
    fn cancelled_diagnostic_query_does_not_block_a_complete_material_ledger() {
        let question = "Explain shell installer function dispatch and completion.";
        let mut plan = build_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        let mut answer = answer(vec![
            lexical_citation("nvm_download", "install.sh", NodeKind::FUNCTION),
            lexical_citation("nvm_use", "nvm.sh", NodeKind::FUNCTION),
            citation("Helper::noop", "src/helper.rs", NodeKind::METHOD),
        ]);
        answer.prompt = question.to_string();
        let material_queries = plan
            .query_obligations
            .iter()
            .filter(|obligation| obligation.material)
            .map(|obligation| obligation.query.clone())
            .collect::<Vec<_>>();
        for query in material_queries {
            answer
                .retrieval_trace
                .packet_sidecar_diagnostics
                .push(query_diagnostic(
                    &query,
                    PacketQueryCompletionDto::Completed,
                ));
        }
        answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .push(query_diagnostic(
                "shell completion",
                PacketQueryCompletionDto::Cancelled {
                    reason: "diagnostic_deadline".to_string(),
                },
            ));

        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget(),
        );

        assert!(plan.query_obligations.iter().any(|obligation| {
            !obligation.material
                && obligation.query == "shell completion"
                && matches!(
                    obligation.completion,
                    Some(PacketQueryCompletionDto::Cancelled { ref reason })
                        if reason == "diagnostic_deadline"
                )
        }));
        assert!(material_packet_obligations_are_proven(&plan), "{plan:#?}");
        let sufficiency = build_packet_sufficiency_with_obligation_context(
            Path::new("/workspace/example"),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &answer,
            &budget(),
            &[],
            &[],
            &plan,
        );
        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(
            sufficiency
                .coverage_report
                .as_ref()
                .is_some_and(|coverage| coverage.unresolved == ["shell completion"]),
            "diagnostic cancellation stays visible without changing the verdict: {sufficiency:?}"
        );
    }

    /// EV-8. A required query whose semantic stage timed out with zero hits reaches the ledger as
    /// a typed cancellation, and the ledger is what demotes the verdict. The demotion lands on
    /// the obligation that lost its evidence rather than on the packet as an undifferentiated
    /// whole, so the caller is told which query to re-run.
    #[test]
    fn a_required_query_whose_semantic_stage_timed_out_demotes_through_its_obligation() {
        let question = "Explain shell installer function dispatch and completion.";
        let mut plan = build_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        let mut answer = answer(vec![
            lexical_citation("nvm_download", "install.sh", NodeKind::FUNCTION),
            lexical_citation("nvm_use", "nvm.sh", NodeKind::FUNCTION),
            citation("Helper::noop", "src/helper.rs", NodeKind::METHOD),
        ]);
        answer.prompt = question.to_string();
        let material_queries = plan
            .query_obligations
            .iter()
            .filter(|obligation| obligation.material)
            .map(|obligation| obligation.query.clone())
            .collect::<Vec<_>>();
        let (timed_out_query, completed_queries) = material_queries
            .split_first()
            .expect("architecture explanation plans at least one material query");
        for query in completed_queries {
            answer
                .retrieval_trace
                .packet_sidecar_diagnostics
                .push(query_diagnostic(query, PacketQueryCompletionDto::Completed));
        }
        let mut timed_out = query_diagnostic(
            timed_out_query,
            PacketQueryCompletionDto::Cancelled {
                reason: "semantic_stage_timeout_zero_hits".to_string(),
            },
        );
        timed_out.semantic_stage_timeout_zero_hits = true;
        answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .push(timed_out);

        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget(),
        );

        assert!(
            plan.query_obligations.iter().any(|obligation| {
                obligation.material
                    && &obligation.query == timed_out_query
                    && matches!(
                        obligation.completion,
                        Some(PacketQueryCompletionDto::Cancelled { ref reason })
                            if reason == "semantic_stage_timeout_zero_hits"
                    )
            }),
            "the typed cancellation must reach the ledger: {plan:#?}"
        );

        let sufficiency = build_packet_sufficiency_with_obligation_context(
            Path::new("/workspace/example"),
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &answer,
            &budget(),
            &[],
            &[],
            &plan,
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "{sufficiency:?}"
        );
        assert!(
            sufficiency.gaps.iter().any(|gap| {
                gap.contains("query obligation") && gap.contains("semantic_stage_timeout_zero_hits")
            }),
            "the gap must name the query obligation that lost its lane: {:?}",
            sufficiency.gaps
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains(timed_out_query.as_str())),
            "the caller is told which query to re-run: {sufficiency:?}"
        );
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

        append_packet_probe_obligations(&mut plan, &resolutions);

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
            obligation.material
                && obligation.probe_binding.as_ref().is_some_and(|binding| {
                    obligation.id == format!("exact_probe:{}", binding.input_index)
                })
        }));
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
        append_packet_probe_obligations(&mut plan, &resolutions);
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
            (
                "Trace request dispatch through an interceptor and transport adapter.",
                PacketTaskClassDto::RouteTracing,
                "request_terminal",
                lexical_citation("HttpTransportAdapter", "src/transport.rs", NodeKind::CLASS),
                lexical_citation("ArrayAdapter", "src/transport.rs", NodeKind::CLASS),
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
    fn indexing_storage_claim_cannot_borrow_a_proven_runtime_entrypoint() {
        let question = "Explain how a full indexing run moves from the CLI into runtime orchestration, workspace file discovery, symbol extraction, persistence, and snapshot refresh.";
        let mut runtime = citation(
            "IndexService::run_indexing_blocking_without_runtime_refresh",
            "crates/codestory-runtime/src/services.rs",
            NodeKind::METHOD,
        );
        runtime.evidence_edge_ids = vec![EdgeId("indexing-entry-call".to_string())];
        let target = citation(
            "Worker::execute",
            "crates/example/src/worker.rs",
            NodeKind::METHOD,
        );
        let mut answer = answer(vec![runtime.clone(), target.clone()]);
        answer.prompt = question.to_string();
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "indexing-entrypoint-flow".to_string(),
            title: "Indexing entrypoint flow".to_string(),
            graph: GraphResponse {
                center_id: runtime.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("indexing-entry-call".to_string()),
                    source: runtime.node_id.clone(),
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
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &[],
        );
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget(),
        );
        assert_eq!(
            plan.claim_obligations
                .iter()
                .find(|obligation| obligation.id == "indexing_entrypoint")
                .map(|obligation| obligation.proof_status),
            Some(PacketObligationProofStatusDto::Proven)
        );
        let storage_obligation = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == "indexing_storage")
            .expect("indexing storage obligation");
        assert_eq!(
            storage_obligation.proof_status,
            PacketObligationProofStatusDto::Unsupported
        );
        assert_eq!(
            storage_obligation.reason.as_deref(),
            Some("required_carrier_missing")
        );

        let mut claims = packet_claims_with_obligation_receipts(&answer, &plan);
        bind_claims_to_packet_obligations(&plan, &mut claims);
        let storage_claim = claims
            .iter()
            .find(|claim| claim.required_obligation_ids == ["indexing_storage"])
            .expect("material storage row emits an exact receipt claim");
        assert_eq!(storage_claim.required_obligation_ids, ["indexing_storage"]);
        assert_eq!(
            storage_claim.proof_status,
            Some(PacketProofStatusDto::Unsupported)
        );
        assert_eq!(storage_claim.eligible_for_sufficiency, Some(false));
        assert!(storage_claim.claim.contains("required_carrier_missing"));
        let entrypoint_claim = claims
            .iter()
            .find(|claim| claim.required_obligation_ids == ["indexing_entrypoint"])
            .expect("material entrypoint row emits an exact receipt claim");
        assert_eq!(
            entrypoint_claim.proof_status,
            Some(PacketProofStatusDto::Proven)
        );
        assert_eq!(entrypoint_claim.citations.len(), 1);
    }

    #[test]
    fn obligation_receipts_use_only_their_exact_rows_own_carriers() {
        let entrypoint = citation("Cli::run", "src/cli.rs", NodeKind::METHOD);
        let storage = citation("Store::write", "src/store.rs", NodeKind::METHOD);
        let answer = answer(vec![entrypoint.clone(), storage.clone()]);
        let plan = PacketObligationPlanDto {
            version: PACKET_OBLIGATION_PLAN_VERSION,
            binding_terms: Vec::new(),
            claim_obligations: vec![
                finalized_obligation(
                    "flow_entrypoint",
                    PacketClaimObligationKindDto::Entrypoint,
                    PacketObligationProofStatusDto::Proven,
                    Some(&entrypoint),
                    None,
                ),
                finalized_obligation(
                    "flow_storage",
                    PacketClaimObligationKindDto::StateWrite,
                    PacketObligationProofStatusDto::Proven,
                    Some(&storage),
                    None,
                ),
            ],
            query_obligations: Vec::new(),
        };

        let mut claims = packet_claims_with_obligation_receipts(&answer, &plan);
        bind_claims_to_packet_obligations(&plan, &mut claims);
        let receipts = claims
            .iter()
            .filter(|claim| {
                claim.coverage_role.as_deref() == Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE)
            })
            .collect::<Vec<_>>();
        assert_eq!(receipts.len(), 2);
        let entrypoint_receipt = receipts
            .iter()
            .find(|claim| claim.required_obligation_ids == ["flow_entrypoint"])
            .expect("entrypoint receipt");
        assert_eq!(entrypoint_receipt.citations.len(), 1);
        assert_eq!(entrypoint_receipt.citations[0].node_id, entrypoint.node_id);
        let storage_receipt = receipts
            .iter()
            .find(|claim| claim.required_obligation_ids == ["flow_storage"])
            .expect("storage receipt");
        assert_eq!(storage_receipt.citations.len(), 1);
        assert_eq!(storage_receipt.citations[0].node_id, storage.node_id);

        let mut borrowed = vec![PacketClaimDto {
            claim: "The entrypoint row is proven.".to_string(),
            required_obligation_ids: vec!["flow_entrypoint".to_string()],
            required_obligation_kinds: vec![PacketClaimObligationKindDto::Entrypoint],
            proof_status: Some(PacketProofStatusDto::Proven),
            required_evidence_role: None,
            citations: vec![storage],
            coverage_role: Some("fixture".to_string()),
            eligible_for_sufficiency: Some(true),
        }];
        bind_claims_to_packet_obligations(&plan, &mut borrowed);
        assert_eq!(
            borrowed[0].proof_status,
            Some(PacketProofStatusDto::Reported)
        );
        assert_eq!(borrowed[0].eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn receipt_rows_preserve_non_proven_status_reason_and_deduplicate_ids() {
        let reported_carrier = citation("MaybeStore", "src/store.rs", NodeKind::METHOD);
        let plan = PacketObligationPlanDto {
            version: PACKET_OBLIGATION_PLAN_VERSION,
            binding_terms: Vec::new(),
            claim_obligations: vec![
                finalized_obligation(
                    "reported_storage",
                    PacketClaimObligationKindDto::StateWrite,
                    PacketObligationProofStatusDto::Reported,
                    Some(&reported_carrier),
                    Some("required_evidence_edge_missing"),
                ),
                finalized_obligation(
                    "reported_storage",
                    PacketClaimObligationKindDto::StateWrite,
                    PacketObligationProofStatusDto::Reported,
                    Some(&reported_carrier),
                    Some("duplicate_row"),
                ),
                finalized_obligation(
                    "unsupported_dispatch",
                    PacketClaimObligationKindDto::Dispatch,
                    PacketObligationProofStatusDto::Unsupported,
                    None,
                    Some("required_carrier_missing"),
                ),
            ],
            query_obligations: Vec::new(),
        };
        let answer = answer(vec![reported_carrier]);
        let mut claims = packet_claims_with_obligation_receipts(&answer, &plan);
        bind_claims_to_packet_obligations(&plan, &mut claims);
        let receipts = claims
            .iter()
            .filter(|claim| {
                claim.coverage_role.as_deref() == Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE)
            })
            .collect::<Vec<_>>();
        assert_eq!(receipts.len(), 2, "duplicate row IDs emit one receipt");
        let reported = receipts
            .iter()
            .find(|claim| claim.required_obligation_ids == ["reported_storage"])
            .expect("reported receipt");
        assert_eq!(reported.proof_status, Some(PacketProofStatusDto::Reported));
        assert_eq!(reported.eligible_for_sufficiency, Some(false));
        assert!(reported.claim.contains("required_evidence_edge_missing"));
        let unsupported = receipts
            .iter()
            .find(|claim| claim.required_obligation_ids == ["unsupported_dispatch"])
            .expect("unsupported receipt");
        assert_eq!(
            unsupported.proof_status,
            Some(PacketProofStatusDto::Unsupported)
        );
        assert!(unsupported.claim.contains("required_carrier_missing"));
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
            assert_eq!(
                requested[0].binding_terms,
                vec!["Widget::run"],
                "{task_class:?}"
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
            assert_eq!(
                plan.claim_obligations
                    .iter()
                    .map(|obligation| obligation.kind)
                    .collect::<HashSet<_>>()
                    .len(),
                5,
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
    fn sufficient_profile_closes_incidental_nonmaterial_guard_path() {
        let question = "Find RuntimeService::run.";
        let mut carrier = citation(
            "RuntimeService::run",
            "src/runtime_service.rs",
            NodeKind::METHOD,
        );
        carrier.evidence_edge_ids = vec![EdgeId("generic-call".to_string())];
        let target = citation("Worker::run", "src/worker.rs", NodeKind::METHOD);
        let incidental_guard_carrier = citation(
            "HttpTransport::send",
            "src/incidental_transport.rs",
            NodeKind::METHOD,
        );
        let mut answer = answer(vec![
            carrier.clone(),
            target.clone(),
            incidental_guard_carrier.clone(),
        ]);
        answer.prompt = question.to_string();
        answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .push(query_diagnostic(
                "RuntimeService::run",
                PacketQueryCompletionDto::Completed,
            ));
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "generic-flow".to_string(),
            title: "Generic flow".to_string(),
            graph: GraphResponse {
                center_id: carrier.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("generic-call".to_string()),
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
        let mut plan =
            build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
        plan.claim_obligations.push(PacketClaimObligationDto {
            id: "incidental_external_io_guard".to_string(),
            kind: PacketClaimObligationKindDto::ExternalIo,
            binding_terms: vec![incidental_guard_carrier.display_name.clone()],
            probe_binding: None,
            material: false,
            allowed_node_kinds: allowed_node_kinds_for_obligation(
                PacketClaimObligationKindDto::ExternalIo,
            ),
            required_edge_kind: Some(EdgeKind::CALL),
            requires_complete_discovery: false,
            proof_status: PacketObligationProofStatusDto::Planned,
            reason: None,
            carrier_node_ids: Vec::new(),
            carrier_paths: Vec::new(),
            open_next_candidates: Vec::new(),
        });
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
        assert_eq!(plan.claim_obligations[0].reason, None);
        assert!(material_packet_obligations_are_proven(&plan), "{plan:#?}");
        assert!(plan.query_obligations.iter().any(|obligation| {
            obligation.material
                && obligation.query == "RuntimeService::run"
                && obligation.completion == Some(PacketQueryCompletionDto::Completed)
        }));
        let incidental_guard = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == "incidental_external_io_guard")
            .expect("incidental guard remains in the finalized ledger");
        assert!(!incidental_guard.material);
        assert_eq!(
            incidental_guard.proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert_eq!(
            incidental_guard.carrier_paths,
            vec!["src/incidental_transport.rs".to_string()]
        );
        assert!(
            packet_obligation_open_next_candidates(&plan)
                .contains(&"src/incidental_transport.rs".to_string()),
            "the raw unproven guard receipt must exercise the terminal open-next gate"
        );
        let sufficiency = build_packet_sufficiency_with_obligation_context(
            Path::new("/workspace/example"),
            question,
            PacketTaskClassDto::SymbolOwnership,
            &answer,
            &budget(),
            &[],
            &[],
            &plan,
        );
        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(
            sufficiency
                .covered_claims
                .iter()
                .any(|claim| claim.proof_status == Some(PacketProofStatusDto::Proven))
        );
        assert!(
            !sufficiency
                .avoid_opening_paths
                .contains(&"src/incidental_transport.rs".to_string()),
            "an unproven incidental guard carrier cannot become avoid-opening"
        );
        let coverage = sufficiency
            .coverage_report
            .as_ref()
            .expect("sufficiency publishes claim-level blockers");
        assert!(
            coverage.ineligible.iter().any(|entry| {
                entry.contains("RuntimeService::run") && entry.contains("claim marked diagnostic")
            }),
            "the per-file safety blocker must remain visible: {coverage:?}"
        );
        assert!(
            sufficiency.open_next.is_empty(),
            "Sufficient is terminal even when a nonmaterial Reported guard carries a path: {sufficiency:?}"
        );
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
    fn case_distinct_exact_symbols_need_separate_carriers() {
        let question = "Find Foo::run and foo::run.";
        let mut answer = answer_with_call_edge(question, "Foo::run", "src/runtime.rs");
        for query in ["Foo::run", "foo::run"] {
            answer
                .retrieval_trace
                .packet_sidecar_diagnostics
                .push(query_diagnostic(query, PacketQueryCompletionDto::Completed));
        }
        let mut plan =
            build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);

        assert!(
            plan.query_obligations
                .iter()
                .any(|obligation| { obligation.material && obligation.query == "Foo::run" })
        );
        assert!(
            plan.query_obligations
                .iter()
                .any(|obligation| { obligation.material && obligation.query == "foo::run" })
        );
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::SymbolOwnership,
            &mut plan,
            &answer,
            &budget(),
        );

        let upper = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.binding_terms == ["Foo::run"])
            .unwrap();
        let lower = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.binding_terms == ["foo::run"])
            .unwrap();
        assert_eq!(upper.proof_status, PacketObligationProofStatusDto::Proven);
        assert_eq!(
            lower.proof_status,
            PacketObligationProofStatusDto::Unsupported
        );
        assert!(lower.carrier_node_ids.is_empty());
        assert!(!material_packet_obligations_are_proven(&plan));

        let sufficiency = build_packet_sufficiency_with_obligation_context(
            Path::new("/workspace/example"),
            question,
            PacketTaskClassDto::SymbolOwnership,
            &answer,
            &budget(),
            &[],
            &[],
            &plan,
        );
        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
    }

    #[test]
    fn case_distinct_slash_qualified_symbols_need_separate_carriers() {
        let question = "Find Foo/run and foo/run.";
        let mut answer = answer_with_call_edge(question, "Foo/run", "src/runtime.rs");
        for query in ["Foo/run", "foo/run"] {
            answer
                .retrieval_trace
                .packet_sidecar_diagnostics
                .push(query_diagnostic(query, PacketQueryCompletionDto::Completed));
        }
        let mut plan =
            build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);

        for expected in ["Foo/run", "foo/run"] {
            assert!(
                plan.query_obligations
                    .iter()
                    .any(|obligation| { obligation.material && obligation.query == expected })
            );
            assert!(plan.claim_obligations.iter().any(|obligation| {
                obligation.material && obligation.binding_terms == [expected]
            }));
        }
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::SymbolOwnership,
            &mut plan,
            &answer,
            &budget(),
        );

        let upper = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.binding_terms == ["Foo/run"])
            .unwrap();
        let lower = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.binding_terms == ["foo/run"])
            .unwrap();
        assert_eq!(upper.proof_status, PacketObligationProofStatusDto::Proven);
        assert_eq!(
            lower.proof_status,
            PacketObligationProofStatusDto::Unsupported
        );
        assert!(lower.carrier_node_ids.is_empty());

        let sufficiency = build_packet_sufficiency_with_obligation_context(
            Path::new("/workspace/example"),
            question,
            PacketTaskClassDto::SymbolOwnership,
            &answer,
            &budget(),
            &[],
            &[],
            &plan,
        );
        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
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
    fn one_proven_claim_cannot_hide_an_unproven_claim_in_the_same_file() {
        let question = "Find RuntimeService::run and CliErrorBody.";
        let shared_path = "src/runtime_service.rs";
        let mut carrier = citation("RuntimeService::run", shared_path, NodeKind::METHOD);
        carrier.evidence_edge_ids = vec![EdgeId("generic-call".to_string())];
        let false_carrier = citation("CliErrorBody", shared_path, NodeKind::STRUCT);
        let target = citation("Worker::run", "src/worker.rs", NodeKind::METHOD);
        let mut answer = answer(vec![carrier.clone(), false_carrier, target.clone()]);
        answer.prompt = question.to_string();
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "generic-flow".to_string(),
            title: "Generic flow".to_string(),
            graph: GraphResponse {
                center_id: carrier.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("generic-call".to_string()),
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
        let mut plan =
            build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::SymbolOwnership,
            &mut plan,
            &answer,
            &budget(),
        );
        assert!(plan.claim_obligations.iter().any(|obligation| {
            obligation.proof_status == PacketObligationProofStatusDto::Proven
                && obligation
                    .carrier_paths
                    .iter()
                    .any(|path| path == shared_path)
        }));
        assert!(plan.claim_obligations.iter().any(|obligation| {
            obligation.proof_status == PacketObligationProofStatusDto::Reported
                && obligation
                    .carrier_paths
                    .iter()
                    .any(|path| path == shared_path)
        }));

        let sufficiency = build_packet_sufficiency_with_obligation_context(
            Path::new("/workspace/example"),
            question,
            PacketTaskClassDto::SymbolOwnership,
            &answer,
            &budget(),
            &[],
            &[],
            &plan,
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(!material_packet_obligations_are_proven(&plan));
        assert!(
            !sufficiency
                .avoid_opening_paths
                .iter()
                .any(|path| path == shared_path),
            "a file with an unproven claim must remain openable: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .open_next
                .iter()
                .any(|command| command.contains(shared_path)),
            "the unproven same-file claim must stay open-next: {sufficiency:?}"
        );
    }

    #[test]
    fn missing_requested_claim_gets_a_material_unsupported_obligation() {
        let question = "Find RuntimeService::run and MissingWidget.";
        let mut carrier = citation(
            "RuntimeService::run",
            "src/runtime_service.rs",
            NodeKind::METHOD,
        );
        carrier.evidence_edge_ids = vec![EdgeId("generic-call".to_string())];
        let target = citation("Worker::run", "src/worker.rs", NodeKind::METHOD);
        let mut answer = answer(vec![carrier.clone(), target.clone()]);
        answer.prompt = question.to_string();
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "generic-flow".to_string(),
            title: "Generic flow".to_string(),
            graph: GraphResponse {
                center_id: carrier.node_id.clone(),
                nodes: Vec::new(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId("generic-call".to_string()),
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
        let mut plan =
            build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::SymbolOwnership,
            &mut plan,
            &answer,
            &budget(),
        );

        let missing = plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.binding_terms == ["MissingWidget"])
            .expect("material obligation for the missing requested claim");
        assert!(missing.material);
        assert_eq!(
            missing.proof_status,
            PacketObligationProofStatusDto::Unsupported
        );
        assert!(missing.carrier_node_ids.is_empty());
        assert!(!material_packet_obligations_are_proven(&plan));

        let sufficiency = build_packet_sufficiency_with_obligation_context(
            Path::new("/workspace/example"),
            question,
            PacketTaskClassDto::SymbolOwnership,
            &answer,
            &budget(),
            &[],
            &[],
            &plan,
        );
        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .open_next
                .iter()
                .any(|command| command.contains("MissingWidget"))
        );
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
    fn generic_profile_name_match_without_edge_stays_reported() {
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
            PacketObligationProofStatusDto::Reported
        );
        assert_eq!(
            plan.claim_obligations[0].reason.as_deref(),
            Some("selected_claim_profile_requires_typed_flow")
        );
        assert!(!material_packet_obligations_are_proven(&plan));
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
    fn false_shape_carrier_stays_reported_partial_and_open_next() {
        let false_carrier = citation(
            "CliErrorBody",
            "crates/example/src/indexer/runtime.rs",
            NodeKind::STRUCT,
        );
        let answer = answer(vec![false_carrier]);
        let budget = budget();
        let mut plan = indexing_entrypoint_plan();
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget,
        );

        assert_eq!(
            plan.claim_obligations[0].proof_status,
            PacketObligationProofStatusDto::Reported
        );
        assert_eq!(
            plan.claim_obligations[0].reason.as_deref(),
            Some("carrier_does_not_satisfy_role_contract")
        );
        let sufficiency = build_packet_sufficiency_with_obligation_context(
            Path::new("/workspace/example"),
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &answer,
            &budget,
            &[],
            &[],
            &plan,
        );
        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(sufficiency.avoid_opening_paths.is_empty());
        assert!(
            sufficiency
                .open_next
                .iter()
                .any(|command| command.contains("crates/example/src/indexer/runtime.rs"))
        );
    }

    #[test]
    fn known_false_carriers_publish_no_proven_claims_and_stay_open_next() {
        let question =
            "Explain CliErrorBody, runtime_path, and CompilationDatabase responsibilities.";
        let false_carriers = vec![
            citation("CliErrorBody", "src/cli/errors.rs", NodeKind::STRUCT),
            citation("runtime_path", "src/runtime/config.rs", NodeKind::VARIABLE),
            citation(
                "CompilationDatabase",
                "src/store/database.rs",
                NodeKind::CLASS,
            ),
        ];
        let mut answer = answer(false_carriers.clone());
        answer.prompt = question.to_string();
        let budget = budget();
        let mut plan =
            build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::SymbolOwnership,
            &mut plan,
            &answer,
            &budget,
        );

        let sufficiency = build_packet_sufficiency_with_obligation_context(
            Path::new("/workspace/example"),
            question,
            PacketTaskClassDto::SymbolOwnership,
            &answer,
            &budget,
            &[],
            &[],
            &plan,
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .covered_claims
                .iter()
                .all(|claim| { claim.proof_status != Some(PacketProofStatusDto::Proven) })
        );
        assert!(sufficiency.avoid_opening_paths.is_empty());
        for carrier in false_carriers {
            let path = carrier.file_path.expect("false carrier path");
            assert!(
                sufficiency
                    .open_next
                    .iter()
                    .any(|command| command.contains(&path)),
                "{path} must remain open-next: {:?}",
                sufficiency.open_next
            );
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
    fn missing_carrier_keeps_requested_path_as_open_next_candidate() {
        let queries = [PacketPlanQueryDto {
            query: "src/indexer.rs".to_string(),
            purpose: "explicit symbol probe from packet request".to_string(),
        }];
        let mut plan = build_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &queries,
        );
        plan.claim_obligations
            .retain(|obligation| obligation.id == "indexing_entrypoint");
        let answer = answer(Vec::new());
        let budget = budget();
        finalize_packet_obligation_plan(
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &mut plan,
            &answer,
            &budget,
        );
        let sufficiency = build_packet_sufficiency_with_obligation_context(
            Path::new("/workspace/example"),
            INDEXING_QUESTION,
            PacketTaskClassDto::ArchitectureExplanation,
            &answer,
            &budget,
            &[],
            &[],
            &plan,
        );

        assert!(
            sufficiency
                .open_next
                .iter()
                .any(|command| command.contains("src/indexer.rs"))
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
        answer.citations.clear();
        let mut truncated_budget = budget();
        truncated_budget.truncated = true;
        truncated_budget.omitted_sections = vec!["citations".to_string()];
        let mut plan = indexing_entrypoint_plan();

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
            Some("packet_budget_truncated")
        );
        assert!(!material_packet_obligations_are_proven(&plan));
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
