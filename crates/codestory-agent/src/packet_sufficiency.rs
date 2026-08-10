use crate::packet_claims::{
    decorate_packet_claims_proof_metadata, packet_supported_claims,
    packet_supported_claims_with_telemetry,
};
use crate::packet_command::next_deeper_packet_argv;
#[allow(unused_imports)]
pub use crate::packet_command::quote_packet_command_value;
pub use crate::packet_command::{
    packet_argv, packet_display_project_arg, packet_follow_up_invocation, render_packet_command,
};
use crate::packet_coverage::PacketCoverageInput;
use crate::packet_degradation::packet_primary_retrieval_truncated;
use crate::packet_evidence::citation_sufficiency_eligible;
use crate::packet_evidence_roles::packet_evidence_role;
pub use crate::packet_execution_graphs::packet_execution_graphs;
#[cfg(test)]
use crate::packet_flow_requirements::FlowRole;
use crate::packet_flow_requirements::{
    CoverageMode, FlowRequirement, packet_flow_requirements_for_terms,
};
use crate::packet_freshness::PacketFreshnessInput;
use crate::packet_obligations::{
    PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE, bind_claims_to_packet_obligations,
    material_packet_obligations_are_proven, packet_claims_with_obligation_receipts,
    packet_obligation_open_next_candidates, packet_proven_obligation_carrier_paths,
};
use crate::packet_plan::packet_symbol_probe_queries;
use crate::packet_required_probes::packet_missing_sufficiency_probe_queries_with_extra;
use crate::packet_scoring::{
    normalize_identifier, packet_citation_key, packet_display_name_is_test_like,
    packet_display_path,
};
use crate::packet_terms::packet_probe_terms;
#[cfg(any(test, feature = "test-support"))]
use crate::workspace_path_identity::MissingPathSpellingIdentity;
use crate::workspace_path_identity::WorkspacePathIdentity;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, AgentRetrievalStepStatusDto, EdgeKind, GraphResponse,
    NodeKind, PacketBudgetDto, PacketBudgetModeDto, PacketClaimDto, PacketClaimObligationDto,
    PacketCoverageReportDto, PacketEvidenceTierDto, PacketObligationPlanDto,
    PacketObligationProofStatusDto, PacketProbeDto, PacketQueryCompletionDto,
    PacketSidecarQueryDiagnosticDto, PacketSufficiencyDto, PacketSufficiencyStatusDto,
    PacketTaskClassDto,
};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::path::Path;

pub const PACKET_MARKDOWN_TRUNCATION_SUFFIX: &str =
    "\n\n... packet section truncated by budget ...\n";

pub struct PacketSufficiencyInput<'a> {
    pub project_root: &'a Path,
    pub question: &'a str,
    pub task_class: PacketTaskClassDto,
    pub answer: &'a AgentAnswerDto,
    pub budget: &'a PacketBudgetDto,
    pub supported_claims: Vec<PacketClaimDto>,
    pub missing_required_probe_queries: Vec<String>,
    pub targeted_follow_up_queries: Vec<String>,
}

#[cfg(any(test, feature = "test-support"))]
pub fn build_packet_sufficiency(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
) -> PacketSufficiencyDto {
    build_packet_sufficiency_with_extra(project_root, question, task_class, answer, budget, &[])
}

#[cfg(any(test, feature = "test-support"))]
pub fn build_packet_sufficiency_with_extra(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    extra_probes: &[String],
) -> PacketSufficiencyDto {
    build_packet_sufficiency_with_probe_context(
        project_root,
        question,
        task_class,
        answer,
        budget,
        extra_probes,
        &[],
    )
}

