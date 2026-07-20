use crate::agent::packet_claims::{decorate_packet_claims_proof_metadata, packet_supported_claims};
use crate::agent::packet_evidence::citation_sufficiency_eligible;
use crate::agent::packet_evidence_roles::{
    PacketEvidenceRole, packet_citation_owns_interceptor_management, packet_evidence_role,
};
use crate::agent::packet_flow_requirements::{
    CoverageMode, FlowRequirement, FlowRole, packet_flow_requirements_for_terms,
};
use crate::agent::packet_plan::packet_symbol_probe_queries;
use crate::agent::packet_required_probes::packet_missing_sufficiency_probe_queries_with_extra;
use crate::agent::packet_scoring::{
    normalize_identifier, packet_display_name_is_test_like, packet_display_path,
};
use crate::agent::packet_terms::{
    packet_probe_terms, packet_terms_indicate_form_validation_flow,
    packet_terms_indicate_html_css_template_structure_flow,
    packet_terms_indicate_log_record_handler_flow,
    packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_runtime_formatting_flow,
    packet_terms_indicate_server_request_dispatch_flow,
    packet_terms_indicate_shell_install_dispatch_flow, packet_terms_indicate_site_build_phase_flow,
    packet_terms_indicate_sql_schema_flow, packet_terms_indicate_string_predicate_flow,
    packet_terms_indicate_stylesheet_animation_flow,
    packet_terms_indicate_url_session_request_flow,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, AgentRetrievalStepStatusDto, EdgeKind, GraphArtifactDto,
    GraphResponse, NodeKind, PacketBudgetDto, PacketBudgetModeDto, PacketClaimDto,
    PacketCoverageReportDto, PacketEvidenceResolutionDto, PacketEvidenceTierDto,
    PacketSidecarQueryDiagnosticDto, PacketSufficiencyDto, PacketSufficiencyStatusDto,
    PacketTaskClassDto,
};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::path::Path;

pub(crate) const PACKET_MARKDOWN_TRUNCATION_SUFFIX: &str =
    "\n\n... packet section truncated by budget ...\n";

pub(crate) struct PacketSufficiencyInput<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) question: &'a str,
    pub(crate) task_class: PacketTaskClassDto,
    pub(crate) answer: &'a AgentAnswerDto,
    pub(crate) budget: &'a PacketBudgetDto,
    pub(crate) supported_claims: Vec<PacketClaimDto>,
    pub(crate) missing_required_probe_queries: Vec<String>,
    pub(crate) targeted_follow_up_queries: Vec<String>,
}

#[cfg(test)]
pub(crate) fn build_packet_sufficiency(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
) -> PacketSufficiencyDto {
    build_packet_sufficiency_with_extra(project_root, question, task_class, answer, budget, &[])
}

#[cfg(test)]
pub(crate) fn build_packet_sufficiency_with_extra(
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

pub(crate) fn build_packet_sufficiency_with_probe_context(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    extra_probes: &[String],
    exact_probe_paths: &[String],
) -> PacketSufficiencyDto {
    let supported_claims = packet_supported_claims(answer);
    let missing_required_probe_queries = packet_missing_sufficiency_probe_queries_with_extra(
        question,
        task_class,
        answer,
        &supported_claims,
        extra_probes,
    );
    assemble_packet_sufficiency_with_probe_context(
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
    )
}

#[cfg(test)]
fn assemble_packet_sufficiency(input: PacketSufficiencyInput<'_>) -> PacketSufficiencyDto {
    assemble_packet_sufficiency_with_probe_context(input, &[], &[])
}

#[cfg(test)]
fn assemble_packet_sufficiency_with_route_probes(
    input: PacketSufficiencyInput<'_>,
    selected_probes: &[String],
) -> PacketSufficiencyDto {
    assemble_packet_sufficiency_with_probe_context(input, selected_probes, &[])
}

fn assemble_packet_sufficiency_with_probe_context(
    input: PacketSufficiencyInput<'_>,
    selected_probes: &[String],
    exact_probe_paths: &[String],
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

    let has_errors = answer
        .retrieval_trace
        .steps
        .iter()
        .any(|step| step.status == AgentRetrievalStepStatusDto::Error);
    let min_citations = packet_sufficiency_min_citations(task_class);
    let min_claims = packet_sufficiency_min_claims(task_class);
    let flow_context = PacketFlowContext::new(question, task_class);
    let route_stages = packet_route_proof_stages(question, selected_probes);
    let sufficiency_claims = supported_claims
        .iter()
        .filter(|claim| {
            packet_claim_can_satisfy_sufficiency_in_context(claim, &flow_context)
                || (task_class == PacketTaskClassDto::RouteTracing
                    && packet_route_claim_binds_stage(&route_stages, selected_probes, claim))
        })
        .cloned()
        .collect::<Vec<_>>();
    let generic_navigation_claim_count = supported_claims
        .iter()
        .filter(|claim| {
            packet_claim_is_generic_navigation_or_source_evidence(claim)
                && !flow_context.claim_carries_required_role(claim, false)
                && !packet_route_claim_binds_stage(&route_stages, selected_probes, claim)
        })
        .count();
    let has_minimum_coverage = answer.citations.len() >= min_citations;
    let has_minimum_claims = sufficiency_claims.len() >= min_claims;
    let claim_family_count = packet_supported_claim_family_count(&sufficiency_claims);
    let has_minimum_claim_families =
        packet_has_minimum_claim_family_coverage(task_class, &sufficiency_claims);
    let missing_exact_path_claims =
        packet_missing_exact_path_claims(task_class, exact_probe_paths, &sufficiency_claims);
    let route_proof = packet_route_proof_assessment(
        task_class,
        answer,
        &supported_claims,
        &route_stages,
        selected_probes,
    );
    let mut missing_required_flow_requirements =
        packet_missing_required_flow_requirements(question, task_class, &sufficiency_claims);
    if task_class == PacketTaskClassDto::RouteTracing && route_proof.complete {
        missing_required_flow_requirements.clear();
    }
    let has_required_flow_roles = missing_required_flow_requirements.is_empty();
    let blocking_missing_probe_queries = packet_blocking_missing_probe_queries(
        question,
        task_class,
        &missing_required_probe_queries,
        &missing_required_flow_requirements,
    );
    let has_sufficiency_blocking_budget_omission = packet_has_sufficiency_blocking_budget_omission(
        budget,
        &missing_required_flow_requirements,
        &missing_required_probe_queries,
    );
    let unresolved_sidecar_queries = unresolved_sidecar_queries(answer);
    let blocking_unresolved_sidecar_queries = packet_blocking_unresolved_sidecar_queries(
        question,
        task_class,
        &unresolved_sidecar_queries,
        &missing_required_probe_queries,
        &blocking_missing_probe_queries,
        &missing_required_flow_requirements,
    );
    let status = packet_sufficiency_status(PacketSufficiencyStatusInput {
        answer,
        budget,
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
    });

    let gaps = packet_sufficiency_gaps(
        task_class,
        answer,
        budget,
        min_citations,
        min_claims,
        sufficiency_claims.len(),
        claim_family_count,
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
    );
    let mut blocking_follow_up_probe_queries = packet_blocking_follow_up_probe_queries(
        &blocking_missing_probe_queries,
        &blocking_unresolved_sidecar_queries,
    );
    if blocking_follow_up_probe_queries.is_empty() {
        for query in &missing_required_probe_queries {
            push_unique_term(&mut blocking_follow_up_probe_queries, query);
        }
    }
    for query in &route_proof.follow_up_queries {
        push_unique_term(&mut blocking_follow_up_probe_queries, query);
    }
    for path in &missing_exact_path_claims {
        push_unique_term(&mut blocking_follow_up_probe_queries, path);
    }
    let follow_up_probe_queries = if blocking_follow_up_probe_queries.is_empty() {
        &missing_required_probe_queries
    } else {
        &blocking_follow_up_probe_queries
    };
    let follow_up_commands = packet_follow_up_commands(
        project_root,
        question,
        status,
        budget,
        follow_up_probe_queries,
        targeted_follow_up_queries,
        packet_full_retrieval_available(answer),
    );
    let coverage_report = packet_coverage_report(PacketCoverageReportInput {
        supported_claims: &supported_claims,
        sufficiency_claims: &sufficiency_claims,
        flow_context: &flow_context,
        missing_required_flow_requirements: &missing_required_flow_requirements,
        route_proof: &route_proof,
        missing_exact_path_claims: &missing_exact_path_claims,
        unresolved_sidecar_queries: &unresolved_sidecar_queries,
        budget,
        has_sufficiency_blocking_budget_omission,
    });
    let open_next = follow_up_commands.clone();
    let avoid_opening_paths = sufficiency_claims
        .iter()
        .flat_map(|claim| &claim.citations)
        .filter(|citation| citation_sufficiency_eligible(citation))
        .filter_map(|citation| citation.file_path.as_ref())
        .map(|path| packet_display_path(path))
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
        covered_claims: supported_claims,
        open_next,
        avoid_opening,
        avoid_opening_paths,
        gaps,
        follow_up_commands,
        coverage_report: Some(coverage_report),
    }
}

pub(crate) fn packet_targeted_follow_up_queries(
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

struct PacketSufficiencyStatusInput<'a> {
    answer: &'a AgentAnswerDto,
    budget: &'a PacketBudgetDto,
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
}

fn packet_sufficiency_status(
    input: PacketSufficiencyStatusInput<'_>,
) -> PacketSufficiencyStatusDto {
    if input.answer.citations.is_empty() {
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
        || packet_budget_exceeded_hard_output_cap(input.budget)
    {
        PacketSufficiencyStatusDto::Partial
    } else {
        PacketSufficiencyStatusDto::Sufficient
    }
}

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
        if from.is_some_and(|index| index != 0) {
            return Vec::new();
        }
        let route_words = from.map_or(words.as_slice(), |_| &words[1..]);
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
    if claim.eligible_for_sufficiency == Some(false) {
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
            crate::retrieval_file_role_from_path(path) == crate::RetrievalFileRole::Source
        })
        && !matches!(
            terminal.as_str(),
            "helper" | "helpers" | "util" | "utils" | "utility" | "generichelper"
        )
}

