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
use crate::agent::packet_scoring::{normalize_identifier, packet_display_path};
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
    AgentAnswerDto, AgentCitationDto, AgentRetrievalStepStatusDto, PacketBudgetDto,
    PacketBudgetModeDto, PacketClaimDto, PacketCoverageReportDto, PacketEvidenceResolutionDto,
    PacketEvidenceTierDto, PacketSidecarQueryDiagnosticDto, PacketSufficiencyDto,
    PacketSufficiencyStatusDto, PacketTaskClassDto,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};
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

    decorate_packet_claims_proof_metadata(&mut supported_claims);

    let has_errors = answer
        .retrieval_trace
        .steps
        .iter()
        .any(|step| step.status == AgentRetrievalStepStatusDto::Error);
    let min_citations = packet_sufficiency_min_citations(task_class);
    let min_claims = packet_sufficiency_min_claims(task_class);
    let flow_context = PacketFlowContext::new(question, task_class);
    let sufficiency_claims = supported_claims
        .iter()
        .filter(|claim| packet_claim_can_satisfy_sufficiency_in_context(claim, &flow_context))
        .cloned()
        .collect::<Vec<_>>();
    let generic_navigation_claim_count = supported_claims
        .iter()
        .filter(|claim| {
            packet_claim_is_generic_navigation_or_source_evidence(claim)
                && !flow_context.claim_carries_required_role(claim, false)
        })
        .count();
    let has_minimum_coverage = answer.citations.len() >= min_citations;
    let has_minimum_claims = sufficiency_claims.len() >= min_claims;
    let claim_family_count = packet_supported_claim_family_count(&sufficiency_claims);
    let has_minimum_claim_families =
        packet_has_minimum_claim_family_coverage(task_class, &sufficiency_claims);
    let missing_required_flow_requirements =
        packet_missing_required_flow_requirements(question, task_class, &sufficiency_claims);
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
        has_sufficiency_blocking_budget_omission,
        &blocking_missing_probe_queries,
        &missing_required_flow_requirements,
        &blocking_unresolved_sidecar_queries,
    );
    let blocking_follow_up_probe_queries = packet_blocking_follow_up_probe_queries(
        &blocking_missing_probe_queries,
        &blocking_unresolved_sidecar_queries,
    );
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
    let coverage_report = packet_coverage_report(
        &supported_claims,
        &sufficiency_claims,
        &flow_context,
        &missing_required_flow_requirements,
        &unresolved_sidecar_queries,
        budget,
        has_sufficiency_blocking_budget_omission,
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

fn packet_role_label_is_generic_source_evidence(role: &str) -> bool {
    normalize_identifier(role) == "sourceevidence"
}

fn packet_coverage_report(
    supported_claims: &[PacketClaimDto],
    sufficiency_claims: &[PacketClaimDto],
    flow_context: &PacketFlowContext,
    missing_required_flow_requirements: &[FlowRequirement],
    unresolved_sidecar_queries: &[String],
    budget: &PacketBudgetDto,
    has_sufficiency_blocking_budget_omission: bool,
) -> PacketCoverageReportDto {
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
    let missing = missing_required_flow_requirements
        .iter()
        .map(|requirement| requirement.id.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
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
    use codestory_contracts::api::{
        AgentAnswerDto, AgentCitationDto, AgentResponseBlockDto, AgentResponseSectionDto,
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalTraceDto, NodeId,
        NodeKind, PacketBudgetDto, PacketBudgetLimitsDto, PacketBudgetUsageDto,
        PacketEvidenceResolutionDto, PacketEvidenceTierDto, PacketSidecarQueryDiagnosticDto,
        RetrievalScoreBreakdownDto, RetrievalShadowDto, SearchHitOrigin,
    };
    use std::path::Path;

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
    fn github_actions_exact_source_does_not_satisfy_semantic_packet_proof() {
        let claim = cited_claim(
            "The CI workflow build job runs the test command.",
            Some("command dispatch"),
            cited_anchor_with_tier(
                "build",
                ".github/workflows/ci.yml",
                PacketEvidenceTierDto::ExactSource,
                None,
            ),
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
        let question = "Trace how a Session creates requests, resumes tasks, validates data requests, and receives URLSession callbacks.";
        let answer = answer_fixture(question);
        let budget = compact_truncated_budget(question, vec!["markdown_blocks", "trail_edges"]);
        let claims = vec![
            claim("Session.request creates request objects before optional eager execution."),
            claim("Request.resume resumes the underlying URLSession task."),
            claim("Request validation methods attach validation behavior."),
            claim("Session delegate callbacks receive URLSession task events."),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/project"),
            question,
            task_class: PacketTaskClassDto::RouteTracing,
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
        let question = "Trace how a generic HTTP service receives a request, registers a route, dispatches to a handler, finalizes the response, and returns control to the server.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let claims = vec![
            claim(
                "Public request entrypoint registers route wrappers before dispatching handler calls.",
            ),
            claim(
                "Dispatch request invokes the selected view function or handler for the matched route.",
            ),
            claim(
                "Response finalization boundary writes response output and returns control to the server.",
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/synthetic-service"),
            question,
            task_class: PacketTaskClassDto::RouteTracing,
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
        assert!(
            report.missing.is_empty(),
            "generic source-shape role coverage should satisfy request dispatch without product-specific strings: {report:?}"
        );
        for expected in ["public api/export", "server view dispatch"] {
            assert!(
                report.covered.iter().any(|entry| entry == expected),
                "expected generic coverage report to include {expected}: {report:?}"
            );
        }
    }

    #[test]
    fn role_safe_sufficiency_requires_cited_requested_interceptor_evidence() {
        let question = "Trace how a client creates a request, runs interceptors, dispatches it, and sends it through a transport.";
        let answer = answer_fixture(question);
        let budget = budget_fixture();
        let mut claims = vec![
            claim("The public client entrypoint creates a request before dispatch."),
            claim("Request dispatch transforms config and invokes the selected handler."),
            claim("The transport boundary sends the request and returns a response."),
        ];

        let assemble = |supported_claims| {
            assemble_packet_sufficiency(PacketSufficiencyInput {
                project_root: Path::new("C:/workspace/generic-client"),
                question,
                task_class: PacketTaskClassDto::RouteTracing,
                answer: &answer,
                budget: &budget,
                supported_claims,
                missing_required_probe_queries: Vec::new(),
                targeted_follow_up_queries: Vec::new(),
            })
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
                    .any(|gap| gap == "request_interceptor_management")),
            "an explicitly requested role must remain missing without compatible cited evidence: {missing_role:?}"
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

        let mut interceptor_registry = cited_anchor("InterceptorRegistry");
        interceptor_registry.kind = NodeKind::CLASS;
        claims.push(cited_claim(
            "InterceptorRegistry stores request interceptor handler pairs for chained execution.",
            Some("interceptor management"),
            interceptor_registry,
            Some(true),
        ));
        let complete = assemble(claims);
        assert_eq!(complete.status, PacketSufficiencyStatusDto::Sufficient);
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
        let question = "Trace how Express creates an application, registers middleware/routes, and handles an incoming request through the router and response helpers.";
        let mut answer = answer_fixture(question);
        answer.retrieval_trace.packet_sidecar_diagnostics = vec![
            unresolved_sidecar_diagnostic("response send helper"),
            unresolved_sidecar_diagnostic("helpers"),
        ];
        let budget = budget_fixture();
        let claims = vec![
            claim(
                "Public request entrypoint registers route wrappers before dispatching handler calls.",
            ),
            claim(
                "Dispatch request invokes the selected view function or handler for the matched route.",
            ),
            claim(
                "Response finalization boundary writes response output and returns control to the server.",
            ),
        ];

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/express"),
            question,
            task_class: PacketTaskClassDto::RouteTracing,
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

        let sufficiency = assemble_packet_sufficiency(PacketSufficiencyInput {
            project_root: Path::new("C:/workspace/express"),
            question,
            task_class: PacketTaskClassDto::RouteTracing,
            answer: &answer,
            budget: &budget,
            supported_claims: claims,
            missing_required_probe_queries: vec!["response send".to_string()],
            targeted_follow_up_queries: Vec::new(),
        });

        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        let report = sufficiency.coverage_report.as_ref().unwrap();
        assert!(
            report.missing.is_empty(),
            "Express route probes may be selected even without canonical flow requirements: {report:?}"
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
                .is_some_and(|command| command.contains("ready --goal agent --repair")),
            "blocked full retrieval should lead with canonical sidecar repair: {sufficiency:?}"
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
                .is_some_and(|command| command.contains("ready --goal agent --repair")),
            "missing retrieval shadow should lead with canonical sidecar repair: {sufficiency:?}"
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
                .any(|command| command.contains("ready --goal agent --repair")),
            "blocked insufficient packet should recommend canonical sidecar repair: {sufficiency:?}"
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
                let mut commands = vec![packet_agent_repair_command(project.as_str())];
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
                    packet_agent_repair_command(project.as_str()),
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

fn packet_agent_repair_command(quoted_project: &str) -> String {
    format!("codestory-cli ready --goal agent --repair --project {quoted_project} --format json")
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