#[cfg(any(test, feature = "test-support"))]
pub fn build_packet_sufficiency_with_probe_context(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    extra_probes: &[String],
    exact_probe_paths: &[String],
) -> PacketSufficiencyDto {
    build_packet_sufficiency_with_optional_obligation_context(
        &MissingPathSpellingIdentity,
        project_root,
        question,
        task_class,
        answer,
        budget,
        extra_probes,
        exact_probe_paths,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_packet_sufficiency_with_obligation_context(
    path_identity: &dyn WorkspacePathIdentity,
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    extra_probes: &[String],
    exact_probe_paths: &[String],
    obligations: &PacketObligationPlanDto,
) -> PacketSufficiencyDto {
    build_packet_sufficiency_with_optional_obligation_context(
        path_identity,
        project_root,
        question,
        task_class,
        answer,
        budget,
        extra_probes,
        exact_probe_paths,
        Some(obligations),
    )
}

#[allow(clippy::too_many_arguments)]
fn build_packet_sufficiency_with_optional_obligation_context(
    path_identity: &dyn WorkspacePathIdentity,
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    extra_probes: &[String],
    exact_probe_paths: &[String],
    obligations: Option<&PacketObligationPlanDto>,
) -> PacketSufficiencyDto {
    let supported_claims = if let Some(plan) = obligations {
        let supported_claims_with_telemetry = packet_supported_claims_with_telemetry(answer);
        packet_claims_with_obligation_receipts(answer, plan, supported_claims_with_telemetry)
    } else {
        packet_supported_claims(answer)
    };
    let missing_required_probe_queries = packet_missing_sufficiency_probe_queries_with_extra(
        question,
        task_class,
        answer,
        &supported_claims,
        extra_probes,
    );
    assemble_packet_sufficiency_with_probe_context(
        path_identity,
        PacketSufficiencyInput {
            project_root,
            question,
            task_class,
            answer,
            budget,
            supported_claims,
            missing_required_probe_queries,
            targeted_follow_up_queries: packet_targeted_follow_up_queries(question, task_class),
        },
        extra_probes,
        exact_probe_paths,
        obligations,
    )
}

#[cfg(any(test, feature = "test-support"))]
pub fn assemble_packet_sufficiency(input: PacketSufficiencyInput<'_>) -> PacketSufficiencyDto {
    assemble_packet_sufficiency_with_probe_context(
        &MissingPathSpellingIdentity,
        input,
        &[],
        &[],
        None,
    )
}

#[cfg(test)]
fn assemble_packet_sufficiency_with_route_probes(
    input: PacketSufficiencyInput<'_>,
    selected_probes: &[String],
) -> PacketSufficiencyDto {
    assemble_packet_sufficiency_with_probe_context(
        &MissingPathSpellingIdentity,
        input,
        selected_probes,
        &[],
        None,
    )
}

#[cfg(test)]
fn assemble_packet_sufficiency_with_exact_paths(
    input: PacketSufficiencyInput<'_>,
    exact_probe_paths: &[String],
) -> PacketSufficiencyDto {
    assemble_packet_sufficiency_with_probe_context(
        &MissingPathSpellingIdentity,
        input,
        &[],
        exact_probe_paths,
        None,
    )
}

fn assemble_packet_sufficiency_with_probe_context(
    path_identity: &dyn WorkspacePathIdentity,
    input: PacketSufficiencyInput<'_>,
    selected_probes: &[String],
    exact_probe_paths: &[String],
    obligations: Option<&PacketObligationPlanDto>,
) -> PacketSufficiencyDto {
    let PacketSufficiencyInput {
        project_root,
        question,
        task_class,
        answer,
        budget,
        mut supported_claims,
        missing_required_probe_queries,
        targeted_follow_up_queries,
    } = input;

    decorate_packet_claims_proof_metadata(&mut supported_claims);
    if let Some(obligations) = obligations {
        bind_claims_to_packet_obligations(obligations, &mut supported_claims);
    }

    let has_errors = answer
        .retrieval_trace
        .steps
        .iter()
        .any(|step| step.status == AgentRetrievalStepStatusDto::Error);
    let min_citations = packet_sufficiency_min_citations(task_class);
    let min_claims = packet_sufficiency_min_claims_with_obligations(task_class, obligations);
    let min_claim_families =
        packet_sufficiency_min_claim_families_with_obligations(task_class, obligations);
    let route_stages = packet_route_proof_stages(question, selected_probes);
    let sufficiency_claims = supported_claims
        .iter()
        .filter(|claim| {
            packet_claim_can_satisfy_sufficiency(claim)
                || (obligations.is_none()
                    && task_class == PacketTaskClassDto::RouteTracing
                    && packet_route_claim_binds_stage(&route_stages, selected_probes, claim))
        })
        .cloned()
        .collect::<Vec<_>>();
    let proven_claims = sufficiency_claims
        .iter()
        .filter(|claim| packet_claim_can_satisfy_sufficiency(claim))
        .cloned()
        .collect::<Vec<_>>();
    let generic_navigation_claim_count = supported_claims
        .iter()
        .filter(|claim| {
            packet_claim_is_generic_navigation_or_source_evidence(claim)
                && !packet_route_claim_binds_stage(&route_stages, selected_probes, claim)
        })
        .count();
    let eligible_citation_count = packet_eligible_citation_count(answer);
    let has_minimum_coverage = eligible_citation_count >= min_citations;
    let has_minimum_claims = sufficiency_claims.len() >= min_claims;
    let claim_family_count = packet_supported_claim_family_count(&sufficiency_claims);
    let has_minimum_claim_families = claim_family_count >= min_claim_families;
    let missing_exact_path_claims = packet_missing_exact_path_claims(
        path_identity,
        project_root,
        exact_probe_paths,
        &sufficiency_claims,
    );
    // Legacy/unit callers without an obligation ledger retain the pre-EV-5 route-probe contract.
    // Production claims must survive typed binding; route stages may additionally use the exact
    // Proven carrier rows from that same plan below.
    let route_claims = if obligations.is_some() {
        &proven_claims
    } else {
        &supported_claims
    };
    let route_proof = packet_route_proof_assessment(
        task_class,
        answer,
        route_claims,
        &route_stages,
        selected_probes,
        obligations,
    );
    let mut missing_required_flow_requirements =
        packet_missing_required_flow_requirements(question, task_class, &sufficiency_claims);
    if task_class == PacketTaskClassDto::RouteTracing && route_proof.complete {
        missing_required_flow_requirements.clear();
    }
    let obligations_proven = obligations
        .map(material_packet_obligations_are_proven)
        .unwrap_or(true);
    let has_required_flow_roles =
        missing_required_flow_requirements.is_empty() && obligations_proven;
    let blocking_missing_probe_queries = obligations
        .map(packet_incomplete_material_query_obligations)
        .unwrap_or_else(|| {
            packet_blocking_missing_probe_queries(
                &missing_required_probe_queries,
                &missing_required_flow_requirements,
            )
        });
    let has_sufficiency_blocking_budget_omission = packet_has_sufficiency_blocking_budget_omission(
        budget,
        &missing_required_flow_requirements,
        &missing_required_probe_queries,
    );
    let blocking_route_probe_queries = if obligations.is_none() {
        packet_blocking_incomplete_route_probe_queries(
            question,
            task_class,
            route_proof.complete,
            &missing_required_probe_queries,
            selected_probes,
        )
    } else {
        Vec::new()
    };
    let unresolved_sidecar_queries = unresolved_sidecar_queries(answer);
    let blocking_unresolved_sidecar_queries = if obligations.is_some() {
        packet_blocking_unresolved_obligation_queries(
            &unresolved_sidecar_queries,
            &blocking_missing_probe_queries,
        )
    } else {
        packet_blocking_unresolved_sidecar_queries(
            &unresolved_sidecar_queries,
            &blocking_missing_probe_queries,
            &missing_required_flow_requirements,
            &blocking_route_probe_queries,
        )
    };
    // EV-7/EV-8: two facts about how this packet was collected, both of which bound what its
    // evidence can be reported as regardless of how well the claims themselves scored.
    let freshness = PacketFreshnessInput::from_observation(answer.freshness.as_ref());
    let coverage = PacketCoverageInput::from_observations(&answer.source_coverage);
    let primary_retrieval_truncated = packet_primary_retrieval_truncated(answer);
    let status = packet_sufficiency_status(PacketSufficiencyStatusInput {
        budget,
        eligible_citation_count,
        has_errors,
        has_minimum_coverage,
        has_minimum_claims,
        has_minimum_claim_families,
        has_required_flow_roles,
        has_route_proof: route_proof.complete,
        missing_exact_path_claims: &missing_exact_path_claims,
        has_sufficiency_blocking_budget_omission,
        missing_required_probe_queries: &blocking_missing_probe_queries,
        unresolved_sidecar_queries: &blocking_unresolved_sidecar_queries,
        freshness,
        coverage: &coverage,
        primary_retrieval_truncated,
    });

    let mut gaps = packet_sufficiency_gaps(
        task_class,
        answer,
        budget,
        min_citations,
        eligible_citation_count,
        min_claims,
        sufficiency_claims.len(),
        claim_family_count,
        min_claim_families,
        generic_navigation_claim_count,
        status,
        has_minimum_coverage,
        has_minimum_claims,
        has_minimum_claim_families,
        has_required_flow_roles,
        &route_proof,
        &missing_exact_path_claims,
        has_sufficiency_blocking_budget_omission,
        &blocking_missing_probe_queries,
        &missing_required_flow_requirements,
        &blocking_unresolved_sidecar_queries,
        freshness,
        &coverage,
        primary_retrieval_truncated,
    );
    if let Some(obligations) = obligations {
        append_packet_obligation_gaps(&mut gaps, obligations);
    }
    let blocking_probe_queries = packet_blocking_follow_up_probe_queries(
        &blocking_missing_probe_queries,
        &blocking_unresolved_sidecar_queries,
    );
    // A requested path the packet never proved anything about is the most specific thing a caller
    // can act on, so it leads the follow-up list. Appending it last let the command cap drop it
    // whenever enough flow probes were also missing — exactly when the caller needed it most.
    // Putting every path first only moved the loss: with enough unproven paths, the flow probes
    // fell off the end instead. Interleaving keeps a path in front and starves neither kind.
    let mut blocking_follow_up_probe_query_seeds = Vec::new();
    for query in &blocking_probe_queries {
        push_unique_sufficiency_term(&mut blocking_follow_up_probe_query_seeds, query);
    }
    for query in &blocking_route_probe_queries {
        push_unique_sufficiency_term(&mut blocking_follow_up_probe_query_seeds, query);
    }
    // The legacy no-ledger path may still report uncovered planner probes whose flow requirement
    // is already covered. Those are hints, not repair work. Production ledgers put every
    // incomplete material query in `blocking_probe_queries`, so command assembly can consume that
    // set directly without resurrecting nonblocking hints.
    if has_sufficiency_blocking_budget_omission {
        // A compact packet can prove every flow role and still omit the source proof required by
        // a proof-critical probe. The deeper-budget command repairs capacity; retain the concrete
        // probe beside it so the caller also knows which evidence to reacquire.
        for query in missing_required_probe_queries
            .iter()
            .filter(|query| packet_missing_probe_requires_compact_proof(query))
        {
            push_unique_sufficiency_term(&mut blocking_follow_up_probe_query_seeds, query);
        }
    }
    for query in &route_proof.follow_up_queries {
        push_unique_sufficiency_term(&mut blocking_follow_up_probe_query_seeds, query);
    }
    let terminally_sufficient = status == PacketSufficiencyStatusDto::Sufficient;
    let obligation_open_next_paths = if terminally_sufficient {
        Vec::new()
    } else {
        obligations
            .map(packet_obligation_open_next_candidates)
            .unwrap_or_default()
    };
    let reported_claim_open_next_paths = if terminally_sufficient || obligations.is_none() {
        // A finalized sufficient packet has no material work left. Diagnostic claim and guard
        // receipts stay visible in the coverage report, but cannot contradict the stop signal by
        // reopening any carrier path. Legacy no-ledger callers likewise keep their previous hint
        // behavior; typed obligation state is what makes a reported carrier actionable here.
        BTreeSet::new()
    } else {
        supported_claims
            .iter()
            .filter(|claim| {
                claim.proof_status == Some(codestory_contracts::api::PacketProofStatusDto::Reported)
                    || claim.eligible_for_sufficiency == Some(false)
            })
            .flat_map(|claim| claim.citations.iter())
            .filter_map(|citation| citation.file_path.as_ref())
            .map(|path| packet_display_path(path))
            .collect::<BTreeSet<_>>()
    };
    let unproven_obligation_carrier_paths = obligations
        .map(|obligations| {
            obligations
                .claim_obligations
                .iter()
                .filter(|obligation| {
                    obligation.proof_status != PacketObligationProofStatusDto::Proven
                })
                .flat_map(|obligation| obligation.carrier_paths.iter())
                .map(|path| packet_display_path(path))
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let mut open_next_paths = if terminally_sufficient {
        Vec::new()
    } else {
        missing_exact_path_claims.clone()
    };
    for path in obligation_open_next_paths {
        push_unique_sufficiency_term(&mut open_next_paths, &path);
    }
    for path in &reported_claim_open_next_paths {
        push_unique_sufficiency_term(&mut open_next_paths, path);
    }
    // Filtered once, after every source has contributed, because leads arrive
    // from three of them. Capping on coverage flips `terminally_sufficient`
    // false, which is exactly what opens follow-up generation — so without
    // this the cap turns a packet that answered and stopped into one that
    // re-probes a permanently unindexable file every round. A path the index
    // can never cover is not a lead.
    let unprovable_paths = coverage.unprovable_paths();
    if !unprovable_paths.is_empty() {
        // Leads arrive as `packet_display_path` output, which strips a named
        // repository root: a path under a cached checkout keeps only its
        // in-repository suffix. Joining the project root back onto that suffix
        // yields a path that does not exist, so a path-identity comparison
        // reports "different file" and the lead survives — leaving this filter
        // inert for exactly the cached-repository packets where the re-probe
        // loop it prevents actually bites. Comparing display form to display
        // form keeps both sides in one vocabulary; the identity comparison
        // stays as the fallback for leads that were never stripped.
        let unprovable_display = unprovable_paths
            .iter()
            .map(|path| packet_display_path(path))
            .collect::<Vec<_>>();
        open_next_paths.retain(|path| {
            let display = packet_display_path(path);
            !unprovable_display
                .iter()
                .any(|unprovable| unprovable == &display)
                && !unprovable_paths.iter().any(|unprovable| {
                    packet_paths_match_exact_probe(path_identity, project_root, unprovable, path)
                })
        });
    }
    let blocking_follow_up_probe_queries = packet_interleave_follow_up_queries(
        &open_next_paths,
        &blocking_follow_up_probe_query_seeds,
    );
    let follow_up_probe_queries = &blocking_follow_up_probe_queries;
    let targeted_follow_up_queries = targeted_follow_up_queries
        .into_iter()
        .filter(|query| {
            !missing_required_probe_queries
                .iter()
                .any(|missing| missing == query)
                || blocking_probe_queries
                    .iter()
                    .any(|blocking| blocking == query)
        })
        .collect::<Vec<_>>();
    let mut follow_up_argv = if terminally_sufficient {
        Vec::new()
    } else {
        packet_follow_up_argv(
            project_root,
            question,
            status,
            budget,
            follow_up_probe_queries,
            targeted_follow_up_queries,
            packet_full_retrieval_available(answer),
        )
    };
    let coverage_report = packet_coverage_report(PacketCoverageReportInput {
        supported_claims: &supported_claims,
        proven_claims: &proven_claims,
        missing_required_flow_requirements: &missing_required_flow_requirements,
        route_proof: &route_proof,
        missing_exact_path_claims: &missing_exact_path_claims,
        unresolved_sidecar_queries: &unresolved_sidecar_queries,
        budget,
        has_sufficiency_blocking_budget_omission,
    });
    if !terminally_sufficient && !open_next_paths.is_empty() {
        let project = packet_display_project_arg(project_root);
        let candidate_commands = if packet_full_retrieval_available(answer) {
            packet_follow_up_search_argv(&project, &open_next_paths)
        } else {
            packet_follow_up_trail_argv(&project, &open_next_paths)
        };
        for command in candidate_commands {
            let rendered = render_packet_command(&command);
            if !follow_up_argv
                .iter()
                .any(|existing| render_packet_command(existing) == rendered)
            {
                follow_up_argv.push(command);
            }
        }
    }
    follow_up_argv.truncate(12);
    let follow_up_commands = follow_up_argv
        .iter()
        .map(|argv| render_packet_command(argv))
        .collect::<Vec<_>>();
    let follow_up_invocations = follow_up_argv
        .iter()
        .map(|argv| packet_follow_up_invocation(argv))
        .collect::<Vec<_>>();
    let open_next = follow_up_commands.clone();
    debug_assert!(
        !terminally_sufficient
            || (open_next_paths.is_empty()
                && follow_up_commands.is_empty()
                && open_next.is_empty()),
        "a Sufficient packet is terminal and cannot publish open-next paths or commands"
    );
    // Only a file the packet actually proved something about is one the caller can skip opening.
    let proven_obligation_paths = obligations.map(|obligations| {
        packet_proven_obligation_carrier_paths(obligations)
            .into_iter()
            .map(|path| packet_display_path(&path))
            .collect::<BTreeSet<_>>()
    });
    let avoid_opening_paths = proven_claims
        .iter()
        .flat_map(|claim| &claim.citations)
        .filter(|citation| citation_sufficiency_eligible(citation))
        .filter_map(|citation| citation.file_path.as_ref())
        .map(|path| packet_display_path(path))
        .filter(|path| {
            proven_obligation_paths
                .as_ref()
                .is_none_or(|ledger_paths| ledger_paths.contains(path))
        })
        .filter(|path| !reported_claim_open_next_paths.contains(path))
        .filter(|path| !unproven_obligation_carrier_paths.contains(path))
        .filter(|path| !missing_exact_path_claims.contains(path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(12)
        .collect::<Vec<_>>();
    let avoid_opening = avoid_opening_paths
        .iter()
        .map(|path| {
            format!(
                "{} because this packet already includes a citation for the current answer.",
                path
            )
        })
        .collect::<Vec<_>>();

    PacketSufficiencyDto {
        status,
        // Callers read covered claims as verified and safe to repeat. Publishing a claim the same
        // packet reports as unproven is the claim-level shape of the false-safe answer #1200 exists
        // to remove; the coverage report still carries every dropped claim with its reason.
        covered_claims: proven_claims,
        open_next,
        avoid_opening,
        avoid_opening_paths,
        gaps,
        follow_up_commands,
        follow_up_invocations,
        coverage_report: Some(coverage_report),
    }
}

pub fn packet_targeted_follow_up_queries(
    question: &str,
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    packet_symbol_probe_queries(question, task_class, PacketBudgetModeDto::Standard)
        .into_iter()
        .filter(|query| is_packet_structured_follow_up_query(query))
        .take(6)
        .collect()
}

fn is_packet_structured_follow_up_query(query: &str) -> bool {
    query.contains('_')
        || query.contains("::")
        || query.contains("Options")
        || query.contains("Params")
        || query.contains("Processor")
        || query.contains("Subcommand")
}

fn packet_eligible_citation_count(answer: &AgentAnswerDto) -> usize {
    let mut seen = HashSet::new();
    answer
        .citations
        .iter()
        .filter(|citation| citation_sufficiency_eligible(citation))
        .filter(|citation| seen.insert(packet_citation_key(citation)))
        .count()
}

struct PacketSufficiencyStatusInput<'a> {
    budget: &'a PacketBudgetDto,
    eligible_citation_count: usize,
    has_errors: bool,
    has_minimum_coverage: bool,
    has_minimum_claims: bool,
    has_minimum_claim_families: bool,
    has_required_flow_roles: bool,
    has_route_proof: bool,
    missing_exact_path_claims: &'a [String],
    has_sufficiency_blocking_budget_omission: bool,
    missing_required_probe_queries: &'a [String],
    unresolved_sidecar_queries: &'a [String],
    /// EV-7: how well this packet's publication was known to match the working tree.
    freshness: PacketFreshnessInput,
    /// CAP-1: whether the index actually covered the files this packet rested on.
    ///
    /// Distinct from `has_minimum_coverage`, which is about how many *claims* carry evidence.
    /// This is about whether the underlying files were indexed at all.
    coverage: &'a PacketCoverageInput,
    /// EV-8: whether the primary retrieval run lost evidence it planned to collect.
    primary_retrieval_truncated: bool,
}

fn packet_sufficiency_status(
    input: PacketSufficiencyStatusInput<'_>,
) -> PacketSufficiencyStatusDto {
    if input.eligible_citation_count == 0 {
        PacketSufficiencyStatusDto::Insufficient
    } else if input.has_errors
        || !input.has_minimum_coverage
        || !input.has_minimum_claims
        || !input.has_minimum_claim_families
        || !input.has_required_flow_roles
        || !input.has_route_proof
        || !input.missing_exact_path_claims.is_empty()
        || !input.missing_required_probe_queries.is_empty()
        || !input.unresolved_sidecar_queries.is_empty()
        || input.has_sufficiency_blocking_budget_omission
        // These three are caps, not floors: they can only stop a packet that would otherwise be
        // Sufficient from claiming it. A packet with no eligible citation stays Insufficient
        // above, and everything that was already Partial stays Partial.
        || input.freshness.caps_sufficiency()
        || input.coverage.caps_sufficiency()
        || input.primary_retrieval_truncated
        || packet_budget_exceeded_hard_output_cap(input.budget)
    {
        PacketSufficiencyStatusDto::Partial
    } else {
        PacketSufficiencyStatusDto::Sufficient
    }
}

/// An uncovered exact path is reported as its own gap so a caller can act on one path at a time.
/// The list stays bounded the same way route stages do; the coverage report keeps the full set.
const MAX_EXACT_PATH_CLAIM_GAPS: usize = 6;
const MAX_ROUTE_PROOF_STAGES: usize = 6;
const MAX_ROUTE_STAGE_WORDS: usize = 6;
const ROUTE_ORDER_GAP: &str = "RouteTracing packet could not resolve at least two ordered endpoints from explicit route syntax in the question.";
const ROUTE_GRAPH_GAP: &str =
    "RouteTracing packet did not include a directed execution graph for the cited route endpoints.";
const ROUTE_FRAGMENT_GAP: &str = "RouteTracing evidence appeared only in separate graph neighborhoods; no single execution graph represented the claimed ordered route.";

#[derive(Debug, Clone)]
struct RouteStageEvidence {
    label: String,
    node_ids: Vec<String>,
}

#[derive(Debug, Clone)]
struct RouteProofAssessment {
    complete: bool,
    gaps: Vec<String>,
    missing: Vec<String>,
    follow_up_queries: Vec<String>,
}

impl RouteProofAssessment {
    fn not_required() -> Self {
        Self {
            complete: true,
            gaps: Vec::new(),
            missing: Vec::new(),
            follow_up_queries: Vec::new(),
        }
    }

    fn blocked(gap: String, missing: Vec<String>, follow_up_queries: Vec<String>) -> Self {
        Self {
            complete: false,
            gaps: vec![gap],
            missing,
            follow_up_queries,
        }
    }
}

fn packet_route_proof_assessment(
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    claims: &[PacketClaimDto],
    stages: &[String],
    selected_probes: &[String],
    obligations: Option<&PacketObligationPlanDto>,
) -> RouteProofAssessment {
    if task_class != PacketTaskClassDto::RouteTracing {
        return RouteProofAssessment::not_required();
    }
    if stages.len() < 2 {
        return RouteProofAssessment::blocked(
            ROUTE_ORDER_GAP.to_string(),
            vec!["route order: unresolved endpoints".to_string()],
            stages.to_vec(),
        );
    }
    if stages.len() > MAX_ROUTE_PROOF_STAGES {
        let omitted = stages[MAX_ROUTE_PROOF_STAGES..].to_vec();
        return RouteProofAssessment::blocked(
            format!(
                "RouteTracing route proof exceeds the bounded {MAX_ROUTE_PROOF_STAGES}-stage capacity; unrepresented required stage(s): {}.",
                omitted.join(", ")
            ),
            omitted
                .iter()
                .map(|stage| format!("route stage overflow: {stage}"))
                .collect(),
            omitted,
        );
    }

    let mut evidence = Vec::new();
    let mut missing = Vec::new();
    for stage in stages {
        let mut node_ids = claims
            .iter()
            .flat_map(|claim| packet_route_claim_node_ids(stage, selected_probes, claim))
            .collect::<Vec<_>>();
        if let Some(obligations) = obligations {
            node_ids.extend(packet_route_obligation_node_ids(
                stage,
                selected_probes,
                answer,
                obligations,
            ));
        }
        node_ids.sort();
        node_ids.dedup();
        if node_ids.is_empty() {
            missing.push(stage.clone());
        } else {
            evidence.push(RouteStageEvidence {
                label: stage.clone(),
                node_ids,
            });
        }
    }
    if !missing.is_empty() {
        return RouteProofAssessment::blocked(
            format!(
                "RouteTracing packet missed relevant cited route endpoint(s): {}.",
                missing.join(", ")
            ),
            missing
                .iter()
                .map(|stage| format!("route endpoint: {stage}"))
                .collect(),
            missing,
        );
    }

    let graphs = packet_execution_graphs(answer);
    if graphs.is_empty() {
        return RouteProofAssessment::blocked(
            ROUTE_GRAPH_GAP.to_string(),
            vec!["route execution graph".to_string()],
            stages.to_vec(),
        );
    }
    let missing_transitions = packet_missing_route_transitions(&graphs, &evidence);
    let has_complete_graph = graphs
        .iter()
        .any(|graph| packet_graph_contains_route(graph, &evidence));
    if !missing_transitions.is_empty() {
        return RouteProofAssessment::blocked(
            format!(
                "RouteTracing execution graph missed ordered transition(s): {}.",
                missing_transitions.join(", ")
            ),
            missing_transitions
                .iter()
                .map(|transition| format!("route transition: {transition}"))
                .collect(),
            missing_transitions
                .iter()
                .filter_map(|transition| {
                    transition
                        .split_once(" -> ")
                        .map(|(_, target)| target.to_string())
                })
                .collect(),
        );
    }
    if !has_complete_graph {
        return RouteProofAssessment::blocked(
            ROUTE_FRAGMENT_GAP.to_string(),
            vec!["route execution graph".to_string()],
            Vec::new(),
        );
    }
    RouteProofAssessment::not_required()
}

fn packet_route_obligation_node_ids(
    stage: &str,
    selected_probes: &[String],
    answer: &AgentAnswerDto,
    obligations: &PacketObligationPlanDto,
) -> Vec<String> {
    let endpoint_citation_ids = answer
        .citations
        .iter()
        .filter(|citation| packet_route_citation_is_endpoint(citation))
        .map(|citation| &citation.node_id)
        .collect::<HashSet<_>>();
    obligations
        .claim_obligations
        .iter()
        .filter(|obligation| {
            obligation.material
                && obligation.proof_status == PacketObligationProofStatusDto::Proven
                && packet_route_obligation_binds_stage(obligation, stage, selected_probes)
        })
        .flat_map(|obligation| &obligation.carrier_node_ids)
        .filter(|node_id| endpoint_citation_ids.contains(node_id))
        .map(|node_id| node_id.0.clone())
        .collect()
}

fn packet_route_obligation_binds_stage(
    obligation: &PacketClaimObligationDto,
    stage: &str,
    selected_probes: &[String],
) -> bool {
    obligation
        .binding_terms
        .iter()
        .any(|binding| packet_route_exact_binding_matches_stage(binding, stage, selected_probes))
        || obligation.probe_binding.as_ref().is_some_and(|binding| {
            let mut exact_bindings = Vec::new();
            match &binding.probe {
                PacketProbeDto::ExactPath { path } => {
                    exact_bindings.push(path.as_str());
                    if let Some(path) = binding.path.as_deref() {
                        exact_bindings.push(path);
                    }
                }
                PacketProbeDto::SymbolId { id } => {
                    exact_bindings.push(id.as_str());
                    if let Some(symbol_id) = binding.symbol_id.as_deref() {
                        exact_bindings.push(symbol_id);
                    }
                }
                PacketProbeDto::FileSymbol { path, symbol } => {
                    exact_bindings.push(path.as_str());
                    exact_bindings.push(symbol.as_str());
                    if let Some(path) = binding.path.as_deref() {
                        exact_bindings.push(path);
                    }
                    if let Some(symbol_id) = binding.symbol_id.as_deref() {
                        exact_bindings.push(symbol_id);
                    }
                }
                PacketProbeDto::Continuation { symbol_id, .. } => {
                    if let Some(symbol_id) = symbol_id.as_deref() {
                        exact_bindings.push(symbol_id);
                    }
                    if let Some(symbol_id) = binding.symbol_id.as_deref() {
                        exact_bindings.push(symbol_id);
                    }
                }
                PacketProbeDto::FreeQuery { .. } => {}
            }
            exact_bindings.into_iter().any(|exact_binding| {
                packet_route_exact_binding_matches_stage(exact_binding, stage, selected_probes)
            })
        })
}

fn packet_route_exact_binding_matches_stage(
    binding: &str,
    stage: &str,
    selected_probes: &[String],
) -> bool {
    packet_route_exact_identities_overlap(binding, stage)
        || selected_probes.iter().any(|probe| {
            packet_route_probe_is_unscoped(probe)
                && packet_route_labels_overlap(stage, probe)
                && packet_route_exact_identities_overlap(binding, probe)
        })
}

fn packet_route_exact_identities_overlap(left: &str, right: &str) -> bool {
    let left = packet_route_exact_identity_segments(left);
    let right = packet_route_exact_identity_segments(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left.len() == 1 || right.len() == 1 {
        return left.last() == right.last();
    }
    left == right
}

fn packet_route_exact_identity_segments(value: &str) -> Vec<&str> {
    value
        .split([':', '.', '#', '/', '\\'])
        .map(str::trim)
        .map(|segment| segment.strip_suffix("()").unwrap_or(segment))
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn packet_route_proof_stages(question: &str, selected_probes: &[String]) -> Vec<String> {
    packet_route_stage_labels(question, selected_probes)
}

fn packet_route_stage_labels(question: &str, selected_probes: &[String]) -> Vec<String> {
    let question = question.replace('→', "->");
    let spans = if question.contains("->") {
        question.split("->").map(str::to_string).collect()
    } else {
        let words = question.split_whitespace().collect::<Vec<_>>();
        let from = words
            .iter()
            .position(|word| packet_route_word_is(word, "from"));
        let route_words = from.map_or(words.as_slice(), |index| &words[index + 1..]);
        let markers = route_words
            .iter()
            .enumerate()
            .filter_map(|(index, word)| {
                ["through", "via", "to"]
                    .iter()
                    .any(|marker| packet_route_word_is(word, marker))
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        if markers.is_empty() {
            return Vec::new();
        }
        let mut spans = Vec::new();
        let mut start = 0;
        for marker in markers {
            spans.push(route_words[start..marker].join(" "));
            start = marker + 1;
        }
        spans.push(route_words[start..].join(" "));
        spans
    };
    spans
        .iter()
        .map(|span| packet_route_stage_label(span, selected_probes))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

fn packet_route_stage_label(span: &str, selected_probes: &[String]) -> Option<String> {
    let span = span.trim();
    match packet_route_quoted_identifier(span) {
        Ok(Some(label)) => return Some(label),
        Ok(None) => {}
        Err(()) => return None,
    }
    let span = packet_route_clean_word(span);
    if span.is_empty() {
        return None;
    }
    let words = span
        .split_whitespace()
        .map(packet_route_clean_word)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.len() == 1 {
        let word = words[0];
        return (packet_route_token_is_explicit_identifier(word)
            || packet_route_token_is_bare_lowercase(word))
        .then(|| word.to_string());
    }
    if words.len() > MAX_ROUTE_STAGE_WORDS
        || !words
            .iter()
            .all(|word| packet_route_token_is_bare_lowercase(word))
    {
        return None;
    }
    let label = words.join(" ");
    packet_route_label_matches_selected_probe(&label, selected_probes).then_some(label)
}

fn packet_route_quoted_identifier(span: &str) -> Result<Option<String>, ()> {
    let mut identifier = None;
    let mut active_quote = None;
    let mut current = String::new();
    let mut outside = String::new();
    for character in span.chars() {
        if let Some(quote) = active_quote {
            if character == quote {
                if current.trim().is_empty() || identifier.is_some() {
                    return Err(());
                }
                identifier = Some(current.trim().to_string());
                active_quote = None;
                current.clear();
            } else {
                current.push(character);
            }
        } else if matches!(character, '`' | '\'' | '"') {
            active_quote = Some(character);
        } else {
            outside.push(character);
        }
    }
    if active_quote.is_some() {
        return Err(());
    }
    if identifier.is_some() && !packet_route_clean_word(outside.trim()).is_empty() {
        return Err(());
    }
    Ok(identifier)
}

fn packet_route_token_is_explicit_identifier(token: &str) -> bool {
    let token = packet_route_clean_word(token);
    token.contains("::")
        || token.contains(['/', '\\', '_', '#', '.'])
        || token
            .chars()
            .skip(1)
            .any(|character| character.is_ascii_uppercase())
}

fn packet_route_token_is_bare_lowercase(token: &str) -> bool {
    let token = packet_route_clean_word(token);
    !token.is_empty()
        && token
            .chars()
            .any(|character| character.is_ascii_lowercase())
        && token
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

fn packet_route_clean_word(word: &str) -> &str {
    word.trim_matches(|character: char| {
        character.is_ascii_punctuation() && !matches!(character, '_' | ':' | '/' | '\\' | '#')
    })
}

fn packet_route_word_is(word: &str, marker: &str) -> bool {
    packet_route_clean_word(word).eq_ignore_ascii_case(marker)
}

fn packet_route_claim_node_ids(
    stage: &str,
    selected_probes: &[String],
    claim: &PacketClaimDto,
) -> Vec<String> {
    if claim.eligible_for_sufficiency == Some(false)
        || claim.coverage_role.as_deref() == Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE)
    {
        return Vec::new();
    }
    claim
        .citations
        .iter()
        .filter(|citation| packet_route_citation_is_endpoint(citation))
        .filter(|citation| {
            packet_route_label_matches_citation(stage, citation)
                || selected_probes.iter().any(|probe| {
                    packet_route_probe_is_unscoped(probe)
                        && packet_route_labels_overlap(stage, probe)
                        && packet_route_label_matches_citation(probe, citation)
                })
        })
        .map(|citation| citation.node_id.0.clone())
        .collect()
}

fn packet_route_claim_binds_stage(
    stages: &[String],
    selected_probes: &[String],
    claim: &PacketClaimDto,
) -> bool {
    stages
        .iter()
        .any(|stage| !packet_route_claim_node_ids(stage, selected_probes, claim).is_empty())
}

fn packet_route_labels_overlap(left: &str, right: &str) -> bool {
    let left = packet_route_identifier_tokens(left);
    !left.is_empty() && left == packet_route_identifier_tokens(right)
}

fn packet_route_label_matches_selected_probe(label: &str, selected_probes: &[String]) -> bool {
    selected_probes.iter().any(|probe| {
        packet_route_probe_is_unscoped(probe) && packet_route_labels_overlap(label, probe)
    })
}

fn packet_route_probe_is_unscoped(probe: &str) -> bool {
    let bare = packet_route_clean_word(probe.trim()).trim();
    !bare.is_empty()
        && !bare.contains(['/', '\\', ':', '#', '.'])
        && !bare.contains(['`', '\'', '"'])
        && packet_route_identifier_tokens(bare).len() <= MAX_ROUTE_STAGE_WORDS
}

fn packet_route_identifier_tokens(identifier: &str) -> Vec<String> {
    let characters = identifier.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut current = String::new();
    for (index, character) in characters.iter().copied().enumerate() {
        if !character.is_ascii_alphanumeric() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        let previous = index.checked_sub(1).and_then(|index| characters.get(index));
        let next = characters.get(index + 1);
        let camel_boundary = character.is_ascii_uppercase()
            && previous.is_some_and(|previous| {
                previous.is_ascii_lowercase()
                    || previous.is_ascii_digit()
                    || (previous.is_ascii_uppercase()
                        && next.is_some_and(|next| next.is_ascii_lowercase()))
            });
        if camel_boundary && !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
        current.push(character.to_ascii_lowercase());
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens.sort();
    tokens
}

fn packet_route_label_matches_citation(label: &str, citation: &AgentCitationDto) -> bool {
    let normalized_label = normalize_identifier(label);
    let display = citation.display_name.as_str();
    let terminal = display
        .rsplit(['.', ':', '#', '/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(display);
    normalize_identifier(display) == normalized_label
        || normalize_identifier(terminal) == normalized_label
        || citation.file_path.as_deref().is_some_and(|path| {
            let path = path.replace('\\', "/");
            let label = label.replace('\\', "/");
            path == label || path.ends_with(&format!("/{label}"))
        })
}

fn packet_route_citation_is_endpoint(citation: &AgentCitationDto) -> bool {
    let terminal = citation
        .display_name
        .rsplit(['.', ':', '#'])
        .next()
        .map(normalize_identifier)
        .unwrap_or_default();
    citation_sufficiency_eligible(citation)
        && matches!(
            citation.kind,
            NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
        )
        && !packet_display_name_is_test_like(&citation.display_name)
        && citation.file_path.as_deref().is_some_and(|path| {
            crate::text::retrieval_file_role_from_path(path)
                == crate::text::RetrievalFileRole::Source
        })
        && !matches!(
            terminal.as_str(),
            "helper" | "helpers" | "util" | "utils" | "utility" | "generichelper"
        )
}

fn packet_graph_contains_route(graph: &GraphResponse, stages: &[RouteStageEvidence]) -> bool {
    let mut reachable = stages
        .first()
        .into_iter()
        .flat_map(|stage| &stage.node_ids)
        .cloned()
        .collect::<HashSet<_>>();
    for stage in stages.iter().skip(1) {
        let next = stage
            .node_ids
            .iter()
            .filter(|target| {
                reachable.iter().any(|source| {
                    source != *target && packet_execution_path_exists(graph, source, target)
                })
            })
            .cloned()
            .collect::<HashSet<_>>();
        if next.is_empty() {
            return false;
        }
        reachable = next;
    }
    !reachable.is_empty()
}

fn packet_missing_route_transitions(
    graphs: &[&GraphResponse],
    stages: &[RouteStageEvidence],
) -> Vec<String> {
    stages
        .windows(2)
        .filter_map(|pair| {
            let [source, target] = pair else {
                return None;
            };
            let found = graphs.iter().any(|graph| {
                source.node_ids.iter().any(|source_id| {
                    target.node_ids.iter().any(|target_id| {
                        source_id != target_id
                            && packet_execution_path_exists(graph, source_id, target_id)
                    })
                })
            });
            (!found).then(|| format!("{} -> {}", source.label, target.label))
        })
        .collect()
}

pub fn packet_execution_path_exists(graph: &GraphResponse, source: &str, target: &str) -> bool {
    if source == target {
        return false;
    }
    let mut queue = VecDeque::from([source.to_string()]);
    let mut visited = HashSet::from([source.to_string()]);
    while let Some(current) = queue.pop_front() {
        for edge in graph.edges.iter().filter(|edge| {
            edge.kind == EdgeKind::CALL
                && edge.source.0 == current
                && edge.source != edge.target
                && !crate::trail::is_speculative_trail_edge(edge)
        }) {
            if edge.target.0 == target {
                return true;
            }
            if visited.insert(edge.target.0.clone()) {
                queue.push_back(edge.target.0.clone());
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn packet_sufficiency_gaps(
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    min_citations: usize,
    eligible_citation_count: usize,
    min_claims: usize,
    supported_claim_count: usize,
    claim_family_count: usize,
    min_claim_families: usize,
    generic_navigation_claim_count: usize,
    status: PacketSufficiencyStatusDto,
    has_minimum_coverage: bool,
    has_minimum_claims: bool,
    has_minimum_claim_families: bool,
    has_required_flow_roles: bool,
    route_proof: &RouteProofAssessment,
    missing_exact_path_claims: &[String],
    has_sufficiency_blocking_budget_omission: bool,
    missing_required_probe_queries: &[String],
    missing_required_flow_requirements: &[FlowRequirement],
    unresolved_sidecar_queries: &[String],
    freshness: PacketFreshnessInput,
    coverage: &PacketCoverageInput,
    primary_retrieval_truncated: bool,
) -> Vec<String> {
    let mut gaps = Vec::new();
    // The collection-condition gaps lead: they explain why an otherwise complete packet is still
    // capped, and a caller that only reads the first gap needs that fact before the claim counts.
    if let Some(gap) = freshness.gap() {
        gaps.push(gap);
    }
    gaps.extend(coverage.gaps());
    if primary_retrieval_truncated {
        gaps.push(
            "primary retrieval truncated: the primary retrieval run ended before collecting the \
             evidence it planned to collect, so absence of a result here is not evidence of \
             absence."
                .to_string(),
        );
    }
    if answer.citations.is_empty() {
        gaps.push("No cited anchors were found for the question.".to_string());
    } else if eligible_citation_count == 0 {
        gaps.push("No sufficiency-eligible cited anchors were found for the question.".to_string());
    }
    if eligible_citation_count > 0 && !has_minimum_coverage {
        gaps.push(format!(
            "{:?} packet found only {} cited anchor(s); at least {} are required before treating the packet as sufficient.",
            task_class,
            eligible_citation_count,
            min_citations
        ));
    }
    if eligible_citation_count > 0 && !has_minimum_claims {
        gaps.push(format!(
            "{:?} packet found only {} role-backed claim(s); at least {} are required before treating the packet as sufficient.",
            task_class, supported_claim_count, min_claims
        ));
    }
    if generic_navigation_claim_count > 0 && !has_minimum_claims {
        gaps.push(format!(
            "{generic_navigation_claim_count} generic navigation claim(s) were ignored for sufficiency because they only point at evidence instead of explaining the flow."
        ));
    }
    if eligible_citation_count > 0 && !has_minimum_claim_families {
        gaps.push(format!(
            "{:?} packet covered only {} distinct claim families; at least {} are required before treating the packet as sufficient.",
            task_class,
            claim_family_count,
            min_claim_families
        ));
    }
    if eligible_citation_count > 0 && !has_required_flow_roles {
        let missing_labels = missing_required_flow_requirements
            .iter()
            .map(flow_requirement_missing_label)
            .collect::<Vec<_>>()
            .join(", ");
        gaps.push(format!(
            "{:?} packet missed required structural coverage: {}.",
            task_class, missing_labels
        ));
    }
    if task_class == PacketTaskClassDto::RouteTracing && !route_proof.complete {
        gaps.extend(route_proof.gaps.clone());
    }
    for path in missing_exact_path_claims
        .iter()
        .take(MAX_EXACT_PATH_CLAIM_GAPS)
    {
        gaps.push(format!(
            "{task_class:?} packet did not establish a proof-bearing claim from explicit exact path: {path}."
        ));
    }
    if let Some(overflow) = missing_exact_path_claims
        .len()
        .checked_sub(MAX_EXACT_PATH_CLAIM_GAPS)
        .filter(|overflow| *overflow > 0)
    {
        gaps.push(format!(
            "{task_class:?} packet left {overflow} further requested exact path(s) without a proof-bearing claim; the coverage report names each one."
        ));
    }
    if !missing_required_probe_queries.is_empty() {
        gaps.push(format!(
            "{:?} packet missed required planned flow probe(s): {}.",
            task_class,
            missing_required_probe_queries.join(", ")
        ));
    }
    if !unresolved_sidecar_queries.is_empty() {
        gaps.push(format!(
            "{:?} packet had sidecar candidates that could not resolve to indexed symbols for: {}.",
            task_class,
            unresolved_sidecar_queries.join(", ")
        ));
    }
    if budget.truncated && status != PacketSufficiencyStatusDto::Sufficient {
        gaps.push(format!(
            "Packet was truncated by {:?} budget: {}.",
            budget.requested,
            budget.omitted_sections.join(", ")
        ));
    }
    if has_sufficiency_blocking_budget_omission {
        gaps.push(format!(
            "Packet omitted answer-critical evidence under {:?} budget; use a deeper packet before treating this as complete.",
            budget.requested
        ));
    }
    for step in answer
        .retrieval_trace
        .steps
        .iter()
        .filter(|step| step.status == AgentRetrievalStepStatusDto::Error)
    {
        gaps.push(format!("{:?} step failed.", step.kind));
    }
    gaps
}

fn unresolved_sidecar_queries(answer: &AgentAnswerDto) -> Vec<String> {
    let mut seen = HashSet::new();
    answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .iter()
        .filter(|diagnostic| sidecar_diagnostic_blocks_sufficiency(diagnostic))
        .filter(|diagnostic| seen.insert(diagnostic.query.clone()))
        .map(|diagnostic| diagnostic.query.clone())
        .collect()
}

fn sidecar_diagnostic_blocks_sufficiency(diagnostic: &PacketSidecarQueryDiagnosticDto) -> bool {
    if diagnostic.blocking_unresolved_candidate_count > 0 {
        return true;
    }
    matches!(
        diagnostic.completion,
        codestory_contracts::api::PacketQueryCompletionDto::Cancelled { .. }
    )
}

fn packet_incomplete_material_query_obligations(
    obligations: &PacketObligationPlanDto,
) -> Vec<String> {
    obligations
        .query_obligations
        .iter()
        .filter(|obligation| obligation.material)
        .filter(|obligation| {
            !matches!(
                obligation.completion,
                Some(PacketQueryCompletionDto::Completed)
            )
        })
        .map(|obligation| obligation.query.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn append_packet_obligation_gaps(gaps: &mut Vec<String>, obligations: &PacketObligationPlanDto) {
    for obligation in obligations.claim_obligations.iter().filter(|obligation| {
        obligation.proof_status != PacketObligationProofStatusDto::Proven
            && (obligation.material || !obligation.carrier_node_ids.is_empty())
    }) {
        push_unique_sufficiency_term(
            gaps,
            &format!(
                "obligation {} ({:?}) is {:?}: {}",
                obligation.id,
                obligation.kind,
                obligation.proof_status,
                obligation.reason.as_deref().unwrap_or("reason_unavailable")
            ),
        );
    }
    for obligation in obligations
        .query_obligations
        .iter()
        .filter(|obligation| obligation.material)
        .filter(|obligation| {
            !matches!(
                obligation.completion,
                Some(PacketQueryCompletionDto::Completed)
            )
        })
    {
        let reason = match obligation.completion.as_ref() {
            Some(PacketQueryCompletionDto::Cancelled { reason }) => reason.as_str(),
            Some(PacketQueryCompletionDto::Completed) => continue,
            None => "completion_missing",
        };
        push_unique_sufficiency_term(
            gaps,
            &format!(
                "query obligation {} ({:?}) is cancelled: {}",
                obligation.id, obligation.kind, reason
            ),
        );
    }
}

fn packet_sufficiency_min_citations(task_class: PacketTaskClassDto) -> usize {
    match task_class {
        PacketTaskClassDto::BugLocalization | PacketTaskClassDto::SymbolOwnership => 2,
        PacketTaskClassDto::ArchitectureExplanation
        | PacketTaskClassDto::ChangeImpact
        | PacketTaskClassDto::RouteTracing
        | PacketTaskClassDto::DataFlow
        | PacketTaskClassDto::EditPlanning => 3,
    }
}

fn packet_sufficiency_min_claims(task_class: PacketTaskClassDto) -> usize {
    match task_class {
        PacketTaskClassDto::BugLocalization | PacketTaskClassDto::SymbolOwnership => 1,
        PacketTaskClassDto::ArchitectureExplanation => 3,
        PacketTaskClassDto::ChangeImpact
        | PacketTaskClassDto::RouteTracing
        | PacketTaskClassDto::DataFlow
        | PacketTaskClassDto::EditPlanning => 2,
    }
}

fn packet_sufficiency_min_claims_with_obligations(
    task_class: PacketTaskClassDto,
    obligations: Option<&PacketObligationPlanDto>,
) -> usize {
    let baseline = packet_sufficiency_min_claims(task_class);
    let planned_material_claims = obligations.map_or(0, |plan| {
        plan.claim_obligations
            .iter()
            .filter(|obligation| {
                obligation.material
                    && obligation.kind
                        != codestory_contracts::api::PacketClaimObligationKindDto::ExactProbe
            })
            .count()
    });
    if planned_material_claims == 0 {
        baseline
    } else {
        baseline.min(planned_material_claims)
    }
}

fn packet_sufficiency_min_claim_families(task_class: PacketTaskClassDto) -> usize {
    match task_class {
        PacketTaskClassDto::ArchitectureExplanation => 3,
        PacketTaskClassDto::DataFlow => 2,
        PacketTaskClassDto::BugLocalization
        | PacketTaskClassDto::ChangeImpact
        | PacketTaskClassDto::RouteTracing
        | PacketTaskClassDto::SymbolOwnership
        | PacketTaskClassDto::EditPlanning => 1,
    }
}

fn packet_sufficiency_min_claim_families_with_obligations(
    task_class: PacketTaskClassDto,
    obligations: Option<&PacketObligationPlanDto>,
) -> usize {
    let baseline = packet_sufficiency_min_claim_families(task_class);
    let planned_material_families = obligations.map_or(0, |plan| {
        plan.claim_obligations
            .iter()
            .filter(|obligation| {
                obligation.material
                    && obligation.kind
                        != codestory_contracts::api::PacketClaimObligationKindDto::ExactProbe
            })
            .map(|obligation| obligation.kind)
            .collect::<HashSet<_>>()
            .len()
    });
    if planned_material_families == 0 {
        baseline
    } else {
        baseline.min(planned_material_families)
    }
}

pub fn packet_supported_claim_family_count(supported_claims: &[PacketClaimDto]) -> usize {
    let mut families: HashSet<&'static str> = HashSet::new();
    for claim in supported_claims {
        if let Some(family) = packet_claim_family(claim) {
            families.insert(family);
        }
    }
    families.len()
}

pub fn packet_claim_family(claim: &PacketClaimDto) -> Option<&'static str> {
    if claim.coverage_role.as_deref() == Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE) {
        return match claim.required_obligation_kinds.as_slice() {
            [codestory_contracts::api::PacketClaimObligationKindDto::Entrypoint] => {
                Some("planned entrypoint evidence")
            }
            [codestory_contracts::api::PacketClaimObligationKindDto::Dispatch] => {
                Some("planned dispatch evidence")
            }
            [codestory_contracts::api::PacketClaimObligationKindDto::Orchestration] => {
                Some("planned orchestration evidence")
            }
            [codestory_contracts::api::PacketClaimObligationKindDto::StateWrite] => {
                Some("planned state-write evidence")
            }
            [codestory_contracts::api::PacketClaimObligationKindDto::ExternalIo] => {
                Some("planned external-I/O evidence")
            }
            [codestory_contracts::api::PacketClaimObligationKindDto::ExactProbe] => None,
            _ => None,
        };
    }

    claim
        .citations
        .iter()
        .find_map(|citation| packet_evidence_role(citation).map(|role| role.as_str()))
        .or_else(|| (!claim.citations.is_empty()).then_some("source evidence"))
}

pub fn packet_claim_can_satisfy_sufficiency(claim: &PacketClaimDto) -> bool {
    packet_claim_ineligibility_reason(claim).is_none()
}

/// Sufficiency is a statement about proof, so a claim only counts when the packet actually carries
/// evidence for it: an unsupported sentence, diagnostic-only evidence, or navigation prose that
/// points at a citation without explaining the flow can never promote a verdict.
fn packet_claim_ineligibility_reason(claim: &PacketClaimDto) -> Option<&'static str> {
    if claim.eligible_for_sufficiency == Some(false) {
        return Some("claim marked diagnostic");
    }
    if claim.citations.is_empty() {
        return Some("claim carries no cited evidence");
    }
    if !claim.citations.iter().any(citation_sufficiency_eligible) {
        return Some("citation evidence is diagnostic-only");
    }
    if packet_claim_is_generic_navigation_or_source_evidence(claim) {
        return Some("generic navigation/source-evidence claim does not explain the flow");
    }
    None
}

fn packet_claim_is_generic_navigation_or_source_evidence(claim: &PacketClaimDto) -> bool {
    if claim
        .coverage_role
        .as_deref()
        .is_some_and(packet_role_label_is_generic_source_evidence)
    {
        return true;
    }
    let lower = claim.claim.to_ascii_lowercase();
    lower.contains("anchored by")
        || lower.contains("inspect it")
        || lower.contains("inspect the cited")
        || (lower.contains("supports ") && lower.contains("inspect"))
        || (lower.contains("ties ")
            && lower.contains(" to cited definitions")
            && lower.contains("adjacent ownership"))
        || (lower.contains(" is defined in cited source ") && lower.contains("exact source anchor"))
}

/// Every resolved in-project path the caller named must be carried by its own proof-bearing claim,
/// whatever the task class: a packet that answers around a requested path has not answered about it.
fn packet_missing_exact_path_claims(
    path_identity: &dyn WorkspacePathIdentity,
    project_root: &Path,
    exact_probe_paths: &[String],
    sufficiency_claims: &[PacketClaimDto],
) -> Vec<String> {
    let exact_probe_paths =
        exact_probe_paths
            .iter()
            .fold(Vec::<&String>::new(), |mut unique_paths, path| {
                if !unique_paths.iter().any(|existing| {
                    packet_paths_match_exact_probe(
                        path_identity,
                        project_root,
                        existing.as_str(),
                        path,
                    )
                }) {
                    unique_paths.push(path);
                }
                unique_paths
            });
    let candidate_claims = exact_probe_paths
        .iter()
        .map(|path| {
            sufficiency_claims
                .iter()
                .enumerate()
                .filter_map(|(claim_index, claim)| {
                    claim
                        .citations
                        .iter()
                        .any(|citation| {
                            citation_sufficiency_eligible(citation)
                                && citation.file_path.as_deref().is_some_and(|citation_path| {
                                    packet_paths_match_exact_probe(
                                        path_identity,
                                        project_root,
                                        citation_path,
                                        path.as_str(),
                                    )
                                })
                        })
                        .then_some(claim_index)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut assigned_path_by_claim = vec![None; sufficiency_claims.len()];
    let covered_paths = (0..exact_probe_paths.len())
        .map(|path_index| {
            let mut visited_claims = vec![false; sufficiency_claims.len()];
            packet_assign_exact_path_claim(
                path_index,
                &candidate_claims,
                &mut visited_claims,
                &mut assigned_path_by_claim,
            )
        })
        .collect::<Vec<_>>();

    exact_probe_paths
        .iter()
        .enumerate()
        .filter(|(path_index, _)| !covered_paths[*path_index])
        .map(|(_, path)| path)
        .map(|path| packet_display_path(path.as_str()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn packet_assign_exact_path_claim(
    path_index: usize,
    candidate_claims: &[Vec<usize>],
    visited_claims: &mut [bool],
    assigned_path_by_claim: &mut [Option<usize>],
) -> bool {
    for &claim_index in &candidate_claims[path_index] {
        if visited_claims[claim_index] {
            continue;
        }
        visited_claims[claim_index] = true;
        let can_assign = assigned_path_by_claim[claim_index].is_none_or(|assigned_path| {
            packet_assign_exact_path_claim(
                assigned_path,
                candidate_claims,
                visited_claims,
                assigned_path_by_claim,
            )
        });
        if can_assign {
            assigned_path_by_claim[claim_index] = Some(path_index);
            return true;
        }
    }
    false
}

fn packet_paths_match_exact_probe(
    path_identity: &dyn WorkspacePathIdentity,
    project_root: &Path,
    citation_path: &str,
    exact_probe_path: &str,
) -> bool {
    let absolute = |path: &str| {
        let path = Path::new(path.trim_start_matches("\\\\?\\"));
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            project_root.join(path)
        }
    };
    path_identity.same_workspace_path(
        absolute(citation_path).as_path(),
        absolute(exact_probe_path).as_path(),
    )
}

fn packet_role_label_is_generic_source_evidence(role: &str) -> bool {
    normalize_identifier(role) == "sourceevidence"
}

struct PacketCoverageReportInput<'a> {
    supported_claims: &'a [PacketClaimDto],
    proven_claims: &'a [PacketClaimDto],
    missing_required_flow_requirements: &'a [FlowRequirement],
    route_proof: &'a RouteProofAssessment,
    missing_exact_path_claims: &'a [String],
    unresolved_sidecar_queries: &'a [String],
    budget: &'a PacketBudgetDto,
    has_sufficiency_blocking_budget_omission: bool,
}

fn packet_coverage_report(input: PacketCoverageReportInput<'_>) -> PacketCoverageReportDto {
    let PacketCoverageReportInput {
        supported_claims,
        proven_claims,
        missing_required_flow_requirements,
        route_proof,
        missing_exact_path_claims,
        unresolved_sidecar_queries,
        budget,
        has_sufficiency_blocking_budget_omission,
    } = input;
    let covered = proven_claims
        .iter()
        .filter_map(packet_claim_coverage_label)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let ineligible = supported_claims
        .iter()
        .filter_map(|claim| {
            packet_claim_ineligibility_reason(claim)
                .map(|reason| packet_ineligible_claim_report_entry(claim, reason))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut missing = missing_required_flow_requirements
        .iter()
        .map(|requirement| requirement.id.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    for route_gap in &route_proof.missing {
        push_unique_sufficiency_term(&mut missing, route_gap);
    }
    for path in missing_exact_path_claims {
        push_unique_sufficiency_term(&mut missing, &format!("exact path: {path}"));
    }
    let budget_omitted = if has_sufficiency_blocking_budget_omission {
        budget.omitted_sections.clone()
    } else {
        Vec::new()
    };
    let provenance_counts = packet_provenance_counts(supported_claims);
    let provenance_labels = provenance_counts.keys().cloned().collect::<Vec<_>>();
    PacketCoverageReportDto {
        covered,
        provenance_labels,
        provenance_counts,
        missing,
        ineligible,
        unresolved: unresolved_sidecar_queries.to_vec(),
        budget_omitted,
    }
}

fn packet_provenance_counts(claims: &[PacketClaimDto]) -> BTreeMap<String, u32> {
    let mut counts = BTreeMap::new();
    for citation in claims.iter().flat_map(|claim| &claim.citations) {
        let labels = packet_citation_provenance_labels(citation);
        for label in labels {
            *counts.entry(label).or_insert(0) += 1;
        }
    }
    counts
}

fn packet_citation_provenance_labels(citation: &AgentCitationDto) -> BTreeSet<String> {
    let mut labels = citation
        .retrieval_score_breakdown
        .as_ref()
        .map(|breakdown| {
            breakdown
                .provenance
                .iter()
                .filter(|label| packet_pass_through_provenance_label(label))
                .cloned()
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    if let Some(tier) = citation.evidence_tier {
        if labels.is_empty() {
            labels.insert(packet_evidence_provenance_label(tier).to_string());
        }
    } else if let Some(breakdown) = citation.retrieval_score_breakdown.as_ref() {
        labels.extend(
            breakdown
                .provenance
                .iter()
                .filter(|label| packet_public_provenance_label(label))
                .cloned(),
        );
    }
    labels
}

fn packet_pass_through_provenance_label(label: &str) -> bool {
    matches!(label, "precise_semantic_import" | "same_file_name_affinity")
}

fn packet_public_provenance_label(label: &str) -> bool {
    packet_pass_through_provenance_label(label)
        || matches!(
            label,
            "exact"
                | "lexical_source"
                | "symbol_doc"
                | "graph_neighbor"
                | "component_report"
                | "dense_anchor"
                | "same_file_name_affinity"
        )
}

fn packet_claim_coverage_label(claim: &PacketClaimDto) -> Option<String> {
    if let Some(role) = claim
        .coverage_role
        .as_deref()
        .filter(|role| !packet_role_label_is_generic_source_evidence(role))
    {
        return Some(role.to_string());
    }
    packet_claim_family(claim)
        .filter(|role| !packet_role_label_is_generic_source_evidence(role))
        .map(str::to_string)
}

fn packet_ineligible_claim_report_entry(claim: &PacketClaimDto, reason: &str) -> String {
    format!(
        "claim=\"{}\" role=\"{}\" tier=\"{}\" reason=\"{}\"",
        packet_escape_coverage_report_value(&claim.claim),
        packet_escape_coverage_report_value(packet_claim_ineligible_role_label(claim).as_str()),
        packet_escape_coverage_report_value(packet_claim_tier_label(claim).as_str()),
        packet_escape_coverage_report_value(reason)
    )
}

fn packet_claim_ineligible_role_label(claim: &PacketClaimDto) -> String {
    claim
        .coverage_role
        .clone()
        .or_else(|| {
            claim
                .citations
                .iter()
                .find_map(|citation| packet_evidence_role(citation).map(|role| role.as_str()))
                .map(str::to_string)
        })
        .or_else(|| packet_claim_family(claim).map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn packet_claim_tier_label(claim: &PacketClaimDto) -> String {
    claim
        .citations
        .first()
        .and_then(|citation| citation.evidence_tier)
        .map(packet_evidence_tier_label)
        .unwrap_or("unknown")
        .to_string()
}

fn packet_evidence_tier_label(tier: PacketEvidenceTierDto) -> &'static str {
    match tier {
        PacketEvidenceTierDto::ExactSource => "exact_source",
        PacketEvidenceTierDto::StructuralText => "structural_text",
        PacketEvidenceTierDto::ResolvedGraph => "resolved_graph",
        PacketEvidenceTierDto::LexicalSource => "lexical_source",
        PacketEvidenceTierDto::SymbolDoc => "symbol_doc",
        PacketEvidenceTierDto::ComponentReport => "component_report",
        PacketEvidenceTierDto::DenseSemantic => "dense_semantic",
        PacketEvidenceTierDto::SyntheticSourceScan => "synthetic_source_scan",
        PacketEvidenceTierDto::GeneratedSummary => "generated_summary",
    }
}

fn packet_evidence_provenance_label(tier: PacketEvidenceTierDto) -> &'static str {
    match tier {
        PacketEvidenceTierDto::ExactSource => "exact",
        PacketEvidenceTierDto::StructuralText => "structural_text",
        PacketEvidenceTierDto::ResolvedGraph => "graph_neighbor",
        PacketEvidenceTierDto::LexicalSource => "lexical_source",
        PacketEvidenceTierDto::SymbolDoc => "symbol_doc",
        PacketEvidenceTierDto::ComponentReport => "component_report",
        PacketEvidenceTierDto::DenseSemantic => "dense_anchor",
        PacketEvidenceTierDto::SyntheticSourceScan => "synthetic_source_scan",
        PacketEvidenceTierDto::GeneratedSummary => "generated_summary",
    }
}

fn packet_escape_coverage_report_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\r', '\n'], " ")
}

/// The structural coverage a question asks for, and whether a claim's own cited evidence proves
/// each requirement separately.
struct PacketFlowContext {
    requirements: Vec<FlowRequirement>,
}

impl PacketFlowContext {
    fn new(question: &str, task_class: PacketTaskClassDto) -> Self {
        Self {
            requirements: packet_flow_requirements_for_terms(
                &packet_probe_terms(question),
                task_class,
            ),
        }
    }

    /// A requirement is covered only by a proof-bearing claim that cites evidence matching *that
    /// requirement's* predicate. Matching on the shared `FlowRole` instead let one citation close
    /// every requirement wearing the role, and let claim wording stand in for evidence.
    fn claim_satisfies_requirement(
        &self,
        claim: &PacketClaimDto,
        requirement: &FlowRequirement,
    ) -> bool {
        packet_claim_can_satisfy_sufficiency(claim)
            && claim
                .citations
                .iter()
                .filter(|citation| citation_sufficiency_eligible(citation))
                .any(|citation| requirement.evidence.citation_proves(citation))
    }
}

#[cfg(test)]
fn packet_missing_required_flow_roles(
    question: &str,
    task_class: PacketTaskClassDto,
    supported_claims: &[PacketClaimDto],
) -> Vec<FlowRole> {
    let missing = packet_missing_required_flow_requirements(question, task_class, supported_claims);
    packet_missing_requirement_roles(&missing)
}

fn packet_missing_required_flow_requirements(
    question: &str,
    task_class: PacketTaskClassDto,
    supported_claims: &[PacketClaimDto],
) -> Vec<FlowRequirement> {
    let flow_context = PacketFlowContext::new(question, task_class);
    flow_context
        .requirements
        .iter()
        .copied()
        .filter(flow_requirement_blocks_sufficiency)
        .filter(|requirement| {
            !supported_claims
                .iter()
                .any(|claim| flow_context.claim_satisfies_requirement(claim, requirement))
        })
        .collect()
}

#[cfg(test)]
fn packet_missing_requirement_roles(requirements: &[FlowRequirement]) -> Vec<FlowRole> {
    let mut roles = Vec::new();
    for requirement in requirements {
        if !roles
            .iter()
            .any(|role: &FlowRole| role.role_id() == requirement.role_id())
        {
            roles.push(requirement.role);
        }
    }
    roles
}

fn flow_requirement_missing_label(requirement: &FlowRequirement) -> String {
    format!("{} ({})", requirement.id, requirement.role.label())
}

fn flow_requirement_blocks_sufficiency(requirement: &FlowRequirement) -> bool {
    !matches!(requirement.coverage_mode, CoverageMode::DiagnosticOnly)
}

fn packet_blocking_missing_probe_queries(
    missing_required_probe_queries: &[String],
    missing_required_flow_requirements: &[FlowRequirement],
) -> Vec<String> {
    if missing_required_probe_queries.is_empty() || missing_required_flow_requirements.is_empty() {
        return Vec::new();
    }

    missing_required_probe_queries
        .iter()
        .filter(|query| {
            missing_required_flow_requirements
                .iter()
                .any(|requirement| {
                    flow_requirement_blocks_sufficiency(requirement)
                        && packet_query_binds_flow_requirement(query, requirement)
                })
        })
        .cloned()
        .collect()
}

fn packet_query_binds_flow_requirement(query: &str, requirement: &FlowRequirement) -> bool {
    if requirement.query_seeds.contains(&query) {
        return true;
    }

    // Selected probes may compose the requirement vocabulary differently from its canonical
    // seeds: `response send`, for example, combines `response finalization` and `transport send`.
    // Requiring every query term to come from one open requirement maps that alias without making
    // an arbitrary raw missing probe blocking.
    let query_terms = packet_probe_terms(query);
    if query_terms.len() < 2 {
        return false;
    }
    let requirement_terms = requirement
        .query_seeds
        .iter()
        .flat_map(|seed| packet_probe_terms(seed))
        .collect::<HashSet<_>>();
    query_terms
        .iter()
        .all(|term| requirement_terms.contains(term))
}

fn packet_blocking_unresolved_sidecar_queries(
    unresolved_sidecar_queries: &[String],
    blocking_missing_probe_queries: &[String],
    missing_required_flow_requirements: &[FlowRequirement],
    blocking_route_probe_queries: &[String],
) -> Vec<String> {
    if unresolved_sidecar_queries.is_empty()
        || (blocking_missing_probe_queries.is_empty()
            && missing_required_flow_requirements.is_empty()
            && blocking_route_probe_queries.is_empty())
    {
        return Vec::new();
    }

    let blocking_query_seeds = missing_required_flow_requirements
        .iter()
        .filter(|requirement| flow_requirement_blocks_sufficiency(requirement))
        .flat_map(|requirement| requirement.query_seeds.iter().copied())
        .collect::<HashSet<_>>();
    let blocking_probe_queries = blocking_missing_probe_queries
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let blocking_route_probe_queries = blocking_route_probe_queries
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    unresolved_sidecar_queries
        .iter()
        .filter(|query| {
            blocking_query_seeds.contains(query.as_str())
                || blocking_probe_queries.contains(query.as_str())
                || blocking_route_probe_queries.contains(query.as_str())
        })
        .cloned()
        .collect()
}

fn packet_blocking_incomplete_route_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
    route_proof_complete: bool,
    missing_required_probe_queries: &[String],
    selected_probes: &[String],
) -> Vec<String> {
    if task_class != PacketTaskClassDto::RouteTracing || route_proof_complete {
        return Vec::new();
    }
    let selected_probes = selected_probes
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    missing_required_probe_queries
        .iter()
        .filter(|query| {
            selected_probes.contains(query.as_str())
                || packet_normalized_query_phrase_present(question, query)
        })
        .cloned()
        .collect()
}

fn packet_normalized_query_phrase_present(question: &str, query: &str) -> bool {
    let question_terms = packet_probe_terms(question);
    let query_terms = packet_probe_terms(query);
    !query_terms.is_empty()
        && question_terms
            .windows(query_terms.len())
            .any(|window| window == query_terms)
}

fn packet_blocking_unresolved_obligation_queries(
    unresolved_sidecar_queries: &[String],
    incomplete_material_queries: &[String],
) -> Vec<String> {
    let incomplete_material_queries = incomplete_material_queries
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    unresolved_sidecar_queries
        .iter()
        .filter(|query| incomplete_material_queries.contains(query.as_str()))
        .cloned()
        .collect()
}

fn packet_blocking_follow_up_probe_queries(
    blocking_missing_probe_queries: &[String],
    blocking_unresolved_sidecar_queries: &[String],
) -> Vec<String> {
    let mut queries = Vec::new();
    let mut seen = HashSet::new();
    for query in blocking_missing_probe_queries
        .iter()
        .chain(blocking_unresolved_sidecar_queries)
    {
        if seen.insert(query.as_str()) {
            queries.push(query.clone());
        }
    }
    queries
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet_obligations::{
        build_packet_obligation_plan, finalize_packet_obligation_plan,
    };
    use codestory_contracts::api::{
        AgentAnswerDto, AgentCitationDto, AgentResponseBlockDto, AgentResponseSectionDto,
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalTraceDto, EdgeId,
        GraphArtifactDto, GraphEdgeDto, GraphNodeDto, GraphResponse, IndexFreshnessDto,
        IndexFreshnessNotCheckedCauseDto, IndexFreshnessStatusDto, NodeId, NodeKind,
        PacketBudgetDto, PacketBudgetLimitsDto, PacketBudgetUsageDto, PacketEvidenceResolutionDto,
        PacketEvidenceTierDto, PacketProofStatusDto, PacketSidecarQueryDiagnosticDto,
        RetrievalScoreBreakdownDto, RetrievalShadowDto, RetrievalStageTimingDto, SearchHitOrigin,
    };
    use codestory_contracts::api::{SourceCoverageObservationDto, SourceCoverageStatusDto};
    use std::path::Path;

    #[test]
    fn structural_text_labels_stay_explicit_in_packet_diagnostics() {
        assert_eq!(
            packet_evidence_tier_label(PacketEvidenceTierDto::StructuralText),
            "structural_text"
        );
        assert_eq!(
            packet_evidence_provenance_label(PacketEvidenceTierDto::StructuralText),
            "structural_text"
        );
    }

    fn claim(text: &str) -> PacketClaimDto {
        PacketClaimDto {
            claim: text.to_string(),
            required_obligation_ids: Vec::new(),
            required_obligation_kinds: Vec::new(),
            proof_status: None,
            required_evidence_role: None,
            citations: Vec::new(),
            coverage_role: None,
            eligible_for_sufficiency: None,
        }
    }

    fn cited_anchor(name: &str) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(name.to_string()),
            display_name: name.to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some(format!("src/{name}.rs")),
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

    fn cited_anchor_with_tier(
        name: &str,
        file_path: &str,
        tier: PacketEvidenceTierDto,
        eligible_for_sufficiency: Option<bool>,
    ) -> AgentCitationDto {
        let mut citation = cited_anchor(name);
        citation.file_path = Some(file_path.to_string());
        citation.evidence_tier = Some(tier);
        citation.resolution_status = Some(if tier == PacketEvidenceTierDto::SyntheticSourceScan {
            PacketEvidenceResolutionDto::SourceRangeOnly
        } else {
            PacketEvidenceResolutionDto::Resolved
        });
        citation.eligible_for_sufficiency = eligible_for_sufficiency;
        citation
    }

    /// A resolved anchor at a specific repository path, the shape sufficiency now requires: a
    /// requirement is closed by the evidence a claim cites, so fixtures have to name real symbols.
    fn anchor_at(name: &str, file_path: &str) -> AgentCitationDto {
        cited_anchor_with_tier(
            name,
            file_path,
            PacketEvidenceTierDto::ResolvedGraph,
            Some(true),
        )
    }

    fn typed_anchor_at(name: &str, file_path: &str, kind: NodeKind) -> AgentCitationDto {
        let mut citation = anchor_at(name, file_path);
        citation.kind = kind;
        citation
    }

    /// A proof-bearing claim whose only support is the cited anchor.
    fn evidence_claim(text: &str, citation: AgentCitationDto) -> PacketClaimDto {
        cited_claim(text, None, citation, Some(true))
    }

    fn cited_claim(
        text: &str,
        coverage_role: Option<&str>,
        citation: AgentCitationDto,
        eligible_for_sufficiency: Option<bool>,
    ) -> PacketClaimDto {
        PacketClaimDto {
            claim: text.to_string(),
            required_obligation_ids: Vec::new(),
            required_obligation_kinds: Vec::new(),
            proof_status: None,
            required_evidence_role: None,
            citations: vec![citation],
            coverage_role: coverage_role.map(str::to_string),
            eligible_for_sufficiency,
        }
    }

    fn answer_fixture(question: &str) -> AgentAnswerDto {
        AgentAnswerDto {
            source_coverage: Vec::new(),
            answer_id: "packet-sufficiency-test".to_string(),
            prompt: question.to_string(),
            summary: "Covered by cited anchors.".to_string(),
            freshness: Some(crate::packet_freshness::fresh_index_observation()),
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "Covered by cited anchors.".to_string(),
                }],
            }],
            citations: vec![
                cited_anchor("first"),
                cited_anchor("second"),
                cited_anchor("third"),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "packet-sufficiency-test".to_string(),
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

    #[test]
    fn diagnostic_and_duplicate_citations_do_not_inflate_minimum_coverage() {
        let question = "Locate the bug.";
        let mut answer = answer_fixture(question);
        let eligible = cited_anchor("only_eligible");
        let mut diagnostic = cited_anchor("diagnostic");
        diagnostic.eligible_for_sufficiency = Some(false);
        answer.citations = vec![eligible.clone(), eligible.clone(), diagnostic];

        assert_eq!(packet_eligible_citation_count(&answer), 1);

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::BugLocalization,
            answer: &answer,
            budget: &budget_fixture(),
            supported_claims: vec![cited_claim(
                "The cited function owns the failing behavior.",
                Some("source evidence"),
                eligible,
                Some(true),
            )],
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("only 1 cited anchor(s)"))
        );
    }

    #[test]
    fn diagnostic_only_citations_are_insufficient() {
        let question = "Locate the bug.";
        let mut answer = answer_fixture(question);
        for citation in &mut answer.citations {
            citation.eligible_for_sufficiency = Some(false);
        }

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::BugLocalization,
            answer: &answer,
            budget: &budget_fixture(),
            supported_claims: Vec::new(),
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Insufficient);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("No sufficiency-eligible cited anchors"))
        );
    }

    fn route_graph_node(id: &str) -> GraphNodeDto {
        GraphNodeDto {
            id: NodeId(id.to_string()),
            label: id.to_string(),
            kind: NodeKind::FUNCTION,
            depth: 1,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: Some(format!("src/{id}.rs")),
            qualified_name: None,
            member_access: None,
        }
    }

    fn route_graph_edge(id: &str, source: &str, target: &str) -> GraphEdgeDto {
        route_graph_edge_with_proof(id, source, target, Some("certain"), Some(1.0))
    }

    fn route_graph_edge_with_proof(
        id: &str,
        source: &str,
        target: &str,
        certainty: Option<&str>,
        confidence: Option<f32>,
    ) -> GraphEdgeDto {
        GraphEdgeDto {
            id: EdgeId(id.to_string()),
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            kind: EdgeKind::CALL,
            confidence,
            certainty: certainty.map(str::to_string),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
    }

    fn route_graph(id: &str, nodes: &[&str], edges: &[(&str, &str)]) -> GraphArtifactDto {
        GraphArtifactDto::Uml {
            id: id.to_string(),
            title: "Execution Route".to_string(),
            graph: GraphResponse {
                center_id: NodeId(nodes.first().copied().unwrap_or("route").to_string()),
                nodes: nodes.iter().map(|node| route_graph_node(node)).collect(),
                edges: edges
                    .iter()
                    .enumerate()
                    .map(|(index, (source, target))| {
                        route_graph_edge(&format!("edge-{index}"), source, target)
                    })
                    .collect(),
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        }
    }

    fn route_claim(name: &str) -> PacketClaimDto {
        cited_claim(
            &format!("`{name}` is a requested route endpoint and calls into downstream work."),
            Some("route endpoint"),
            cited_anchor(name),
            Some(true),
        )
    }

    #[test]
    fn obligation_receipts_use_only_their_planned_family_and_never_route_endpoints() {
        let receipt = PacketClaimDto {
            // Deliberately contains unrelated dispatch/cache words: receipt classification must use
            // the exact planned kind instead of inferring a semantic family from prose or citation.
            claim: "Dispatch cache evidence is present at RequestedEndpoint.".to_string(),
            required_obligation_ids: vec!["state_row".to_string()],
            required_obligation_kinds: vec![
                codestory_contracts::api::PacketClaimObligationKindDto::StateWrite,
            ],
            proof_status: Some(PacketProofStatusDto::Proven),
            required_evidence_role: None,
            citations: vec![cited_anchor("RequestedEndpoint")],
            coverage_role: Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE.to_string()),
            eligible_for_sufficiency: Some(true),
        };

        assert_eq!(
            packet_claim_family(&receipt),
            Some("planned state-write evidence")
        );
        assert_eq!(
            packet_supported_claim_family_count(std::slice::from_ref(&receipt)),
            1
        );
        assert!(packet_route_claim_node_ids("RequestedEndpoint", &[], &receipt).is_empty());
        assert!(!packet_route_claim_binds_stage(
            &["RequestedEndpoint".to_string()],
            &[],
            &receipt
        ));

        let answer = route_answer(
            "RequestedEndpoint -> DownstreamEndpoint",
            &["RequestedEndpoint", "DownstreamEndpoint"],
            &[("RequestedEndpoint", "DownstreamEndpoint")],
        );
        let mut obligations = PacketObligationPlanDto {
            version: codestory_contracts::api::PACKET_OBLIGATION_PLAN_VERSION,
            binding_terms: Vec::new(),
            claim_obligations: vec![PacketClaimObligationDto {
                id: "requested_claim:other".to_string(),
                kind: codestory_contracts::api::PacketClaimObligationKindDto::Dispatch,
                binding_terms: vec!["OtherEndpoint".to_string()],
                probe_binding: None,
                material: true,
                allowed_node_kinds: vec![NodeKind::FUNCTION],
                required_edge_kind: Some(EdgeKind::CALL),
                requires_complete_discovery: false,
                proof_status: PacketObligationProofStatusDto::Proven,
                reason: None,
                carrier_node_ids: vec![NodeId("RequestedEndpoint".to_string())],
                carrier_paths: vec!["src/RequestedEndpoint.rs".to_string()],
                carrier_edge_proofs: Vec::new(),
                open_next_candidates: Vec::new(),
            }],
            query_obligations: Vec::new(),
        };
        assert!(
            packet_route_obligation_node_ids("RequestedEndpoint", &[], &answer, &obligations,)
                .is_empty(),
            "a Proven receipt bound to a different exact identity must not satisfy this endpoint"
        );
        obligations.claim_obligations[0].binding_terms.clear();
        assert!(
            packet_route_obligation_node_ids("RequestedEndpoint", &[], &answer, &obligations,)
                .is_empty(),
            "a generic role obligation with no exact binding must not satisfy an endpoint"
        );
    }

    fn route_transition_claim(source: &str, target: &str) -> PacketClaimDto {
        let mut claim = route_claim(source);
        claim.claim = format!("`{source}` calls `{target}` on the requested route.");
        claim.citations.push(cited_anchor(target));
        claim
    }

    fn route_answer(question: &str, names: &[&str], edges: &[(&str, &str)]) -> AgentAnswerDto {
        let mut answer = answer_fixture(question);
        answer.citations = names.iter().map(|name| cited_anchor(name)).collect();
        answer.graphs = vec![route_graph("route", names, edges)];
        answer
    }

    fn route_sufficiency(
        question: &str,
        answer: &AgentAnswerDto,
        budget: &PacketBudgetDto,
        claims: Vec<PacketClaimDto>,
    ) -> PacketSufficiencyDto {
        assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::RouteTracing,
            answer,
            budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        })
    }

    fn production_route_sufficiency(
        question: &str,
        names: &[&str],
        edges: &[(&str, &str)],
    ) -> (PacketSufficiencyDto, Vec<PacketClaimDto>) {
        production_route_sufficiency_with_probes(question, names, edges, &[])
    }

    fn production_route_sufficiency_with_probes(
        question: &str,
        names: &[&str],
        edges: &[(&str, &str)],
        extra_probes: &[String],
    ) -> (PacketSufficiencyDto, Vec<PacketClaimDto>) {
        production_route_sufficiency_with_coverage(question, names, edges, extra_probes, Vec::new())
    }

    fn production_route_sufficiency_with_coverage(
        question: &str,
        names: &[&str],
        edges: &[(&str, &str)],
        extra_probes: &[String],
        source_coverage: Vec<SourceCoverageObservationDto>,
    ) -> (PacketSufficiencyDto, Vec<PacketClaimDto>) {
        let mut answer = route_answer(question, names, edges);
        answer.source_coverage = source_coverage;
        for citation in &mut answer.citations {
            citation.file_path = Some(format!("src/router/{}.rs", citation.display_name));
        }
        let graph_edges = answer
            .graphs
            .iter()
            .flat_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. } => graph.edges.as_slice(),
                _ => &[],
            })
            .cloned()
            .collect::<Vec<_>>();
        for citation in &mut answer.citations {
            citation.evidence_edge_ids = graph_edges
                .iter()
                .filter(|edge| edge.source == citation.node_id || edge.target == citation.node_id)
                .map(|edge| edge.id.clone())
                .collect();
        }
        let mut obligations =
            build_packet_obligation_plan(question, PacketTaskClassDto::RouteTracing, &[]);
        answer.retrieval_trace.packet_sidecar_diagnostics.extend(
            obligations
                .query_obligations
                .iter()
                .filter(|query| query.material)
                .map(|query| PacketSidecarQueryDiagnosticDto {
                    query: query.query.clone(),
                    completion: PacketQueryCompletionDto::Completed,
                    retrieval_mode: "full".to_string(),
                    sidecar_query_ms: Some(1),
                    candidate_resolution_ms: Some(0),
                    total_elapsed_ms: Some(1),
                    sidecar_stage_count: 1,
                    sidecar_stage_total_ms: Some(1),
                    batch_query_wall_ms: Some(1),
                    candidate_count: 1,
                    resolved_hit_count: 1,
                    unresolved_candidate_count: 0,
                    blocking_unresolved_candidate_count: 0,
                    semantic_stage_timeout_zero_hits: false,
                    semantic_abstained: false,
                    diagnostic: None,
                }),
        );
        let budget = budget_fixture();
        finalize_packet_obligation_plan(
            question,
            PacketTaskClassDto::RouteTracing,
            &mut obligations,
            &answer,
            &budget,
        );
        let supported_claims_with_telemetry = packet_supported_claims_with_telemetry(&answer);
        let claims = packet_claims_with_obligation_receipts(
            &answer,
            &obligations,
            supported_claims_with_telemetry,
        );
        let sufficiency = build_packet_sufficiency_with_obligation_context(
            &MissingPathSpellingIdentity,
            Path::new("C:/workspace/project"),
            question,
            PacketTaskClassDto::RouteTracing,
            &answer,
            &budget,
            extra_probes,
            &[],
            &obligations,
        );
        (sufficiency, claims)
    }

    /// CAP-1: a packet resting on a file the index refused must not report
    /// `Sufficient`.
    ///
    /// This is the Route B hole. A *required* file-scoped citation is minted
    /// `eligible_for_sufficiency`, unlike an explicitly probed one, so before
    /// this cap a packet could carry a proof-bearing claim over a file the
    /// index deliberately never read and still claim sufficiency. The
    /// probe-side route already capped, which is exactly why this one was
    /// invisible.
    #[test]
    fn a_packet_resting_on_an_excluded_file_cannot_be_sufficient() {
        let question = "alpha -> omega";
        let names = ["alpha", "omega", "RouteSupport"];
        let edges = [("alpha", "omega")];

        let (baseline, _) = production_route_sufficiency(question, &names, &edges);
        assert_eq!(
            baseline.status,
            PacketSufficiencyStatusDto::Sufficient,
            "the control must be Sufficient or this test proves nothing: {baseline:?}"
        );

        let (capped, _) = production_route_sufficiency_with_coverage(
            question,
            &names,
            &edges,
            &[],
            vec![SourceCoverageObservationDto {
                path: "src/router/alpha.rs".to_string(),
                status: SourceCoverageStatusDto::PolicyExcluded,
                reason: None,
                not_established_cause: None,
                observed_size: Some(1_500_000),
                byte_cap: Some(1_048_576),
            }],
        );
        assert_eq!(
            capped.status,
            PacketSufficiencyStatusDto::Partial,
            "{capped:?}"
        );
        assert!(
            capped
                .gaps
                .iter()
                .any(|gap| gap.contains("source coverage") && gap.contains("alpha.rs")),
            "the gap must name the file and say it is a coverage problem: {capped:?}"
        );
        assert!(
            capped
                .gaps
                .iter()
                .any(|gap| gap.contains("1500000") && gap.contains("1048576")),
            "the gap must name the numbers, not just the word: {capped:?}"
        );
    }

    /// An empty observation list must cap nothing.
    ///
    /// The asymmetry with freshness at the level that matters: freshness treats
    /// a missing observation as unknown-and-capping, so copying it wholesale
    /// would turn every packet that cites nothing into `Partial`.
    #[test]
    fn a_packet_with_no_coverage_observations_is_unaffected() {
        let (sufficiency, _) = production_route_sufficiency(
            "alpha -> omega",
            &["alpha", "omega", "RouteSupport"],
            &[("alpha", "omega")],
        );
        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{sufficiency:?}"
        );
        assert!(
            !sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("source coverage")),
            "{sufficiency:?}"
        );
    }

    /// Step 7 without step 8 turns a packet that answered and stopped into one
    /// that re-probes a permanently unindexable file forever: capping flips
    /// `terminally_sufficient` false, which is what opens follow-up generation.
    #[test]
    fn an_excluded_file_is_never_offered_as_a_follow_up_lead() {
        let (capped, _) = production_route_sufficiency_with_coverage(
            "alpha -> omega",
            &["alpha", "omega", "RouteSupport"],
            &[("alpha", "omega")],
            &[],
            vec![SourceCoverageObservationDto {
                path: "src/router/alpha.rs".to_string(),
                status: SourceCoverageStatusDto::PolicyExcluded,
                reason: None,
                not_established_cause: None,
                observed_size: Some(1_500_000),
                byte_cap: Some(1_048_576),
            }],
        );
        assert!(
            !capped
                .open_next
                .iter()
                .any(|lead| lead.contains("src/router/alpha.rs")),
            "a file the index can never cover is not a lead: {capped:?}"
        );
        assert!(
            !capped
                .follow_up_commands
                .iter()
                .any(|command| command.contains("src/router/alpha.rs")),
            "{capped:?}"
        );
    }

    fn assert_unresolved_route_order(sufficiency: &PacketSufficiencyDto) {
        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "{sufficiency:?}"
        );
        assert!(
            sufficiency
                .coverage_report
                .as_ref()
                .expect("route sufficiency should include a coverage report")
                .missing
                .contains(&"route order: unresolved endpoints".to_string()),
            "{sufficiency:?}"
        );
    }

    fn mark_full_retrieval_unavailable(answer: &mut AgentAnswerDto) {
        answer.retrieval_trace.retrieval_shadow = Some(RetrievalShadowDto {
            retrieval_mode: "unavailable".to_string(),
            degraded_reason: Some("retrieval_manifest_missing".to_string()),
            retrieval_total_ms: 0,
            total_budget_ms: None,
            cancel_reason: None,
            cache_hit: false,
            stage_timings: Vec::new(),
            candidates: Vec::new(),
            would_rank: Vec::new(),
            error: None,
            candidate_count: 0,
            resolved_hit_count: 0,
            unresolved_candidate_count: 0,
            diagnostic_only: false,
            candidate_resolution_counts: Vec::new(),
        });
    }

    fn mark_full_retrieval_available(answer: &mut AgentAnswerDto) {
        answer.retrieval_trace.retrieval_shadow = Some(RetrievalShadowDto {
            retrieval_mode: "full".to_string(),
            degraded_reason: None,
            retrieval_total_ms: 1,
            total_budget_ms: Some(500),
            cancel_reason: None,
            cache_hit: false,
            stage_timings: Vec::new(),
            candidates: Vec::new(),
            would_rank: Vec::new(),
            error: None,
            candidate_count: 0,
            resolved_hit_count: 0,
            unresolved_candidate_count: 0,
            diagnostic_only: false,
            candidate_resolution_counts: Vec::new(),
        });
    }

    fn unresolved_sidecar_diagnostic(query: &str) -> PacketSidecarQueryDiagnosticDto {
        PacketSidecarQueryDiagnosticDto {
            query: query.to_string(),
            completion: codestory_contracts::api::PacketQueryCompletionDto::Completed,
            retrieval_mode: "full".to_string(),
            sidecar_query_ms: None,
            candidate_resolution_ms: None,
            total_elapsed_ms: None,
            sidecar_stage_count: 0,
            sidecar_stage_total_ms: None,
            batch_query_wall_ms: None,
            candidate_count: 1,
            resolved_hit_count: 0,
            unresolved_candidate_count: 1,
            blocking_unresolved_candidate_count: 1,
            semantic_stage_timeout_zero_hits: false,
            semantic_abstained: false,
            diagnostic: Some("unresolved test candidate".to_string()),
        }
    }

    fn cancelled_sidecar_diagnostic(query: &str) -> PacketSidecarQueryDiagnosticDto {
        PacketSidecarQueryDiagnosticDto {
            query: query.to_string(),
            completion: codestory_contracts::api::PacketQueryCompletionDto::Cancelled {
                reason: "stage_deadline".to_string(),
            },
            retrieval_mode: "full".to_string(),
            sidecar_query_ms: None,
            candidate_resolution_ms: None,
            total_elapsed_ms: None,
            sidecar_stage_count: 0,
            sidecar_stage_total_ms: None,
            batch_query_wall_ms: None,
            candidate_count: 0,
            resolved_hit_count: 0,
            unresolved_candidate_count: 0,
            blocking_unresolved_candidate_count: 0,
            semantic_stage_timeout_zero_hits: false,
            semantic_abstained: false,
            diagnostic: Some(
                "sidecar query has blocking cancel reason `stage_deadline`".to_string(),
            ),
        }
    }

    fn budget_fixture() -> PacketBudgetDto {
        PacketBudgetDto {
            requested: PacketBudgetModeDto::Standard,
            limits: PacketBudgetLimitsDto {
                max_anchors: 16,
                max_files: 16,
                max_snippets: 16,
                max_trail_edges: 32,
                max_output_bytes: 32_000,
            },
            used: PacketBudgetUsageDto {
                anchors: 3,
                files: 3,
                snippets: 0,
                trail_edges: 0,
                output_bytes: 512,
            },
            truncated: false,
            omitted_sections: Vec::new(),
            next_deeper_command: None,
        }
    }

    fn compact_truncated_budget(question: &str, omitted_sections: Vec<&str>) -> PacketBudgetDto {
        let mut budget = budget_fixture();
        budget.requested = PacketBudgetModeDto::Compact;
        budget.truncated = true;
        budget.omitted_sections = omitted_sections.into_iter().map(str::to_string).collect();
        budget.next_deeper_command = Some(format!(
            "codestory-cli packet --project 'C:/workspace/project' --question '{}' --budget standard",
            question.replace('\'', "''")
        ));
        budget
    }

    #[test]
    fn route_proof_rejects_wrong_task_types_helpers_fixture_prose_and_self_edges() {
        let question = "RequestOwner -> ValidationGate";
        let mut answer = answer_fixture(question);
        let mut wrong_task = cited_anchor("RequestOwner");
        wrong_task.kind = NodeKind::ENUM_CONSTANT;
        let generic_helper = cited_anchor("helper");
        let mut fixture = cited_anchor("FixtureRoute");
        fixture.file_path = Some("tests/fixtures/route.md".to_string());
        answer.citations = vec![wrong_task.clone(), generic_helper.clone(), fixture.clone()];
        answer.graphs = vec![route_graph("self", &["helper"], &[("helper", "helper")])];
        let claims = vec![
            cited_claim(
                "RequestOwner starts the requested route.",
                None,
                wrong_task,
                Some(true),
            ),
            cited_claim(
                "ValidationGate completes the requested route.",
                Some("terminal_boundary"),
                generic_helper,
                Some(true),
            ),
            cited_claim(
                "Fixture prose describes the expected RequestOwner to ValidationGate path.",
                Some("route endpoint"),
                fixture,
                Some(true),
            ),
        ];

        let sufficiency = route_sufficiency(question, &answer, &budget_fixture(), claims);

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("route endpoint"))
        );
        assert!(!sufficiency.follow_up_commands.is_empty());
    }

    #[test]
    fn route_proof_rejects_self_edges_with_exact_endpoints() {
        let question = "EndpointA -> EndpointB";
        let mut answer = route_answer(question, &["EndpointA", "EndpointB"], &[]);
        answer.graphs = vec![route_graph(
            "self-edges",
            &["EndpointA", "EndpointB"],
            &[("EndpointA", "EndpointA"), ("EndpointB", "EndpointB")],
        )];

        let sufficiency = route_sufficiency(
            question,
            &answer,
            &budget_fixture(),
            vec![route_claim("EndpointA"), route_claim("EndpointB")],
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("execution graph"))
        );
    }

    #[test]
    fn structural_text_cannot_prove_route_endpoints_or_transitions() {
        let question = "EndpointA -> EndpointB";
        let answer = route_answer(
            question,
            &["EndpointA", "EndpointB"],
            &[("EndpointA", "EndpointB")],
        );
        let claims = [
            ("EndpointA", "src/EndpointA.html"),
            ("EndpointB", "src/EndpointB.html"),
        ]
        .into_iter()
        .map(|(name, path)| {
            let mut citation = cited_anchor(name);
            citation.file_path = Some(path.to_string());
            citation.evidence_tier = Some(PacketEvidenceTierDto::StructuralText);
            citation.evidence_producer = Some("structural_html_collector".to_string());
            citation.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);
            citation.eligible_for_sufficiency = Some(true);
            cited_claim(
                &format!("`{name}` is a requested route endpoint."),
                Some("route endpoint"),
                citation,
                Some(true),
            )
        })
        .collect();

        let sufficiency = route_sufficiency(question, &answer, &budget_fixture(), claims);

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("route endpoint")),
            "a real graph transition cannot promote structural endpoint citations: {sufficiency:?}"
        );
    }

    #[test]
    fn route_proof_rejects_speculative_call_edges() {
        for (case, certainty, confidence) in [
            ("speculative", Some("speculative"), Some(1.0)),
            ("uncertain", Some("uncertain"), Some(1.0)),
            ("probable", Some("probable"), Some(0.70)),
            ("low-confidence", Some("certain"), Some(0.20)),
        ] {
            let question = "EndpointA -> EndpointB";
            let mut answer = route_answer(
                question,
                &["EndpointA", "EndpointB", "RouteSupport"],
                &[("EndpointA", "EndpointB")],
            );
            let GraphArtifactDto::Uml { graph, .. } = &mut answer.graphs[0] else {
                unreachable!("route fixture must contain UML")
            };
            graph.edges[0] = route_graph_edge_with_proof(
                "route-edge",
                "EndpointA",
                "EndpointB",
                certainty,
                confidence,
            );

            let sufficiency = route_sufficiency(
                question,
                &answer,
                &budget_fixture(),
                vec![route_claim("EndpointA"), route_claim("EndpointB")],
            );

            assert_eq!(
                sufficiency.status,
                PacketSufficiencyStatusDto::Partial,
                "{case} CALL edge must not prove a route: {sufficiency:?}"
            );
            assert!(
                sufficiency
                    .gaps
                    .iter()
                    .any(|gap| gap.contains("directed execution graph")),
                "{case} CALL edge must produce an execution graph gap: {sufficiency:?}"
            );
        }
    }

    #[test]
    fn retained_false_safe_packet_shapes_fail_closed_without_route_proof() {
        let retained_shapes = [
            (
                "ask-1784390162944430000",
                "Identify ownership and validation gates for the complete v0.16 program.",
            ),
            (
                "ask-1784391431801551000",
                "Where is packet sufficiency for RouteTracing computed, how are selected probes, citations, claims, and execution graphs evaluated, and which tests cover false sufficient routes versus positive compact, standard, and deep route packets?",
            ),
        ];

        for (packet_id, question) in retained_shapes {
            let mut answer = answer_fixture(question);
            answer.answer_id = packet_id.to_string();
            mark_full_retrieval_available(&mut answer);
            let mut task_enum = cited_anchor("RouteTracing");
            task_enum.kind = NodeKind::ENUM_CONSTANT;
            let mut evidence_enum = cited_anchor("PacketEvidenceRole");
            evidence_enum.kind = NodeKind::ENUM;
            let mut storage_type = cited_anchor("Storage");
            storage_type.kind = NodeKind::STRUCT;
            let mut generic_probe = cited_anchor("probe");
            generic_probe.kind = NodeKind::VARIABLE;
            let eval_helper = cited_anchor("eval_probes_enabled");
            answer.citations = vec![
                task_enum.clone(),
                evidence_enum.clone(),
                storage_type.clone(),
                generic_probe.clone(),
                eval_helper.clone(),
            ];
            answer.graphs = vec![route_graph(
                "unrelated-eval-neighborhood",
                &["eval_probes_enabled"],
                &[("eval_probes_enabled", "eval_probes_enabled")],
            )];
            let claims = vec![
                cited_claim(
                    "RouteTracing identifies the requested task class.",
                    Some("route handling"),
                    task_enum,
                    Some(true),
                ),
                cited_claim(
                    "PacketEvidenceRole identifies evidence constants.",
                    Some("source evidence"),
                    evidence_enum,
                    Some(true),
                ),
                cited_claim(
                    "Storage owns generic persistence state.",
                    Some("state_or_storage"),
                    storage_type,
                    Some(true),
                ),
                cited_claim(
                    "probe names generic evaluation inputs.",
                    Some("source evidence"),
                    generic_probe,
                    Some(true),
                ),
                cited_claim(
                    "eval_probes_enabled controls an evaluation helper neighborhood.",
                    Some("dispatch"),
                    eval_helper,
                    Some(true),
                ),
            ];

            let sufficiency = route_sufficiency(question, &answer, &budget_fixture(), claims);

            assert_eq!(
                sufficiency.status,
                PacketSufficiencyStatusDto::Partial,
                "retained false-safe packet {packet_id} must fail closed: {sufficiency:?}"
            );
            assert!(
                sufficiency.gaps.iter().any(|gap| gap.contains("route")),
                "retained false-safe packet {packet_id} needs an explicit route gap: {sufficiency:?}"
            );
            assert!(
                !sufficiency.follow_up_commands.is_empty()
                    && sufficiency.follow_up_commands.len() <= 8,
                "retained false-safe packet {packet_id} needs bounded useful follow-up: {sufficiency:?}"
            );
        }
    }

    #[test]
    fn production_claims_prove_lowercase_arrow_route() {
        let (sufficiency, claims) = production_route_sufficiency(
            "alpha -> omega",
            &["alpha", "omega", "RouteSupport"],
            &[("alpha", "omega")],
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{sufficiency:?}"
        );
        assert!(
            claims
                .iter()
                .all(|claim| { claim.coverage_role.as_deref() != Some("route endpoint") })
        );
    }

    #[test]
    fn production_framed_and_suffix_routes_fail_closed() {
        let (framed_single, _) = production_route_sufficiency_with_probes(
            "Trace start -> run",
            &["start", "run", "RouteSupport"],
            &[("start", "run")],
            &["start".to_string()],
        );
        let (framed_phrase, _) = production_route_sufficiency_with_probes(
            "Trace request dispatch -> CustomExit",
            &["dispatch_request", "CustomExit", "RouteSupport"],
            &[("dispatch_request", "CustomExit")],
            &["dispatch_request".to_string()],
        );

        assert_unresolved_route_order(&framed_single);
        assert_unresolved_route_order(&framed_phrase);
    }

    #[test]
    fn production_scoped_probe_owner_mismatch_fails_closed() {
        for probe in ["router::dispatch_request", "src/router.rs dispatch_request"] {
            let (sufficiency, _) = production_route_sufficiency_with_probes(
                "request dispatch -> CustomExit",
                &["dispatch_request", "CustomExit", "RouteSupport"],
                &[("dispatch_request", "CustomExit")],
                &[probe.to_string()],
            );

            assert_unresolved_route_order(&sufficiency);
        }
    }

    #[test]
    fn production_exact_plain_phrase_matches_unscoped_probe() {
        let (sufficiency, _) = production_route_sufficiency_with_probes(
            "sha256Digest -> DigestExit",
            &["sha256Digest", "DigestExit", "RouteSupport"],
            &[("sha256Digest", "DigestExit")],
            &["sha256Digest".to_string()],
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{sufficiency:?}"
        );
    }

    #[test]
    fn production_multiple_quoted_leading_identifiers_fail_closed() {
        let (sufficiency, _) = production_route_sufficiency_with_probes(
            "Trace `EntryOne` and \"EntryTwo\" -> ExitStage",
            &["EntryOne", "EntryTwo", "ExitStage"],
            &[("EntryOne", "ExitStage")],
            &["EntryOne".to_string()],
        );

        assert_unresolved_route_order(&sufficiency);
    }

    #[test]
    fn production_multiple_unquoted_and_unclosed_identifiers_fail_closed() {
        let (multiple, _) = production_route_sufficiency(
            "EntryOne EntryTwo -> ExitStage",
            &["EntryOne", "EntryTwo", "ExitStage"],
            &[("EntryOne", "ExitStage")],
        );
        let (unclosed, _) = production_route_sufficiency(
            "`EntryOne -> ExitStage",
            &["EntryOne", "ExitStage", "RouteSupport"],
            &[("EntryOne", "ExitStage")],
        );

        assert_unresolved_route_order(&multiple);
        assert_unresolved_route_order(&unclosed);
    }

    #[test]
    fn production_source_evidence_proves_explicit_marker_route() {
        let question = "from CustomEntry through CustomDispatcher to CustomExit";
        let names = ["CustomEntry", "CustomDispatcher", "CustomExit"];
        let (sufficiency, claims) = production_route_sufficiency_with_probes(
            question,
            &names,
            &[
                ("CustomEntry", "CustomDispatcher"),
                ("CustomDispatcher", "CustomExit"),
            ],
            &["CustomDispatcher".to_string()],
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{sufficiency:?}"
        );
        for endpoint in ["CustomEntry", "CustomExit"] {
            assert!(claims.iter().any(|claim| {
                claim.coverage_role.as_deref() == Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE)
                    && claim.proof_status == Some(PacketProofStatusDto::Proven)
                    && claim
                        .citations
                        .iter()
                        .any(|citation| citation.display_name == endpoint)
            }));
        }
        assert!(
            claims
                .iter()
                .all(|claim| claim.coverage_role.as_deref() != Some("route endpoint"))
        );
    }

    #[test]
    fn production_unrelated_probe_does_not_alias_explicit_route_stage() {
        let question = "from CustomEntry through request dispatch to CustomExit";
        let (sufficiency, _) = production_route_sufficiency_with_probes(
            question,
            &["CustomEntry", "request_handler", "CustomExit"],
            &[
                ("CustomEntry", "request_handler"),
                ("request_handler", "CustomExit"),
            ],
            &["request_handler".to_string()],
        );

        assert_unresolved_route_order(&sufficiency);
    }

    #[test]
    fn production_claims_prove_explicit_packaged_runtime_route() {
        let question = "PluginLauncher -> StdioTransport -> RuntimePacket -> RetrievalStage -> PacketSufficiency";
        let names = [
            "PluginLauncher",
            "StdioTransport",
            "RuntimePacket",
            "RetrievalStage",
            "PacketSufficiency",
        ];
        let edges = [
            ("PluginLauncher", "StdioTransport"),
            ("StdioTransport", "RuntimePacket"),
            ("RuntimePacket", "RetrievalStage"),
            ("RetrievalStage", "PacketSufficiency"),
        ];
        let (sufficiency, claims) = production_route_sufficiency(question, &names, &edges);

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{sufficiency:?}"
        );
        assert!(
            claims
                .iter()
                .all(|claim| claim.coverage_role.as_deref() != Some("route endpoint"))
        );
    }

    #[test]
    fn production_claims_fail_closed_for_retained_ambiguous_release_questions() {
        let questions = [
            "Identify ownership and validation gates for the complete v0.16 program.",
            "Where is packet sufficiency for RouteTracing computed, how are selected probes, citations, claims, and execution graphs evaluated?",
            "Explain packaged MCP project selection, activation, runtime orchestration, retrieval, and status.",
        ];
        for question in questions {
            let (sufficiency, _) = production_route_sufficiency(
                question,
                &["PacketEvidenceRole", "Storage", "eval_probes_enabled"],
                &[("eval_probes_enabled", "eval_probes_enabled")],
            );

            assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
            assert!(sufficiency.coverage_report.as_ref().is_some_and(|report| {
                report
                    .missing
                    .contains(&"route order: unresolved endpoints".to_string())
            }));
        }
    }

    #[test]
    fn route_proof_uses_graph_order_when_claim_relevance_order_differs() {
        let question = "RouteIngress -> RouteDispatch -> RouteEgress";
        let answer = route_answer(
            question,
            &["RouteIngress", "RouteDispatch", "RouteEgress"],
            &[
                ("RouteIngress", "RouteDispatch"),
                ("RouteDispatch", "RouteEgress"),
            ],
        );
        let claims = vec![
            route_claim("RouteDispatch"),
            route_claim("RouteIngress"),
            route_claim("RouteEgress"),
        ];

        let sufficiency = route_sufficiency(question, &answer, &budget_fixture(), claims);

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "claim relevance order must not override the cited execution graph: {sufficiency:?}"
        );
        assert!(sufficiency.gaps.is_empty());
    }

    #[test]
    fn one_transition_claim_can_bind_both_adjacent_stages() {
        let question = "EndpointA -> EndpointB -> EndpointC";
        let answer = route_answer(
            question,
            &["EndpointA", "EndpointB", "EndpointC"],
            &[("EndpointA", "EndpointB"), ("EndpointB", "EndpointC")],
        );
        let claims = vec![
            route_transition_claim("EndpointB", "EndpointC"),
            route_transition_claim("EndpointA", "EndpointB"),
        ];

        let sufficiency = route_sufficiency(question, &answer, &budget_fixture(), claims);

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "one accurate transition claim may bind both cited endpoints: {sufficiency:?}"
        );
    }

    #[test]
    fn stage_binding_does_not_promote_unrelated_citations_from_the_same_claim() {
        let question = "EndpointA to EndpointC";
        let answer = route_answer(
            question,
            &["EndpointA", "EndpointB", "EndpointC"],
            &[("EndpointB", "EndpointC")],
        );
        let claims = vec![
            route_transition_claim("EndpointA", "EndpointB"),
            route_claim("EndpointC"),
        ];

        let sufficiency = route_sufficiency(question, &answer, &budget_fixture(), claims);

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("EndpointA -> EndpointC")),
            "EndpointB's edge must not stand in for EndpointA: {sufficiency:?}"
        );
    }

    #[test]
    fn route_proof_rejects_transitions_split_across_unrelated_neighborhoods() {
        let question = "RouteIngress -> RouteDispatch -> RouteEgress";
        let mut answer = route_answer(
            question,
            &["RouteIngress", "RouteDispatch", "RouteEgress"],
            &[],
        );
        answer.graphs = vec![
            route_graph(
                "first-neighborhood",
                &["RouteIngress", "RouteDispatch"],
                &[("RouteIngress", "RouteDispatch")],
            ),
            route_graph(
                "second-neighborhood",
                &["RouteDispatch", "RouteEgress"],
                &[("RouteDispatch", "RouteEgress")],
            ),
        ];
        let claims = vec![
            route_claim("RouteIngress"),
            route_claim("RouteDispatch"),
            route_claim("RouteEgress"),
        ];

        let sufficiency = route_sufficiency(question, &answer, &budget_fixture(), claims);

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("separate graph neighborhoods"))
        );
    }

    #[test]
    fn route_stages_come_only_from_explicit_question_order() {
        let question =
            "IndexingEntrypoint -> FileDiscovery -> SymbolExtraction -> StoragePersistence";
        let names = [
            "IndexingEntrypoint",
            "FileDiscovery",
            "SymbolExtraction",
            "StoragePersistence",
            "SearchPublication",
        ];
        let answer = route_answer(
            question,
            &names,
            &[
                ("IndexingEntrypoint", "FileDiscovery"),
                ("FileDiscovery", "SymbolExtraction"),
                ("SymbolExtraction", "StoragePersistence"),
                ("StoragePersistence", "SearchPublication"),
            ],
        );
        let selected_probes = vec![
            "SearchPublication".to_string(),
            "SymbolExtraction".to_string(),
        ];
        let sufficiency = assemble_packet_sufficiency_with_route_probes(
            PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/project"),
                question,
                task_class: PacketTaskClassDto::RouteTracing,
                answer: &answer,
                budget: &budget_fixture(),
                supported_claims: names.into_iter().map(route_claim).collect(),
                missing_required_probe_queries: Vec::new(),
                targeted_follow_up_queries: Vec::new(),
            },
            &selected_probes,
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "question checkpoints must define the route independently of probe order: {sufficiency:?}"
        );
        assert_eq!(
            packet_route_proof_stages(question, &[]),
            [
                "IndexingEntrypoint",
                "FileDiscovery",
                "SymbolExtraction",
                "StoragePersistence"
            ]
        );
    }

    #[test]
    fn unscoped_selected_probe_aliases_require_exact_identifier_token_multisets() {
        assert!(packet_route_labels_overlap(
            "request dispatch",
            "dispatch_request"
        ));
        assert!(packet_route_labels_overlap("URL session", "urlSession"));
        assert!(packet_route_labels_overlap("sha256 digest", "sha256Digest"));
        assert!(packet_route_label_matches_selected_probe(
            "request dispatch",
            &["dispatch_request".to_string()]
        ));
        assert!(!packet_route_label_matches_selected_probe(
            "request dispatch",
            &["router::dispatch_request".to_string()]
        ));
        assert!(!packet_route_label_matches_selected_probe(
            "request dispatch",
            &["src/router.rs dispatch_request".to_string()]
        ));
        assert!(!packet_route_labels_overlap(
            "request dispatch",
            "request_handler"
        ));
        assert!(!packet_route_labels_overlap(
            "request dispatch",
            "dispatch_request_request"
        ));
    }

    #[test]
    fn route_stage_overflow_fails_closed_instead_of_dropping_question_stage() {
        let names = [
            "StageOne",
            "StageTwo",
            "StageThree",
            "StageFour",
            "StageFive",
            "StageSix",
            "StageSeven",
        ];
        let question = "StageOne -> StageTwo -> StageThree -> StageFour -> StageFive -> StageSix -> StageSeven";
        let answer = route_answer(
            question,
            &names,
            &[
                ("StageOne", "StageTwo"),
                ("StageTwo", "StageThree"),
                ("StageThree", "StageFour"),
                ("StageFour", "StageFive"),
                ("StageFive", "StageSix"),
                ("StageSix", "StageSeven"),
            ],
        );
        let probes = vec!["StageSeven".to_string(), "StageOne".to_string()];
        let sufficiency = assemble_packet_sufficiency_with_route_probes(
            PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/project"),
                question,
                task_class: PacketTaskClassDto::RouteTracing,
                answer: &answer,
                budget: &budget_fixture(),
                supported_claims: names.into_iter().map(route_claim).collect(),
                missing_required_probe_queries: Vec::new(),
                targeted_follow_up_queries: Vec::new(),
            },
            &probes,
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency.gaps.iter().any(|gap| {
                gap.contains("bounded 6-stage capacity") && gap.contains("StageSeven")
            }),
            "overflow must identify the omitted required stage: {sufficiency:?}"
        );
        assert!(sufficiency.coverage_report.as_ref().is_some_and(|report| {
            report
                .missing
                .contains(&"route stage overflow: StageSeven".to_string())
        }));
    }

    #[test]
    fn normal_route_wording_parses_the_explicit_from_to_route() {
        let question = "Follow the requested execution call route from IngressHook to EgressHook.";
        let answer = route_answer(
            question,
            &["IngressHook", "RouteSupport", "EgressHook"],
            &[
                ("IngressHook", "RouteSupport"),
                ("RouteSupport", "EgressHook"),
            ],
        );
        let sufficiency = route_sufficiency(
            question,
            &answer,
            &budget_fixture(),
            vec![
                route_claim("IngressHook"),
                route_claim("RouteSupport"),
                route_claim("EgressHook"),
            ],
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{sufficiency:?}"
        );
    }

    #[test]
    fn selected_probes_do_not_create_route_order_for_ambiguous_prose() {
        let question = "Follow the requested execution route.";
        let answer = route_answer(
            question,
            &["IngressHook", "RouteSupport", "EgressHook"],
            &[
                ("IngressHook", "RouteSupport"),
                ("RouteSupport", "EgressHook"),
            ],
        );
        let selected_probes = vec!["IngressHook".to_string(), "EgressHook".to_string()];
        let sufficiency = assemble_packet_sufficiency_with_route_probes(
            PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/project"),
                question,
                task_class: PacketTaskClassDto::RouteTracing,
                answer: &answer,
                budget: &budget_fixture(),
                supported_claims: vec![
                    route_claim("IngressHook"),
                    route_claim("RouteSupport"),
                    route_claim("EgressHook"),
                ],
                missing_required_probe_queries: Vec::new(),
                targeted_follow_up_queries: Vec::new(),
            },
            &selected_probes,
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "{sufficiency:?}"
        );
        assert!(sufficiency.coverage_report.as_ref().is_some_and(|report| {
            report
                .missing
                .contains(&"route order: unresolved endpoints".to_string())
        }));
    }

    #[test]
    fn html_form_validation_generic_source_evidence_is_diagnostic_only() {
        let question = "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            cited_claim(
                "`index.html` in `src/index.html` ties page markup in this flow to cited definitions and adjacent ownership.",
                Some("source evidence"),
                cited_anchor_with_tier(
                    "index.html",
                    "src/index.html",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                Some(true),
            ),
            cited_claim(
                "Page markup supports `Main`; inspect the cited source for details.",
                Some("source evidence"),
                cited_anchor_with_tier(
                    "Main",
                    "src/main.js",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                Some(true),
            ),
            cited_claim(
                "`PageState` is defined in cited source `src/main.js` and should be treated as an exact source anchor for this flow.",
                Some("source evidence"),
                cited_anchor_with_tier(
                    "PageState",
                    "src/main.js",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                Some(true),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.covered.is_empty(),
            "generic navigation claims should not appear as covered proof: {report:?}"
        );
        assert!(
            report
                .missing
                .contains(&"form_native_constraints".to_string())
        );
        assert!(
            report
                .missing
                .contains(&"form_custom_validation".to_string())
        );
        assert!(report.missing.contains(&"form_submit_guard".to_string()));
        assert_eq!(report.ineligible.len(), 3);
        assert!(
            report
                .ineligible
                .iter()
                .all(|entry| entry.contains("role=\"source evidence\"")),
            "generic HTML diagnostics should preserve source-evidence role labels: {report:?}"
        );
        assert!(
            report.ineligible.iter().all(|entry| entry.contains(
                "reason=\"generic navigation/source-evidence claim does not explain the flow\""
            )),
            "generic HTML claims should explain diagnostic demotion: {report:?}"
        );
        assert!(
            report
                .ineligible
                .iter()
                .all(|entry| entry.contains("tier=\"resolved_graph\"")),
            "ineligible diagnostics should include the citation tier: {report:?}"
        );
    }

    #[test]
    fn architecture_exact_paths_require_proof_bearing_claims_from_each_path() {
        let question = "Explain the architecture represented by these exact paths.";
        let budget = budget_fixture();
        let stdio_path = "crates/codestory-cli/src/stdio_transport.rs";
        let runtime_path = "crates/codestory-runtime/src/agent/orchestrator.rs";
        let launcher_path = "plugins/codestory/scripts/launcher.mjs";
        let stdio = cited_anchor_with_tier(
            "dispatch_stdio_request",
            stdio_path,
            PacketEvidenceTierDto::ResolvedGraph,
            Some(true),
        );
        let runtime = cited_anchor_with_tier(
            "agent_packet",
            runtime_path,
            PacketEvidenceTierDto::ResolvedGraph,
            Some(true),
        );
        let publication = cited_anchor_with_tier(
            "publish_generation",
            "crates/codestory-store/src/publication.rs",
            PacketEvidenceTierDto::ResolvedGraph,
            Some(true),
        );
        let mut launcher_probe = cited_anchor_with_tier(
            launcher_path,
            launcher_path,
            PacketEvidenceTierDto::ExactSource,
            Some(false),
        );
        launcher_probe.evidence_producer = Some("packet_exact_path_probe".to_string());
        launcher_probe.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);

        let mut answer = answer_fixture(question);
        answer.citations = vec![
            stdio.clone(),
            runtime.clone(),
            publication.clone(),
            launcher_probe,
        ];
        let claims = vec![
            cited_claim(
                "The stdio adapter dispatches the host request.",
                Some("transport adapter"),
                stdio,
                Some(true),
            ),
            cited_claim(
                "Runtime orchestration coordinates the packet request.",
                Some("runtime orchestration"),
                runtime,
                Some(true),
            ),
            cited_claim(
                "Publication evidence exposes the completed generation.",
                Some("evidence publication"),
                publication,
                Some(true),
            ),
        ];
        let input = || PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims.clone(),
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        };

        let without_exact_paths = assemble_packet_sufficiency(input());
        assert_eq!(
            without_exact_paths.status,
            PacketSufficiencyStatusDto::Sufficient,
            "fixture should isolate the exact-path relevance contract: {without_exact_paths:?}"
        );

        let exact_paths = vec![
            launcher_path.to_string(),
            stdio_path.to_string(),
            runtime_path.to_string(),
        ];
        let sufficiency = assemble_packet_sufficiency_with_probe_context(
            &MissingPathSpellingIdentity,
            input(),
            &[],
            &exact_paths,
            None,
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains(launcher_path)),
            "missing exact-path relevance should be explicit: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains(launcher_path)),
            "missing exact path should produce a targeted follow-up: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .coverage_report
                .as_ref()
                .is_some_and(|report| report
                    .missing
                    .contains(&format!("exact path: {launcher_path}"))),
            "coverage report should retain the exact missing path: {sufficiency:?}"
        );
        assert!(
            !sufficiency
                .avoid_opening_paths
                .contains(&launcher_path.to_string()),
            "diagnostic exact-path citations must not discourage source inspection: {sufficiency:?}"
        );
    }

    #[test]
    fn architecture_exact_path_matching_rejects_same_suffix_collisions() {
        let project_root = Path::new("C:/workspace/project");

        assert!(packet_paths_match_exact_probe(
            &MissingPathSpellingIdentity,
            project_root,
            "crates/foo/src/lib.rs",
            "crates/foo/src/lib.rs"
        ));
        assert!(
            !packet_paths_match_exact_probe(
                &MissingPathSpellingIdentity,
                project_root,
                "src/lib.rs",
                "crates/foo/src/lib.rs"
            ),
            "a shorter same-suffix citation must not satisfy a different exact path"
        );
    }

    #[test]
    fn architecture_exact_paths_use_distinct_claims_with_overlapping_citations() {
        let project_root = Path::new("C:/workspace/project");
        let paths = [
            "plugins/codestory/scripts/launcher.mjs",
            "crates/codestory-cli/src/stdio_transport.rs",
        ];
        let citations = paths.map(|path| {
            cited_anchor_with_tier(path, path, PacketEvidenceTierDto::ResolvedGraph, Some(true))
        });
        let mut overlapping_claim = cited_claim(
            "The request crosses the launcher and stdio transport boundary.",
            Some("transport adapter"),
            citations[0].clone(),
            Some(true),
        );
        overlapping_claim.citations = citations.to_vec();
        let launcher_claim = cited_claim(
            "The launcher delegates the packaged request.",
            Some("package entrypoint"),
            citations[0].clone(),
            Some(true),
        );

        let missing = packet_missing_exact_path_claims(
            &MissingPathSpellingIdentity,
            project_root,
            &paths.map(str::to_string),
            &[overlapping_claim, launcher_claim],
        );

        assert!(
            missing.is_empty(),
            "matching should reassign the overlapping claim to the only path it can uniquely cover: {missing:?}"
        );
    }

    #[test]
    fn architecture_exact_paths_reject_one_broad_claim_for_multiple_paths() {
        let project_root = Path::new("C:/workspace/project");
        let paths = [
            "plugins/codestory/scripts/launcher.mjs",
            "crates/codestory-cli/src/stdio_transport.rs",
            "crates/codestory-runtime/src/agent/orchestrator.rs",
        ];
        let citations = paths.map(|path| {
            cited_anchor_with_tier(path, path, PacketEvidenceTierDto::ResolvedGraph, Some(true))
        });
        let mut broad_claim = cited_claim(
            "The request crosses the packaged plugin, stdio, and runtime boundaries.",
            Some("runtime orchestration"),
            citations[0].clone(),
            Some(true),
        );
        broad_claim.citations = citations.to_vec();

        let missing = packet_missing_exact_path_claims(
            &MissingPathSpellingIdentity,
            project_root,
            &paths.map(str::to_string),
            &[broad_claim],
        );

        assert_eq!(
            missing.len(),
            2,
            "one proof-bearing claim may bind at most one requested exact path: {missing:?}"
        );
    }

    #[test]
    fn architecture_exact_paths_remain_non_promoting_when_role_backed_claims_cover_them() {
        let question = "Explain the architecture represented by these exact paths.";
        let budget = budget_fixture();
        let paths = [
            "plugins/codestory/scripts/launcher.mjs",
            "crates/codestory-cli/src/stdio_transport.rs",
            "crates/codestory-runtime/src/agent/orchestrator.rs",
        ];
        let citations = [
            cited_anchor_with_tier(
                "launch",
                paths[0],
                PacketEvidenceTierDto::LexicalSource,
                Some(true),
            ),
            cited_anchor_with_tier(
                "dispatch_stdio_request",
                paths[1],
                PacketEvidenceTierDto::ResolvedGraph,
                Some(true),
            ),
            cited_anchor_with_tier(
                "agent_packet",
                paths[2],
                PacketEvidenceTierDto::ResolvedGraph,
                Some(true),
            ),
        ];
        let mut answer = answer_fixture(question);
        answer.citations = citations.to_vec();
        let claims = vec![
            cited_claim(
                "The launcher delegates a packaged request to the managed CLI.",
                Some("package entrypoint"),
                citations[0].clone(),
                Some(true),
            ),
            cited_claim(
                "The stdio adapter dispatches the host request.",
                Some("transport adapter"),
                citations[1].clone(),
                Some(true),
            ),
            cited_claim(
                "Runtime orchestration coordinates the packet request.",
                Some("runtime orchestration"),
                citations[2].clone(),
                Some(true),
            ),
        ];
        let exact_paths = paths.map(str::to_string);

        let sufficiency = assemble_packet_sufficiency_with_probe_context(
            &MissingPathSpellingIdentity,
            PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/project"),
                question,
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                answer: &answer,
                budget: &budget,
                supported_claims: claims,
                missing_required_probe_queries: Vec::new(),
                targeted_follow_up_queries: Vec::new(),
            },
            &[],
            &exact_paths,
            None,
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "exact probes should constrain relevance without promoting diagnostic citations: {sufficiency:?}"
        );
        assert!(sufficiency.gaps.is_empty(), "{sufficiency:?}");
        assert!(sufficiency.follow_up_commands.is_empty(), "{sufficiency:?}");
    }

    #[test]
    fn claim_level_diagnostic_flag_overrides_required_role_on_generic_claim() {
        let question = "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![cited_claim(
            "`validateForm` in `src/forms.js` ties form validation in this flow to cited definitions and adjacent ownership.",
            Some("transform_or_validate"),
            cited_anchor_with_tier(
                "validateForm",
                "src/forms.js",
                PacketEvidenceTierDto::ResolvedGraph,
                Some(true),
            ),
            Some(false),
        )];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert_eq!(report.ineligible.len(), 1);
        assert!(report.ineligible[0].contains("role=\"transform_or_validate\""));
        assert!(report.ineligible[0].contains("reason=\"claim marked diagnostic\""));
        assert!(
            !report
                .covered
                .contains(&"transform_or_validate".to_string()),
            "claim-level diagnostic flags must keep role-backed generic claims out of covered proof: {report:?}"
        );
    }

    #[test]
    fn ineligible_claim_report_escapes_claim_text() {
        let question = "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![cited_claim(
            "Page \"markup\" uses C:\\forms\nand adjacent ownership.",
            Some("source evidence"),
            cited_anchor_with_tier(
                "PageMarkup",
                "src/forms.js",
                PacketEvidenceTierDto::ResolvedGraph,
                Some(true),
            ),
            Some(false),
        )];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert_eq!(report.ineligible.len(), 1);
        let entry = &report.ineligible[0];
        assert!(
            entry
                .contains("claim=\"Page \\\"markup\\\" uses C:\\\\forms and adjacent ownership.\""),
            "quoted, backslash, and newline claim text should be escaped in ineligible diagnostics: {entry}"
        );
        assert!(!entry.contains('\n'));
    }

    #[test]
    fn sql_synthetic_source_scan_evidence_never_covers_schema_requirements() {
        // Same prompt and the same three source-scan anchors this fixture always used. A synthetic
        // source scan is a text match, not a resolved definition, and it used to be admitted as
        // proof for the two SQL requirements by a per-requirement bypass. Sufficiency is a statement
        // about proof, so the bypass is gone: the scan is still reported, with its concrete role,
        // as evidence worth following up rather than as a covered requirement.
        let question = "Explain SQL schema relationships between artists, albums, tracks, invoices, and invoice lines across seed scripts.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            cited_claim(
                "SQL schema defines tables Artist, Album, Track, Invoice, and InvoiceLine.",
                Some("source evidence"),
                cited_anchor_with_tier(
                    "CREATE TABLE Artist",
                    "schema.sql",
                    PacketEvidenceTierDto::SyntheticSourceScan,
                    Some(false),
                ),
                Some(false),
            ),
            cited_claim(
                "Track rows reference Album, Genre, and MediaType rows.",
                Some("source evidence"),
                cited_anchor_with_tier(
                    "FOREIGN KEY",
                    "schema.sql",
                    PacketEvidenceTierDto::SyntheticSourceScan,
                    Some(false),
                ),
                Some(false),
            ),
            cited_claim(
                "`schema.sql` in `schema.sql` ties sql schema in this flow to cited definitions and adjacent ownership.",
                Some("sql schema scripts"),
                cited_anchor_with_tier(
                    "schema.sql",
                    "schema.sql",
                    PacketEvidenceTierDto::SyntheticSourceScan,
                    Some(false),
                ),
                Some(false),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        for requirement in ["sql_tables", "sql_relationships"] {
            assert!(
                report.missing.contains(&requirement.to_string()),
                "a source scan does not prove {requirement}: {report:?}"
            );
        }
        assert!(
            report.covered.is_empty(),
            "source-scan evidence must not appear as covered proof: {report:?}"
        );
        assert_eq!(report.ineligible.len(), 3);
        assert!(
            report
                .ineligible
                .iter()
                .all(|entry| entry.contains("tier=\"synthetic_source_scan\"")
                    && entry.contains("reason=\"claim marked diagnostic\"")),
            "every source-scan claim is reported with its tier and reason: {report:?}"
        );
        assert!(
            report
                .ineligible
                .iter()
                .any(|entry| entry.contains("role=\"sql schema scripts\"")),
            "the concrete role each scan carried is preserved in the report: {report:?}"
        );
        assert!(
            sufficiency.covered_claims.is_empty(),
            "no source-scan claim is published as safe to repeat: {sufficiency:?}"
        );
    }

    #[test]
    fn log_record_handler_source_claims_make_data_flow_sufficient() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let answer = answer_fixture(question);
        let budget = compact_truncated_budget(question, vec!["citations"]);
        let claims = vec![
            cited_claim(
                "addRecord creates a log record before passing it to handlers.",
                None,
                cited_anchor_with_tier(
                    "Logger::addRecord",
                    "src/logging/Logger.php",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                None,
            ),
            cited_claim(
                "The processing handler handles records by processing and writing them.",
                None,
                cited_anchor_with_tier(
                    "LogProcessingHandler::handle",
                    "src/logging/ProcessingHandler.php",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                None,
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(
            sufficiency.gaps.is_empty(),
            "eligible logger/record/handler claims should not leave citation-budget or family gaps: {sufficiency:?}"
        );
        let report = sufficiency.coverage_report.as_ref().unwrap();
        for expected in ["log record creation", "handler processing"] {
            assert!(
                report.covered.contains(&expected.to_string()),
                "log-record DataFlow should report concrete covered family `{expected}`: {report:?}"
            );
        }
        assert!(
            report.ineligible.is_empty(),
            "role-backed log-record source claims should be sufficiency-eligible: {report:?}"
        );
    }

    /// The exact packet that `log_record_handler_source_claims_make_data_flow_sufficient` proves
    /// reaches `Sufficient`, assembled over a caller-supplied answer.
    ///
    /// EV-7/EV-8 are about the *collection conditions* of a packet, not its claims. Reusing a
    /// packet whose claim side is independently proven sufficient is what makes the resulting
    /// `Partial` attributable to the condition under test and nothing else.
    fn data_flow_sufficient_packet(answer: &AgentAnswerDto) -> PacketSufficiencyDto {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let budget = compact_truncated_budget(question, vec!["citations"]);
        let claims = vec![
            cited_claim(
                "addRecord creates a log record before passing it to handlers.",
                None,
                cited_anchor_with_tier(
                    "Logger::addRecord",
                    "src/logging/Logger.php",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                None,
            ),
            cited_claim(
                "The processing handler handles records by processing and writing them.",
                None,
                cited_anchor_with_tier(
                    "LogProcessingHandler::handle",
                    "src/logging/ProcessingHandler.php",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                None,
            ),
        ];

        assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        })
    }

    fn not_checked_observation(
        cause: Option<IndexFreshnessNotCheckedCauseDto>,
    ) -> IndexFreshnessDto {
        IndexFreshnessDto {
            status: IndexFreshnessStatusDto::NotChecked,
            changed_file_count: 0,
            new_file_count: 0,
            removed_file_count: 0,
            checked_file_count: 0,
            indexed_file_count: 8,
            duration_ms: 1,
            reason: Some("indexed file inventory exceeds bounded freshness cap".to_string()),
            not_checked_cause: cause,
            samples: Vec::new(),
        }
    }

    /// CR-001. The serving waiver deliberately keeps a bounded-inventory repository usable, and
    /// the same packet that is sufficient over an observed publication must not claim proof over
    /// one whose drift was never compared.
    #[test]
    fn bounded_inventory_freshness_caps_an_otherwise_sufficient_packet_at_partial() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let mut answer = answer_fixture(question);
        answer.freshness = Some(not_checked_observation(Some(
            IndexFreshnessNotCheckedCauseDto::BoundedInventory,
        )));

        let sufficiency = data_flow_sufficient_packet(&answer);

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "a packet over an unobserved publication cannot report proof: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("freshness unknown (bounded_inventory)")),
            "the cap must name its typed cause: {:?}",
            sufficiency.gaps
        );
    }

    /// A check that could not run establishes even less than a bounded one, and must be reported
    /// under its own cause rather than collapsed into the bounded case.
    #[test]
    fn unavailable_freshness_check_caps_with_its_own_typed_cause() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let mut answer = answer_fixture(question);
        answer.freshness = Some(not_checked_observation(Some(
            IndexFreshnessNotCheckedCauseDto::InventoryUnavailable,
        )));

        let sufficiency = data_flow_sufficient_packet(&answer);

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("freshness unknown (inventory_unavailable)")),
            "{:?}",
            sufficiency.gaps
        );
    }

    /// On the production path a missing observation means `controller.index_freshness()` itself
    /// failed. Reading that as fresh is the CR-001 exposure in its purest form.
    #[test]
    fn a_packet_with_no_freshness_observation_at_all_caps_at_partial() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let mut answer = answer_fixture(question);
        answer.freshness = None;

        let sufficiency = data_flow_sufficient_packet(&answer);

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("freshness unknown (observation_unavailable)")),
            "{:?}",
            sufficiency.gaps
        );
    }

    #[test]
    fn stale_freshness_caps_an_otherwise_sufficient_packet_at_partial() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let mut answer = answer_fixture(question);
        let mut stale = crate::packet_freshness::fresh_index_observation();
        stale.status = IndexFreshnessStatusDto::Stale;
        stale.changed_file_count = 2;
        answer.freshness = Some(stale);

        let sufficiency = data_flow_sufficient_packet(&answer);

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.starts_with("freshness stale:")),
            "{:?}",
            sufficiency.gaps
        );
    }

    /// The freshness input is a cap, never a floor. A packet with no eligible citation stays
    /// `Insufficient`; observing the index does not upgrade it.
    #[test]
    fn fresh_freshness_does_not_promote_a_packet_with_no_eligible_citation() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let mut answer = answer_fixture(question);
        answer.citations.clear();

        let sufficiency = data_flow_sufficient_packet(&answer);

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Insufficient,
            "{sufficiency:?}"
        );
        assert!(
            !sufficiency
                .gaps
                .iter()
                .any(|gap| gap.starts_with("freshness")),
            "an observed publication publishes no freshness gap: {:?}",
            sufficiency.gaps
        );
    }

    /// EV-8. A primary run that lost a stage to its deadline collected less evidence than it
    /// planned to, so the absence of a result is not evidence of absence and the packet cannot
    /// report proof.
    #[test]
    fn truncated_primary_retrieval_caps_an_otherwise_sufficient_packet_at_partial() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let mut answer = answer_fixture(question);
        let mut shadow = sufficiency_test_shadow();
        shadow.stage_timings[0].completion_status = "pending_after_deadline".to_string();
        answer.retrieval_trace.retrieval_shadow = Some(shadow);

        let sufficiency = data_flow_sufficient_packet(&answer);

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "{sufficiency:?}"
        );
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.starts_with("primary retrieval truncated:")),
            "{:?}",
            sufficiency.gaps
        );
    }

    /// The planner stops deliberately once marginal gain flattens. Treating that as truncation
    /// would demote every healthy packet, so the cap must stay off for it.
    #[test]
    fn the_planned_marginal_gain_stop_leaves_a_sufficient_packet_sufficient() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let mut answer = answer_fixture(question);
        let mut shadow = sufficiency_test_shadow();
        shadow.cancel_reason = Some("marginal_gain".to_string());
        answer.retrieval_trace.retrieval_shadow = Some(shadow);

        let sufficiency = data_flow_sufficient_packet(&answer);

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{sufficiency:?}"
        );
        assert!(sufficiency.gaps.is_empty(), "{:?}", sufficiency.gaps);
    }

    fn sufficiency_test_shadow() -> RetrievalShadowDto {
        RetrievalShadowDto {
            retrieval_mode: "full".to_string(),
            degraded_reason: None,
            retrieval_total_ms: 12,
            total_budget_ms: Some(200),
            cancel_reason: None,
            cache_hit: false,
            stage_timings: vec![RetrievalStageTimingDto {
                stage: "stage1_lexical".to_string(),
                deadline_ms: Some(80),
                elapsed_ms: 20,
                admission_wait_ms: None,
                queue_wait_ms: None,
                execution_ms: Some(20),
                candidates_added: 4,
                marginal_gain: 0.4,
                cancel_reason: None,
                cache_hit: false,
                sidecar_latency_ms: None,
                degraded: false,
                stub_reason: None,
                completion_status: "completed".to_string(),
            }],
            candidates: Vec::new(),
            would_rank: Vec::new(),
            error: None,
            candidate_count: 4,
            resolved_hit_count: 4,
            unresolved_candidate_count: 0,
            diagnostic_only: false,
            candidate_resolution_counts: Vec::new(),
        }
    }

    #[test]
    fn unrelated_handler_cannot_complete_logger_flow() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let mut answer = answer_fixture(question);
        let record = anchor_at("Logger.addRecord", "src/logging/Logger.php");
        let unrelated_handler = anchor_at("DefaultHandler.process", "src/http/default_handler.rs");
        answer.citations = vec![
            record.clone(),
            unrelated_handler.clone(),
            anchor_at("HttpResponse.write", "src/http/response.rs"),
        ];
        let claims = vec![
            evidence_claim(
                "addRecord creates a log record before passing it to handlers.",
                record,
            ),
            evidence_claim(
                "The processing handler handles records by processing and writing them.",
                unrelated_handler,
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget_fixture(),
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "an unrelated HTTP handler must not complete a logging flow: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .coverage_report
                .as_ref()
                .is_some_and(|report| report.missing.contains(&"handler_processing".to_string())),
            "the packet must name the unproved handler-processing step: {sufficiency:?}"
        );
    }

    #[test]
    fn unrelated_source_and_sink_cannot_complete_buffered_io() {
        let question = "Explain how Buffer, Source, Sink, and buffered wrappers cooperate to move bytes through reads and writes.";
        let mut answer = answer_fixture(question);
        let buffer = anchor_at("Buffer", "src/io/buffer.kt");
        let unrelated_source = anchor_at("DatabaseSource.read", "src/database/source.rs");
        let unrelated_sink = anchor_at("TelemetrySink.write", "src/telemetry/sink.rs");
        answer.citations = vec![
            buffer.clone(),
            unrelated_source.clone(),
            unrelated_sink.clone(),
        ];
        let claims = vec![
            evidence_claim(
                "Buffer is the in-memory byte store used by buffered reads and writes.",
                buffer,
            ),
            evidence_claim(
                "A buffered source wrapper reads from an upstream Source into a Buffer.",
                unrelated_source,
            ),
            evidence_claim(
                "A buffered sink wrapper writes buffered bytes to an upstream Sink.",
                unrelated_sink,
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget_fixture(),
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "unrelated database and telemetry operations must not complete buffered IO: \
             {sufficiency:?}"
        );
        assert!(
            sufficiency
                .coverage_report
                .as_ref()
                .is_some_and(|report| report.missing.contains(&"buffered_read_write".to_string())),
            "the packet must name the unproved buffered read/write step: {sufficiency:?}"
        );
    }

    #[test]
    fn every_carrier_flow_is_partial_and_names_the_unproved_step() {
        struct Case {
            label: &'static str,
            question: &'static str,
            citations: Vec<AgentCitationDto>,
            expected_missing: &'static [&'static str],
        }

        let cases = vec![
            Case {
                label: "form validation across component and markup surfaces",
                question: "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation and a submit guard.",
                citations: vec![
                    anchor_at("clampMin", "src/forms/Widget.vue"),
                    anchor_at("validateCoupon", "src/forms/Coupon.vue"),
                    anchor_at("submitJob", "app/forms/Checkout.svelte"),
                    anchor_at("maxRetries", "examples/forms/page.html"),
                ],
                expected_missing: &["form_custom_validation", "form_submit_guard"],
            },
            Case {
                label: "static-site phases filed on site and component surfaces",
                question: "Trace how the static site build command creates a site and runs the read, generate, render, and write phases.",
                citations: vec![
                    anchor_at("AssetPipeline.run", "lib/site/assets.rb"),
                    anchor_at("Layout.render", "lib/site/Layout.vue"),
                    anchor_at("ThemeGenerator.output", "src/ui/Theme.svelte"),
                    anchor_at("PageTemplate.write", "public/static/page.html"),
                ],
                expected_missing: &["site_lifecycle", "site_terminal"],
            },
            Case {
                label: "logger record plus unrelated handlers",
                question: "Explain how a logger turns a log call into a record object and passes it through handlers.",
                citations: vec![
                    anchor_at("Logger.addRecord", "src/logging/Logger.php"),
                    anchor_at("DefaultHandler.process", "src/http/default_handler.rs"),
                    anchor_at("PaymentHandler.write", "src/logging/Payment.vue"),
                    anchor_at("GenericHandler.process", "src/logging/Panel.svelte"),
                ],
                expected_missing: &["handler_processing"],
            },
            Case {
                label: "object-map words over navigation and graphics objects",
                question: "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects through type map plans.",
                citations: vec![
                    anchor_at("sourceMapOptions", "src/mapping/Config.vue"),
                    anchor_at("RoadMapPlanner", "src/mapping/Planner.svelte"),
                    anchor_at("HeatMapExecutor", "src/charts/heat.html"),
                    anchor_at("TileMapPipeline", "src/mapping/tiles.rs"),
                ],
                expected_missing: &["mapper_config", "mapper_execution"],
            },
            Case {
                label: "buffer plus unrelated source and sink operations",
                question: "Explain how Buffer, Source, Sink, and buffered wrappers cooperate to move bytes through reads and writes.",
                citations: vec![
                    anchor_at("Buffer", "src/io/buffer.kt"),
                    anchor_at("DatabaseSource.read", "src/database/Source.vue"),
                    anchor_at("TelemetrySink.write", "src/telemetry/Sink.svelte"),
                    anchor_at("NetworkStream.flush", "src/network/stream.html"),
                ],
                expected_missing: &["buffered_read_write"],
            },
            Case {
                label: "near-prefix words from a different subsystem",
                question: "Explain how formatting arguments become type-erased format args and reach the vformat error fallback path.",
                citations: vec![
                    anchor_at("FormationArgs", "src/geometry/FormationArgs.vue"),
                    anchor_at("FormativeValues", "src/geometry/FormativeValues.svelte"),
                    anchor_at("FormationsStore", "src/geometry/FormationsStore.html"),
                    anchor_at("FormationError", "src/geometry/FormationError.svelte"),
                    anchor_at("FormativelyFallback", "src/geometry/Fallback.vue"),
                ],
                expected_missing: &["format_arguments", "format_errors"],
            },
        ];

        assert_eq!(
            cases.len(),
            6,
            "one executable verdict case per carrier flow"
        );

        for case in cases {
            let mut answer = answer_fixture(case.question);
            answer.citations = case.citations.clone();
            let claims = case
                .citations
                .into_iter()
                .enumerate()
                .map(|(index, citation)| {
                    let text = if index % 2 == 0 {
                        "The public API entrypoint exposes the requested flow."
                    } else {
                        "The implementation delegates downstream work through this step."
                    };
                    evidence_claim(text, citation)
                })
                .collect::<Vec<_>>();
            assert!(
                packet_supported_claim_family_count(&claims) >= 2,
                "{} must satisfy the DataFlow family floor so a Partial verdict cannot come from \
                 an unrelated family-count gate",
                case.label
            );

            let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/project"),
                question: case.question,
                task_class: PacketTaskClassDto::DataFlow,
                answer: &answer,
                budget: &budget_fixture(),
                supported_claims: claims,
                missing_required_probe_queries: Vec::new(),
                targeted_follow_up_queries: Vec::new(),
            });

            assert_eq!(
                sufficiency.status,
                PacketSufficiencyStatusDto::Partial,
                "{} returned a false-safe verdict: {sufficiency:?}",
                case.label
            );
            let report = sufficiency
                .coverage_report
                .as_ref()
                .expect("carrier-flow verdict includes a coverage report");
            for requirement in case.expected_missing {
                assert!(
                    report.missing.contains(&requirement.to_string()),
                    "{} did not name the unproved `{requirement}` step: {sufficiency:?}",
                    case.label
                );
            }
        }
    }

    #[test]
    fn unrelated_handler_registration_does_not_get_logger_handler_family() {
        let unrelated = claim("Request handler registration wires middleware callbacks.");
        assert_ne!(
            packet_claim_family(&unrelated),
            Some("logger handler stack"),
            "unrelated handler registration should not be labeled as log handler stack"
        );
        assert_eq!(
            packet_supported_claim_family_count(&[unrelated]),
            0,
            "unrelated handler registration should not add log-record sufficiency-family coverage"
        );

        let exact_stack =
            claim("The logger owns a handler stack populated by handler registration.");
        assert_eq!(
            packet_claim_family(&exact_stack),
            Some("logger handler stack"),
            "log/logger handler-stack wording should still carry the family"
        );
    }

    #[test]
    fn add_record_only_claim_does_not_satisfy_handler_processing_dispatch() {
        let claims = vec![claim(
            "addRecord creates a log record before passing it to handlers.",
        )];

        let missing = packet_missing_required_flow_roles(
            "Explain how a logger turns a log call into a record object and passes it through handlers.",
            PacketTaskClassDto::DataFlow,
            &claims,
        );
        assert!(
            missing.contains(&FlowRole::Dispatch),
            "addRecord-only evidence should not close handler processing through generic handler fallback: {missing:?}"
        );
    }

    #[test]
    fn handler_stack_without_processing_or_write_evidence_stays_partial() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            cited_claim(
                "addRecord creates a log record before passing it to handlers.",
                None,
                cited_anchor_with_tier(
                    "Logger::addRecord",
                    "src/logging/Logger.php",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                None,
            ),
            cited_claim(
                "The logger owns a handler stack populated by handler registration.",
                None,
                cited_anchor_with_tier(
                    "Logger::pushHandler",
                    "src/logging/Logger.php",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                None,
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.contains(&"handler_processing".to_string()),
            "handler stack/registration should not close processing without process/write evidence: {report:?}"
        );
        assert!(
            report.covered.contains(&"logger handler stack".to_string()),
            "handler stack evidence should remain covered context even when processing is missing: {report:?}"
        );
    }

    #[test]
    fn handler_stack_and_processing_without_record_creation_stays_partial() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            cited_claim(
                "The logger owns a handler stack populated by handler registration.",
                None,
                cited_anchor_with_tier(
                    "Logger::pushHandler",
                    "src/logging/Logger.php",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                None,
            ),
            cited_claim(
                "The processing handler handles records by processing and writing them.",
                None,
                cited_anchor_with_tier(
                    "LogProcessingHandler::handle",
                    "src/logging/ProcessingHandler.php",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                None,
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.contains(&"logger_event".to_string()),
            "handler stack plus processing should not close logger event without record-creation evidence: {report:?}"
        );
        assert!(
            report.covered.contains(&"handler processing".to_string()),
            "processing evidence should still cover handler processing: {report:?}"
        );
    }

    #[test]
    fn generic_source_navigation_handler_claim_stays_diagnostic() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            cited_claim(
                "addRecord creates a log record before passing it to handlers.",
                None,
                cited_anchor_with_tier(
                    "Logger::addRecord",
                    "src/logging/Logger.php",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                None,
            ),
            cited_claim(
                "`HandlerInterface` ties handler interface record handling boundaries in this flow to cited definitions and adjacent ownership.",
                Some("source evidence"),
                cited_anchor_with_tier(
                    "HandlerInterface",
                    "src/logging/HandlerInterface.php",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
                ),
                None,
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.contains(&"handler_processing".to_string()),
            "generic source-navigation handler text should not close handler processing: {report:?}"
        );
        assert!(
            report.ineligible.iter().any(|entry| {
                entry.contains("role=\"source evidence\"")
                    && entry.contains(
                        "generic navigation/source-evidence claim does not explain the flow",
                    )
            }),
            "source-navigation handler claim should remain diagnostic-only: {report:?}"
        );
    }

    #[test]
    fn sql_looking_claim_text_without_structural_citations_stays_partial() {
        let question = "Explain SQL schema relationships between artists, albums, tracks, invoices, and invoice lines across seed scripts.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        // Both claims are proof-bearing and cite resolved anchors, so nothing but the shape of that
        // evidence can decide the schema requirements. The anchors are ordinary application
        // symbols in a `.rb` file: `packet_evidence_role` classifies them as source evidence, which
        // is neither a table definition nor a relationship constraint.
        let claims = vec![
            evidence_claim(
                "SQL schema defines tables Artist, Album, Track, Invoice, and InvoiceLine.",
                anchor_at("Catalog.load", "app/models/catalog.rb"),
            ),
            evidence_claim(
                "Track rows reference Album, Genre, and MediaType rows.",
                anchor_at("Catalog.render", "app/views/catalog.rb"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.contains(&"sql_tables".to_string()),
            "SQL table wording without a table citation must stay missing: {report:?}"
        );
        assert!(
            report.missing.contains(&"sql_relationships".to_string()),
            "SQL relationship wording without an FK citation must stay missing: {report:?}"
        );
        // Pin the reason rather than relying on how the fixture happens to classify: the two SQL
        // requirements are refused because their evidence predicates reject these anchors, not
        // because the anchors landed on some other role or were ruled ineligible.
        let context = PacketFlowContext::new(question, PacketTaskClassDto::DataFlow);
        for requirement_id in ["sql_tables", "sql_relationships"] {
            let requirement = context
                .requirements
                .iter()
                .find(|requirement| requirement.id == requirement_id)
                .unwrap_or_else(|| panic!("the prompt should raise {requirement_id}"));
            for anchor in [
                anchor_at("Catalog.load", "app/models/catalog.rb"),
                anchor_at("Catalog.render", "app/views/catalog.rb"),
            ] {
                assert!(
                    citation_sufficiency_eligible(&anchor),
                    "the fixture anchors must be proof-bearing for this to test the predicate"
                );
                assert!(
                    !requirement.evidence.citation_proves(&anchor),
                    "{requirement_id} must reject `{}` by its own evidence predicate",
                    anchor.display_name
                );
            }
        }
    }

    #[test]
    fn synthetic_source_scan_stays_nonproof_for_non_structural_requirements() {
        let question = "Explain how formatting arguments become type-erased format args and reach vformat or format_to output paths.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            evidence_claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
                anchor_at("basic_format_args", "include/fmt/base.h"),
            ),
            evidence_claim(
                "Runtime formatting writes formatted output through output iterator helpers.",
                anchor_at("vformat_to", "include/fmt/format.h"),
            ),
            cited_claim(
                "SQL schema defines tables Artist and Album.",
                Some("source evidence"),
                cited_anchor_with_tier(
                    "CREATE TABLE Artist",
                    "schema.sql",
                    PacketEvidenceTierDto::SyntheticSourceScan,
                    Some(false),
                ),
                Some(false),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert_eq!(report.ineligible.len(), 1);
        assert!(report.ineligible[0].contains("tier=\"synthetic_source_scan\""));
        assert!(
            report.ineligible[0].contains("reason=\"claim marked diagnostic\""),
            "synthetic source-scan evidence should not become proof outside SQL structural requirements: {report:?}"
        );
    }

    #[test]
    fn github_actions_structural_source_does_not_satisfy_semantic_packet_proof() {
        let mut citation = cited_anchor_with_tier(
            "build",
            ".github/workflows/ci.yml",
            PacketEvidenceTierDto::StructuralText,
            Some(false),
        );
        citation.evidence_producer =
            Some("structural_github_actions_workflow_collector".to_string());
        citation.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);
        let claim = cited_claim(
            "The CI workflow build job runs the test command.",
            Some("command dispatch"),
            citation,
            None,
        );

        assert!(
            !packet_claim_can_satisfy_sufficiency(&claim),
            "structural workflow exact-source evidence must not satisfy semantic packet proof roles"
        );
    }

    #[test]
    fn openapi_endpoint_exact_source_does_not_satisfy_semantic_packet_proof() {
        let mut citation = cited_anchor_with_tier(
            "GET /api/users",
            "openapi.json",
            PacketEvidenceTierDto::ExactSource,
            None,
        );
        citation.evidence_producer = Some("openapi_endpoint_schema".to_string());
        citation.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);
        let claim = cited_claim(
            "The schema declares GET /api/users.",
            Some("request_entrypoint"),
            citation,
            None,
        );

        assert!(
            !packet_claim_can_satisfy_sufficiency(&claim),
            "OpenAPI endpoint schema anchors are diagnostic source ranges, not handler/runtime proof"
        );
    }

    fn form_native_constraint_claim() -> PacketClaimDto {
        evidence_claim(
            "The form validation examples use native required, pattern, min, and max constraints.",
            anchor_at("required", "examples/form-validation/index.html"),
        )
    }

    fn form_custom_validation_claim() -> PacketClaimDto {
        evidence_claim(
            "A custom validation example applies script-driven validity checks before rendering messages.",
            anchor_at("setCustomValidity", "examples/form-validation/validate.js"),
        )
    }

    fn form_submit_guard_claim() -> PacketClaimDto {
        evidence_claim(
            "Submit handlers prevent submission when the form is invalid.",
            anchor_at("onSubmitGuard", "examples/form-validation/submit.js"),
        )
    }

    #[test]
    fn covered_flow_roles_make_missing_probe_queries_follow_up_hints() {
        let question = "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            form_native_constraint_claim(),
            form_submit_guard_claim(),
            evidence_claim(
                "Custom error rendering branches on ValidityState fields to choose messages.",
                anchor_at(
                    "renderValidityMessage",
                    "examples/form-validation/messages.js",
                ),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: vec![
                "native form constraints".to_string(),
                "constraint validation".to_string(),
                "submit prevent default".to_string(),
            ],
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(sufficiency.follow_up_commands.is_empty());
        assert!(
            sufficiency
                .coverage_report
                .as_ref()
                .is_some_and(|report| report.missing.is_empty()),
            "covered flow roles should keep missing exact probe strings out of blocking coverage: {sufficiency:?}"
        );
    }

    #[test]
    fn form_validation_native_and_submit_without_custom_is_partial() {
        let question = "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        // Both claims are proof-bearing and cite real anchors, so only the shape of that evidence
        // can decide whether the custom-validation slot is covered.
        let claims = vec![form_native_constraint_claim(), form_submit_guard_claim()];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report
                .missing
                .contains(&"form_custom_validation".to_string()),
            "native constraints plus submit guard should still require custom validation: {report:?}"
        );
    }

    #[test]
    fn form_validation_custom_and_submit_without_native_is_partial() {
        let question = "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        // Both claims are proof-bearing and cite real anchors, so only the shape of that evidence
        // can decide whether the native-constraint slot is covered.
        let claims = vec![form_custom_validation_claim(), form_submit_guard_claim()];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report
                .missing
                .contains(&"form_native_constraints".to_string()),
            "custom validation plus submit guard should still require native constraints: {report:?}"
        );
    }

    #[test]
    fn form_validation_native_custom_and_submit_is_sufficient() {
        let question = "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            form_native_constraint_claim(),
            form_custom_validation_claim(),
            form_submit_guard_claim(),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(
            sufficiency
                .coverage_report
                .as_ref()
                .is_some_and(|report| report.missing.is_empty()),
            "all three form proof slots should satisfy the form-validation flow: {sufficiency:?}"
        );
    }

    #[test]
    fn missing_flow_role_keeps_matching_probe_query_blocking() {
        let question = "Trace how a WSGI app receives a request, opens request handling, dispatches to a view, finalizes the response, and returns control to the server.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            claim(
                "full_dispatch_request wraps preprocessing, dispatch, exception handling, and response finalization.",
            ),
            claim("dispatch_request invokes the view function selected by URL matching."),
            claim("The response finalization path returns control to the server."),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: vec!["route registration".to_string()],
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(report.missing.iter().any(|gap| gap == "request_entrypoint"));
        assert!(!report.missing.iter().any(|gap| gap == "route registration"));
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("route registration"))
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--query 'route registration'"))
        );
    }

    #[test]
    fn runtime_formatting_output_claims_do_not_cover_error_fallback_role() {
        let question = "Explain how formatting arguments become type-erased format args and reach vformat or format_to output paths.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        // Three proof-bearing claims over real formatting anchors. `format_arguments` and
        // `format_errors` are separate requirements, so argument, output, and buffer evidence must
        // leave the error/fallback requirement open — the case wording used to close.
        let claims = vec![
            evidence_claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
                anchor_at("basic_format_args", "include/fmt/base.h"),
            ),
            evidence_claim(
                "Runtime formatting writes formatted output through output iterator helpers.",
                anchor_at("vformat_to", "include/fmt/format.h"),
            ),
            evidence_claim(
                "Runtime formatting appends formatted output to a buffer.",
                anchor_at("basic_memory_buffer.append", "include/fmt/format.h"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: vec!["format error".to_string()],
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(report.missing.iter().any(|gap| gap == "format_errors"));
        assert!(!report.missing.iter().any(|gap| gap == "format error"));
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("format error"))
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--query 'format error'"))
        );
    }

    #[test]
    fn runtime_formatting_compact_verbose_truncation_keeps_complete_roles_sufficient() {
        let question = "Explain how formatting arguments become type-erased format args and reach vformat or format_to output paths.";
        let answer = answer_fixture(question);
        let budget = compact_truncated_budget(
            question,
            vec!["citations", "markdown_blocks", "trail_edges"],
        );
        let claims = vec![
            evidence_claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
                anchor_at("basic_format_args", "include/fmt/base.h"),
            ),
            evidence_claim(
                "Runtime formatting writes formatted output through output iterator helpers.",
                anchor_at("vformat_to", "include/fmt/format.h"),
            ),
            evidence_claim(
                "Runtime formatting defines format_error for formatting failures.",
                anchor_at("format_error", "include/fmt/format.h"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(sufficiency.gaps.is_empty());
        assert!(sufficiency.follow_up_commands.is_empty());
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(report.missing.is_empty());
        assert!(
            report.budget_omitted.is_empty(),
            "verbose compact truncation should not be reported as proof omission when roles are complete: {report:?}"
        );
    }

    #[test]
    fn url_session_compact_verbose_truncation_keeps_complete_roles_sufficient() {
        let question = "SessionRequest -> RequestResume -> RequestValidation -> SessionCallbacks";
        let answer = route_answer(
            question,
            &[
                "SessionRequest",
                "RequestResume",
                "RequestValidation",
                "SessionCallbacks",
            ],
            &[
                ("SessionRequest", "RequestResume"),
                ("RequestResume", "RequestValidation"),
                ("RequestValidation", "SessionCallbacks"),
            ],
        );
        let budget = compact_truncated_budget(question, vec!["markdown_blocks", "trail_edges"]);
        let claims = vec![
            cited_claim(
                "Session.request creates request objects before optional eager execution.",
                None,
                cited_anchor("SessionRequest"),
                Some(true),
            ),
            cited_claim(
                "Request.resume resumes the underlying URLSession task.",
                None,
                cited_anchor("RequestResume"),
                Some(true),
            ),
            cited_claim(
                "Request validation methods attach validation behavior.",
                None,
                cited_anchor("RequestValidation"),
                Some(true),
            ),
            cited_claim(
                "Session delegate callbacks receive URLSession task events.",
                None,
                cited_anchor("SessionCallbacks"),
                Some(true),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency_with_route_probes(
            PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/project"),
                question,
                task_class: PacketTaskClassDto::RouteTracing,
                answer: &answer,
                budget: &budget,
                supported_claims: claims,
                missing_required_probe_queries: Vec::new(),
                targeted_follow_up_queries: Vec::new(),
            },
            &[
                "SessionRequest".to_string(),
                "RequestResume".to_string(),
                "RequestValidation".to_string(),
                "SessionCallbacks".to_string(),
            ],
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{sufficiency:?}"
        );
        assert!(sufficiency.gaps.is_empty());
        assert!(sufficiency.follow_up_commands.is_empty());
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(report.missing.is_empty());
        assert!(report.budget_omitted.is_empty());
    }

    #[test]
    fn compact_truncated_packet_retains_proof_provenance_counts() {
        let question = "Explain compact packet provenance.";
        let answer = answer_fixture(question);
        let budget = compact_truncated_budget(question, vec!["citations", "markdown_blocks"]);
        let score_breakdown = |provenance: Vec<&str>| RetrievalScoreBreakdownDto {
            lexical: 0.0,
            semantic: 1.0,
            graph: 0.0,
            total: 1.0,
            tier_cap: None,
            boosts: Vec::new(),
            dampening: Vec::new(),
            final_rank_reason: None,
            provenance: provenance.into_iter().map(str::to_string).collect(),
        };
        let tier_cases = [
            ("exact", PacketEvidenceTierDto::ExactSource),
            ("lexical_source", PacketEvidenceTierDto::LexicalSource),
            ("symbol_doc", PacketEvidenceTierDto::SymbolDoc),
            ("graph_neighbor", PacketEvidenceTierDto::ResolvedGraph),
            ("component_report", PacketEvidenceTierDto::ComponentReport),
            ("dense_anchor", PacketEvidenceTierDto::DenseSemantic),
        ];
        let mut claims = tier_cases
            .iter()
            .map(|(label, tier)| {
                let text = format!("{label} proves provenance.");
                let path = format!("src/{label}.rs");
                let mut citation = cited_anchor_with_tier(label, &path, *tier, Some(true));
                if *label == "lexical_source" {
                    citation.retrieval_score_breakdown = Some(score_breakdown(vec![
                        "packet_required_file_scoped_source_probe",
                    ]));
                }
                cited_claim(&text, None, citation, None)
            })
            .collect::<Vec<_>>();
        let mut future_precise_import = cited_anchor_with_tier(
            "precise_semantic_import",
            "src/imports.rs",
            PacketEvidenceTierDto::DenseSemantic,
            Some(true),
        );
        future_precise_import.retrieval_score_breakdown =
            Some(score_breakdown(vec!["precise_semantic_import"]));
        claims.push(cited_claim(
            "Future precise semantic import provenance passes through.",
            None,
            future_precise_import,
            None,
        ));
        let mut same_file_name_affinity = cited_anchor_with_tier(
            "same_file_name_affinity",
            "src/service.rs",
            PacketEvidenceTierDto::DenseSemantic,
            Some(false),
        );
        assert_eq!(
            same_file_name_affinity.evidence_tier,
            Some(PacketEvidenceTierDto::DenseSemantic),
            "the affinity fixture must exercise the typed-tier pass-through path"
        );
        same_file_name_affinity.retrieval_score_breakdown =
            Some(score_breakdown(vec!["same_file_name_affinity"]));
        claims.push(cited_claim(
            "Same-file name affinity remains visible without becoming graph proof.",
            None,
            same_file_name_affinity,
            None,
        ));
        let claim_count = claims.len();
        let diagnostic_tier_claim_count = claims
            .iter()
            .filter(|claim| !claim.citations.iter().any(citation_sufficiency_eligible))
            .count();
        assert!(
            diagnostic_tier_claim_count > 0,
            "the fixture must still contain diagnostic-tier evidence for this to mean anything"
        );

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        // Provenance is a report over everything the packet retrieved, so all seven tiers still
        // appear below. `covered_claims` is narrower on purpose: it is what the caller may repeat,
        // and a claim whose only anchor is diagnostic-tier evidence is not proven.
        assert_eq!(
            sufficiency.covered_claims.len(),
            claim_count - diagnostic_tier_claim_count,
            "only the claims whose evidence is sufficiency-eligible are published: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .covered_claims
                .iter()
                .all(|claim| claim.citations.iter().any(citation_sufficiency_eligible)),
            "no published claim rests on diagnostic-only evidence: {sufficiency:?}"
        );
        assert!(!sufficiency.follow_up_commands.is_empty());
        let report = sufficiency.coverage_report.as_ref().unwrap();
        let expected_labels = [
            "component_report",
            "dense_anchor",
            "exact",
            "graph_neighbor",
            "lexical_source",
            "precise_semantic_import",
            "same_file_name_affinity",
            "symbol_doc",
        ];
        assert_eq!(
            report.provenance_labels,
            expected_labels
                .iter()
                .map(|label| (*label).to_string())
                .collect::<Vec<_>>()
        );
        for label in expected_labels {
            assert_eq!(report.provenance_counts.get(label), Some(&1));
        }
        assert!(
            !report
                .provenance_counts
                .contains_key("packet_required_file_scoped_source_probe")
        );
        assert!(
            budget.truncated
                && budget
                    .omitted_sections
                    .iter()
                    .any(|item| item == "citations"),
            "compact truncation state should remain visible beside provenance"
        );
    }

    /// The client-send lifecycle proved by cited evidence, one anchor per requirement, with the
    /// response boundary deliberately left out. Every claim keeps the wording it had when this
    /// fixture proved the same requirements through claim text.
    fn client_send_covering_claims() -> Vec<PacketClaimDto> {
        vec![
            evidence_claim(
                "Top-level HTTP helpers delegate to a Client.",
                anchor_at("createClient", "lib/client.dart"),
            ),
            evidence_claim(
                "Client convenience methods live on the client interface helper.",
                typed_anchor_at("Client.get", "lib/client.dart", NodeKind::METHOD),
            ),
            evidence_claim(
                "Base request finalize prepares request bodies for sending.",
                anchor_at("BaseRequest.finalize", "lib/base_request.dart"),
            ),
            evidence_claim(
                "The client dispatches the prepared request.",
                anchor_at("dispatchRequest", "lib/dispatch.dart"),
            ),
            evidence_claim(
                "The transport send implementation sends through an HTTP client adapter.",
                anchor_at("selectAdapter", "lib/adapters/select.dart"),
            ),
        ]
    }

    #[test]
    fn client_send_split_requirements_remain_distinct() {
        let question = "Explain how an HTTP client exposes top-level helpers, provides client convenience methods, finalizes requests before transport send, and materializes responses.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = client_send_covering_claims();

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/http-client"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert_eq!(
            report.missing,
            vec!["client_response_materialization".to_string()],
            "client-send coverage must preserve the missing response boundary slot: {report:?}"
        );
    }

    #[test]
    fn client_send_complete_split_requirements_are_sufficient() {
        let question = "Explain how an HTTP client exposes top-level helpers, provides client convenience methods, finalizes requests before transport send, and materializes responses.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let mut claims = client_send_covering_claims();
        claims.push(evidence_claim(
            "Response.fromStream materializes the response stream boundary.",
            anchor_at("Response.fromStream", "lib/response.dart"),
        ));

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/http-client"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.is_empty(),
            "complete client-send split roles should leave no flow gaps: {report:?}"
        );
    }

    /// The hook/cache flow proved by cited evidence, with the cache helper deliberately left out.
    fn hook_cache_covering_claims() -> Vec<PacketClaimDto> {
        vec![
            evidence_claim(
                "The public useData export wraps useDataHandler with argument normalization.",
                anchor_at("useData", "src/index/use-data.ts"),
            ),
            evidence_claim(
                "useDataHandler serializes hook keys into cache keys.",
                anchor_at("serializeKey", "src/_internal/utils/serialize.ts"),
            ),
            evidence_claim(
                "applyMutation routes mutate behavior through the mutation helper.",
                anchor_at("applyMutation", "src/_internal/utils/mutate.ts"),
            ),
        ]
    }

    #[test]
    fn hook_cache_requirements_remain_distinct() {
        let question = "Explain how a public hook serializes keys, connects cache helpers, and routes mutate behavior through a mutation helper.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = hook_cache_covering_claims();

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/hook-cache"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert_eq!(
            report.missing,
            vec!["hook_cache_helper".to_string()],
            "hook/cache coverage must preserve the missing cache-helper slot: {report:?}"
        );
    }

    #[test]
    fn hook_cache_complete_requirements_are_sufficient() {
        let question = "Explain how a public hook serializes keys, connects cache helpers, and routes mutate behavior through a mutation helper.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let mut claims = hook_cache_covering_claims();
        claims.push(evidence_claim(
            "makeCacheHelper provides cache get, set, subscribe, and snapshot helpers.",
            anchor_at("makeCacheHelper", "src/_internal/utils/helper.ts"),
        ));

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/hook-cache"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.is_empty(),
            "complete hook/cache roles should leave no flow gaps: {report:?}"
        );
    }

    /// The command-loop flow proved by cited evidence, with network input deliberately left out.
    fn command_loop_covering_claims() -> Vec<PacketClaimDto> {
        vec![
            evidence_claim(
                "Server bootstrap initializes the command server main loop.",
                anchor_at("main", "src/server.c"),
            ),
            evidence_claim(
                "The event loop source polls file events.",
                anchor_at("aeProcessEvents", "src/event/ae.c"),
            ),
            evidence_claim(
                "Command table dispatch routes commands to handlers.",
                anchor_at("processCommand", "src/server.c"),
            ),
        ]
    }

    #[test]
    fn command_loop_split_requirements_remain_distinct() {
        let question = "Trace how a command server bootstrap enters an event loop, reads network command input, and dispatches commands through a command table.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = command_loop_covering_claims();

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/command-server"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert_eq!(
            report.missing,
            vec!["command_network_input".to_string()],
            "command-loop coverage must not let generic dispatch close network input: {report:?}"
        );
    }

    #[test]
    fn command_dispatch_prompt_does_not_require_bootstrap_or_event_loop() {
        let question =
            "Trace how network command input reaches command table dispatch and command handlers.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            evidence_claim(
                "Network command input reads commands from socket input.",
                anchor_at("readQueryFromClient", "src/networking.c"),
            ),
            evidence_claim(
                "Command table dispatch routes commands to handlers.",
                anchor_at("processCommand", "src/server.c"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/command-server"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.is_empty(),
            "dispatch/input prompt should not inherit bootstrap or event-loop gaps: {report:?}"
        );
    }

    #[test]
    fn command_loop_complete_split_requirements_are_sufficient() {
        let question = "Trace how a command server bootstrap enters an event loop, reads network command input, and dispatches commands through a command table.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let mut claims = command_loop_covering_claims();
        claims.push(evidence_claim(
            "Network command input reads commands from socket input.",
            anchor_at("readQueryFromClient", "src/networking.c"),
        ));

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/command-server"),
            question,
            task_class: PacketTaskClassDto::DataFlow,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.is_empty(),
            "complete command-loop split roles should leave no flow gaps: {report:?}"
        );
    }

    #[test]
    fn compact_proof_omission_reports_missing_role_and_standard_budget_follow_up() {
        let question = "Explain how formatting arguments become type-erased format args and reach vformat or format_to output paths.";
        let mut answer = answer_fixture(question);
        mark_full_retrieval_available(&mut answer);
        let budget = compact_truncated_budget(question, vec!["citations", "markdown_blocks"]);
        let claims = vec![
            evidence_claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
                anchor_at("basic_format_args", "include/fmt/base.h"),
            ),
            evidence_claim(
                "Runtime formatting writes formatted output through output iterator helpers.",
                anchor_at("vformat_to", "include/fmt/format.h"),
            ),
            evidence_claim(
                "Runtime formatting appends formatted output to a buffer.",
                anchor_at("basic_memory_buffer.append", "include/fmt/format.h"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: vec!["format error".to_string()],
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(report.missing.iter().any(|gap| gap == "format_errors"));
        assert!(
            report
                .budget_omitted
                .iter()
                .any(|section| section == "citations")
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--budget standard")),
            "proof omission under compact budget should recommend the standard packet: {sufficiency:?}"
        );
    }

    #[test]
    fn compact_budget_blocks_sufficiency_when_source_proof_probe_is_missing() {
        let question = "Explain how buffered Source and Sink wrappers use Buffer state during reads and writes.";
        let mut answer = answer_fixture(question);
        mark_full_retrieval_available(&mut answer);
        let budget = compact_truncated_budget(question, vec!["citations", "trail_edges"]);
        let claims = vec![
            evidence_claim(
                "Buffer is the in-memory byte store used by buffered reads and writes.",
                anchor_at("Buffer", "src/io/buffer.rs"),
            ),
            evidence_claim(
                "Buffer::read moves bytes from an upstream source through the buffer.",
                anchor_at("Buffer::read", "src/io/buffer.rs"),
            ),
            evidence_claim(
                "Buffer::write moves buffered bytes to an upstream sink.",
                anchor_at("Buffer::write", "src/io/buffer.rs"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: vec!["source read buffer".to_string()],
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        for requirement in ["buffered_storage", "buffered_read_write"] {
            assert!(
                !report.missing.contains(&requirement.to_string()),
                "the fixture must cover flow roles so only compact proof omission drives repair: {report:?}"
            );
        }
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("answer-critical evidence")),
            "compact packets missing source-proof probes should not report sufficient: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--query 'source read buffer'")),
            "missing source-proof probe should remain the first repair path: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--budget standard")),
            "compact source-proof omissions should recommend the standard packet: {sufficiency:?}"
        );
    }

    #[test]
    fn route_tracing_site_build_prompts_use_lifecycle_flow_roles() {
        let claims = vec![
            evidence_claim(
                "Build.process constructs or processes a site.",
                anchor_at("Build.process", "lib/site/build.rb"),
            ),
            evidence_claim(
                "Site.process runs reset, read, generate, render, cleanup, and write phases.",
                anchor_at("Site.process", "lib/site/site.rb"),
            ),
            evidence_claim(
                "Reader is responsible for reading site content.",
                anchor_at("Reader.read_content", "lib/site/reader.rb"),
            ),
            // `Site.render`, not `Renderer.render`. The site's terminal boundary reads the name
            // and not the `lib/site/` folder now, and a renderer that does not say which renderer
            // it is is the shape `Layout.render` and `Page.render` evaded with. The render phase
            // hangs off the site object, which is where this claim's anchor takes it from.
            evidence_claim(
                "The site render phase renders pages and documents.",
                anchor_at("Site.render", "lib/site/renderer.rb"),
            ),
        ];

        let missing = packet_missing_required_flow_roles(
            "Trace how the build command creates a site and runs the read, generate, render, and write phases.",
            PacketTaskClassDto::RouteTracing,
            &claims,
        );
        assert!(
            missing.is_empty(),
            "site-build route-tracing prompts should use lifecycle flow roles: {missing:?}"
        );

        let route_missing = packet_missing_required_flow_roles(
            "Trace how a server request enters route registration, reaches request handler dispatch, and finalizes a response.",
            PacketTaskClassDto::RouteTracing,
            &claims,
        );
        assert!(
            route_missing.contains(&FlowRole::Registration),
            "server request tracing should still require request registration roles: {route_missing:?}"
        );
    }

    #[test]
    fn route_tracing_server_request_prompts_use_wsgi_flow_roles() {
        let claims = vec![
            evidence_claim(
                "wsgi_app is the WSGI entry point and creates or uses request context before dispatch.",
                anchor_at("Flask.wsgi_app", "src/flask/protocol/wsgi.py"),
            ),
            evidence_claim(
                "full_dispatch_request wraps preprocessing, dispatch, exception handling, and response finalization.",
                anchor_at("Flask.full_dispatch_request", "src/flask/app.py"),
            ),
            evidence_claim(
                "dispatch_request invokes the view function selected by URL matching.",
                anchor_at("Flask.dispatch_request", "src/flask/app.py"),
            ),
            evidence_claim(
                "Route registration decorator adds URL rules without performing request dispatch itself.",
                anchor_at("Flask.add_url_rule", "src/flask/scaffold.py"),
            ),
            evidence_claim(
                "The response buffer writes the finalized body back to the server.",
                anchor_at("ResponseBuffer.write", "src/flask/wrappers.py"),
            ),
        ];

        let missing = packet_missing_required_flow_roles(
            "Trace how a WSGI app receives a request, opens request handling, dispatches to a view, finalizes the response, and returns control to the server.",
            PacketTaskClassDto::RouteTracing,
            &claims,
        );

        assert!(
            missing.is_empty(),
            "server request dispatch prompts should use WSGI/request/view roles: {missing:?}"
        );
    }

    #[test]
    fn generic_request_dispatch_prompt_succeeds_without_benchmark_product_terms() {
        let question = "RouteRegistration -> HandlerDispatch -> ResponseFinalization";
        let answer = route_answer(
            question,
            &[
                "RouteRegistration",
                "HandlerDispatch",
                "ResponseFinalization",
            ],
            &[
                ("RouteRegistration", "HandlerDispatch"),
                ("HandlerDispatch", "ResponseFinalization"),
            ],
        );
        let budget = budget_fixture();
        let claims = vec![
            cited_claim(
                "Public request entrypoint registers route wrappers before dispatching handler calls.",
                Some("entrypoint"),
                cited_anchor("RouteRegistration"),
                Some(true),
            ),
            cited_claim(
                "Dispatch request invokes the selected view function or handler for the matched route.",
                None,
                cited_anchor("HandlerDispatch"),
                Some(true),
            ),
            cited_claim(
                "Response finalization boundary writes response output and returns control to the server.",
                None,
                cited_anchor("ResponseFinalization"),
                Some(true),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency_with_route_probes(
            PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/synthetic-service"),
                question,
                task_class: PacketTaskClassDto::RouteTracing,
                answer: &answer,
                budget: &budget,
                supported_claims: claims,
                missing_required_probe_queries: Vec::new(),
                targeted_follow_up_queries: Vec::new(),
            },
            &[
                "RouteRegistration".to_string(),
                "HandlerDispatch".to_string(),
                "ResponseFinalization".to_string(),
            ],
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(sufficiency.gaps.is_empty());
        assert!(sufficiency.follow_up_commands.is_empty());
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.is_empty(),
            "generic source-shape role coverage should satisfy request dispatch without product-specific strings: {report:?}"
        );
        for expected in ["entrypoint", "server view dispatch"] {
            assert!(
                report.covered.iter().any(|entry| entry == expected),
                "expected generic coverage report to include {expected}: {report:?}"
            );
        }
    }

    #[test]
    fn role_safe_sufficiency_requires_cited_requested_interceptor_evidence() {
        let question =
            "RequestEntry -> InterceptorRegistry::new -> RequestDispatch -> TransportSend";
        let answer = route_answer(
            question,
            &[
                "RequestEntry",
                "InterceptorRegistry::new",
                "RequestDispatch",
                "TransportSend",
            ],
            &[
                ("RequestEntry", "InterceptorRegistry::new"),
                ("InterceptorRegistry::new", "RequestDispatch"),
                ("RequestDispatch", "TransportSend"),
            ],
        );
        let budget = budget_fixture();
        let mut claims = vec![
            cited_claim(
                "The public client entrypoint creates a request before dispatch.",
                None,
                cited_anchor("RequestEntry"),
                Some(true),
            ),
            cited_claim(
                "Request dispatch transforms config and invokes the selected handler.",
                None,
                cited_anchor("RequestDispatch"),
                Some(true),
            ),
            cited_claim(
                "The transport boundary sends the request and returns a response.",
                None,
                cited_anchor("TransportSend"),
                Some(true),
            ),
        ];

        let selected_probes = [
            "RequestEntry".to_string(),
            "InterceptorRegistry::new".to_string(),
            "RequestDispatch".to_string(),
            "TransportSend".to_string(),
        ];
        let assemble = |supported_claims| {
            assemble_packet_sufficiency_with_route_probes(
                PacketSufficiencyInput {
                    project_root: Path::new("C:/workspace/generic-client"),
                    question,
                    task_class: PacketTaskClassDto::RouteTracing,
                    answer: &answer,
                    budget: &budget,
                    supported_claims,
                    missing_required_probe_queries: Vec::new(),
                    targeted_follow_up_queries: Vec::new(),
                },
                &selected_probes,
            )
        };

        let missing_role = assemble(claims.clone());
        assert_eq!(missing_role.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            missing_role
                .coverage_report
                .as_ref()
                .is_some_and(|report| report
                    .missing
                    .iter()
                    .any(|gap| gap == "route endpoint: InterceptorRegistry::new")),
            "an explicitly requested endpoint must remain missing without compatible cited evidence: {missing_role:?}"
        );

        let mut unrelated_helper = cited_anchor("requestInterceptorHandler");
        unrelated_helper.kind = NodeKind::FIELD;
        claims.push(cited_claim(
            "requestInterceptorHandler stores request interceptor handler pairs for chained execution.",
            Some("interceptor management"),
            unrelated_helper,
            Some(true),
        ));
        let unrelated_path = assemble(claims.clone());
        assert_eq!(unrelated_path.status, PacketSufficiencyStatusDto::Partial);

        let mut unrelated_type = cited_anchor("InterceptorOptions");
        unrelated_type.kind = NodeKind::CLASS;
        claims.push(cited_claim(
            "InterceptorOptions stores request interceptor handler pairs for chained execution.",
            Some("interceptor management"),
            unrelated_type,
            Some(true),
        ));
        let unrelated_type = assemble(claims.clone());
        assert_eq!(unrelated_type.status, PacketSufficiencyStatusDto::Partial);

        let mut interceptor_registry = cited_anchor("InterceptorRegistry::new");
        interceptor_registry.kind = NodeKind::METHOD;
        claims.insert(
            1,
            cited_claim(
                "InterceptorRegistry::new creates the request interceptor chain before dispatch.",
                Some("interceptor management"),
                interceptor_registry,
                Some(true),
            ),
        );
        let complete = assemble(claims);
        assert_eq!(
            complete.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{complete:?}"
        );
        assert!(
            complete
                .coverage_report
                .as_ref()
                .is_some_and(|report| report.missing.is_empty()),
            "role-compatible cited evidence should complete the requested flow: {complete:?}"
        );
    }

    #[test]
    fn unresolved_sidecar_diagnostics_do_not_block_when_required_roles_are_covered() {
        let question =
            "AppInitialization -> MiddlewareRegistration -> RequestHandler -> ResponseSend";
        let mut answer = route_answer(
            question,
            &[
                "AppInitialization",
                "MiddlewareRegistration",
                "RequestHandler",
                "ResponseSend",
            ],
            &[
                ("AppInitialization", "MiddlewareRegistration"),
                ("MiddlewareRegistration", "RequestHandler"),
                ("RequestHandler", "ResponseSend"),
            ],
        );
        answer.retrieval_trace.packet_sidecar_diagnostics = vec![
            unresolved_sidecar_diagnostic("response send"),
            unresolved_sidecar_diagnostic("response send helper"),
            unresolved_sidecar_diagnostic("helpers"),
        ];
        let budget = budget_fixture();
        let claims = vec![
            cited_claim(
                "AppInitialization creates the public request entrypoint.",
                None,
                cited_anchor("AppInitialization"),
                Some(true),
            ),
            cited_claim(
                "MiddlewareRegistration registers route wrappers before dispatch.",
                None,
                cited_anchor("MiddlewareRegistration"),
                Some(true),
            ),
            cited_claim(
                "RequestHandler invokes the selected handler for the matched route.",
                None,
                cited_anchor("RequestHandler"),
                Some(true),
            ),
            cited_claim(
                "ResponseSend finalizes response output and returns control to the server.",
                None,
                cited_anchor("ResponseSend"),
                Some(true),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency_with_route_probes(
            PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/express"),
                question,
                task_class: PacketTaskClassDto::RouteTracing,
                answer: &answer,
                budget: &budget,
                supported_claims: claims,
                missing_required_probe_queries: vec!["response send".to_string()],
                targeted_follow_up_queries: Vec::new(),
            },
            &[
                "app initialization".to_string(),
                "middleware registration".to_string(),
                "request handler".to_string(),
                "response send".to_string(),
            ],
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(sufficiency.gaps.is_empty());
        assert!(sufficiency.follow_up_commands.is_empty());
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(report.missing.is_empty());
        assert_eq!(
            report.unresolved,
            vec![
                "response send".to_string(),
                "response send helper".to_string(),
                "helpers".to_string(),
            ]
        );
    }

    #[test]
    fn unresolved_selected_probe_blocks_when_express_response_coverage_is_missing() {
        let question = "Trace how Express creates an application, registers middleware/routes, and handles an incoming request through the router and response helpers.";
        let mut answer = answer_fixture(question);
        mark_full_retrieval_available(&mut answer);
        answer.retrieval_trace.packet_sidecar_diagnostics =
            vec![unresolved_sidecar_diagnostic("response send")];
        let budget = budget_fixture();
        let claims = vec![
            PacketClaimDto {
                claim: "Selected callback invocation happens.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: Some("dispatch".to_string()),
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "Selected handler invocation happens.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: Some("dispatch".to_string()),
                eligible_for_sufficiency: None,
            },
        ];

        let selected_probes = vec![
            "app initialization".to_string(),
            "middleware registration".to_string(),
            "request handler".to_string(),
            "response send".to_string(),
        ];
        let sufficiency = assemble_packet_sufficiency_with_route_probes(
            PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/express"),
                question,
                task_class: PacketTaskClassDto::RouteTracing,
                answer: &answer,
                budget: &budget,
                supported_claims: claims,
                missing_required_probe_queries: vec!["response send".to_string()],
                targeted_follow_up_queries: Vec::new(),
            },
            &selected_probes,
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report
                .missing
                .contains(&"route order: unresolved endpoints".to_string()),
            "natural-language framing must fail closed before selected probes imply route order: {report:?}"
        );
        assert_eq!(report.unresolved, vec!["response send".to_string()]);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("response send"))
        );
        assert!(
            sufficiency
                .follow_up_commands
                .first()
                .is_some_and(|command| command.contains("--query 'response send'")),
            "unresolved selected probe should become the follow-up when no missing flow seed exists: {:?}",
            sufficiency.follow_up_commands
        );
    }

    #[test]
    fn missing_flow_seed_follow_up_precedes_unresolved_selected_probe() {
        let question = "Trace how a server request enters route registration, reaches request handler dispatch, and finalizes a response.";
        let mut answer = answer_fixture(question);
        mark_full_retrieval_available(&mut answer);
        answer.retrieval_trace.packet_sidecar_diagnostics =
            vec![unresolved_sidecar_diagnostic("response send")];
        let budget = budget_fixture();
        let claims = vec![
            PacketClaimDto {
                claim: "Selected callback invocation happens.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: Some("dispatch".to_string()),
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "Selected handler invocation happens.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: Some("dispatch".to_string()),
                eligible_for_sufficiency: None,
            },
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/service"),
            question,
            task_class: PacketTaskClassDto::RouteTracing,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: vec![
                "route registration".to_string(),
                "response send".to_string(),
            ],
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(report.missing.iter().any(|gap| gap == "request_entrypoint"));
        assert!(report.missing.iter().any(|gap| gap == "request_terminal"));
        assert_eq!(report.unresolved, vec!["response send".to_string()]);
        assert!(
            sufficiency.follow_up_commands.len() >= 2,
            "expected both missing flow seed and unresolved selected probe follow-ups: {sufficiency:?}"
        );
        assert!(
            sufficiency.follow_up_commands[0].contains("--query 'route registration'"),
            "missing flow seed should remain first follow-up: {:?}",
            sufficiency.follow_up_commands
        );
        assert!(
            sufficiency.follow_up_commands[1].contains("--query 'response send'"),
            "unresolved selected probe should follow missing flow seed: {:?}",
            sufficiency.follow_up_commands
        );
    }

    #[test]
    fn mixed_sidecar_diagnostics_block_when_required_coverage_is_missing() {
        let question = "Trace how a server request enters route registration, reaches request handler dispatch, and finalizes a response.";
        let mut answer = answer_fixture(question);
        let mut diagnostic = unresolved_sidecar_diagnostic("response finalization");
        diagnostic.candidate_count = 2;
        diagnostic.resolved_hit_count = 1;
        answer.retrieval_trace.packet_sidecar_diagnostics = vec![diagnostic];
        let budget = budget_fixture();
        let claims = vec![
            claim(
                "Public request entrypoint registers route wrappers before dispatching handler calls.",
            ),
            claim(
                "Dispatch request invokes the selected view function or handler for the matched route.",
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/service"),
            question,
            task_class: PacketTaskClassDto::RouteTracing,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(report.missing.iter().any(|gap| gap == "request_terminal"));
        assert_eq!(report.unresolved, vec!["response finalization".to_string()]);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("response finalization"))
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--query 'response finalization'"))
        );
    }

    #[test]
    fn cancelled_sidecar_diagnostics_block_when_required_coverage_is_missing() {
        let question = "Trace how a server request enters route registration, reaches request handler dispatch, and finalizes a response.";
        let mut answer = answer_fixture(question);
        answer.retrieval_trace.packet_sidecar_diagnostics =
            vec![cancelled_sidecar_diagnostic("response finalization")];
        let budget = budget_fixture();
        let claims = vec![
            claim(
                "Public request entrypoint registers route wrappers before dispatching handler calls.",
            ),
            claim(
                "Dispatch request invokes the selected view function or handler for the matched route.",
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/service"),
            question,
            task_class: PacketTaskClassDto::RouteTracing,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(report.missing.iter().any(|gap| gap == "request_terminal"));
        assert_eq!(report.unresolved, vec!["response finalization".to_string()]);
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("response finalization"))
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--query 'response finalization'"))
        );
    }

    #[test]
    fn partial_packets_with_blocked_full_retrieval_recommend_repair_and_local_graph() {
        let question = "Trace how route registration reaches response finalization.";
        let mut answer = answer_fixture(question);
        mark_full_retrieval_unavailable(&mut answer);
        let budget = budget_fixture();
        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/service"),
            question,
            task_class: PacketTaskClassDto::RouteTracing,
            answer: &answer,
            budget: &budget,
            supported_claims: vec![claim(
                "Dispatch request invokes the selected view function or handler for the matched route.",
            )],
            missing_required_probe_queries: vec!["route registration".to_string()],
            targeted_follow_up_queries: vec!["response finalization".to_string()],
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .follow_up_commands
                .first()
                .is_some_and(|command| command.contains("retrieval index")),
            "blocked full retrieval should lead with retrieval activation: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("codestory-cli trail")
                    && command.contains("--query 'route registration'")),
            "blocked full retrieval should still offer local graph follow-up: {sufficiency:?}"
        );
        assert!(
            sufficiency.follow_up_commands.iter().all(|command| {
                !command.contains("codestory-cli search")
                    && !command.contains("codestory-cli context")
            }),
            "blocked full retrieval must not recommend blocked search/context surfaces: {sufficiency:?}"
        );
    }

    #[test]
    fn partial_packets_with_missing_retrieval_shadow_recommend_repair() {
        let question = "Trace how route registration reaches response finalization.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/service"),
            question,
            task_class: PacketTaskClassDto::RouteTracing,
            answer: &answer,
            budget: &budget,
            supported_claims: vec![claim(
                "Dispatch request invokes the selected view function or handler for the matched route.",
            )],
            missing_required_probe_queries: vec!["route registration".to_string()],
            targeted_follow_up_queries: vec!["response finalization".to_string()],
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .follow_up_commands
                .first()
                .is_some_and(|command| command.contains("retrieval index")),
            "missing retrieval metadata should lead with retrieval activation: {sufficiency:?}"
        );
        assert!(
            sufficiency.follow_up_commands.iter().all(|command| {
                !command.contains("codestory-cli search")
                    && !command.contains("codestory-cli context")
            }),
            "missing retrieval shadow must not recommend unproven search/context surfaces: {sufficiency:?}"
        );
    }

    #[test]
    fn insufficient_packets_with_blocked_full_retrieval_avoid_search_recovery() {
        let question = "Explain route dispatch with enough evidence to stop.";
        let mut answer = answer_fixture(question);
        answer.citations.clear();
        mark_full_retrieval_unavailable(&mut answer);
        let budget = budget_fixture();
        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/service"),
            question,
            task_class: PacketTaskClassDto::RouteTracing,
            answer: &answer,
            budget: &budget,
            supported_claims: Vec::new(),
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Insufficient);
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("retrieval index")),
            "blocked insufficient packet should recommend retrieval activation: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("codestory-cli ground")),
            "blocked insufficient packet should retain a local graph surface: {sufficiency:?}"
        );
        assert!(
            sufficiency.follow_up_commands.iter().all(|command| {
                !command.contains("codestory-cli search")
                    && !command.contains("codestory-cli context")
            }),
            "blocked insufficient packet must not recommend blocked search/context surfaces: {sufficiency:?}"
        );
    }

    #[test]
    fn architecture_html_css_template_prompts_use_structural_roles() {
        let claims = vec![
            evidence_claim(
                "home.html provides the app shell with viewport metadata, div#app, and a script[type=\"module\"] module script entry.",
                anchor_at("div#app", "src/home.html"),
            ),
            evidence_claim(
                "main.css owns :root typography, color-scheme, smoothing, and body layout defaults.",
                anchor_at(":root", "src/main.css"),
            ),
            evidence_claim(
                "CSS app container rules constrain mounted content and center it with padding.",
                anchor_at("#app", "src/main.css"),
            ),
            evidence_claim(
                "CSS interaction selectors define hover, focus, and transition behavior.",
                anchor_at("a:hover", "src/main.css"),
            ),
            evidence_claim(
                "Light color-scheme media query rules override root, link-hover, and button colors.",
                anchor_at("@media (prefers-color-scheme: light)", "src/main.css"),
            ),
        ];

        let missing = packet_missing_required_flow_roles(
            "Explain how the HTML app shell and CSS structure split template selectors, theme defaults, and interactive element styling.",
            PacketTaskClassDto::ArchitectureExplanation,
            &claims,
        );

        assert!(
            missing.is_empty(),
            "HTML/CSS template prompts should use structural app-shell/style roles: {missing:?}"
        );
    }

    #[test]
    fn css_animation_prompt_with_animation_evidence_does_not_require_html_app_shell() {
        let question = "Explain how a stylesheet defines shared animation variables, base classes, and connects named animation classes to keyframes.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            evidence_claim(
                "The animation stylesheet entrypoint imports variable, base, and animation files.",
                anchor_at("@import \"animations/base\"", "src/animations/index.css"),
            ),
            evidence_claim(
                "Shared CSS custom properties define animation duration, delay, and repeat defaults.",
                anchor_at("--animation-duration", "src/animations/variables.css"),
            ),
            evidence_claim(
                "The base class applies animation duration and fill mode, while named classes set animation-name to matching keyframes.",
                anchor_at("@keyframes fade-in", "src/animations/fade.css"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            !report.missing.contains(&"html_app_shell".to_string()),
            "CSS animation prompts should not inherit HTML app-shell requirements: {report:?}"
        );
    }

    #[test]
    fn generic_html_css_template_prompt_still_requires_app_shell_plus_css_structure() {
        let question = "Explain how the HTML app shell and CSS structure split template selectors, theme defaults, and interactive element styling.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            evidence_claim(
                "main.css owns :root typography, color-scheme, smoothing, and body layout defaults.",
                anchor_at(":root", "src/main.css"),
            ),
            evidence_claim(
                "CSS app container rules constrain mounted content and center it with padding.",
                anchor_at("#app", "src/main.css"),
            ),
            evidence_claim(
                "CSS interaction selectors define hover, focus, and transition behavior.",
                anchor_at("a:hover", "src/main.css"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.contains(&"html_app_shell".to_string()),
            "generic HTML/CSS prompts should still require app-shell evidence: {report:?}"
        );
        assert!(
            !report.missing.contains(&"css_structure".to_string()),
            "CSS structure evidence should cover the stylesheet side of the template prompt: {report:?}"
        );
    }

    #[test]
    fn data_flow_mapper_plan_prompts_use_mapping_flow_roles() {
        let claims = vec![
            evidence_claim(
                "Mapper runtime source exposes the public object-mapping entry point.",
                anchor_at("Mapper.Map", "src/AutoMapper/Mapper.cs"),
            ),
            evidence_claim(
                "Mapping configuration source builds and owns runtime mapping plans.",
                anchor_at(
                    "MapperConfiguration.BuildProfile",
                    "src/AutoMapper/MapperConfiguration.cs",
                ),
            ),
            evidence_claim(
                "Type-map source contributes lambda plans used by the mapping execution pipeline.",
                anchor_at(
                    "TypeMapPlanBuilder",
                    "src/AutoMapper/Execution/TypeMapPlanBuilder.cs",
                ),
            ),
            evidence_claim(
                "The mapping plan builder participates in building expression plans for mappings.",
                anchor_at(
                    "ExpressionPlanBuilder",
                    "src/AutoMapper/Execution/ExpressionPlanBuilder.cs",
                ),
            ),
        ];

        let missing = packet_missing_required_flow_roles(
            "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects through type map plans.",
            PacketTaskClassDto::DataFlow,
            &claims,
        );
        assert!(
            missing.is_empty(),
            "mapper plan prompts should use mapping flow roles: {missing:?}"
        );
    }

    #[test]
    fn data_flow_sql_schema_prompts_use_schema_relationship_roles() {
        // The same schema anchors as before, resolved rather than source-scanned: a text scan is
        // diagnostic evidence and no longer promotes a verdict, so the covering case has to cite
        // resolved schema anchors. `sql_looking_claim_text_without_structural_citations_stays_partial`
        // keeps the uncovered direction.
        let claims = vec![
            evidence_claim(
                "SQL schema defines tables Artist, Album, Track, Invoice, and InvoiceLine.",
                anchor_at("CREATE TABLE Artist", "db/schema.sql"),
            ),
            evidence_claim(
                "Track rows reference Album, Genre, and MediaType rows.",
                anchor_at("FOREIGN KEY", "db/schema.sql"),
            ),
            evidence_claim(
                "The repository carries multiple SQL dialect scripts for the same schema.",
                anchor_at("CHECK constraint", "db/postgres.sql"),
            ),
        ];

        let missing = packet_missing_required_flow_roles(
            "Explain SQL schema relationships between artists, albums, tracks, invoices, and invoice lines across seed scripts.",
            PacketTaskClassDto::DataFlow,
            &claims,
        );
        assert!(
            missing.is_empty(),
            "SQL schema prompts should use table, relationship, and dialect roles: {missing:?}"
        );
    }

    #[test]
    fn data_flow_log_record_handler_prompts_use_record_and_handler_roles() {
        let claims = vec![
            evidence_claim(
                "The logger owns a handler stack populated by handler registration.",
                anchor_at("Logger.pushHandler", "src/logging/Logger.php"),
            ),
            evidence_claim(
                "addRecord creates a log record before passing it to handlers.",
                anchor_at("Logger.addRecord", "src/logging/Logger.php"),
            ),
            evidence_claim(
                "The handler interface defines record handling and batch handling boundaries.",
                anchor_at(
                    "LogHandlerInterface.handleBatch",
                    "src/logging/HandlerInterface.php",
                ),
            ),
            evidence_claim(
                "The processing handler handles records by processing and writing them.",
                anchor_at(
                    "LogProcessingHandler.write",
                    "src/logging/AbstractProcessingHandler.php",
                ),
            ),
        ];

        let missing = packet_missing_required_flow_roles(
            "Explain how a logger turns a log call into a record object and passes it through handlers.",
            PacketTaskClassDto::DataFlow,
            &claims,
        );
        assert!(
            missing.is_empty(),
            "log-record handler prompts should use record creation and handler processing roles: {missing:?}"
        );
        assert!(
            packet_supported_claim_family_count(&claims) >= 3,
            "log-record handler claims should cover distinct sufficiency families"
        );
    }

    #[test]
    fn architecture_runtime_formatting_prompts_use_argument_output_error_roles() {
        let claims = vec![
            evidence_claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
                anchor_at("basic_format_args", "include/fmt/base.h"),
            ),
            evidence_claim(
                "Runtime formatting writes formatted output through output iterator helpers.",
                anchor_at("vformat_to", "include/fmt/format.h"),
            ),
            evidence_claim(
                "Runtime formatting defines an error type for formatting failures.",
                anchor_at("format_error", "include/fmt/format.h"),
            ),
        ];

        let missing = packet_missing_required_flow_roles(
            "Explain how formatting arguments become type-erased format args and reach vformat or format_to output paths.",
            PacketTaskClassDto::ArchitectureExplanation,
            &claims,
        );
        assert!(
            missing.is_empty(),
            "runtime formatting prompts should use argument, output, and error roles: {missing:?}"
        );
        assert!(
            packet_supported_claim_family_count(&claims) >= 3,
            "runtime formatting claims should cover distinct sufficiency families"
        );
    }

    #[test]
    fn architecture_form_validation_prompts_use_constraint_submit_and_validity_roles() {
        let claims = vec![
            evidence_claim(
                "The form validation examples use native required, pattern, min, and max constraints.",
                anchor_at("required", "examples/form-validation/index.html"),
            ),
            evidence_claim(
                "A custom validation example applies script-driven validity checks before rendering messages.",
                anchor_at("setCustomValidity", "examples/form-validation/validate.js"),
            ),
            evidence_claim(
                "Submit handlers prevent submission when the form is invalid.",
                anchor_at("onSubmitGuard", "examples/form-validation/submit.js"),
            ),
            evidence_claim(
                "Custom error rendering branches on ValidityState fields to choose messages.",
                anchor_at(
                    "renderValidityMessage",
                    "examples/form-validation/messages.js",
                ),
            ),
        ];

        let missing = packet_missing_required_flow_roles(
            "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation.",
            PacketTaskClassDto::ArchitectureExplanation,
            &claims,
        );
        assert!(
            missing.is_empty(),
            "form validation prompts should use constraint, submit, and validity-state roles: {missing:?}"
        );
        assert!(
            packet_supported_claim_family_count(&claims) >= 3,
            "form validation claims should cover distinct sufficiency families"
        );
    }

    #[test]
    fn architecture_string_predicate_prompts_use_blank_empty_region_roles() {
        let claims = vec![
            evidence_claim(
                "StringUtils.isBlank treats null, empty, and whitespace-only inputs as blank.",
                anchor_at(
                    "StringUtils.isBlank",
                    "src/main/java/org/apache/commons/lang3/StringUtils.java",
                ),
            ),
            evidence_claim(
                "StringUtils.isEmpty does not trim whitespace before deciding emptiness.",
                anchor_at(
                    "StringUtils.isEmpty",
                    "src/main/java/org/apache/commons/lang3/StringUtils.java",
                ),
            ),
            evidence_claim(
                "Strings delegates region matching work to CharSequenceUtils.regionMatches.",
                anchor_at(
                    "Strings.regionMatches",
                    "src/main/java/org/apache/commons/lang3/Strings.java",
                ),
            ),
        ];

        let missing = packet_missing_required_flow_roles(
            "Explain how string helpers implement blank, empty, and case-sensitive string checks across StringUtils, Strings, and CharSequenceUtils.",
            PacketTaskClassDto::ArchitectureExplanation,
            &claims,
        );
        assert!(
            missing.is_empty(),
            "string predicate prompts should use public helper, behavior, and region handoff roles: {missing:?}"
        );
        assert!(
            packet_supported_claim_family_count(&claims) >= 3,
            "string predicate claims should cover distinct sufficiency families"
        );
    }

    #[test]
    fn a_claim_without_cited_evidence_cannot_satisfy_sufficiency() {
        let question = "Explain what owns this behavior.";
        let answer = answer_fixture(question);
        let unsupported = claim("The runtime validates every request before it is dispatched.");

        assert!(!packet_claim_can_satisfy_sufficiency(&unsupported));

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::SymbolOwnership,
            answer: &answer,
            budget: &budget_fixture(),
            supported_claims: vec![unsupported],
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency.covered_claims.is_empty(),
            "an unsupported sentence must not be published as a covered claim: {sufficiency:?}"
        );
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report
                .ineligible
                .iter()
                .any(|entry| entry.contains("reason=\"claim carries no cited evidence\"")),
            "an unsupported sentence must be reported as unproven, not counted: {report:?}"
        );
        assert!(report.covered.is_empty(), "{report:?}");
    }

    #[test]
    fn a_claim_the_packet_reports_as_unproven_is_never_published_as_covered() {
        // Callers read covered_claims as verified and safe to repeat. Publishing a claim that the
        // same packet lists as ineligible would restate #1200's false-safe answer one claim down.
        let question = "Explain what owns this behavior.";
        let mut answer = answer_fixture(question);
        let anchor = anchor_at(
            "publish_generation",
            "crates/codestory-store/src/publication.rs",
        );
        answer.citations = vec![anchor.clone()];
        let navigation = cited_claim(
            "`publish_generation` ties publication in this flow to cited definitions and adjacent ownership.",
            Some("source evidence"),
            anchor,
            Some(true),
        );

        assert!(!packet_claim_can_satisfy_sufficiency(&navigation));

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::SymbolOwnership,
            answer: &answer,
            budget: &budget_fixture(),
            supported_claims: vec![navigation],
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency.covered_claims.is_empty(),
            "a cited claim that only points at evidence must not be published: {sufficiency:?}"
        );
        assert!(
            sufficiency.avoid_opening_paths.is_empty(),
            "a file only named by an unproven claim stays worth opening: {sufficiency:?}"
        );
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.ineligible.iter().any(|entry| entry.contains(
                "reason=\"generic navigation/source-evidence claim does not explain the flow\""
            )),
            "the dropped claim must still be explained in the coverage report: {report:?}"
        );
    }

    #[test]
    fn every_task_class_needs_a_proof_bearing_claim_for_each_resolved_exact_path() {
        let covered_path = "crates/codestory-cli/src/stdio_transport.rs";
        let uncovered_path = "crates/codestory-runtime/src/agent/orchestrator.rs";
        let covered = anchor_at("dispatch_stdio_request", covered_path);
        let exact_paths = [covered_path.to_string(), uncovered_path.to_string()];

        for task_class in [
            PacketTaskClassDto::ArchitectureExplanation,
            PacketTaskClassDto::RouteTracing,
            PacketTaskClassDto::DataFlow,
            PacketTaskClassDto::ChangeImpact,
            PacketTaskClassDto::EditPlanning,
            PacketTaskClassDto::BugLocalization,
            PacketTaskClassDto::SymbolOwnership,
        ] {
            let question = "Explain what these exact paths do.";
            let mut answer = answer_fixture(question);
            answer.citations = vec![covered.clone()];

            let sufficiency = assemble_packet_sufficiency_with_probe_context(
                &MissingPathSpellingIdentity,
                PacketSufficiencyInput {
                    project_root: Path::new("C:/workspace/project"),
                    question,
                    task_class,
                    answer: &answer,
                    budget: &budget_fixture(),
                    supported_claims: vec![evidence_claim(
                        "The stdio adapter dispatches the host request.",
                        covered.clone(),
                    )],
                    missing_required_probe_queries: Vec::new(),
                    targeted_follow_up_queries: Vec::new(),
                },
                &[],
                &exact_paths,
                None,
            );

            assert_ne!(
                sufficiency.status,
                PacketSufficiencyStatusDto::Sufficient,
                "{task_class:?} packet must not report sufficient while an exact path is unproven: {sufficiency:?}"
            );
            assert!(
                sufficiency
                    .gaps
                    .iter()
                    .any(|gap| gap.contains(uncovered_path)),
                "{task_class:?} packet needs a path-specific gap: {sufficiency:?}"
            );
            assert!(
                !sufficiency
                    .gaps
                    .iter()
                    .any(|gap| gap.contains(covered_path)),
                "{task_class:?} packet must not report a proven path as missing: {sufficiency:?}"
            );
            assert!(
                sufficiency
                    .follow_up_commands
                    .iter()
                    .any(|command| command.contains(uncovered_path)),
                "{task_class:?} packet needs a targeted follow-up for the unproven path: {sufficiency:?}"
            );
        }
    }

    #[test]
    fn more_uncovered_exact_paths_than_the_gap_budget_are_summarized_not_dropped() {
        let question = "Explain what these exact paths do.";
        let answer = answer_fixture(question);
        let exact_paths = (0..MAX_EXACT_PATH_CLAIM_GAPS + 2)
            .map(|index| format!("crates/example/src/module_{index}.rs"))
            .collect::<Vec<_>>();

        let sufficiency = assemble_packet_sufficiency_with_probe_context(
            &MissingPathSpellingIdentity,
            PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/project"),
                question,
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                answer: &answer,
                budget: &budget_fixture(),
                supported_claims: Vec::new(),
                missing_required_probe_queries: Vec::new(),
                targeted_follow_up_queries: Vec::new(),
            },
            &[],
            &exact_paths,
            None,
        );

        let path_gaps = sufficiency
            .gaps
            .iter()
            .filter(|gap| gap.contains("explicit exact path"))
            .count();
        assert_eq!(
            path_gaps, MAX_EXACT_PATH_CLAIM_GAPS,
            "path-specific gaps stay bounded: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .gaps
                .iter()
                .any(|gap| gap.contains("2 further requested exact path(s)")),
            "the paths beyond the gap budget are still reported: {sufficiency:?}"
        );
        let report = sufficiency.coverage_report.as_ref().unwrap();
        for path in &exact_paths {
            assert!(
                report.missing.contains(&format!("exact path: {path}")),
                "the coverage report names every unproven path: {report:?}"
            );
        }
    }

    #[test]
    fn requirement_coverage_comes_from_cited_evidence_not_claim_wording() {
        let context =
            PacketFlowContext::new("Explain request dispatch.", PacketTaskClassDto::DataFlow);
        let requirement = *context
            .requirements
            .iter()
            .find(|requirement| requirement.id == "request_dispatch")
            .expect("a request-dispatch prompt raises the dispatch requirement");
        let wording_only = evidence_claim(
            "The runtime dispatches every request through a central handler.",
            anchor_at("ProjectSettings", "src/settings.rs"),
        );
        let evidence_backed = evidence_claim(
            "The runtime dispatches every request through a central handler.",
            anchor_at("dispatchRequest", "src/dispatch.rs"),
        );

        assert!(
            !context.claim_satisfies_requirement(&wording_only, &requirement),
            "dispatch wording over unrelated evidence must not cover a dispatch requirement"
        );
        assert!(
            context.claim_satisfies_requirement(&evidence_backed, &requirement),
            "a cited dispatch symbol covers the dispatch requirement"
        );
    }

    #[test]
    fn evidence_at_one_flow_role_does_not_close_the_next_role_in_the_same_flow() {
        let question = "Explain how a logger turns a log call into a record object and passes it through handlers.";
        let claims = vec![evidence_claim(
            "The log entrypoint builds a record before handlers see it.",
            anchor_at("Logger.addRecord", "src/logging/Logger.php"),
        )];

        let missing =
            packet_missing_required_flow_roles(question, PacketTaskClassDto::DataFlow, &claims);
        assert!(
            !missing.contains(&FlowRole::Entrypoint),
            "cited record-creation evidence should close the entrypoint requirement: {missing:?}"
        );
        assert!(
            missing.contains(&FlowRole::Dispatch),
            "record-creation evidence must not also close the handler requirement beside it: {missing:?}"
        );
    }

    /// The three holdout prompts in `benchmarks/tasks/holdout-retrieval/`, each with every
    /// component cited except the one the manifest names. This lane's acceptance criterion is that
    /// the packet refuses in exactly that case, so it belongs in the unit suite rather than only in
    /// a corpus run.
    #[test]
    fn holdout_prompts_stay_partial_when_the_named_component_is_uncited() {
        struct HoldoutCase {
            id: &'static str,
            question: &'static str,
            cited: &'static [(&'static str, &'static str)],
            uncited_requirement: &'static str,
        }

        let cases = [
            HoldoutCase {
                id: "axios-request-dispatch",
                question: "Explain how the default axios instance is created and how an HTTP request flows through interceptors, dispatchRequest, and the transport adapter. Cite the source files that support the path.",
                cited: &[
                    ("createInstance", "lib/axios.js"),
                    ("dispatchRequest", "lib/core/dispatchRequest.js"),
                    ("getAdapter", "lib/adapters/adapters.js"),
                ],
                uncited_requirement: "request_interceptor_management",
            },
            HoldoutCase {
                id: "redis-server-event-loop",
                question: "Explain how the Redis server starts its event loop, reads client commands from the network, and dispatches them through processCommand and call. Cite the source files that support the path.",
                cited: &[
                    ("main", "src/server.c"),
                    ("aeProcessEvents", "src/event/ae.c"),
                    ("processCommand", "src/server.c"),
                ],
                uncited_requirement: "command_network_input",
            },
            HoldoutCase {
                id: "ripgrep-search-pipeline",
                question: "Explain how ripgrep parses CLI flags, walks candidate files, and executes a search over each haystack through matcher, searcher, and printer components. Cite the source files that support the path.",
                cited: &[("main", "crates/core/main.rs")],
                uncited_requirement: "search_dispatch",
            },
        ];

        for case in cases {
            let mut answer = answer_fixture(case.question);
            answer.citations = case
                .cited
                .iter()
                .map(|(name, path)| anchor_at(name, path))
                .collect();
            let claims = case
                .cited
                .iter()
                .map(|(name, path)| {
                    evidence_claim(
                        &format!("`{name}` participates in the traced path."),
                        anchor_at(name, path),
                    )
                })
                .collect::<Vec<_>>();

            let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/project"),
                question: case.question,
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                answer: &answer,
                budget: &budget_fixture(),
                supported_claims: claims,
                missing_required_probe_queries: Vec::new(),
                targeted_follow_up_queries: Vec::new(),
            });

            assert_eq!(
                sufficiency.status,
                PacketSufficiencyStatusDto::Partial,
                "holdout {} must refuse while {} is uncited: {sufficiency:?}",
                case.id,
                case.uncited_requirement
            );
            let report = sufficiency.coverage_report.as_ref().unwrap();
            assert!(
                report
                    .missing
                    .contains(&case.uncited_requirement.to_string()),
                "holdout {} should name {} as missing: {report:?}",
                case.id,
                case.uncited_requirement
            );
        }
    }

    #[test]
    fn holdout_axios_interceptor_evidence_closes_the_interceptor_requirement() {
        // The opposite direction of the axios holdout gate: the same packet with an interceptor
        // owner cited stops reporting that requirement missing, so the refusal above is caused by
        // the uncited component and not by an unclosable requirement.
        let question = "Explain how the default axios instance is created and how an HTTP request flows through interceptors, dispatchRequest, and the transport adapter. Cite the source files that support the path.";
        let mut interceptor = anchor_at("InterceptorManager", "lib/core/InterceptorManager.js");
        interceptor.kind = NodeKind::CLASS;
        let cited = [
            anchor_at("createInstance", "lib/axios.js"),
            anchor_at("dispatchRequest", "lib/core/dispatchRequest.js"),
            anchor_at("getAdapter", "lib/adapters/adapters.js"),
            interceptor,
        ];
        let mut answer = answer_fixture(question);
        answer.citations = cited.to_vec();
        let claims = cited
            .iter()
            .map(|citation| {
                evidence_claim(
                    &format!(
                        "`{}` participates in the traced path.",
                        citation.display_name
                    ),
                    citation.clone(),
                )
            })
            .collect::<Vec<_>>();

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget_fixture(),
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            !report
                .missing
                .contains(&"request_interceptor_management".to_string()),
            "a cited interceptor owner closes the interceptor requirement: {report:?}"
        );
    }

    #[test]
    fn retained_route_tracing_packet_reports_the_unproven_route_instead_of_sufficient() {
        // Retained shape of ask-1784386505488682000 (#1200): a route_tracing request whose packet
        // answered with generic router/application-factory prose, an unrelated task-class enum, and
        // an import-only `Context -> Context` graph, yet reported sufficient with no gaps and told
        // the caller not to open the very files the route runs through.
        //
        // Route order and avoid-opening already failed closed before this lane; what this pins in
        // addition is that each requested path is held to its own proof in a route_tracing packet,
        // and that the navigation claim over one of those paths is neither counted nor published.
        let question = "plugins/codestory/scripts/codestory-mcp.cjs -> crates/codestory-cli/src/stdio_transport.rs -> crates/codestory-runtime/src/agent/orchestrator.rs -> crates/codestory-retrieval/src/lib.rs";
        let route_paths = [
            "plugins/codestory/scripts/codestory-mcp.cjs",
            "crates/codestory-cli/src/stdio_transport.rs",
            "crates/codestory-runtime/src/agent/orchestrator.rs",
            "crates/codestory-retrieval/src/lib.rs",
        ];

        let router = anchor_at("create_router", "src/application/router.rs");
        let factory = anchor_at("create_app", "src/application/factory.rs");
        let mut task_enum = anchor_at("EditPlanning", "crates/codestory-contracts/src/api.rs");
        task_enum.kind = NodeKind::ENUM_CONSTANT;
        let mut import_node = anchor_at("Context", route_paths[2]);
        import_node.kind = NodeKind::STRUCT;

        let mut answer = answer_fixture(question);
        answer.answer_id = "ask-1784386505488682000".to_string();
        mark_full_retrieval_available(&mut answer);
        answer.citations = vec![
            router.clone(),
            factory.clone(),
            task_enum.clone(),
            import_node.clone(),
        ];
        answer.graphs = vec![route_graph(
            "import-neighborhood",
            &["Context"],
            &[("Context", "Context")],
        )];
        let claims = vec![
            evidence_claim(
                "`create_router` builds the application router for incoming requests.",
                router,
            ),
            evidence_claim(
                "`create_app` wires the application factory before requests are served.",
                factory,
            ),
            evidence_claim(
                "`EditPlanning` names the requested packet task class.",
                task_enum,
            ),
            cited_claim(
                "`Context` in `crates/codestory-runtime/src/agent/orchestrator.rs` ties context in this flow to cited definitions and adjacent ownership.",
                Some("source evidence"),
                import_node,
                Some(true),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency_with_probe_context(
            &MissingPathSpellingIdentity,
            PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/project"),
                question,
                task_class: PacketTaskClassDto::RouteTracing,
                answer: &answer,
                budget: &budget_fixture(),
                supported_claims: claims,
                missing_required_probe_queries: Vec::new(),
                targeted_follow_up_queries: Vec::new(),
            },
            &[],
            &route_paths.map(str::to_string),
            None,
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "generic router prose over an import-only graph cannot report a proven route: {sufficiency:?}"
        );
        assert!(
            sufficiency.gaps.iter().any(|gap| gap
                .contains("did not establish a proof-bearing claim from explicit exact path")),
            "route tracing must hold every requested path to its own proof, not only architecture: {sufficiency:?}"
        );
        let report = sufficiency
            .coverage_report
            .as_ref()
            .expect("retained route packet should carry a coverage report");
        assert!(
            report.ineligible.iter().any(|entry| entry
                .contains("generic navigation/source-evidence claim does not explain the flow")),
            "navigation prose over a requested file stays unproven: {report:?}"
        );
        assert!(
            sufficiency
                .covered_claims
                .iter()
                .all(|claim| !claim.claim.contains("adjacent ownership")),
            "a claim the same packet reports as unproven must not be published as covered: {sufficiency:?}"
        );
        for path in route_paths {
            assert!(
                report.missing.contains(&format!("exact path: {path}")),
                "coverage report should retain each unproven requested path: {report:?}"
            );
        }
        let exact_path_gaps = sufficiency
            .gaps
            .iter()
            .filter(|gap| gap.contains("explicit exact path"))
            .collect::<Vec<_>>();
        assert_eq!(
            exact_path_gaps.len(),
            route_paths.len(),
            "every unproven requested path needs a gap of its own: {sufficiency:?}"
        );
        for path in route_paths {
            assert_eq!(
                exact_path_gaps
                    .iter()
                    .filter(|gap| gap.contains(path))
                    .count(),
                1,
                "{path} needs exactly one path-specific gap: {sufficiency:?}"
            );
            assert!(
                sufficiency
                    .follow_up_commands
                    .iter()
                    .any(|command| command.contains(path)),
                "each unproven route path needs a targeted follow-up: {path} missing from {sufficiency:?}"
            );
            assert!(
                !sufficiency.avoid_opening_paths.contains(&path.to_string()),
                "an unproven route path must never be advertised as already covered: {sufficiency:?}"
            );
        }
    }

    #[test]
    fn a_flow_probe_survives_the_follow_up_cap_when_exact_paths_fill_it() {
        let question = "Explain how formatting arguments become type-erased format args and reach vformat or format_to output paths.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        // Enough unproven exact paths to fill the eight-command cap on their own. Leading with the
        // paths fixed one drop and created its mirror image: the flow probe the packet is actually
        // missing fell off the end instead. Both kinds have to survive.
        let exact_paths = [
            "src/one.rs",
            "src/two.rs",
            "src/three.rs",
            "src/four.rs",
            "src/five.rs",
            "src/six.rs",
            "src/seven.rs",
            "src/eight.rs",
            "src/nine.rs",
        ]
        .map(str::to_string);
        let claims = vec![
            evidence_claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
                anchor_at("basic_format_args", "include/fmt/base.h"),
            ),
            evidence_claim(
                "Runtime formatting writes formatted output through output iterator helpers.",
                anchor_at("vformat_to", "include/fmt/format.h"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency_with_exact_paths(
            PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/project"),
                question,
                task_class: PacketTaskClassDto::ArchitectureExplanation,
                answer: &answer,
                budget: &budget,
                supported_claims: claims,
                missing_required_probe_queries: vec!["format error".to_string()],
                targeted_follow_up_queries: Vec::new(),
            },
            &exact_paths,
        );

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--query 'format error'")),
            "the missing flow probe must survive the command cap even when unproven exact paths \
             could fill it: {:?}",
            sufficiency.follow_up_commands
        );
        assert!(
            sufficiency
                .follow_up_commands
                .iter()
                .any(|command| command.contains("src/one.rs")),
            "an unproven exact path must still lead the follow-up list: {:?}",
            sufficiency.follow_up_commands
        );
    }

    // -----------------------------------------------------------------------
    // Off-subject anchors must not close a requirement.
    //
    // The per-requirement carriers read the citation, which is the right shape, but a carrier that
    // matches an unanchored substring of the symbol name accepts anchors from anywhere in the
    // repository. These fixtures plant exactly that shape: a packet that genuinely proves some of
    // its flow, plus one anchor whose name merely *contains* another requirement's needle while
    // belonging to an unrelated subsystem. Each of these returned `Sufficient` before the carriers
    // were scoped.
    // -----------------------------------------------------------------------

    #[test]
    fn an_unrelated_error_type_does_not_close_the_runtime_formatting_error_requirement() {
        let question = "Explain how formatting arguments become type-erased format args and reach vformat or format_to output paths.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        // `CliParseError` is a command-line parser error in a different subsystem. Its name contains
        // "error", which is all the `format_errors` carrier used to ask for.
        let claims = vec![
            evidence_claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
                anchor_at("basic_format_args", "include/fmt/base.h"),
            ),
            evidence_claim(
                "Runtime formatting writes formatted output through output iterator helpers.",
                anchor_at("vformat_to", "include/fmt/format.h"),
            ),
            evidence_claim(
                "Command-line parsing reports malformed arguments to the caller.",
                anchor_at("CliParseError", "src/cli/parse.cc"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "an error type from an unrelated subsystem must not prove the formatting error path: \
             {sufficiency:?}"
        );
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.iter().any(|gap| gap == "format_errors"),
            "the formatting error requirement must still be named as missing: {report:?}"
        );
    }

    #[test]
    fn an_unrelated_use_prefixed_symbol_does_not_close_the_hook_export_requirement() {
        let question =
            "Explain how the data fetching hook serializes cache keys and applies mutations.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        // `userProfile` is a session model. It starts with "use", which is all the
        // `hook_public_export` carrier used to ask for.
        let claims = vec![
            evidence_claim(
                "Cache keys are serialized to a stable string before lookup.",
                anchor_at("serializeKey", "src/_internal/utils/serialize.ts"),
            ),
            evidence_claim(
                "A cache helper owns the shared store the hook reads through.",
                anchor_at("makeCacheHelper", "src/_internal/utils/helper.ts"),
            ),
            evidence_claim(
                "Mutations revalidate the cached entry after they apply.",
                anchor_at("applyMutation", "src/_internal/utils/mutate.ts"),
            ),
            evidence_claim(
                "The session model describes the signed-in user.",
                anchor_at("userProfile", "src/session/user.ts"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "a session model that merely starts with `use` must not prove the hook's public \
             export: {sufficiency:?}"
        );
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.iter().any(|gap| gap == "hook_public_export"),
            "the hook export requirement must still be named as missing: {report:?}"
        );
    }

    #[test]
    fn unrelated_javascript_symbols_do_not_close_the_form_validation_flow() {
        let question = "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        // Form-shaped prose over anchors that touch no form at all. This is the shape that evades:
        // the wording clears the claim-family floor, so nothing else holds the packet back, and the
        // anchors match only because their names *contain* a needle — "determineFieldOrder"
        // contains "min", "invalidateRecordCache" contains "validate", "submitTelemetry" contains
        // "submit". Together they closed the entire flow and the packet published as sufficient.
        let claims = vec![
            evidence_claim(
                "The form validation examples use native required, pattern, min, and max constraints.",
                anchor_at("determineFieldOrder", "src/layout.js"),
            ),
            evidence_claim(
                "A custom validation example applies script-driven validity checks before rendering messages.",
                anchor_at("invalidateRecordCache", "src/cache.js"),
            ),
            evidence_claim(
                "Submit handlers prevent submission when the form is invalid.",
                anchor_at("submitTelemetry", "src/telemetry.js"),
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: Vec::new(),
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Partial,
            "layout, cache, and telemetry symbols prove nothing about form validation: \
             {sufficiency:?}"
        );
        let report = sufficiency.coverage_report.as_ref().unwrap();
        for requirement in [
            "form_native_constraints",
            "form_custom_validation",
            "form_submit_guard",
        ] {
            assert!(
                report.missing.iter().any(|gap| gap == requirement),
                "{requirement} must still be named as missing: {report:?}"
            );
        }
    }
}

fn packet_has_sufficiency_blocking_budget_omission(
    budget: &PacketBudgetDto,
    missing_required_flow_requirements: &[FlowRequirement],
    missing_required_probe_queries: &[String],
) -> bool {
    if !budget.truncated {
        return false;
    }

    if budget
        .omitted_sections
        .iter()
        .any(|section| section == "packet_payload")
    {
        return true;
    }

    let missing_proof_probe = missing_required_probe_queries
        .iter()
        .any(|query| packet_missing_probe_requires_compact_proof(query));
    if missing_required_flow_requirements.is_empty() && !missing_proof_probe {
        return false;
    }

    budget.omitted_sections.iter().any(|section| {
        matches!(
            section.as_str(),
            "citations" | "markdown_blocks" | "trail_edges" | "output_bytes"
        )
    })
}

fn packet_missing_probe_requires_compact_proof(query: &str) -> bool {
    let normalized = normalize_identifier(query);
    matches!(
        normalized.as_str(),
        "sourcereadbuffer"
            | "sinkwritebuffer"
            | "requestresumedispatch"
            | "requestvalidationpipeline"
            | "delegatecallbackhandling"
            | "urlsessioncallbackboundary"
    ) || normalized.ends_with("requestvalidation")
}

pub fn packet_budget_exceeded_hard_output_cap(budget: &PacketBudgetDto) -> bool {
    budget.used.output_bytes > budget.limits.max_output_bytes
}

/// Build the follow-up contract as typed argv.
///
/// Argv is the product, not a rendering of one. Composing shell text first is
/// exactly what let a PowerShell-style quote doubling ship: on a POSIX shell it
/// silently deleted apostrophes from the suggested query. Callers that execute
/// a follow-up read `follow_up_argv`; `follow_up_commands` is the display
/// projection of the same argv through one correct quoter.
fn packet_follow_up_argv(
    project_root: &Path,
    question: &str,
    status: PacketSufficiencyStatusDto,
    budget: &PacketBudgetDto,
    missing_required_probe_queries: &[String],
    targeted_follow_up_queries: Vec<String>,
    full_retrieval_available: bool,
) -> Vec<Vec<String>> {
    let project = packet_display_project_arg(project_root);
    match status {
        PacketSufficiencyStatusDto::Sufficient => Vec::new(),
        PacketSufficiencyStatusDto::Partial => {
            let queries = if missing_required_probe_queries.is_empty() {
                targeted_follow_up_queries
            } else {
                missing_required_probe_queries.to_vec()
            };
            if !full_retrieval_available {
                let mut commands = vec![packet_retrieval_activation_argv(&project)];
                commands.extend(packet_follow_up_trail_argv(&project, &queries));
                commands.truncate(8);
                return commands;
            }
            let mut commands = packet_follow_up_search_argv(&project, &queries);
            commands.truncate(8);
            commands
                .into_iter()
                .chain(next_deeper_packet_argv(
                    project_root,
                    question,
                    budget.requested,
                ))
                .chain(std::iter::once(packet_search_argv(&project, question)))
                .collect()
        }
        PacketSufficiencyStatusDto::Insufficient => {
            if full_retrieval_available {
                vec![
                    packet_argv(&["index", "--project", project.as_str(), "--refresh", "full"]),
                    packet_search_argv(&project, question),
                ]
            } else {
                vec![
                    packet_retrieval_activation_argv(&project),
                    packet_argv(&["ground", "--project", project.as_str(), "--why"]),
                ]
            }
        }
    }
}

fn packet_search_argv(project: &str, query: &str) -> Vec<String> {
    packet_argv(&["search", "--project", project, "--query", query, "--why"])
}

fn packet_full_retrieval_available(answer: &AgentAnswerDto) -> bool {
    answer
        .retrieval_trace
        .retrieval_shadow
        .as_ref()
        .is_some_and(|shadow| shadow.retrieval_mode == "full")
}

fn packet_retrieval_activation_argv(project: &str) -> Vec<String> {
    packet_argv(&[
        "retrieval",
        "index",
        "--profile",
        "agent",
        "--refresh",
        "auto",
        "--project",
        project,
        "--format",
        "json",
    ])
}

fn packet_follow_up_trail_argv(project: &str, queries: &[String]) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    for query in queries {
        push_unique_argv(
            &mut commands,
            packet_argv(&[
                "trail",
                "--project",
                project,
                "--query",
                query,
                "--story",
                "--hide-speculative",
            ]),
        );
    }
    commands
}

/// Merge unproven exact paths with missing flow probes so the eight-command cap cannot silently
/// drop either kind. Taking one from each list in turn keeps a path in the lead — it is the most
/// specific thing a caller can act on — while guaranteeing the probes the packet is actually
/// missing are still represented once the list is truncated.
fn packet_interleave_follow_up_queries(paths: &[String], probes: &[String]) -> Vec<String> {
    let mut merged = Vec::new();
    let mut paths = paths.iter();
    let mut probes = probes.iter();
    loop {
        let path = paths.next();
        let probe = probes.next();
        if path.is_none() && probe.is_none() {
            break;
        }
        if let Some(path) = path {
            push_unique_sufficiency_term(&mut merged, path);
        }
        if let Some(probe) = probe {
            push_unique_sufficiency_term(&mut merged, probe);
        }
    }
    merged
}

fn packet_follow_up_search_argv(project: &str, queries: &[String]) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    for query in queries {
        push_unique_argv(&mut commands, packet_search_argv(project, query));
    }
    commands
}

#[cfg(test)]
fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
fn contains_all(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().all(|needle| haystack.contains(needle))
}

fn push_unique_argv(commands: &mut Vec<Vec<String>>, argv: Vec<String>) {
    if !commands.contains(&argv) {
        commands.push(argv);
    }
}

fn push_unique_sufficiency_term(terms: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    if !terms.iter().any(|existing| existing == value) {
        terms.push(value.to_string());
    }
}