fn packet_execution_graphs(answer: &AgentAnswerDto) -> Vec<&GraphResponse> {
    answer
        .graphs
        .iter()
        .filter_map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => Some(graph),
            GraphArtifactDto::Mermaid { .. } => None,
        })
        .filter(|graph| {
            graph.edges.iter().any(|edge| {
                edge.kind == EdgeKind::CALL
                    && edge.source != edge.target
                    && !crate::graph_builders::is_speculative_trail_edge(edge)
            })
        })
        .collect()
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

fn packet_execution_path_exists(graph: &GraphResponse, source: &str, target: &str) -> bool {
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
                && !crate::graph_builders::is_speculative_trail_edge(edge)
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
    min_claims: usize,
    supported_claim_count: usize,
    claim_family_count: usize,
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
) -> Vec<String> {
    let mut gaps = Vec::new();
    if answer.citations.is_empty() {
        gaps.push("No cited anchors were found for the question.".to_string());
    }
    if !answer.citations.is_empty() && !has_minimum_coverage {
        gaps.push(format!(
            "{:?} packet found only {} cited anchor(s); at least {} are required before treating the packet as sufficient.",
            task_class,
            answer.citations.len(),
            min_citations
        ));
    }
    if !answer.citations.is_empty() && !has_minimum_claims {
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
    if !answer.citations.is_empty() && !has_minimum_claim_families {
        gaps.push(format!(
            "{:?} packet covered only {} distinct claim families; at least {} are required before treating the packet as sufficient.",
            task_class,
            claim_family_count,
            packet_sufficiency_min_claim_families(task_class)
        ));
    }
    if !answer.citations.is_empty() && !has_required_flow_roles {
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
    if !missing_exact_path_claims.is_empty() {
        gaps.push(format!(
            "ArchitectureExplanation packet did not establish a proof-bearing claim from explicit exact path(s): {}.",
            missing_exact_path_claims.join(", ")
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
    diagnostic
        .diagnostic
        .as_deref()
        .is_some_and(|message| message.starts_with("sidecar query has blocking cancel reason "))
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

fn packet_has_minimum_claim_family_coverage(
    task_class: PacketTaskClassDto,
    supported_claims: &[PacketClaimDto],
) -> bool {
    packet_supported_claim_family_count(supported_claims)
        >= packet_sufficiency_min_claim_families(task_class)
}

pub(crate) fn packet_supported_claim_family_count(supported_claims: &[PacketClaimDto]) -> usize {
    let mut families: HashSet<&'static str> = HashSet::new();
    for claim in supported_claims {
        if let Some(family) = packet_claim_family(claim) {
            families.insert(family);
        }
    }
    families.len()
}

pub(crate) fn packet_claim_family(claim: &PacketClaimDto) -> Option<&'static str> {
    let normalized_claim = normalize_identifier(&claim.claim);
    if !normalized_claim.is_empty() {
        if normalized_claim.contains("serialize") && normalized_claim.contains("key") {
            return Some("key serialization");
        }
        if normalized_claim.contains("cache")
            && contains_any(
                &normalized_claim,
                &["helper", "state", "snapshot", "subscribe", "getset"],
            )
        {
            return Some("cache state");
        }
        if contains_any(&normalized_claim, &["mutation", "mutate", "internalmutate"]) {
            return Some("mutation flow");
        }
        if normalized_claim.contains("isblank")
            || (normalized_claim.contains("blank") && normalized_claim.contains("whitespace"))
        {
            return Some("predicate blank behavior");
        }
        if normalized_claim.contains("isempty")
            || (normalized_claim.contains("empty") && normalized_claim.contains("trim"))
        {
            return Some("predicate empty behavior");
        }
        if normalized_claim.contains("regionmatches")
            || (normalized_claim.contains("region") && normalized_claim.contains("delegate"))
            || normalized_claim.contains("casesensitive")
            || normalized_claim.contains("ignorecase")
        {
            return Some("predicate region/case flow");
        }
        if contains_any(
            &normalized_claim,
            &[
                "blank",
                "empty",
                "casesensitive",
                "ignorecase",
                "region",
                "regionmatches",
                "whitespace",
                "trim",
            ],
        ) && contains_any(
            &normalized_claim,
            &[
                "treats",
                "tests",
                "doesnot",
                "deciding",
                "return",
                "compares",
                "delegates",
            ],
        ) {
            return Some("predicate behavior");
        }
        if normalized_claim.contains("csscustomproperties")
            || (normalized_claim.contains("animationduration")
                && normalized_claim.contains("delay")
                && normalized_claim.contains("repeat"))
        {
            return Some("css variables");
        }
        if normalized_claim.contains("baseclass")
            || normalized_claim.contains("animationfillmode")
            || normalized_claim.contains("animatedisthebase")
        {
            return Some("css base class");
        }
        if normalized_claim.contains("keyframes") || normalized_claim.contains("animationname") {
            return Some("css keyframes");
        }
        if normalized_claim.contains("imports")
            && (normalized_claim.contains("animationfiles")
                || normalized_claim.contains("variablebase"))
        {
            return Some("css imports");
        }
        if normalized_claim.contains("appshell") && normalized_claim.contains("divapp") {
            return Some("html app shell");
        }
        if normalized_claim.contains("roottypography")
            || normalized_claim.contains("colorscheme")
            || normalized_claim.contains("bodylayout")
        {
            return Some("css template defaults");
        }
        if normalized_claim.contains("appconstrains")
            || normalized_claim.contains("appcontainer")
            || (normalized_claim.contains("mountedapplication")
                && normalized_claim.contains("padding"))
            || (normalized_claim.contains("mountedcontent") && normalized_claim.contains("padding"))
        {
            return Some("css app layout");
        }
        if (normalized_claim.contains("logo") && normalized_claim.contains("button")
            || normalized_claim.contains("interactionselectors"))
            && contains_any(&normalized_claim, &["hover", "focus", "transition"])
        {
            return Some("css interaction styles");
        }
        if normalized_claim.contains("preferscolorschemelight")
            || normalized_claim.contains("mediaquery")
        {
            return Some("css light theme");
        }
        if contains_all(&normalized_claim, &["wsgi", "app"])
            && normalized_claim.contains("entrypoint")
        {
            return Some("server request entrypoint");
        }
        if contains_all(&normalized_claim, &["full", "dispatch", "request"])
            && contains_any(&normalized_claim, &["finalization", "finalize"])
            && contains_any(&normalized_claim, &["preprocess", "exception", "wrap"])
        {
            return Some("server request dispatch wrapper");
        }
        if contains_all(
            &normalized_claim,
            &["dispatch", "request", "view", "function"],
        ) && !normalized_claim.contains("full")
        {
            return Some("server view dispatch");
        }
        if (normalized_claim.contains("routedecorator")
            && normalized_claim.contains("registersviewfunctions"))
            || (normalized_claim.contains("routeregistrationdecorator")
                && normalized_claim.contains("urlrules"))
        {
            return Some("server route registration");
        }
        if normalized_claim.contains("sqlschema")
            && (normalized_claim.contains("definestables")
                || normalized_claim.contains("tables")
                || normalized_claim.contains("createtable"))
        {
            return Some("sql table definitions");
        }
        if normalized_claim.contains("rowsreference")
            || normalized_claim.contains("foreignkey")
            || (normalized_claim.contains("reference") && normalized_claim.contains("rows"))
        {
            return Some("sql relationships");
        }
        if normalized_claim.contains("sqldialect")
            || normalized_claim.contains("schemascripts")
            || normalized_claim.contains("dialectscripts")
        {
            return Some("sql dialect scripts");
        }
        if (normalized_claim.contains("typeerased")
            && (normalized_claim.contains("formatargs")
                || normalized_claim.contains("formatarguments")
                || normalized_claim.contains("formattingarguments")
                || normalized_claim.contains("arguments")))
            || (normalized_claim.contains("runtimeformatting")
                && normalized_claim.contains("centralruntimeargumentpath"))
        {
            return Some("runtime format arguments");
        }
        if (normalized_claim.contains("formatto")
            || normalized_claim.contains("outputiterator")
            || normalized_claim.contains("formattedoutputhelpers"))
            && (normalized_claim.contains("outputiterator")
                || normalized_claim.contains("formattedoutput")
                || normalized_claim.contains("output"))
        {
            return Some("runtime format output");
        }
        if normalized_claim.contains("formaterror")
            || normalized_claim.contains("formattingfailures")
            || normalized_claim.contains("systemerrors")
        {
            return Some("runtime format errors");
        }
        if normalized_claim.contains("buffer")
            && normalized_claim.contains("append")
            && normalized_claim.contains("formattedoutput")
        {
            return Some("runtime format buffer");
        }
        if (normalized_claim.contains("native")
            || normalized_claim.contains("constraint")
            || normalized_claim.contains("constraints")
            || normalized_claim.contains("formvalidationexamples"))
            && contains_any(&normalized_claim, &["required", "pattern", "min", "max"])
        {
            return Some("form native constraints");
        }
        if normalized_claim.contains("custom")
            && normalized_claim.contains("validation")
            && contains_any(&normalized_claim, &["browser", "defaultui", "ui"])
        {
            return Some("form custom validation ui");
        }
        if normalized_claim.contains("submit")
            && contains_any(
                &normalized_claim,
                &["prevent", "prevents", "submission", "invalid"],
            )
        {
            return Some("form submit guard");
        }
        if normalized_claim.contains("validitystate")
            || (normalized_claim.contains("validity")
                && contains_any(
                    &normalized_claim,
                    &[
                        "valuemissing",
                        "typemismatch",
                        "tooshort",
                        "message",
                        "messages",
                    ],
                ))
        {
            return Some("form validity messages");
        }
        if normalized_claim.contains("public")
            && contains_any(
                &normalized_claim,
                &["api", "export", "entrypoint", "hook", "method"],
            )
        {
            return Some("public api/export");
        }
        if (normalized_claim.contains("toplevelhttphelper")
            || normalized_claim.contains("toplevelhttphelpers")
            || normalized_claim.contains("delegate")
                && normalized_claim.contains("client")
                && normalized_claim.contains("helper"))
            && normalized_claim.contains("client")
        {
            return Some("client public facade");
        }
        if normalized_claim.contains("conveniencemethod")
            || normalized_claim.contains("conveniencemethods")
        {
            return Some("client convenience methods");
        }
        if normalized_claim.contains("finalize")
            && contains_any(&normalized_claim, &["request", "body", "sending"])
        {
            return Some("client request finalization");
        }
        if normalized_claim.contains("transportimplementation")
            || (normalized_claim.contains("send")
                && contains_any(&normalized_claim, &["transport", "httpclient", "adapter"]))
        {
            return Some("client transport send");
        }
        if normalized_claim.contains("responsefromstream")
            || normalized_claim.contains("responsematerialization")
            || normalized_claim.contains("responsestream")
        {
            return Some("client response materialization");
        }
        if normalized_claim.contains("server")
            && contains_any(&normalized_claim, &["bootstrap", "initializes", "main"])
        {
            return Some("command server bootstrap");
        }
        if normalized_claim.contains("eventloop")
            || (normalized_claim.contains("event") && normalized_claim.contains("loop"))
        {
            return Some("command event loop");
        }
        if normalized_claim.contains("socketinput")
            || normalized_claim.contains("networkcommandinput")
            || (normalized_claim.contains("network")
                && normalized_claim.contains("input")
                && normalized_claim.contains("command"))
        {
            return Some("command network input");
        }
        if normalized_claim.contains("commandtable")
            || normalized_claim.contains("commanddispatch")
            || (normalized_claim.contains("command")
                && contains_any(&normalized_claim, &["dispatch", "proc", "slowlog"]))
        {
            return Some("command dispatch");
        }
        if normalized_claim.contains("handlerstack")
            || normalized_claim.contains("pushhandler")
            || (normalized_claim.contains("handler")
                && contains_any(&normalized_claim, &["registration", "registered"])
                && contains_any(&normalized_claim, &["log", "logger", "record"]))
        {
            return Some("logger handler stack");
        }
        if normalized_claim.contains("addrecord")
            || (normalized_claim.contains("log")
                && normalized_claim.contains("record")
                && contains_any(&normalized_claim, &["creates", "creation"]))
            || (normalized_claim.contains("record")
                && normalized_claim.contains("creates")
                && normalized_claim.contains("handlers"))
        {
            return Some("log record creation");
        }
        if normalized_claim.contains("handlerinterface")
            && contains_any(
                &normalized_claim,
                &["handlebatch", "handlingboundaries", "contract"],
            )
        {
            return Some("handler interface contract");
        }
        if normalized_claim.contains("processing")
            && normalized_claim.contains("handler")
            && normalized_claim.contains("records")
            && contains_any(&normalized_claim, &["processing", "writing", "write"])
        {
            return Some("handler processing");
        }
        if contains_any(
            &normalized_claim,
            &[
                "delegates",
                "delegate",
                "handoff",
                "wraps",
                "invokes",
                "callsinto",
            ],
        ) {
            return Some("delegation/handoff");
        }
    }

    claim
        .citations
        .iter()
        .find_map(|citation| packet_evidence_role(citation).map(|role| role.as_str()))
        .or_else(|| (!claim.citations.is_empty()).then_some("source evidence"))
}

#[cfg(test)]
pub(crate) fn packet_claim_can_satisfy_sufficiency(claim: &PacketClaimDto) -> bool {
    packet_claim_ineligibility_reason(claim, false, false).is_none()
}

fn packet_claim_can_satisfy_sufficiency_in_context(
    claim: &PacketClaimDto,
    flow_context: &PacketFlowContext,
) -> bool {
    let generic_navigation = packet_claim_is_generic_navigation_or_source_evidence(claim);
    let carries_required_role =
        flow_context.claim_carries_required_role(claim, !generic_navigation);
    let structural_policy_admitted = flow_context.claim_has_structural_policy_admission(claim);
    packet_claim_ineligibility_reason(claim, carries_required_role, structural_policy_admitted)
        .is_none()
}

fn packet_claim_ineligibility_reason(
    claim: &PacketClaimDto,
    carries_required_role: bool,
    structural_policy_admitted: bool,
) -> Option<&'static str> {
    let generic_navigation = packet_claim_is_generic_navigation_or_source_evidence(claim);
    if claim.eligible_for_sufficiency == Some(false) && !structural_policy_admitted {
        return Some("claim marked diagnostic");
    }
    if !claim.citations.is_empty()
        && !claim.citations.iter().any(citation_sufficiency_eligible)
        && !structural_policy_admitted
    {
        return Some("citation evidence is diagnostic-only");
    }
    if generic_navigation && !carries_required_role {
        return Some("generic navigation/source-evidence claim lacks required coverage role");
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

fn packet_missing_exact_path_claims(
    task_class: PacketTaskClassDto,
    exact_probe_paths: &[String],
    sufficiency_claims: &[PacketClaimDto],
) -> Vec<String> {
    if task_class != PacketTaskClassDto::ArchitectureExplanation {
        return Vec::new();
    }

    exact_probe_paths
        .iter()
        .filter(|path| {
            !sufficiency_claims.iter().any(|claim| {
                claim.citations.iter().any(|citation| {
                    citation_sufficiency_eligible(citation)
                        && citation.file_path.as_deref().is_some_and(|citation_path| {
                            packet_paths_match_exact_probe(citation_path, path)
                        })
                })
            })
        })
        .map(|path| packet_display_path(path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn packet_paths_match_exact_probe(citation_path: &str, exact_probe_path: &str) -> bool {
    let citation_path = citation_path
        .trim_start_matches("\\\\?\\")
        .replace('\\', "/");
    let exact_probe_path = exact_probe_path
        .trim_start_matches("\\\\?\\")
        .replace('\\', "/");
    citation_path == exact_probe_path
        || citation_path.ends_with(&format!("/{exact_probe_path}"))
        || exact_probe_path.ends_with(&format!("/{citation_path}"))
}

fn packet_role_label_is_generic_source_evidence(role: &str) -> bool {
    normalize_identifier(role) == "sourceevidence"
}

struct PacketCoverageReportInput<'a> {
    supported_claims: &'a [PacketClaimDto],
    sufficiency_claims: &'a [PacketClaimDto],
    flow_context: &'a PacketFlowContext,
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
        sufficiency_claims,
        flow_context,
        missing_required_flow_requirements,
        route_proof,
        missing_exact_path_claims,
        unresolved_sidecar_queries,
        budget,
        has_sufficiency_blocking_budget_omission,
    } = input;
    let covered = sufficiency_claims
        .iter()
        .filter_map(packet_claim_coverage_label)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let ineligible = supported_claims
        .iter()
        .filter_map(|claim| {
            let generic_navigation = packet_claim_is_generic_navigation_or_source_evidence(claim);
            let carries_required_role =
                flow_context.claim_carries_required_role(claim, !generic_navigation);
            let structural_policy_admitted =
                flow_context.claim_has_structural_policy_admission(claim);
            packet_claim_ineligibility_reason(
                claim,
                carries_required_role,
                structural_policy_admitted,
            )
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
        push_unique_term(&mut missing, route_gap);
    }
    for path in missing_exact_path_claims {
        push_unique_term(&mut missing, &format!("exact path: {path}"));
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
    matches!(label, "precise_semantic_import")
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

struct PacketFlowContext {
    requirements: Vec<FlowRequirement>,
    required_roles: Vec<FlowRole>,
    site_build_flow: bool,
    mapper_flow: bool,
    shell_install_dispatch_flow: bool,
    url_session_request_flow: bool,
    form_validation_flow: bool,
    server_request_dispatch_flow: bool,
    html_css_template_structure_flow: bool,
    stylesheet_animation_flow: bool,
    sql_schema_flow: bool,
    runtime_formatting_flow: bool,
    string_predicate_flow: bool,
    log_record_handler_flow: bool,
}

impl PacketFlowContext {
    fn new(question: &str, task_class: PacketTaskClassDto) -> Self {
        let question_terms = packet_probe_terms(question);
        let requirements = packet_flow_requirements_for_terms(&question_terms, task_class);
        Self {
            requirements: requirements.clone(),
            required_roles: packet_required_flow_roles(&requirements),
            site_build_flow: packet_terms_indicate_site_build_phase_flow(&question_terms),
            mapper_flow: packet_terms_indicate_mapper_configuration_plan_flow(&question_terms),
            shell_install_dispatch_flow: packet_terms_indicate_shell_install_dispatch_flow(
                &question_terms,
            ),
            url_session_request_flow: packet_terms_indicate_url_session_request_flow(
                &question_terms,
            ),
            form_validation_flow: packet_terms_indicate_form_validation_flow(&question_terms),
            server_request_dispatch_flow: packet_terms_indicate_server_request_dispatch_flow(
                &question_terms,
            ),
            html_css_template_structure_flow:
                packet_terms_indicate_html_css_template_structure_flow(&question_terms),
            stylesheet_animation_flow: packet_terms_indicate_stylesheet_animation_flow(
                &question_terms,
            ),
            sql_schema_flow: packet_terms_indicate_sql_schema_flow(&question_terms),
            runtime_formatting_flow: packet_terms_indicate_runtime_formatting_flow(&question_terms),
            string_predicate_flow: packet_terms_indicate_string_predicate_flow(&question_terms),
            log_record_handler_flow: packet_terms_indicate_log_record_handler_flow(&question_terms),
        }
    }

    fn claim_carries_required_role(
        &self,
        claim: &PacketClaimDto,
        include_generic_fallback_roles: bool,
    ) -> bool {
        if self.required_roles.is_empty() {
            return false;
        }
        self.requirements.iter().any(|requirement| {
            self.claim_satisfies_requirement(claim, requirement, include_generic_fallback_roles)
        })
    }

    fn claim_satisfies_requirement(
        &self,
        claim: &PacketClaimDto,
        requirement: &FlowRequirement,
        include_generic_fallback_roles: bool,
    ) -> bool {
        if flow_requirement_is_log_record_handler(requirement)
            && packet_claim_is_generic_navigation_or_source_evidence(claim)
        {
            return false;
        }
        let structural_match =
            StructuralLanguagePolicy::claim_satisfies_requirement(requirement, claim);
        if structural_match || StructuralLanguagePolicy::requires_cited_role(requirement) {
            return structural_match;
        }
        if StructuralLanguagePolicy::requires_specific_proof(requirement) {
            return self.claim_declares_exact_requirement_id(claim, requirement);
        }
        if self.claim_declares_requirement_role(claim, requirement) {
            return true;
        }
        let claim_roles = packet_flow_roles_for_claim(
            claim,
            self.site_build_flow,
            self.mapper_flow,
            self.shell_install_dispatch_flow,
            self.url_session_request_flow,
            self.form_validation_flow,
            self.server_request_dispatch_flow,
            self.html_css_template_structure_flow,
            self.stylesheet_animation_flow,
            self.sql_schema_flow,
            self.runtime_formatting_flow,
            self.string_predicate_flow,
            self.log_record_handler_flow,
            include_generic_fallback_roles,
        );
        claim_roles.contains(&requirement.role)
    }

    fn claim_has_structural_policy_admission(&self, claim: &PacketClaimDto) -> bool {
        self.requirements.iter().any(|requirement| {
            StructuralLanguagePolicy::admits_diagnostic_evidence(requirement, claim)
        })
    }

    fn claim_declares_requirement_role(
        &self,
        claim: &PacketClaimDto,
        requirement: &FlowRequirement,
    ) -> bool {
        let Some(role_label) = claim.coverage_role.as_deref() else {
            return false;
        };
        let normalized = normalize_identifier(role_label);
        normalized == normalize_identifier(requirement.role_id())
            || normalized == normalize_identifier(requirement.role.label())
    }

    fn claim_declares_exact_requirement_id(
        &self,
        claim: &PacketClaimDto,
        requirement: &FlowRequirement,
    ) -> bool {
        claim.coverage_role.as_deref().is_some_and(|role_label| {
            normalize_identifier(role_label) == normalize_identifier(requirement.id)
        })
    }
}

struct StructuralLanguagePolicy;

impl StructuralLanguagePolicy {
    fn requires_cited_role(requirement: &FlowRequirement) -> bool {
        requirement.id == "request_interceptor_management"
    }

    fn requires_specific_proof(requirement: &FlowRequirement) -> bool {
        matches!(
            requirement.id,
            "sql_tables"
                | "sql_relationships"
                | "form_native_constraints"
                | "form_custom_validation"
                | "form_submit_guard"
                | "client_public_facade"
                | "client_interface_helpers"
                | "client_request_finalization"
                | "client_transport_send"
                | "client_response_materialization"
                | "hook_public_export"
                | "hook_key_serialization"
                | "hook_cache_helper"
                | "hook_mutation_flow"
                | "command_server_bootstrap"
                | "command_event_loop"
                | "command_network_input"
                | "command_dispatch"
                | "logger_event"
                | "handler_processing"
                | "css_animation_entrypoint"
                | "css_animation_structure"
        )
    }

    fn claim_satisfies_requirement(requirement: &FlowRequirement, claim: &PacketClaimDto) -> bool {
        let normalized = normalize_identifier(&claim.claim);
        match requirement.id {
            "request_interceptor_management" => {
                normalize_identifier(claim.coverage_role.as_deref().unwrap_or_default())
                    == "interceptormanagement"
                    && claim
                        .citations
                        .iter()
                        .any(packet_citation_owns_interceptor_management)
            }
            "sql_tables" => claim.citations.iter().any(Self::citation_is_sql_table),
            "sql_relationships" => claim
                .citations
                .iter()
                .any(Self::citation_is_sql_relationship),
            "form_native_constraints" => Self::claim_text_names_native_constraints(&normalized),
            "form_custom_validation" => Self::claim_text_names_custom_validation(&normalized),
            "form_submit_guard" => Self::claim_text_names_submit_guard(&normalized),
            "client_public_facade" => Self::claim_text_names_client_public_facade(&normalized),
            "client_interface_helpers" => {
                Self::claim_text_names_client_interface_helpers(&normalized)
            }
            "client_request_finalization" => {
                Self::claim_text_names_client_request_finalization(&normalized)
            }
            "client_transport_send" => Self::claim_text_names_client_transport_send(&normalized),
            "client_response_materialization" => {
                Self::claim_text_names_client_response_materialization(&normalized)
            }
            "hook_public_export" => Self::claim_text_names_hook_public_export(&normalized),
            "hook_key_serialization" => Self::claim_text_names_hook_key_serialization(&normalized),
            "hook_cache_helper" => Self::claim_text_names_hook_cache_helper(&normalized),
            "hook_mutation_flow" => Self::claim_text_names_hook_mutation_flow(&normalized),
            "command_server_bootstrap" => {
                Self::claim_text_names_command_server_bootstrap(&normalized)
                    || claim.citations.iter().any(|citation| {
                        packet_evidence_role(citation)
                            == Some(PacketEvidenceRole::CommandEntrypoint)
                    })
            }
            "command_event_loop" => {
                Self::claim_text_names_command_event_loop(&normalized)
                    || claim.citations.iter().any(|citation| {
                        packet_evidence_role(citation) == Some(PacketEvidenceRole::EventLoop)
                    })
            }
            "command_network_input" => {
                Self::claim_text_names_command_network_input(&normalized)
                    || claim.citations.iter().any(|citation| {
                        packet_evidence_role(citation)
                            == Some(PacketEvidenceRole::NetworkCommandInput)
                    })
            }
            "command_dispatch" => {
                Self::claim_text_names_command_dispatch(&normalized)
                    || claim.citations.iter().any(|citation| {
                        packet_evidence_role(citation) == Some(PacketEvidenceRole::CommandDispatch)
                    })
            }
            "logger_event" => Self::claim_text_names_log_record_creation(&normalized),
            "handler_processing" => Self::claim_text_names_log_handler_processing(&normalized),
            "css_animation_entrypoint" => {
                normalized.contains("animationstylesheetentrypoint")
                    || (normalized.contains("imports") && normalized.contains("animationfiles"))
            }
            "css_animation_structure" => {
                normalized.contains("baseclass")
                    || normalized.contains("animationname")
                    || normalized.contains("matchingkeyframes")
                    || normalized.contains("customproperties")
                    || normalized.contains("duration")
                    || normalized.contains("delay")
                    || normalized.contains("repeat")
                    || normalized.contains("keyframes")
            }
            _ => false,
        }
    }

    fn admits_diagnostic_evidence(requirement: &FlowRequirement, claim: &PacketClaimDto) -> bool {
        matches!(requirement.id, "sql_tables" | "sql_relationships")
            && claim.citations.iter().any(|citation| {
                Self::citation_is_sql_source_scan(citation)
                    && match requirement.id {
                        "sql_tables" => Self::citation_is_sql_table(citation),
                        "sql_relationships" => Self::citation_is_sql_relationship(citation),
                        _ => false,
                    }
            })
    }

    fn citation_is_sql_source_scan(citation: &AgentCitationDto) -> bool {
        citation.evidence_tier == Some(PacketEvidenceTierDto::SyntheticSourceScan)
            && citation.resolution_status == Some(PacketEvidenceResolutionDto::SourceRangeOnly)
    }

    fn citation_is_sql_table(citation: &AgentCitationDto) -> bool {
        packet_evidence_role(citation) == Some(PacketEvidenceRole::SqlTableDefinition)
    }

    fn citation_is_sql_relationship(citation: &AgentCitationDto) -> bool {
        packet_evidence_role(citation) == Some(PacketEvidenceRole::SqlRelationshipConstraint)
    }

    fn claim_text_names_native_constraints(normalized: &str) -> bool {
        (normalized.contains("native")
            || normalized.contains("constraint")
            || normalized.contains("constraints")
            || normalized.contains("formvalidationexamples"))
            && contains_any(normalized, &["required", "pattern", "min", "max"])
    }

    fn claim_text_names_custom_validation(normalized: &str) -> bool {
        normalized.contains("custom")
            && contains_any(
                normalized,
                &[
                    "validation",
                    "validity",
                    "validitystate",
                    "error",
                    "errors",
                    "message",
                    "messages",
                    "browser",
                    "defaultui",
                    "ui",
                ],
            )
    }

    fn claim_text_names_submit_guard(normalized: &str) -> bool {
        normalized.contains("submit")
            && contains_any(
                normalized,
                &["prevent", "prevents", "submission", "invalid"],
            )
    }

    fn claim_text_names_client_public_facade(normalized: &str) -> bool {
        (normalized.contains("toplevelhttphelper")
            || normalized.contains("toplevelhttphelpers")
            || normalized.contains("publicfacade"))
            && normalized.contains("client")
    }

    fn claim_text_names_client_interface_helpers(normalized: &str) -> bool {
        normalized.contains("conveniencemethod")
            || normalized.contains("conveniencemethods")
            || normalized.contains("clientinterfacehelper")
    }

    fn claim_text_names_client_request_finalization(normalized: &str) -> bool {
        normalized.contains("finalize")
            && normalized.contains("request")
            && contains_any(
                normalized,
                &["body", "sending", "transportready", "prepare"],
            )
    }

    fn claim_text_names_client_transport_send(normalized: &str) -> bool {
        normalized.contains("send")
            && contains_any(
                normalized,
                &["transport", "httpclient", "adapter", "dartio"],
            )
    }

    fn claim_text_names_client_response_materialization(normalized: &str) -> bool {
        normalized.contains("responsefromstream")
            || normalized.contains("responsematerialization")
            || normalized.contains("responsestream")
            || (normalized.contains("response") && normalized.contains("streamboundary"))
    }

    fn claim_text_names_hook_public_export(normalized: &str) -> bool {
        normalized.contains("public")
            && normalized.contains("export")
            && normalized.contains("wraps")
            && contains_any(normalized, &["hook", "argumentnormalization", "handler"])
    }

    fn claim_text_names_hook_key_serialization(normalized: &str) -> bool {
        normalized.contains("serialize")
            && contains_any(normalized, &["key", "keys", "cachekey", "cachekeys"])
    }

    fn claim_text_names_hook_cache_helper(normalized: &str) -> bool {
        normalized.contains("cache")
            && normalized.contains("helper")
            && contains_any(
                normalized,
                &["get", "set", "subscribe", "snapshot", "state"],
            )
    }

    fn claim_text_names_hook_mutation_flow(normalized: &str) -> bool {
        contains_any(normalized, &["mutate", "mutation", "internalmutate"])
            && contains_any(normalized, &["helper", "routes", "flows", "dispatch"])
    }

    fn claim_text_names_command_server_bootstrap(normalized: &str) -> bool {
        normalized.contains("server")
            && contains_any(normalized, &["bootstrap", "initializes", "main"])
    }

    fn claim_text_names_command_event_loop(normalized: &str) -> bool {
        normalized.contains("eventloop")
            || (normalized.contains("event") && normalized.contains("loop"))
    }

    fn claim_text_names_command_network_input(normalized: &str) -> bool {
        normalized.contains("socketinput")
            || normalized.contains("networkcommandinput")
            || (normalized.contains("network")
                && normalized.contains("input")
                && normalized.contains("command"))
    }

    fn claim_text_names_command_dispatch(normalized: &str) -> bool {
        normalized.contains("commandtable")
            || normalized.contains("commanddispatch")
            || (normalized.contains("command")
                && contains_any(normalized, &["dispatch", "proc", "slowlog"]))
    }

    fn claim_text_names_log_record_creation(normalized: &str) -> bool {
        normalized.contains("addrecord")
            || normalized.contains("recordcreation")
            || (normalized.contains("log")
                && normalized.contains("record")
                && contains_any(normalized, &["create", "creates", "created", "creation"]))
            || (normalized.contains("record")
                && contains_any(normalized, &["create", "creates", "created", "creation"])
                && normalized.contains("handler"))
    }

    fn claim_text_names_log_handler_processing(normalized: &str) -> bool {
        normalized.contains("handler")
            && normalized.contains("record")
            && ((contains_any(normalized, &["process", "processing", "processed"])
                && contains_any(
                    normalized,
                    &[
                        "handle", "handles", "handling", "write", "writes", "writing",
                    ],
                ))
                || (normalized.contains("batch")
                    && normalized.contains("boundar")
                    && contains_any(
                        normalized,
                        &[
                            "execute",
                            "executes",
                            "execution",
                            "handle",
                            "handles",
                            "processing",
                            "write",
                            "writing",
                        ],
                    )))
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

fn flow_requirement_is_log_record_handler(requirement: &FlowRequirement) -> bool {
    matches!(requirement.id, "logger_event" | "handler_processing")
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
                .any(|claim| flow_context.claim_satisfies_requirement(claim, requirement, true))
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

fn packet_required_flow_roles(requirements: &[FlowRequirement]) -> Vec<FlowRole> {
    let mut required = Vec::new();
    for requirement in requirements
        .iter()
        .filter(|requirement| flow_requirement_blocks_sufficiency(requirement))
    {
        if !required
            .iter()
            .any(|role: &FlowRole| role.role_id() == requirement.role_id())
        {
            required.push(requirement.role);
        }
    }
    required
}

fn flow_requirement_blocks_sufficiency(requirement: &FlowRequirement) -> bool {
    !matches!(requirement.coverage_mode, CoverageMode::DiagnosticOnly)
}

fn packet_blocking_missing_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
    missing_required_probe_queries: &[String],
    missing_required_flow_requirements: &[FlowRequirement],
) -> Vec<String> {
    if missing_required_probe_queries.is_empty() || missing_required_flow_requirements.is_empty() {
        return Vec::new();
    }

    let missing_requirement_ids = missing_required_flow_requirements
        .iter()
        .map(|requirement| requirement.id)
        .collect::<HashSet<_>>();
    let question_terms = packet_probe_terms(question);
    let blocking_query_seeds = packet_flow_requirements_for_terms(&question_terms, task_class)
        .into_iter()
        .filter(|requirement| {
            flow_requirement_blocks_sufficiency(requirement)
                && missing_requirement_ids.contains(requirement.id)
        })
        .flat_map(|requirement| requirement.query_seeds.iter().copied())
        .collect::<HashSet<_>>();

    missing_required_probe_queries
        .iter()
        .filter(|query| blocking_query_seeds.contains(query.as_str()))
        .cloned()
        .collect()
}

fn packet_blocking_unresolved_sidecar_queries(
    question: &str,
    task_class: PacketTaskClassDto,
    unresolved_sidecar_queries: &[String],
    missing_required_probe_queries: &[String],
    blocking_missing_probe_queries: &[String],
    missing_required_flow_requirements: &[FlowRequirement],
) -> Vec<String> {
    if unresolved_sidecar_queries.is_empty()
        || (missing_required_probe_queries.is_empty()
            && missing_required_flow_requirements.is_empty())
    {
        return Vec::new();
    }

    let missing_requirement_ids = missing_required_flow_requirements
        .iter()
        .map(|requirement| requirement.id)
        .collect::<HashSet<_>>();
    let question_terms = packet_probe_terms(question);
    let blocking_query_seeds = packet_flow_requirements_for_terms(&question_terms, task_class)
        .into_iter()
        .filter(|requirement| {
            flow_requirement_blocks_sufficiency(requirement)
                && missing_requirement_ids.contains(requirement.id)
        })
        .flat_map(|requirement| requirement.query_seeds.iter().copied())
        .collect::<HashSet<_>>();
    let blocking_probe_queries = blocking_missing_probe_queries
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let missing_probe_queries = missing_required_probe_queries
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();

    unresolved_sidecar_queries
        .iter()
        .filter(|query| {
            blocking_query_seeds.contains(query.as_str())
                || blocking_probe_queries.contains(query.as_str())
                || missing_probe_queries.contains(query.as_str())
        })
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

#[allow(clippy::too_many_arguments)]
fn packet_flow_roles_for_claim(
    claim: &PacketClaimDto,
    site_build_flow: bool,
    mapper_flow: bool,
    shell_install_dispatch_flow: bool,
    url_session_request_flow: bool,
    form_validation_flow: bool,
    server_request_dispatch_flow: bool,
    html_css_template_structure_flow: bool,
    stylesheet_animation_flow: bool,
    sql_schema_flow: bool,
    runtime_formatting_flow: bool,
    string_predicate_flow: bool,
    log_record_handler_flow: bool,
    include_generic_fallback_roles: bool,
) -> HashSet<FlowRole> {
    let mut roles = HashSet::new();
    let lower = claim.claim.to_ascii_lowercase();
    let normalized = normalize_identifier(&claim.claim);

    if site_build_flow {
        if normalized.contains("buildprocess")
            && contains_any(&normalized, &["constructs", "processes"])
            && normalized.contains("site")
        {
            roles.insert(FlowRole::Entrypoint);
        }
        if normalized.contains("siteprocess")
            && contains_any(&normalized, &["read", "generate", "render", "write"])
        {
            roles.insert(FlowRole::Dispatch);
        }
        if (normalized.contains("reader") && normalized.contains("read"))
            || (normalized.contains("renderer") && normalized.contains("render"))
            || (normalized.contains("sitewrite") || normalized.contains("writephases"))
        {
            roles.insert(FlowRole::TerminalBoundary);
        }
    }

    if mapper_flow {
        if (normalized.contains("mapper") || normalized.contains("objectmapping"))
            && normalized.contains("entrypoint")
        {
            roles.insert(FlowRole::Entrypoint);
        }
        if normalized.contains("mappingconfiguration")
            && (normalized.contains("configuration")
                || normalized.contains("runtime")
                || normalized.contains("plans"))
        {
            roles.insert(FlowRole::Configuration);
        }
        if (normalized.contains("typemap") && normalized.contains("plan"))
            || normalized.contains("typemapsource")
            || normalized.contains("mappingplanbuilder")
            || normalized.contains("planbuilder")
            || normalized.contains("executionpipeline")
        {
            roles.insert(FlowRole::Dispatch);
        }
        if normalized.contains("expressionplans") || normalized.contains("mappingconfiguration") {
            roles.insert(FlowRole::Configuration);
        }
    }

    if shell_install_dispatch_flow {
        if normalized.contains("installsh")
            && (normalized.contains("bootstrap") || normalized.contains("sourced"))
        {
            roles.insert(FlowRole::Entrypoint);
        }
        if normalized.contains("dispatcher")
            || normalized.contains("dispatch")
            || normalized.contains("installhelper")
            || normalized.contains("downloadhelper")
            || normalized.contains("downloadassets")
        {
            roles.insert(FlowRole::Dispatch);
        }
        if normalized.contains("bashcompletion")
            || normalized.contains("completion")
            || normalized.contains("currentversion")
            || normalized.contains("alreadyactive")
            || normalized.contains("configurednodeversion")
        {
            roles.insert(FlowRole::TerminalBoundary);
        }
    }

    if url_session_request_flow {
        if normalized.contains("sessionrequest")
            && (normalized.contains("creates") || normalized.contains("requestobjects"))
        {
            roles.insert(FlowRole::Entrypoint);
        }
        if normalized.contains("requestresume")
            || normalized.contains("resumes")
            || normalized.contains("urlsessiontask")
            || normalized.contains("eagerexecution")
        {
            roles.insert(FlowRole::Dispatch);
        }
        if normalized.contains("validation")
            || normalized.contains("requestvalidation")
            || (normalized.contains("request") && normalized.contains("validate"))
            || normalized.contains("delegatecallback")
            || normalized.contains("delegatecallbacks")
            || normalized.contains("callback")
            || normalized.contains("callbacks")
        {
            roles.insert(FlowRole::Dispatch);
        }
        if normalized.contains("delegatecallback")
            || normalized.contains("delegatecallbacks")
            || normalized.contains("urlsessioncallback")
            || normalized.contains("urlsessioncallbacks")
            || (normalized.contains("delegate") && normalized.contains("callback"))
        {
            roles.insert(FlowRole::Dispatch);
        }
    }

    if form_validation_flow {
        if (normalized.contains("native")
            || normalized.contains("constraint")
            || normalized.contains("constraints")
            || normalized.contains("formvalidationexamples"))
            && contains_any(&normalized, &["required", "pattern", "min", "max"])
        {
            roles.insert(FlowRole::TransformOrValidate);
        }
        if normalized.contains("custom")
            && normalized.contains("validation")
            && contains_any(&normalized, &["browser", "defaultui", "ui"])
        {
            roles.insert(FlowRole::TransformOrValidate);
        }
        if normalized.contains("submit")
            && contains_any(
                &normalized,
                &["prevent", "prevents", "submission", "invalid"],
            )
        {
            roles.insert(FlowRole::TerminalBoundary);
        }
        if normalized.contains("validitystate")
            || (normalized.contains("validity")
                && contains_any(
                    &normalized,
                    &[
                        "valid",
                        "valuemissing",
                        "typemismatch",
                        "tooshort",
                        "message",
                        "messages",
                    ],
                ))
        {
            roles.insert(FlowRole::TransformOrValidate);
        }
    }

    if server_request_dispatch_flow {
        if contains_all(&normalized, &["wsgi", "app"]) && normalized.contains("entrypoint") {
            roles.insert(FlowRole::Registration);
        }
        if contains_all(&normalized, &["full", "dispatch", "request"])
            && contains_any(&normalized, &["finalization", "finalize"])
            && contains_any(&normalized, &["preprocess", "exception", "wrap"])
        {
            roles.insert(FlowRole::Dispatch);
            roles.insert(FlowRole::TerminalBoundary);
        }
        if contains_all(&normalized, &["dispatch", "request", "view", "function"])
            && !normalized.contains("full")
        {
            roles.insert(FlowRole::Dispatch);
        }
        if (normalized.contains("routedecorator") && normalized.contains("registersviewfunctions"))
            || (normalized.contains("routeregistrationdecorator")
                && normalized.contains("urlrules"))
        {
            roles.insert(FlowRole::Registration);
        }
    }

    if html_css_template_structure_flow {
        if normalized.contains("appshell")
            && (normalized.contains("divapp") || normalized.contains("modulescript"))
        {
            roles.insert(FlowRole::Entrypoint);
        }
        if normalized.contains("roottypography")
            || normalized.contains("colorscheme")
            || normalized.contains("bodylayout")
        {
            roles.insert(FlowRole::Configuration);
        }
        if normalized.contains("appconstrains")
            || (normalized.contains("mountedapplication") && normalized.contains("padding"))
        {
            roles.insert(FlowRole::Configuration);
        }
        if normalized.contains("logo")
            && normalized.contains("button")
            && contains_any(&normalized, &["hover", "focus", "transition"])
        {
            roles.insert(FlowRole::Configuration);
        }
        if normalized.contains("preferscolorschemelight") || normalized.contains("mediaquery") {
            roles.insert(FlowRole::Configuration);
        }
    }

    if stylesheet_animation_flow {
        if normalized.contains("animationstylesheetentrypoint")
            || (normalized.contains("imports") && normalized.contains("animationfiles"))
            || normalized.contains("baseclass")
        {
            roles.insert(FlowRole::Entrypoint);
        }
        if normalized.contains("imports")
            || normalized.contains("animationname")
            || normalized.contains("matchingkeyframes")
        {
            roles.insert(FlowRole::Configuration);
        }
        if normalized.contains("customproperties")
            || normalized.contains("duration")
            || normalized.contains("delay")
            || normalized.contains("repeat")
            || normalized.contains("keyframes")
        {
            roles.insert(FlowRole::Configuration);
        }
    }

    if sql_schema_flow {
        if normalized.contains("sqlschema")
            && (normalized.contains("definestables")
                || normalized.contains("tables")
                || normalized.contains("createtable"))
        {
            roles.insert(FlowRole::StateOrStorage);
        }
        if normalized.contains("rowsreference")
            || normalized.contains("foreignkey")
            || (normalized.contains("reference") && normalized.contains("rows"))
        {
            roles.insert(FlowRole::Configuration);
        }
        if normalized.contains("sqldialect")
            || normalized.contains("schemascripts")
            || normalized.contains("dialectscripts")
        {
            roles.insert(FlowRole::StateOrStorage);
        }
    }

    if runtime_formatting_flow {
        if (normalized.contains("typeerased")
            && (normalized.contains("formatargs")
                || normalized.contains("formatarguments")
                || normalized.contains("formattingarguments")
                || normalized.contains("arguments")))
            || (normalized.contains("runtimeformatting")
                && normalized.contains("centralruntimeargumentpath"))
        {
            roles.insert(FlowRole::TransformOrValidate);
        }
        if (normalized.contains("formatto")
            || normalized.contains("outputiterator")
            || normalized.contains("formattedoutputhelpers"))
            && (normalized.contains("outputiterator")
                || normalized.contains("formattedoutput")
                || normalized.contains("output"))
        {
            roles.insert(FlowRole::TerminalBoundary);
        }
        if normalized.contains("buffer") && normalized.contains("append") {
            roles.insert(FlowRole::StateOrStorage);
        }
        if normalized.contains("formaterror")
            || normalized.contains("formattingfailures")
            || normalized.contains("systemerrors")
        {
            roles.insert(FlowRole::ErrorOrFallback);
        }
    }

    if string_predicate_flow {
        if (normalized.contains("string") && normalized.contains("utils"))
            || normalized.contains("strings")
            || (normalized.contains("charsequence") && normalized.contains("utils"))
        {
            roles.insert(FlowRole::Entrypoint);
        }
        if normalized.contains("delegates") || normalized.contains("regionmatches") {
            roles.insert(FlowRole::Dispatch);
        }
        if contains_any(
            &normalized,
            &[
                "null",
                "empty",
                "blank",
                "whitespace",
                "trim",
                "case",
                "ignorecase",
                "casesensitive",
            ],
        ) {
            roles.insert(FlowRole::StateOrStorage);
        }
    }

    if log_record_handler_flow {
        if normalized.contains("addrecord")
            || normalized.contains("logmethod")
            || normalized.contains("recordcreation")
            || (normalized.contains("log")
                && normalized.contains("record")
                && normalized.contains("creates"))
        {
            roles.insert(FlowRole::Entrypoint);
        }
        if normalized.contains("handlerstack")
            || normalized.contains("handlerregistration")
            || normalized.contains("pushhandler")
            || (normalized.contains("handler")
                && normalized.contains("interface")
                && contains_any(
                    &normalized,
                    &["handlebatch", "handlingboundaries", "contract"],
                ))
            || (normalized.contains("processing")
                && normalized.contains("handler")
                && contains_any(&normalized, &["processing", "writing", "write"]))
        {
            roles.insert(FlowRole::Dispatch);
        }
    }

    if include_generic_fallback_roles {
        if contains_any(
            &normalized,
            &[
                "entrypoint",
                "toplevel",
                "public",
                "command",
                "route",
                "router",
                "registration",
                "register",
                "helper",
                "helpers",
                "wrapper",
                "wrappers",
                "clientfactory",
                "factory",
                "api",
                "apis",
            ],
        ) {
            insert_generic_entrypoint_roles(&mut roles);
        }
        if contains_any(
            &normalized,
            &[
                "delegate",
                "delegates",
                "handoff",
                "dispatch",
                "calls",
                "calling",
                "send",
                "routes",
                "handler",
                "executes",
                "coordinates",
                "maps",
                "wrap",
                "wraps",
                "wrapper",
                "wrappers",
                "read",
                "reads",
                "write",
                "writes",
                "execution",
                "pipeline",
                "plan",
                "plans",
                "lambda",
                "mapping",
            ],
        ) {
            insert_generic_dispatch_roles(&mut roles);
        }
        if contains_any(
            &normalized,
            &[
                "boundary",
                "transport",
                "persist",
                "project",
                "store",
                "cache",
                "state",
                "prepare",
                "response",
                "serialize",
                "extract",
                "refresh",
                "output",
                "schema",
                "buffer",
                "bytes",
                "byte",
                "record",
                "records",
                "format",
                "formatted",
                "write",
                "writes",
                "writing",
                "source",
                "sink",
                "upstream",
                "configuration",
                "plan",
                "plans",
                "lambda",
                "expression",
                "destination",
            ],
        ) || lower.contains("side effect")
        {
            insert_generic_boundary_roles(&mut roles);
        }

        for citation in &claim.citations {
            match packet_evidence_role(citation) {
                Some(PacketEvidenceRole::CommandEntrypoint)
                | Some(PacketEvidenceRole::ClientFactory)
                | Some(PacketEvidenceRole::SearchDriver)
                | Some(PacketEvidenceRole::RouteHandling)
                | Some(PacketEvidenceRole::CollectionConfiguration)
                | Some(PacketEvidenceRole::AppServerRequestProtocol) => {
                    insert_generic_entrypoint_roles(&mut roles);
                }
                Some(PacketEvidenceRole::RequestDispatch)
                | Some(PacketEvidenceRole::CommandDispatch)
                | Some(PacketEvidenceRole::TransportAdapter)
                | Some(PacketEvidenceRole::SearchExecutionUnit)
                | Some(PacketEvidenceRole::RuntimeOrchestration)
                | Some(PacketEvidenceRole::EventLoop)
                | Some(PacketEvidenceRole::NetworkCommandInput)
                | Some(PacketEvidenceRole::IndexingWorkQueue)
                | Some(PacketEvidenceRole::BufferedIo)
                | Some(PacketEvidenceRole::InterceptorManagement) => {
                    insert_generic_dispatch_roles(&mut roles);
                }
                _ => {}
            }

            match packet_evidence_role(citation) {
                Some(PacketEvidenceRole::TransportAdapter)
                | Some(PacketEvidenceRole::PersistenceAndSearchProjection)
                | Some(PacketEvidenceRole::SnapshotRefresh)
                | Some(PacketEvidenceRole::EventOutputProcessing)
                | Some(PacketEvidenceRole::SymbolExtraction)
                | Some(PacketEvidenceRole::SourceGroupConfiguration)
                | Some(PacketEvidenceRole::WorkspaceDiscoveryAndPlanning)
                | Some(PacketEvidenceRole::CollectionConfiguration)
                | Some(PacketEvidenceRole::BufferedIo)
                | Some(PacketEvidenceRole::SqlTableDefinition)
                | Some(PacketEvidenceRole::SqlRelationshipConstraint)
                | Some(PacketEvidenceRole::SqlSchemaFile)
                | Some(PacketEvidenceRole::CandidateFileConstruction) => {
                    insert_generic_boundary_roles(&mut roles);
                }
                _ => {}
            }

            if sql_schema_flow {
                match packet_evidence_role(citation) {
                    Some(PacketEvidenceRole::SqlTableDefinition) => {
                        roles.insert(FlowRole::StateOrStorage);
                    }
                    Some(PacketEvidenceRole::SqlRelationshipConstraint) => {
                        roles.insert(FlowRole::Configuration);
                    }
                    Some(PacketEvidenceRole::SqlSchemaFile) => {
                        roles.insert(FlowRole::StateOrStorage);
                    }
                    _ => {}
                }
            }
        }
    }

    roles
}

fn insert_generic_entrypoint_roles(roles: &mut HashSet<FlowRole>) {
    roles.insert(FlowRole::Entrypoint);
    roles.insert(FlowRole::Registration);
}

fn insert_generic_dispatch_roles(roles: &mut HashSet<FlowRole>) {
    roles.insert(FlowRole::Dispatch);
}

fn insert_generic_boundary_roles(roles: &mut HashSet<FlowRole>) {
    roles.insert(FlowRole::Configuration);
    roles.insert(FlowRole::StateOrStorage);
    roles.insert(FlowRole::TerminalBoundary);
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::packet_budget::{apply_packet_budget, packet_budget_limits};
    use codestory_contracts::api::{
        AgentAnswerDto, AgentCitationDto, AgentResponseBlockDto, AgentResponseSectionDto,
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalTraceDto, EdgeId,
        GraphArtifactDto, GraphEdgeDto, GraphNodeDto, GraphResponse, NodeId, NodeKind,
        PacketBudgetDto, PacketBudgetLimitsDto, PacketBudgetUsageDto, PacketEvidenceResolutionDto,
        PacketEvidenceTierDto, PacketSidecarQueryDiagnosticDto, RetrievalScoreBreakdownDto,
        RetrievalShadowDto, SearchHitOrigin,
    };
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

    fn cited_claim(
        text: &str,
        coverage_role: Option<&str>,
        citation: AgentCitationDto,
        eligible_for_sufficiency: Option<bool>,
    ) -> PacketClaimDto {
        PacketClaimDto {
            claim: text.to_string(),
            proof_status: None,
            required_evidence_role: None,
            citations: vec![citation],
            coverage_role: coverage_role.map(str::to_string),
            eligible_for_sufficiency,
        }
    }

    fn answer_fixture(question: &str) -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "packet-sufficiency-test".to_string(),
            prompt: question.to_string(),
            summary: "Covered by cited anchors.".to_string(),
            freshness: None,
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
                annotations: Vec::new(),
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        }
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
        let answer = route_answer(question, names, edges);
        let claims = packet_supported_claims(&answer);
        let sufficiency = build_packet_sufficiency_with_extra(
            Path::new("C:/workspace/project"),
            question,
            PacketTaskClassDto::RouteTracing,
            &answer,
            &budget_fixture(),
            extra_probes,
        );
        (sufficiency, claims)
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
            diagnostic: Some("unresolved test candidate".to_string()),
        }
    }

    fn cancelled_sidecar_diagnostic(query: &str) -> PacketSidecarQueryDiagnosticDto {
        PacketSidecarQueryDiagnosticDto {
            query: query.to_string(),
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
            "main -> run",
            &["main", "run", "RouteSupport"],
            &[("main", "run")],
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
            "sha256 digest -> DigestExit",
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
        let question = "from CustomEntry through request dispatch to CustomExit";
        let names = ["CustomEntry", "dispatch_request", "CustomExit"];
        let (sufficiency, claims) = production_route_sufficiency_with_probes(
            question,
            &names,
            &[
                ("CustomEntry", "dispatch_request"),
                ("dispatch_request", "CustomExit"),
            ],
            &["dispatch_request".to_string()],
        );

        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "{sufficiency:?}"
        );
        assert!(claims.iter().any(|claim| {
            claim.coverage_role.as_deref() == Some("source evidence")
                && claim.citations.iter().any(|citation| {
                    citation.display_name == "CustomEntry" || citation.display_name == "CustomExit"
                })
        }));
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
        let question = "`plugin launcher` through `stdio transport` via `runtime packet orchestration` to `retrieval` to `packet sufficiency`";
        let names = [
            "plugin launcher",
            "stdio transport",
            "runtime packet orchestration",
            "retrieval",
            "packet sufficiency",
        ];
        let edges = [
            ("plugin launcher", "stdio transport"),
            ("stdio transport", "runtime packet orchestration"),
            ("runtime packet orchestration", "retrieval"),
            ("retrieval", "packet sufficiency"),
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
    fn route_proof_observes_actual_citation_and_edge_caps_across_packet_budgets() {
        let question = "RouteIngress -> RouteDispatch -> RouteEgress";
        let mut uncapped_answer = route_answer(
            question,
            &["RouteIngress", "RouteDispatch", "RouteEgress"],
            &[
                ("RouteIngress", "RouteDispatch"),
                ("RouteDispatch", "RouteEgress"),
            ],
        );
        uncapped_answer
            .citations
            .extend((0..12).map(|index| cited_anchor(&format!("Filler{index}"))));
        let GraphArtifactDto::Uml { graph, .. } = &mut uncapped_answer.graphs[0] else {
            unreachable!("route fixture must contain UML")
        };
        let route_edges = std::mem::take(&mut graph.edges);
        graph
            .nodes
            .extend((0..22).map(|index| route_graph_node(&format!("Filler{index}"))));
        graph.edges.extend((0..21).map(|index| {
            route_graph_edge(
                &format!("filler-edge-{index}"),
                &format!("Filler{index}"),
                &format!("Filler{}", index + 1),
            )
        }));
        graph.edges.extend(route_edges);

        for requested in [
            PacketBudgetModeDto::Compact,
            PacketBudgetModeDto::Standard,
            PacketBudgetModeDto::Deep,
        ] {
            let mut answer = uncapped_answer.clone();
            let limits = packet_budget_limits(requested);
            let budget = apply_packet_budget(
                Path::new("C:/workspace/project"),
                question,
                PacketTaskClassDto::RouteTracing,
                requested,
                limits.clone(),
                &mut answer,
            );
            let retained = answer
                .citations
                .iter()
                .map(|citation| citation.node_id.0.as_str())
                .collect::<HashSet<_>>();
            let claims = ["RouteIngress", "RouteDispatch", "RouteEgress"]
                .into_iter()
                .filter(|name| retained.contains(name))
                .map(route_claim)
                .collect();
            let sufficiency = route_sufficiency(question, &answer, &budget, claims);

            if requested == PacketBudgetModeDto::Compact {
                assert!(budget.truncated, "compact must exercise real caps");
                assert!(answer.citations.len() <= limits.max_anchors as usize);
                let GraphArtifactDto::Uml { graph, .. } = &answer.graphs[0] else {
                    unreachable!("route fixture must retain UML")
                };
                assert_eq!(graph.edges.len(), limits.max_trail_edges as usize);
                assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
            } else {
                assert!(!budget.truncated, "{requested:?} should retain the route");
                assert_eq!(
                    sufficiency.status,
                    PacketSufficiencyStatusDto::Sufficient,
                    "retained route should remain sufficient for {requested:?}: {sufficiency:?}"
                );
                assert!(sufficiency.gaps.is_empty());
            }
        }
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
    fn normal_route_wording_does_not_turn_control_words_into_stages() {
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

        assert_unresolved_route_order(&sufficiency);
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
                "reason=\"generic navigation/source-evidence claim lacks required coverage role\""
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
        let sufficiency =
            assemble_packet_sufficiency_with_probe_context(input(), &[], &exact_paths);

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
        let claims = vec![
            claim(
                "The form validation examples use native required, pattern, min, and max constraints.",
            ),
            claim("Submit handlers prevent submission when the form is invalid."),
            cited_claim(
                "`validateForm` in `src/forms.js` ties form validation in this flow to cited definitions and adjacent ownership.",
                Some("transform_or_validate"),
                cited_anchor_with_tier(
                    "validateForm",
                    "src/forms.js",
                    PacketEvidenceTierDto::ResolvedGraph,
                    Some(true),
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
    fn sql_synthetic_source_scan_table_and_foreign_key_cover_schema_requirements() {
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

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report
                .covered
                .contains(&"sql table definitions".to_string()),
            "source-scan SQL table text should report the concrete role: {report:?}"
        );
        assert!(
            report.covered.contains(&"sql relationships".to_string()),
            "source-scan SQL relationship text should report the concrete role: {report:?}"
        );
        assert!(
            !report.covered.contains(&"source evidence".to_string()),
            "covered roles must not imply generic source evidence is proof: {report:?}"
        );
        assert_eq!(report.ineligible.len(), 1);
        assert!(report.ineligible[0].contains("role=\"sql schema scripts\""));
        assert!(report.ineligible[0].contains("tier=\"synthetic_source_scan\""));
        assert!(
            report.ineligible[0].contains("reason=\"claim marked diagnostic\""),
            "plain SQL source-scan file evidence should remain diagnostic: {report:?}"
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
                    "ProcessingHandler::handle",
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
                    "ProcessingHandler::handle",
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
                        "generic navigation/source-evidence claim lacks required coverage role",
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
        let claims = vec![
            claim("SQL schema defines tables Artist, Album, Track, Invoice, and InvoiceLine."),
            claim("Track rows reference Album, Genre, and MediaType rows."),
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
    }

    #[test]
    fn synthetic_source_scan_stays_nonproof_for_non_structural_requirements() {
        let question = "Explain how formatting arguments become type-erased format args and reach vformat or format_to output paths.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
            ),
            claim("Runtime formatting writes formatted output through output iterator helpers."),
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

    #[test]
    fn covered_flow_roles_make_missing_probe_queries_follow_up_hints() {
        let question = "Explain how the form validation examples combine native HTML constraints with custom JavaScript validation.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            claim(
                "The form validation examples use native required, pattern, min, and max constraints.",
            ),
            claim("Submit handlers prevent submission when the form is invalid."),
            claim("Custom error rendering branches on ValidityState fields to choose messages."),
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
        let claims = vec![
            claim(
                "The form validation examples use native required, pattern, min, and max constraints.",
            ),
            claim("Submit handlers prevent submission when the form is invalid."),
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
        let claims = vec![
            claim(
                "A custom validation example applies script-driven validity checks before rendering messages.",
            ),
            claim("Submit handlers prevent submission when the form is invalid."),
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
            claim(
                "The form validation examples use native required, pattern, min, and max constraints.",
            ),
            claim(
                "A custom validation example applies script-driven validity checks before rendering messages.",
            ),
            claim("Submit handlers prevent submission when the form is invalid."),
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
        let claims = vec![
            claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
            ),
            claim("Runtime formatting writes formatted output through output iterator helpers."),
            claim("Runtime formatting appends formatted output to a buffer."),
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
            claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
            ),
            claim("Runtime formatting writes formatted output through output iterator helpers."),
            claim("Runtime formatting defines format_error for formatting failures."),
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

        assert_eq!(sufficiency.covered_claims.len(), 7);
        assert!(!sufficiency.follow_up_commands.is_empty());
        let report = sufficiency.coverage_report.as_ref().unwrap();
        let expected_labels = [
            "component_report",
            "dense_anchor",
            "exact",
            "graph_neighbor",
            "lexical_source",
            "precise_semantic_import",
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

    #[test]
    fn client_send_split_requirements_remain_distinct() {
        let question = "Explain how an HTTP client exposes top-level helpers, provides client convenience methods, finalizes requests before transport send, and materializes responses.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            claim("Top-level HTTP helpers delegate to a Client."),
            claim("Client convenience methods live on the client interface helper."),
            claim("Base request finalize prepares request bodies for sending."),
            claim("The transport send implementation sends through an HTTP client adapter."),
        ];

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
        let claims = vec![
            claim("Top-level HTTP helpers delegate to a Client."),
            claim("Client convenience methods live on the client interface helper."),
            claim("Base request finalize prepares request bodies for sending."),
            claim("The transport send implementation sends through an HTTP client adapter."),
            claim("Response.fromStream materializes the response stream boundary."),
        ];

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

    #[test]
    fn hook_cache_requirements_remain_distinct() {
        let question = "Explain how a public hook serializes keys, connects cache helpers, and routes mutate behavior through a mutation helper.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            claim("The public useData export wraps useDataHandler with argument normalization."),
            claim("useDataHandler serializes hook keys into cache keys."),
            claim("applyMutation routes mutate behavior through the mutation helper."),
        ];

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
        let claims = vec![
            claim("The public useData export wraps useDataHandler with argument normalization."),
            claim("useDataHandler serializes hook keys into cache keys."),
            claim("makeCacheHelper provides cache get, set, subscribe, and snapshot helpers."),
            claim("applyMutation routes mutate behavior through the mutation helper."),
        ];

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

    #[test]
    fn command_loop_split_requirements_remain_distinct() {
        let question = "Trace how a command server bootstrap enters an event loop, reads network command input, and dispatches commands through a command table.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            claim("Server bootstrap initializes the command server main loop."),
            claim("The event loop source polls file events."),
            claim("Command table dispatch routes commands to handlers."),
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
            claim("Network command input reads commands from socket input."),
            claim("Command table dispatch routes commands to handlers."),
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
        let claims = vec![
            claim("Server bootstrap initializes the command server main loop."),
            claim("The event loop source polls file events."),
            claim("Network command input reads commands from socket input."),
            claim("Command table dispatch routes commands to handlers."),
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
            claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
            ),
            claim("Runtime formatting writes formatted output through output iterator helpers."),
            claim("Runtime formatting appends formatted output to a buffer."),
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
            claim("Buffer is the in-memory byte store used by buffered reads and writes."),
            claim("A buffered source wrapper reads from an upstream Source into a Buffer."),
            claim("A buffered sink wrapper writes buffered bytes to an upstream Sink."),
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
            claim("Build.process constructs or processes a site."),
            claim("Site.process runs reset, read, generate, render, cleanup, and write phases."),
            claim("Reader is responsible for reading site content."),
            claim("Renderer renders pages and documents."),
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
            claim(
                "wsgi_app is the WSGI entry point and creates or uses request context before dispatch.",
            ),
            claim(
                "full_dispatch_request wraps preprocessing, dispatch, exception handling, and response finalization.",
            ),
            claim("dispatch_request invokes the view function selected by URL matching."),
            claim(
                "Route registration decorator adds URL rules without performing request dispatch itself.",
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
                missing_required_probe_queries: Vec::new(),
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
            vec!["response send helper".to_string(), "helpers".to_string()]
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
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: Some("dispatch".to_string()),
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "Selected handler invocation happens.".to_string(),
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
                proof_status: None,
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: Some("dispatch".to_string()),
                eligible_for_sufficiency: None,
            },
            PacketClaimDto {
                claim: "Selected handler invocation happens.".to_string(),
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
            claim(
                "home.html provides the app shell with viewport metadata, div#app, and a script[type=\"module\"] module script entry.",
            ),
            claim(
                "main.css owns :root typography, color-scheme, smoothing, and body layout defaults.",
            ),
            claim("CSS app container rules constrain mounted content and center it with padding."),
            claim("CSS interaction selectors define hover, focus, and transition behavior."),
            claim(
                "Light color-scheme media query rules override root, link-hover, and button colors.",
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
            claim(
                "The animation stylesheet entrypoint imports variable, base, and animation files.",
            ),
            claim(
                "Shared CSS custom properties define animation duration, delay, and repeat defaults.",
            ),
            claim(
                "The base class applies animation duration and fill mode, while named classes set animation-name to matching keyframes.",
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
            claim(
                "main.css owns :root typography, color-scheme, smoothing, and body layout defaults.",
            ),
            claim("CSS app container rules constrain mounted content and center it with padding."),
            claim("CSS interaction selectors define hover, focus, and transition behavior."),
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
            claim("Mapper runtime source exposes the public object-mapping entry point."),
            claim("Mapping configuration source builds and owns runtime mapping plans."),
            claim(
                "Type-map source contributes lambda plans used by the mapping execution pipeline.",
            ),
            claim(
                "The mapping plan builder participates in building expression plans for mappings.",
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
            claim("The repository carries multiple SQL dialect scripts for the same schema."),
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
            claim("The logger owns a handler stack populated by handler registration."),
            claim("addRecord creates a log record before passing it to handlers."),
            claim("The handler interface defines record handling and batch handling boundaries."),
            claim("The processing handler handles records by processing and writing them."),
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
            claim(
                "Runtime formatting uses type-erased arguments before dispatching formatted output helpers.",
            ),
            claim("Runtime formatting writes formatted output through output iterator helpers."),
            claim("Runtime formatting defines an error type for formatting failures."),
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
            claim(
                "The form validation examples use native required, pattern, min, and max constraints.",
            ),
            claim(
                "A custom validation example applies script-driven validity checks before rendering messages.",
            ),
            claim("Submit handlers prevent submission when the form is invalid."),
            claim("Custom error rendering branches on ValidityState fields to choose messages."),
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
            claim("StringUtils.isBlank treats null, empty, and whitespace-only inputs as blank."),
            claim("StringUtils.isEmpty does not trim whitespace before deciding emptiness."),
            claim("Strings delegates region matching work to CharSequenceUtils.regionMatches."),
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

pub(crate) fn packet_budget_exceeded_hard_output_cap(budget: &PacketBudgetDto) -> bool {
    budget.used.output_bytes > budget.limits.max_output_bytes
}

fn packet_follow_up_commands(
    project_root: &Path,
    question: &str,
    status: PacketSufficiencyStatusDto,
    budget: &PacketBudgetDto,
    missing_required_probe_queries: &[String],
    targeted_follow_up_queries: Vec<String>,
    full_retrieval_available: bool,
) -> Vec<String> {
    let project = quote_packet_project_arg(project_root);
    match status {
        PacketSufficiencyStatusDto::Sufficient => Vec::new(),
        PacketSufficiencyStatusDto::Partial => {
            let queries = if missing_required_probe_queries.is_empty() {
                targeted_follow_up_queries
            } else {
                missing_required_probe_queries.to_vec()
            };
            if !full_retrieval_available {
                let mut commands = vec![packet_retrieval_activation_command(project.as_str())];
                commands.extend(packet_follow_up_trail_commands(project.as_str(), &queries));
                commands.truncate(8);
                return commands;
            }
            let mut commands = packet_follow_up_search_commands(project.as_str(), &queries);
            commands.truncate(8);
            commands
                .into_iter()
                .chain(budget.next_deeper_command.clone())
                .chain(std::iter::once(format!(
                    "codestory-cli search --project {project} --query {} --why",
                    quote_packet_command_value(question)
                )))
                .collect()
        }
        PacketSufficiencyStatusDto::Insufficient => {
            if full_retrieval_available {
                vec![
                    format!("codestory-cli index --project {project} --refresh full"),
                    format!(
                        "codestory-cli search --project {project} --query {} --why",
                        quote_packet_command_value(question)
                    ),
                ]
            } else {
                vec![
                    packet_retrieval_activation_command(project.as_str()),
                    format!("codestory-cli ground --project {project} --why"),
                ]
            }
        }
    }
}

fn packet_full_retrieval_available(answer: &AgentAnswerDto) -> bool {
    answer
        .retrieval_trace
        .retrieval_shadow
        .as_ref()
        .is_some_and(|shadow| shadow.retrieval_mode == "full")
}

fn packet_retrieval_activation_command(quoted_project: &str) -> String {
    format!(
        "codestory-cli retrieval index --profile agent --refresh auto --project {quoted_project} --format json"
    )
}

fn packet_follow_up_trail_commands(quoted_project: &str, queries: &[String]) -> Vec<String> {
    let mut commands = Vec::new();
    for query in queries {
        push_unique_term(
            &mut commands,
            &format!(
                "codestory-cli trail --project {quoted_project} --query {} --story --hide-speculative",
                quote_packet_command_value(query)
            ),
        );
    }
    commands
}

fn packet_follow_up_search_commands(quoted_project: &str, queries: &[String]) -> Vec<String> {
    let mut commands = Vec::new();
    for query in queries {
        push_unique_term(
            &mut commands,
            &format!(
                "codestory-cli search --project {quoted_project} --query {} --why",
                quote_packet_command_value(query)
            ),
        );
    }
    commands
}

pub(crate) fn quote_packet_project_arg(project_root: &Path) -> String {
    quote_packet_command_value(project_root.to_string_lossy().as_ref())
}

pub(crate) fn quote_packet_command_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn contains_all(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().all(|needle| haystack.contains(needle))
}

fn push_unique_term(terms: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    if !terms.iter().any(|existing| existing == value) {
        terms.push(value.to_string());
    }
}
