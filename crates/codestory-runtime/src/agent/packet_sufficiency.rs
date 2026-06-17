use crate::agent::packet_claims::packet_supported_claims;
use crate::agent::packet_evidence_roles::{PacketEvidenceRole, packet_evidence_role};
use crate::agent::packet_plan::packet_symbol_probe_queries;
use crate::agent::packet_required_probes::packet_missing_sufficiency_probe_queries_with_extra;
use crate::agent::packet_scoring::{normalize_identifier, packet_display_path};
use crate::agent::packet_terms::{
    packet_probe_terms, packet_terms_indicate_form_validation_flow,
    packet_terms_indicate_html_css_template_structure_flow,
    packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_runtime_formatting_flow,
    packet_terms_indicate_server_request_dispatch_flow,
    packet_terms_indicate_shell_install_dispatch_flow, packet_terms_indicate_site_build_phase_flow,
    packet_terms_indicate_sql_schema_flow, packet_terms_indicate_string_predicate_flow,
    packet_terms_indicate_stylesheet_animation_flow,
    packet_terms_indicate_url_session_request_flow,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentResponseBlockDto, AgentRetrievalStepStatusDto, GraphArtifactDto,
    PacketBudgetDto, PacketBudgetModeDto, PacketClaimDto, PacketSufficiencyDto,
    PacketSufficiencyStatusDto, PacketTaskClassDto,
};
use std::collections::{BTreeSet, HashSet};
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
        supported_claims,
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
    let sufficiency_claims = supported_claims
        .iter()
        .filter(|claim| packet_claim_can_satisfy_sufficiency(claim))
        .cloned()
        .collect::<Vec<_>>();
    let generic_navigation_claim_count = supported_claims
        .len()
        .saturating_sub(sufficiency_claims.len());
    let has_minimum_coverage = answer.citations.len() >= min_citations;
    let has_minimum_claims = sufficiency_claims.len() >= min_claims;
    let claim_family_count = packet_supported_claim_family_count(&sufficiency_claims);
    let has_minimum_claim_families =
        packet_has_minimum_claim_family_coverage(task_class, &sufficiency_claims);
    let missing_required_flow_roles =
        packet_missing_required_flow_roles(question, task_class, &sufficiency_claims);
    let has_required_flow_roles = missing_required_flow_roles.is_empty();
    let has_sufficiency_blocking_budget_omission = packet_has_sufficiency_blocking_budget_omission(
        answer,
        budget,
        min_citations,
        min_claims,
        sufficiency_claims.len(),
    );
    let unresolved_sidecar_queries = unresolved_sidecar_queries(answer);
    let status = packet_sufficiency_status(PacketSufficiencyStatusInput {
        answer,
        budget,
        has_errors,
        has_minimum_coverage,
        has_minimum_claims,
        has_minimum_claim_families,
        has_required_flow_roles,
        has_sufficiency_blocking_budget_omission,
        missing_required_probe_queries: &missing_required_probe_queries,
        unresolved_sidecar_queries: &unresolved_sidecar_queries,
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
        has_sufficiency_blocking_budget_omission,
        &missing_required_probe_queries,
        &missing_required_flow_roles,
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
    let avoid_opening_paths = answer
        .citations
        .iter()
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
    has_sufficiency_blocking_budget_omission: bool,
    missing_required_probe_queries: &[String],
    missing_required_flow_roles: &[&'static str],
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
        gaps.push(format!(
            "{:?} packet missed required flow-role coverage: {}.",
            task_class,
            missing_required_flow_roles.join(", ")
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
        .filter(|diagnostic| seen.insert(diagnostic.query.clone()))
        .map(|diagnostic| diagnostic.query.clone())
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
            || (normalized_claim.contains("mountedapplication")
                && normalized_claim.contains("padding"))
        {
            return Some("css app layout");
        }
        if normalized_claim.contains("logo")
            && normalized_claim.contains("button")
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
        if normalized_claim.contains("routedecorator")
            && normalized_claim.contains("registersviewfunctions")
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
        if normalized_claim.contains("typeerased")
            && (normalized_claim.contains("formatargs")
                || normalized_claim.contains("formatarguments")
                || normalized_claim.contains("formattingarguments"))
        {
            return Some("runtime format arguments");
        }
        if normalized_claim.contains("formatto")
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

pub(crate) fn packet_claim_can_satisfy_sufficiency(claim: &PacketClaimDto) -> bool {
    let lower = claim.claim.to_ascii_lowercase();
    !(lower.contains("anchored by")
        || lower.contains("inspect it")
        || lower.contains("inspect the cited")
        || (lower.contains("supports ") && lower.contains("inspect"))
        || (lower.contains("ties ")
            && lower.contains(" to cited definitions")
            && lower.contains("adjacent ownership"))
        || (lower.contains(" is defined in cited source ")
            && lower.contains("exact source anchor")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PacketFlowRole {
    EntryPoint,
    Handoff,
    BoundaryOrState,
}

impl PacketFlowRole {
    fn label(self) -> &'static str {
        match self {
            Self::EntryPoint => "entrypoint/registration",
            Self::Handoff => "handoff/dispatch",
            Self::BoundaryOrState => "boundary/effect/state",
        }
    }
}

fn packet_missing_required_flow_roles(
    question: &str,
    task_class: PacketTaskClassDto,
    supported_claims: &[PacketClaimDto],
) -> Vec<&'static str> {
    let question_terms = packet_probe_terms(question);
    let site_build_flow = packet_terms_indicate_site_build_phase_flow(&question_terms);
    let mapper_flow = packet_terms_indicate_mapper_configuration_plan_flow(&question_terms);
    let shell_install_dispatch_flow =
        packet_terms_indicate_shell_install_dispatch_flow(&question_terms);
    let url_session_request_flow = packet_terms_indicate_url_session_request_flow(&question_terms);
    let form_validation_flow = packet_terms_indicate_form_validation_flow(&question_terms);
    let server_request_dispatch_flow =
        packet_terms_indicate_server_request_dispatch_flow(&question_terms);
    let html_css_template_structure_flow =
        packet_terms_indicate_html_css_template_structure_flow(&question_terms);
    let stylesheet_animation_flow =
        packet_terms_indicate_stylesheet_animation_flow(&question_terms);
    let sql_schema_flow = packet_terms_indicate_sql_schema_flow(&question_terms);
    let runtime_formatting_flow = packet_terms_indicate_runtime_formatting_flow(&question_terms);
    let string_predicate_flow = packet_terms_indicate_string_predicate_flow(&question_terms);
    let required = packet_required_flow_roles(
        task_class,
        site_build_flow,
        shell_install_dispatch_flow,
        url_session_request_flow,
        form_validation_flow,
        server_request_dispatch_flow,
        html_css_template_structure_flow,
        stylesheet_animation_flow,
        sql_schema_flow,
        runtime_formatting_flow,
        string_predicate_flow,
    );
    if required.is_empty() {
        return Vec::new();
    }

    let mut covered = HashSet::new();
    for claim in supported_claims {
        for role in packet_flow_roles_for_claim(
            claim,
            site_build_flow,
            mapper_flow,
            shell_install_dispatch_flow,
            url_session_request_flow,
            form_validation_flow,
            server_request_dispatch_flow,
            html_css_template_structure_flow,
            stylesheet_animation_flow,
            sql_schema_flow,
            runtime_formatting_flow,
            string_predicate_flow,
        ) {
            covered.insert(role);
        }
    }
    required
        .iter()
        .copied()
        .filter(|role| !covered.contains(role))
        .map(PacketFlowRole::label)
        .collect()
}

fn packet_required_flow_roles(
    task_class: PacketTaskClassDto,
    site_build_flow: bool,
    shell_install_dispatch_flow: bool,
    url_session_request_flow: bool,
    form_validation_flow: bool,
    server_request_dispatch_flow: bool,
    html_css_template_structure_flow: bool,
    stylesheet_animation_flow: bool,
    sql_schema_flow: bool,
    runtime_formatting_flow: bool,
    string_predicate_flow: bool,
) -> &'static [PacketFlowRole] {
    if shell_install_dispatch_flow {
        return match task_class {
            PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::RouteTracing => &[
                PacketFlowRole::EntryPoint,
                PacketFlowRole::Handoff,
                PacketFlowRole::BoundaryOrState,
            ],
            PacketTaskClassDto::BugLocalization
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::EditPlanning => &[],
        };
    }

    if url_session_request_flow {
        return match task_class {
            PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::RouteTracing => &[
                PacketFlowRole::EntryPoint,
                PacketFlowRole::Handoff,
                PacketFlowRole::BoundaryOrState,
            ],
            PacketTaskClassDto::BugLocalization
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::EditPlanning => &[],
        };
    }

    if form_validation_flow {
        return match task_class {
            PacketTaskClassDto::ArchitectureExplanation | PacketTaskClassDto::DataFlow => &[
                PacketFlowRole::EntryPoint,
                PacketFlowRole::Handoff,
                PacketFlowRole::BoundaryOrState,
            ],
            PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::BugLocalization
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::EditPlanning => &[],
        };
    }

    if server_request_dispatch_flow {
        return match task_class {
            PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::RouteTracing => &[
                PacketFlowRole::EntryPoint,
                PacketFlowRole::Handoff,
                PacketFlowRole::BoundaryOrState,
            ],
            PacketTaskClassDto::BugLocalization
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::EditPlanning => &[],
        };
    }

    if html_css_template_structure_flow {
        return match task_class {
            PacketTaskClassDto::ArchitectureExplanation | PacketTaskClassDto::DataFlow => &[
                PacketFlowRole::EntryPoint,
                PacketFlowRole::Handoff,
                PacketFlowRole::BoundaryOrState,
            ],
            PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::BugLocalization
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::EditPlanning => &[],
        };
    }

    if stylesheet_animation_flow {
        return match task_class {
            PacketTaskClassDto::ArchitectureExplanation | PacketTaskClassDto::DataFlow => &[
                PacketFlowRole::EntryPoint,
                PacketFlowRole::Handoff,
                PacketFlowRole::BoundaryOrState,
            ],
            PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::BugLocalization
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::EditPlanning => &[],
        };
    }

    if sql_schema_flow {
        return match task_class {
            PacketTaskClassDto::ArchitectureExplanation | PacketTaskClassDto::DataFlow => &[
                PacketFlowRole::EntryPoint,
                PacketFlowRole::Handoff,
                PacketFlowRole::BoundaryOrState,
            ],
            PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::BugLocalization
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::EditPlanning => &[],
        };
    }

    if runtime_formatting_flow {
        return match task_class {
            PacketTaskClassDto::ArchitectureExplanation | PacketTaskClassDto::DataFlow => &[
                PacketFlowRole::EntryPoint,
                PacketFlowRole::Handoff,
                PacketFlowRole::BoundaryOrState,
            ],
            PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::BugLocalization
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::EditPlanning => &[],
        };
    }

    if string_predicate_flow {
        return match task_class {
            PacketTaskClassDto::ArchitectureExplanation | PacketTaskClassDto::DataFlow => &[
                PacketFlowRole::EntryPoint,
                PacketFlowRole::Handoff,
                PacketFlowRole::BoundaryOrState,
            ],
            PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::BugLocalization
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::EditPlanning => &[],
        };
    }

    if site_build_flow {
        return match task_class {
            PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::RouteTracing => &[
                PacketFlowRole::EntryPoint,
                PacketFlowRole::Handoff,
                PacketFlowRole::BoundaryOrState,
            ],
            PacketTaskClassDto::BugLocalization
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::EditPlanning => &[],
        };
    }

    match task_class {
        PacketTaskClassDto::ArchitectureExplanation | PacketTaskClassDto::DataFlow => &[
            PacketFlowRole::EntryPoint,
            PacketFlowRole::Handoff,
            PacketFlowRole::BoundaryOrState,
        ],
        PacketTaskClassDto::RouteTracing => &[PacketFlowRole::EntryPoint, PacketFlowRole::Handoff],
        PacketTaskClassDto::BugLocalization
        | PacketTaskClassDto::ChangeImpact
        | PacketTaskClassDto::SymbolOwnership
        | PacketTaskClassDto::EditPlanning => &[],
    }
}

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
) -> HashSet<PacketFlowRole> {
    let mut roles = HashSet::new();
    let lower = claim.claim.to_ascii_lowercase();
    let normalized = normalize_identifier(&claim.claim);

    if site_build_flow {
        if normalized.contains("buildprocess")
            && contains_any(&normalized, &["constructs", "processes"])
            && normalized.contains("site")
        {
            roles.insert(PacketFlowRole::EntryPoint);
        }
        if normalized.contains("siteprocess")
            && contains_any(&normalized, &["read", "generate", "render", "write"])
        {
            roles.insert(PacketFlowRole::Handoff);
        }
        if (normalized.contains("reader") && normalized.contains("read"))
            || (normalized.contains("renderer") && normalized.contains("render"))
            || (normalized.contains("sitewrite") || normalized.contains("writephases"))
        {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
    }

    if mapper_flow {
        if normalized.contains("mappermap") && normalized.contains("entrypoint") {
            roles.insert(PacketFlowRole::EntryPoint);
        }
        if normalized.contains("mapperconfiguration")
            && (normalized.contains("configuration") || normalized.contains("runtime"))
        {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
        if (normalized.contains("typemap") && normalized.contains("plan"))
            || normalized.contains("mappingplanbuilder")
            || normalized.contains("planbuilder")
            || normalized.contains("executionpipeline")
        {
            roles.insert(PacketFlowRole::Handoff);
        }
        if normalized.contains("expressionplans") || normalized.contains("mappingconfiguration") {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
    }

    if shell_install_dispatch_flow {
        if normalized.contains("installsh")
            && (normalized.contains("bootstrap") || normalized.contains("sourced"))
        {
            roles.insert(PacketFlowRole::EntryPoint);
        }
        if normalized.contains("dispatcher")
            || normalized.contains("dispatch")
            || normalized.contains("nvminstallnode")
            || normalized.contains("nvmdownload")
        {
            roles.insert(PacketFlowRole::Handoff);
        }
        if normalized.contains("bashcompletion")
            || normalized.contains("completion")
            || normalized.contains("currentversion")
            || normalized.contains("alreadyactive")
            || normalized.contains("configurednodeversion")
        {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
    }

    if url_session_request_flow {
        if normalized.contains("sessionrequest")
            && (normalized.contains("creates") || normalized.contains("requestobjects"))
        {
            roles.insert(PacketFlowRole::EntryPoint);
        }
        if normalized.contains("requestresume")
            || normalized.contains("resumes")
            || normalized.contains("urlsessiontask")
            || normalized.contains("eagerexecution")
        {
            roles.insert(PacketFlowRole::Handoff);
        }
        if normalized.contains("validation")
            || normalized.contains("requestvalidation")
            || (normalized.contains("request") && normalized.contains("validate"))
            || normalized.contains("delegatecallback")
            || normalized.contains("delegatecallbacks")
            || normalized.contains("callback")
            || normalized.contains("callbacks")
        {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
        if normalized.contains("delegatecallback")
            || normalized.contains("delegatecallbacks")
            || normalized.contains("urlsessioncallback")
            || normalized.contains("urlsessioncallbacks")
            || (normalized.contains("delegate") && normalized.contains("callback"))
        {
            roles.insert(PacketFlowRole::Handoff);
        }
    }

    if form_validation_flow {
        if (normalized.contains("native")
            || normalized.contains("constraint")
            || normalized.contains("constraints")
            || normalized.contains("formvalidationexamples"))
            && contains_any(&normalized, &["required", "pattern", "min", "max"])
        {
            roles.insert(PacketFlowRole::EntryPoint);
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
        if normalized.contains("custom")
            && normalized.contains("validation")
            && contains_any(&normalized, &["browser", "defaultui", "ui"])
        {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
        if normalized.contains("submit")
            && contains_any(
                &normalized,
                &["prevent", "prevents", "submission", "invalid"],
            )
        {
            roles.insert(PacketFlowRole::Handoff);
            roles.insert(PacketFlowRole::BoundaryOrState);
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
            roles.insert(PacketFlowRole::Handoff);
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
    }

    if server_request_dispatch_flow {
        if contains_all(&normalized, &["wsgi", "app"]) && normalized.contains("entrypoint") {
            roles.insert(PacketFlowRole::EntryPoint);
        }
        if contains_all(&normalized, &["full", "dispatch", "request"])
            && contains_any(&normalized, &["finalization", "finalize"])
            && contains_any(&normalized, &["preprocess", "exception", "wrap"])
        {
            roles.insert(PacketFlowRole::Handoff);
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
        if contains_all(&normalized, &["dispatch", "request", "view", "function"])
            && !normalized.contains("full")
        {
            roles.insert(PacketFlowRole::Handoff);
        }
        if normalized.contains("routedecorator") && normalized.contains("registersviewfunctions") {
            roles.insert(PacketFlowRole::EntryPoint);
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
    }

    if html_css_template_structure_flow {
        if normalized.contains("appshell")
            && (normalized.contains("divapp") || normalized.contains("modulescript"))
        {
            roles.insert(PacketFlowRole::EntryPoint);
            roles.insert(PacketFlowRole::Handoff);
        }
        if normalized.contains("roottypography")
            || normalized.contains("colorscheme")
            || normalized.contains("bodylayout")
        {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
        if normalized.contains("appconstrains")
            || (normalized.contains("mountedapplication") && normalized.contains("padding"))
        {
            roles.insert(PacketFlowRole::Handoff);
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
        if normalized.contains("logo")
            && normalized.contains("button")
            && contains_any(&normalized, &["hover", "focus", "transition"])
        {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
        if normalized.contains("preferscolorschemelight") || normalized.contains("mediaquery") {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
    }

    if stylesheet_animation_flow {
        if normalized.contains("animationstylesheetentrypoint")
            || (normalized.contains("imports") && normalized.contains("animationfiles"))
            || normalized.contains("baseclass")
        {
            roles.insert(PacketFlowRole::EntryPoint);
        }
        if normalized.contains("imports")
            || normalized.contains("animationname")
            || normalized.contains("matchingkeyframes")
        {
            roles.insert(PacketFlowRole::Handoff);
        }
        if normalized.contains("customproperties")
            || normalized.contains("duration")
            || normalized.contains("delay")
            || normalized.contains("repeat")
            || normalized.contains("keyframes")
        {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
    }

    if sql_schema_flow {
        if normalized.contains("sqlschema")
            && (normalized.contains("definestables")
                || normalized.contains("tables")
                || normalized.contains("createtable"))
        {
            roles.insert(PacketFlowRole::EntryPoint);
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
        if normalized.contains("rowsreference")
            || normalized.contains("foreignkey")
            || (normalized.contains("reference") && normalized.contains("rows"))
        {
            roles.insert(PacketFlowRole::Handoff);
        }
        if normalized.contains("sqldialect")
            || normalized.contains("schemascripts")
            || normalized.contains("dialectscripts")
        {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
    }

    if runtime_formatting_flow {
        if normalized.contains("typeerased")
            && (normalized.contains("formatargs")
                || normalized.contains("formatarguments")
                || normalized.contains("formattingarguments"))
        {
            roles.insert(PacketFlowRole::EntryPoint);
            roles.insert(PacketFlowRole::Handoff);
        }
        if normalized.contains("formatto")
            && (normalized.contains("outputiterator")
                || normalized.contains("formattedoutput")
                || normalized.contains("output"))
        {
            roles.insert(PacketFlowRole::EntryPoint);
            roles.insert(PacketFlowRole::Handoff);
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
        if normalized.contains("buffer") && normalized.contains("append") {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
        if normalized.contains("formaterror")
            || normalized.contains("formattingfailures")
            || normalized.contains("systemerrors")
        {
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
    }

    if string_predicate_flow {
        if (normalized.contains("string") && normalized.contains("utils"))
            || normalized.contains("strings")
            || (normalized.contains("charsequence") && normalized.contains("utils"))
        {
            roles.insert(PacketFlowRole::EntryPoint);
        }
        if normalized.contains("delegates") || normalized.contains("regionmatches") {
            roles.insert(PacketFlowRole::Handoff);
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
            roles.insert(PacketFlowRole::BoundaryOrState);
        }
    }

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
        roles.insert(PacketFlowRole::EntryPoint);
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
        roles.insert(PacketFlowRole::Handoff);
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
        roles.insert(PacketFlowRole::BoundaryOrState);
    }

    for citation in &claim.citations {
        match packet_evidence_role(citation) {
            Some(PacketEvidenceRole::CommandEntrypoint)
            | Some(PacketEvidenceRole::ClientFactory)
            | Some(PacketEvidenceRole::SearchDriver)
            | Some(PacketEvidenceRole::RouteHandling)
            | Some(PacketEvidenceRole::CollectionConfiguration)
            | Some(PacketEvidenceRole::AppServerRequestProtocol) => {
                roles.insert(PacketFlowRole::EntryPoint);
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
                roles.insert(PacketFlowRole::Handoff);
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
                roles.insert(PacketFlowRole::BoundaryOrState);
            }
            _ => {}
        }

        if sql_schema_flow {
            match packet_evidence_role(citation) {
                Some(PacketEvidenceRole::SqlTableDefinition) => {
                    roles.insert(PacketFlowRole::EntryPoint);
                    roles.insert(PacketFlowRole::BoundaryOrState);
                }
                Some(PacketEvidenceRole::SqlRelationshipConstraint) => {
                    roles.insert(PacketFlowRole::Handoff);
                }
                Some(PacketEvidenceRole::SqlSchemaFile) => {
                    roles.insert(PacketFlowRole::BoundaryOrState);
                }
                _ => {}
            }
        }
    }

    roles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(text: &str) -> PacketClaimDto {
        PacketClaimDto {
            claim: text.to_string(),
            citations: Vec::new(),
        }
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
            "Trace how route registration reaches request handler dispatch through a router.",
            PacketTaskClassDto::RouteTracing,
            &claims,
        );
        assert!(
            !route_missing.is_empty(),
            "ordinary route tracing should still require route-flow roles"
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
                "The route decorator registers view functions through the scaffold URL rule path rather than performing request dispatch itself.",
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
    fn architecture_html_css_template_prompts_use_structural_roles() {
        let claims = vec![
            claim(
                "home.html provides the app shell with viewport metadata, div#app, and a script[type=\"module\"] module script entry.",
            ),
            claim(
                "main.css owns :root typography, color-scheme, smoothing, and body layout defaults.",
            ),
            claim("#app constrains the mounted application content and centers it with padding."),
            claim("Logo and button selectors define hover, focus, and transition behavior in CSS."),
            claim(
                "The @media (prefers-color-scheme: light) media query overrides root, link-hover, and button colors.",
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
    fn data_flow_mapper_plan_prompts_use_mapping_flow_roles() {
        let claims = vec![
            claim("Mapper.Map is the public runtime entry point for object mapping."),
            claim("MapperConfiguration builds and owns mapping configuration used at runtime."),
            claim("TypeMap contributes mapper lambda plans used by the execution pipeline."),
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
            claim("SQL schema defines tables Artist, Album, Track, Invoice, and InvoiceLine."),
            claim("Track rows reference Album, Genre, and MediaType rows."),
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
    fn architecture_runtime_formatting_prompts_use_argument_output_error_roles() {
        let claims = vec![
            claim(
                "Runtime formatting uses type-erased format arguments before dispatching formatted output helpers.",
            ),
            claim("format_to writes formatted output through an output iterator."),
            claim(
                "Runtime formatting failures use format_error, which represents the failure type.",
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
