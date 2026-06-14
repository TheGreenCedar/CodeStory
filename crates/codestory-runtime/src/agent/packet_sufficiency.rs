use crate::agent::packet_claims::packet_supported_claims;
use crate::agent::packet_evidence_roles::packet_evidence_role;
use crate::agent::packet_plan::packet_symbol_probe_queries;
use crate::agent::packet_required_probes::packet_missing_sufficiency_probe_queries_with_extra;
use crate::agent::packet_scoring::{normalize_identifier, packet_display_path};
use codestory_contracts::api::{
    AgentAnswerDto, AgentResponseBlockDto, AgentRetrievalStepStatusDto, GraphArtifactDto,
    PacketBudgetDto, PacketBudgetModeDto, PacketClaimDto, PacketSufficiencyDto,
    PacketSufficiencyStatusDto, PacketTaskClassDto,
};
use std::collections::HashSet;
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

pub(crate) fn build_packet_sufficiency_with_extra(
    project_root: &Path,
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    extra_probes: &[String],
) -> PacketSufficiencyDto {
    let supported_claims = packet_supported_claims(answer);
    let missing_required_probe_queries = packet_missing_sufficiency_probe_queries_with_extra(
        question,
        task_class,
        answer,
        &supported_claims,
        extra_probes,
    );
    assemble_packet_sufficiency(PacketSufficiencyInput {
        project_root,
        question,
        task_class,
        answer,
        budget,
        supported_claims,
        missing_required_probe_queries,
        targeted_follow_up_queries: packet_targeted_follow_up_queries(question, task_class),
    })
}

fn assemble_packet_sufficiency(input: PacketSufficiencyInput<'_>) -> PacketSufficiencyDto {
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

    let has_errors = answer
        .retrieval_trace
        .steps
        .iter()
        .any(|step| step.status == AgentRetrievalStepStatusDto::Error);
    let min_citations = packet_sufficiency_min_citations(task_class);
    let min_claims = packet_sufficiency_min_claims(task_class);
    let has_minimum_coverage = answer.citations.len() >= min_citations;
    let has_minimum_claims = supported_claims.len() >= min_claims;
    let claim_family_count = packet_supported_claim_family_count(&supported_claims);
    let has_minimum_claim_families =
        packet_has_minimum_claim_family_coverage(task_class, &supported_claims);
    let has_sufficiency_blocking_budget_omission = packet_has_sufficiency_blocking_budget_omission(
        answer,
        budget,
        min_citations,
        min_claims,
        supported_claims.len(),
    );
    let unresolved_sidecar_queries = unresolved_sidecar_queries(answer);
    let status = packet_sufficiency_status(
        answer,
        budget,
        has_errors,
        has_minimum_coverage,
        has_minimum_claims,
        has_minimum_claim_families,
        has_sufficiency_blocking_budget_omission,
        &missing_required_probe_queries,
        &unresolved_sidecar_queries,
    );

    let gaps = packet_sufficiency_gaps(
        task_class,
        answer,
        budget,
        min_citations,
        min_claims,
        supported_claims.len(),
        claim_family_count,
        status,
        has_minimum_coverage,
        has_minimum_claims,
        has_minimum_claim_families,
        has_sufficiency_blocking_budget_omission,
        &missing_required_probe_queries,
        &unresolved_sidecar_queries,
    );
    let follow_up_commands = packet_follow_up_commands(
        project_root,
        question,
        status,
        budget,
        &missing_required_probe_queries,
        targeted_follow_up_queries,
    );
    let open_next = follow_up_commands.clone();
    let avoid_opening = answer
        .citations
        .iter()
        .filter_map(|citation| citation.file_path.as_ref())
        .map(|path| packet_display_path(path))
        .collect::<HashSet<_>>()
        .into_iter()
        .take(12)
        .map(|path| {
            format!(
                "{} because this packet already includes a citation for the current answer.",
                path
            )
        })
        .collect::<Vec<_>>();

    if supported_claims.is_empty() {
        supported_claims.push(PacketClaimDto {
            claim: answer.summary.clone(),
            citations: answer.citations.iter().take(6).cloned().collect(),
        });
    }

    PacketSufficiencyDto {
        status,
        covered_claims: supported_claims,
        open_next,
        avoid_opening,
        gaps,
        follow_up_commands,
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

fn packet_sufficiency_status(
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    has_errors: bool,
    has_minimum_coverage: bool,
    has_minimum_claims: bool,
    has_minimum_claim_families: bool,
    has_sufficiency_blocking_budget_omission: bool,
    missing_required_probe_queries: &[String],
    unresolved_sidecar_queries: &[String],
) -> PacketSufficiencyStatusDto {
    if answer.citations.is_empty() {
        PacketSufficiencyStatusDto::Insufficient
    } else if has_errors
        || !has_minimum_coverage
        || !has_minimum_claims
        || !has_minimum_claim_families
        || !missing_required_probe_queries.is_empty()
        || !unresolved_sidecar_queries.is_empty()
        || has_sufficiency_blocking_budget_omission
        || packet_budget_exceeded_hard_output_cap(budget)
    {
        PacketSufficiencyStatusDto::Partial
    } else {
        PacketSufficiencyStatusDto::Sufficient
    }
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
    status: PacketSufficiencyStatusDto,
    has_minimum_coverage: bool,
    has_minimum_claims: bool,
    has_minimum_claim_families: bool,
    has_sufficiency_blocking_budget_omission: bool,
    missing_required_probe_queries: &[String],
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
    if !answer.citations.is_empty() && !has_minimum_claim_families {
        gaps.push(format!(
            "{:?} packet covered only {} distinct claim families; at least {} are required before treating the packet as sufficient.",
            task_class,
            claim_family_count,
            packet_sufficiency_min_claim_families(task_class)
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
        .filter(|diagnostic| {
            diagnostic.candidate_count > 0
                && diagnostic.resolved_hit_count == 0
                && diagnostic.unresolved_candidate_count > 0
        })
        .filter_map(|diagnostic| {
            seen.insert(diagnostic.query.clone())
                .then(|| diagnostic.query.clone())
        })
        .collect()
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
        if contains_any(
            &normalized_claim,
            &[
                "blank",
                "empty",
                "casesensitive",
                "ignorecase",
                "whitespace",
                "trim",
            ],
        ) && contains_any(
            &normalized_claim,
            &[
                "treats", "tests", "doesnot", "deciding", "return", "compares",
            ],
        ) {
            return Some("predicate behavior");
        }
        if normalized_claim.contains("public")
            && contains_any(
                &normalized_claim,
                &["api", "export", "entrypoint", "hook", "method"],
            )
        {
            return Some("public api/export");
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

fn packet_has_sufficiency_blocking_budget_omission(
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    min_citations: usize,
    min_claims: usize,
    supported_claim_count: usize,
) -> bool {
    if !budget.truncated {
        return false;
    }

    let has_claim_stop_signal =
        answer.citations.len() >= min_citations && supported_claim_count >= min_claims;
    let has_retained_graph = packet_has_retained_graph(answer);

    budget
        .omitted_sections
        .iter()
        .any(|section| match section.as_str() {
            "packet_payload" => true,
            "markdown_blocks" => {
                !has_claim_stop_signal || packet_markdown_truncation_blocks_sufficiency(answer)
            }
            "trail_edges" => !has_claim_stop_signal || !has_retained_graph,
            _ => false,
        })
}

fn packet_has_retained_graph(answer: &AgentAnswerDto) -> bool {
    answer.graphs.iter().any(|artifact| match artifact {
        GraphArtifactDto::Uml { graph, .. } => !graph.edges.is_empty(),
        GraphArtifactDto::Mermaid { .. } => false,
    })
}

fn packet_markdown_truncation_blocks_sufficiency(answer: &AgentAnswerDto) -> bool {
    let mut saw_truncated_markdown = false;
    for section in &answer.sections {
        for block in &section.blocks {
            let AgentResponseBlockDto::Markdown { markdown } = block else {
                continue;
            };
            if !markdown.contains(PACKET_MARKDOWN_TRUNCATION_SUFFIX.trim()) {
                continue;
            }
            saw_truncated_markdown = true;
            if !packet_section_allows_nonblocking_truncation(section.id.as_str()) {
                return true;
            }
        }
    }
    !saw_truncated_markdown
}

fn packet_section_allows_nonblocking_truncation(section_id: &str) -> bool {
    section_id == "retrieval-evidence"
        || section_id == "diagrams"
        || section_id.starts_with("packet-subquery-")
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
        PacketSufficiencyStatusDto::Insufficient => vec![
            format!("codestory-cli index --project {project} --refresh full"),
            format!(
                "codestory-cli search --project {project} --query {} --why",
                quote_packet_command_value(question)
            ),
        ],
    }
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

fn push_unique_term(terms: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    if !terms.iter().any(|existing| existing == value) {
        terms.push(value.to_string());
    }
}
