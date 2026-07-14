#[cfg(test)]
use crate::agent::eval_probes::{
    eval_citation_shaped_claim, eval_flow_template_claims, eval_probes_enabled,
    eval_supporting_claim_flow_sentence,
};
use crate::agent::packet_citations::packet_citation_source_text;
use crate::agent::packet_claim_profiles::{
    packet_source_derived_claim_for_role, packet_source_derived_claims_for_citation,
};
use crate::agent::packet_command_profiles::packet_append_command_flow_template_claims;
use crate::agent::packet_evidence::{
    citation_sufficiency_eligible, evidence_resolution_for_citation, evidence_tier_for_citation,
};
use crate::agent::packet_evidence_roles::{
    PacketEvidenceRole, packet_claim_key_for_citation, packet_evidence_role,
};
use crate::agent::packet_plan::packet_rank_terms;
use crate::agent::packet_scoring::{
    normalize_identifier, packet_adjacent_query_stop_term, packet_claim_carry_rank,
    packet_display_path, packet_query_stop_term,
};
use crate::agent::packet_terms::{packet_probe_terms, packet_terms_indicate_sql_schema_flow};
use crate::query_mentions_non_primary_source;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, PacketClaimDto, PacketEvidenceResolutionDto,
    PacketEvidenceTierDto, PacketProofStatusDto,
};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt::Write as _;

const PACKET_SOURCE_DEFINITION_CLAIM_LIMIT: usize = 6;

pub(crate) fn packet_flow_claims_markdown(claims: &[PacketClaimDto]) -> String {
    let mut markdown = String::new();
    markdown.push_str("Supported claims for a compact agent answer:\n");
    for claim in claims {
        let citation = claim.citations.first();
        let suffix = citation
            .and_then(|citation| citation.file_path.as_deref())
            .map(packet_display_path)
            .map(|path| format!(" (`{path}`)"))
            .unwrap_or_default();
        let _ = writeln!(markdown, "- {}{}", claim.claim, suffix);
    }
    markdown
}

pub(crate) fn packet_supported_claims(answer: &AgentAnswerDto) -> Vec<PacketClaimDto> {
    let mut claims = Vec::new();
    let mut seen_claims = HashSet::new();
    let rank_terms = packet_rank_terms(&answer.prompt);
    let prefer_primary_sources = !query_mentions_non_primary_source(&answer.prompt);
    let citations = answer.citations.clone();

    append_flow_template_claims(&answer.prompt, &citations, &mut claims, &mut seen_claims);
    append_ranked_citation_claims(
        &answer.prompt,
        &citations,
        &rank_terms,
        prefer_primary_sources,
        &mut claims,
        &mut seen_claims,
    );
    decorate_packet_claims_proof_metadata(&mut claims);
    claims
}

pub(crate) fn decorate_packet_claims_proof_metadata(claims: &mut [PacketClaimDto]) {
    for claim in claims {
        decorate_packet_claim_proof_metadata(claim);
    }
}

fn decorate_packet_claim_proof_metadata(claim: &mut PacketClaimDto) {
    let proven_tier = claim
        .citations
        .iter()
        .find(|citation| citation_sufficiency_eligible(citation))
        .map(evidence_tier_for_citation);
    claim.required_evidence_role = Some(proven_tier.unwrap_or(PacketEvidenceTierDto::ExactSource));
    claim.proof_status = Some(packet_claim_proof_status(claim, proven_tier.is_some()));
}

fn packet_claim_proof_status(
    claim: &PacketClaimDto,
    has_proof_bearing_citation: bool,
) -> PacketProofStatusDto {
    if claim.citations.is_empty() {
        return PacketProofStatusDto::Unsupported;
    }
    if has_proof_bearing_citation && claim.eligible_for_sufficiency != Some(false) {
        return PacketProofStatusDto::Proven;
    }
    if claim
        .citations
        .iter()
        .all(packet_citation_is_diagnostic_only)
    {
        return PacketProofStatusDto::Diagnostic;
    }
    PacketProofStatusDto::Likely
}

fn packet_citation_is_diagnostic_only(citation: &AgentCitationDto) -> bool {
    if citation.eligible_for_sufficiency == Some(false) {
        return true;
    }
    matches!(
        evidence_tier_for_citation(citation),
        PacketEvidenceTierDto::DenseSemantic
            | PacketEvidenceTierDto::GeneratedSummary
            | PacketEvidenceTierDto::SyntheticSourceScan
    ) || matches!(
        evidence_resolution_for_citation(citation),
        PacketEvidenceResolutionDto::DiagnosticOnly
    )
}

pub(crate) fn append_flow_template_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    let normalized_prompt = normalize_identifier(prompt);

    packet_append_command_flow_template_claims(prompt, citations, claims, seen);
    packet_append_event_output_flow_template_claims(&normalized_prompt, citations, claims, seen);
    packet_append_indexing_pipeline_flow_template_claims(prompt, citations, claims, seen);
    packet_append_source_derived_flow_claims(prompt, citations, claims, seen);
    packet_append_sql_schema_file_claims(prompt, citations, claims, seen);
    #[cfg(test)]
    if eval_probes_enabled() {
        packet_append_indexing_storage_flow_template_claims(prompt, citations, claims, seen);
        for (claim, citation) in eval_flow_template_claims(&normalized_prompt, citations) {
            packet_push_flow_template_claim(claims, seen, &claim, Some(citation));
        }
    }
}

fn packet_append_event_output_flow_template_claims(
    normalized_prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    if (normalized_prompt.contains("json") || normalized_prompt.contains("jsonl"))
        && (normalized_prompt.contains("event") || normalized_prompt.contains("output"))
        && let Some(json_output_citation) = citations.iter().find(|citation| {
            packet_evidence_role(citation) == Some(PacketEvidenceRole::EventOutputProcessing)
        })
    {
        packet_push_flow_template_claim(
            claims,
            seen,
            "Event-output processing evidence describes how structured runtime events are serialized for JSON/JSONL output.",
            Some(json_output_citation.clone()),
        );
    }
}

fn packet_append_indexing_pipeline_flow_template_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    let normalized_prompt = normalize_identifier(prompt);
    let indexing_prompt = normalized_prompt.contains("indexing")
        || normalized_prompt.contains("indexed")
        || normalized_prompt.contains("indexer")
        || normalized_prompt.contains("indexcommand");
    if !(indexing_prompt
        && normalized_prompt.contains("runtime")
        && (normalized_prompt.contains("workspace")
            || normalized_prompt.contains("sourcefile")
            || normalized_prompt.contains("filediscovery"))
        && (normalized_prompt.contains("persistence") || normalized_prompt.contains("store"))
        && normalized_prompt.contains("snapshot"))
    {
        return;
    }

    let cli_entry = packet_citation_matching_role(citations, PacketEvidenceRole::CommandEntrypoint);
    let runtime_entry =
        packet_citation_matching_role(citations, PacketEvidenceRole::RuntimeOrchestration);
    if let Some(runtime_entry) = &runtime_entry {
        let mut claim_citations = Vec::new();
        if let Some(cli_entry) = cli_entry {
            claim_citations.push(cli_entry.clone());
        }
        claim_citations.push(runtime_entry.clone());
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "Indexing entrypoint evidence delegates indexing work into the runtime orchestration layer.",
            claim_citations,
        );
    }

    let workspace_plan =
        packet_citation_matching_role(citations, PacketEvidenceRole::WorkspaceDiscoveryAndPlanning);
    if let Some(runtime_entry) = &runtime_entry {
        let mut claim_citations = vec![runtime_entry.clone()];
        if let Some(workspace_plan) = &workspace_plan {
            claim_citations.push(workspace_plan.clone());
        }
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "Runtime orchestration evidence opens workspace/store state and coordinates refresh phases.",
            claim_citations,
        );
    }

    if let Some(workspace_plan) = &workspace_plan {
        packet_push_flow_template_claim(
            claims,
            seen,
            "Workspace discovery evidence plans source-file discovery and refresh work.",
            Some(workspace_plan.clone()),
        );
    }

    let workspace_indexer =
        packet_citation_matching_role(citations, PacketEvidenceRole::IndexingWorkQueue);
    let index_file = packet_citation_matching_role(citations, PacketEvidenceRole::SymbolExtraction);
    if workspace_indexer.is_some() || index_file.is_some() {
        let mut claim_citations = Vec::new();
        if let Some(workspace_indexer) = workspace_indexer {
            claim_citations.push(workspace_indexer.clone());
        }
        if let Some(index_file) = index_file {
            claim_citations.push(index_file.clone());
        }
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "Symbol extraction evidence builds graph nodes, edges, occurrences, and related source data.",
            claim_citations,
        );
    }

    let storage_flush = packet_citation_matching_role(
        citations,
        PacketEvidenceRole::PersistenceAndSearchProjection,
    );
    let search_projection = storage_flush.clone();
    if storage_flush.is_some() || search_projection.is_some() {
        let mut claim_citations = Vec::new();
        if let Some(storage_flush) = storage_flush {
            claim_citations.push(storage_flush.clone());
        }
        if let Some(search_projection) = search_projection {
            claim_citations.push(search_projection.clone());
        }
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "Persistence evidence stores graph/file data and rebuilds query/search projections.",
            claim_citations,
        );
    }

    if let Some(snapshot_refresh) =
        packet_citation_matching_role(citations, PacketEvidenceRole::SnapshotRefresh)
    {
        packet_push_flow_template_claim(
            claims,
            seen,
            "Snapshot refresh evidence updates read models after persisted graph changes.",
            Some(snapshot_refresh.clone()),
        );
    }
}

fn packet_citation_matching_role(
    citations: &[AgentCitationDto],
    role: PacketEvidenceRole,
) -> Option<AgentCitationDto> {
    citations
        .iter()
        .find(|citation| packet_evidence_role(citation) == Some(role))
        .cloned()
}

fn packet_append_source_derived_flow_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    for citation in citations.iter().take(24) {
        let source = match packet_citation_source_text(citation) {
            Some(source) if source.len() <= 800_000 => source,
            _ => continue,
        };
        for claim in packet_source_derived_claims_for_citation(prompt, citation, &source) {
            let claim_citation =
                packet_preferred_source_derived_claim_citation(&claim, citation, citations);
            packet_push_flow_template_claim(claims, seen, &claim, Some(claim_citation));
            if claims.len() >= 18 {
                return;
            }
        }
    }
}

fn packet_preferred_source_derived_claim_citation(
    claim: &str,
    source_citation: &AgentCitationDto,
    citations: &[AgentCitationDto],
) -> AgentCitationDto {
    if packet_claim_text_indicates_sql_relationship(claim)
        && let Some(relationship_citation) =
            packet_matching_sql_relationship_citation(source_citation, citations)
    {
        return relationship_citation;
    }
    source_citation.clone()
}

fn packet_matching_sql_relationship_citation(
    source_citation: &AgentCitationDto,
    citations: &[AgentCitationDto],
) -> Option<AgentCitationDto> {
    let source_path = source_citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .map(|path| normalize_identifier(&path));
    citations
        .iter()
        .filter(|citation| {
            packet_evidence_role(citation) == Some(PacketEvidenceRole::SqlRelationshipConstraint)
        })
        .find(|citation| {
            source_path.as_deref().is_some_and(|source_path| {
                citation
                    .file_path
                    .as_deref()
                    .map(packet_display_path)
                    .map(|path| normalize_identifier(&path) == source_path)
                    .unwrap_or(false)
            })
        })
        .or_else(|| {
            citations.iter().find(|citation| {
                packet_evidence_role(citation)
                    == Some(PacketEvidenceRole::SqlRelationshipConstraint)
            })
        })
        .cloned()
}

fn packet_claim_text_indicates_sql_relationship(claim: &str) -> bool {
    let normalized = normalize_identifier(claim);
    normalized.contains("rowsreference")
        || normalized.contains("foreignkey")
        || normalized.contains("references")
        || ((normalized.contains("relationship")
            || normalized.contains("relationships")
            || normalized.contains("constraint")
            || normalized.contains("constraints"))
            && (normalized.contains("sql")
                || normalized.contains("schema")
                || normalized.contains("table")
                || normalized.contains("rows")
                || normalized.contains("foreign")
                || normalized.contains("reference")
                || normalized.contains("referential")))
}

fn packet_append_sql_schema_file_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    let terms = packet_probe_terms(prompt);
    if !packet_terms_indicate_sql_schema_flow(&terms) {
        return;
    }

    let mut sql_schema_citations = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut dialects = HashSet::new();
    for citation in citations {
        let Some(path) = citation.file_path.as_deref() else {
            continue;
        };
        let display_path = packet_display_path(path);
        if !display_path.to_ascii_lowercase().ends_with(".sql") {
            continue;
        }
        let normalized_path = display_path.to_ascii_lowercase();
        if !seen_paths.insert(normalized_path.clone()) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        if !source.to_ascii_lowercase().contains("create table") {
            continue;
        }
        if let Some(dialect) = packet_sql_dialect_key(&normalized_path) {
            dialects.insert(dialect);
        }
        sql_schema_citations.push(citation.clone());
    }

    if sql_schema_citations.len() < 2 {
        return;
    }

    let subject = packet_sql_schema_prompt_subject(prompt);
    let claim = match (dialects.len() >= 2, subject.as_deref()) {
        (true, Some(subject)) => {
            format!(
                "The repository carries multiple SQL dialect scripts for the same {subject} schema."
            )
        }
        (true, None) => {
            "The repository carries multiple SQL dialect scripts for the same schema.".to_string()
        }
        (false, Some(subject)) => {
            format!(
                "The repository carries multiple SQL schema scripts for the same {subject} schema."
            )
        }
        (false, None) => {
            "The repository carries multiple SQL schema scripts for the same schema.".to_string()
        }
    };
    packet_push_flow_template_claim_with_citations(
        claims,
        seen,
        &claim,
        sql_schema_citations.into_iter().take(3).collect(),
    );
}

fn packet_sql_dialect_key(normalized_path: &str) -> Option<&'static str> {
    if normalized_path.contains("sqlite") {
        Some("sqlite")
    } else if normalized_path.contains("mysql") {
        Some("mysql")
    } else if normalized_path.contains("postgres") || normalized_path.contains("pgsql") {
        Some("postgres")
    } else if normalized_path.contains("sqlserver") || normalized_path.contains("mssql") {
        Some("sqlserver")
    } else if normalized_path.contains("db2") {
        Some("db2")
    } else if normalized_path.contains("oracle") {
        Some("oracle")
    } else {
        None
    }
}

fn packet_sql_schema_prompt_subject(prompt: &str) -> Option<String> {
    let stop_words = [
        "Explain",
        "Trace",
        "Cite",
        "Name",
        "SQL",
        "Schema",
        "Relationships",
        "Relation",
        "Tables",
        "Table",
    ];
    prompt
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .map(str::trim)
        .find(|token| {
            token.len() >= 4
                && token
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase())
                && !stop_words
                    .iter()
                    .any(|stop| stop.eq_ignore_ascii_case(token))
        })
        .map(str::to_string)
}

#[cfg(test)]
fn packet_append_indexing_storage_flow_template_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    let normalized_prompt = normalize_identifier(prompt);
    let indexing_prompt = normalized_prompt.contains("indexing")
        || normalized_prompt.contains("indexed")
        || normalized_prompt.contains("indexer");
    let storage_prompt = normalized_prompt.contains("storage")
        || normalized_prompt.contains("persistent")
        || normalized_prompt.contains("sourcegroup")
        || normalized_prompt.contains("sourcegroupconfiguration");
    if !(indexing_prompt && storage_prompt) {
        return;
    }

    let source_group = citations.iter().find(|citation| {
        packet_evidence_role(citation) == Some(PacketEvidenceRole::SourceGroupConfiguration)
    });
    let indexing_work = citations.iter().find(|citation| {
        packet_evidence_role(citation) == Some(PacketEvidenceRole::IndexingWorkQueue)
    });
    if let Some(source_group) = source_group
        && let Some(indexing_work) = indexing_work
    {
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "Source-group configuration and indexing command evidence describe how repository configuration becomes indexing work.",
            vec![source_group.clone(), indexing_work.clone()],
        );
    }

    if let Some(persistence) = citations.iter().find(|citation| {
        packet_evidence_role(citation) == Some(PacketEvidenceRole::PersistenceAndSearchProjection)
    }) {
        packet_push_flow_template_claim(
            claims,
            seen,
            "Persistence/search-projection evidence describes how indexed data remains available to later application reads.",
            Some(persistence.clone()),
        );
    }
}

fn packet_push_flow_template_claim(
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
    claim_text: &str,
    citation: Option<AgentCitationDto>,
) {
    packet_push_flow_template_claim_with_citations(
        claims,
        seen,
        claim_text,
        citation.map(|value| vec![value]).unwrap_or_default(),
    );
}

fn packet_push_flow_template_claim_with_citations(
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
    claim_text: &str,
    citations: Vec<AgentCitationDto>,
) {
    let key = normalize_identifier(claim_text);
    if key.is_empty() || !seen.insert(key) {
        return;
    }
    claims.push(PacketClaimDto {
        claim: claim_text.to_string(),
        proof_status: None,
        required_evidence_role: None,
        citations,
        coverage_role: Some("flow template".to_string()),
        eligible_for_sufficiency: Some(true),
    });
}

pub(crate) fn append_ranked_citation_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    rank_terms: &[String],
    prefer_primary_sources: bool,
    claims: &mut Vec<PacketClaimDto>,
    seen_claims: &mut HashSet<String>,
) {
    let mut ordered_citations = citations.to_vec();
    ordered_citations.sort_by(|left, right| {
        packet_claim_carry_rank(right, rank_terms, prefer_primary_sources)
            .partial_cmp(&packet_claim_carry_rank(
                left,
                rank_terms,
                prefer_primary_sources,
            ))
            .unwrap_or(Ordering::Equal)
    });
    for citation in &ordered_citations {
        if let Some(shaped) = packet_citation_shaped_claim(citation, prompt) {
            let key = normalize_identifier(&shaped);
            if seen_claims.insert(key) {
                claims.push(PacketClaimDto {
                    claim: shaped,
                    proof_status: None,
                    required_evidence_role: None,
                    citations: vec![citation.clone()],
                    coverage_role: citation.coverage_role.clone(),
                    eligible_for_sufficiency: Some(false),
                });
            }
            continue;
        }
        let role = match packet_evidence_role(citation) {
            Some(PacketEvidenceRole::TestsAndRegressionCoverage) => {
                let lower = prompt.to_ascii_lowercase();
                if lower.contains("test")
                    || lower.contains("regression")
                    || lower.contains("edit")
                    || lower.contains("plan")
                {
                    PacketEvidenceRole::TestsAndRegressionCoverage
                } else {
                    continue;
                }
            }
            Some(role) => role,
            None => PacketEvidenceRole::SourceEvidence,
        };
        let claim_key = packet_claim_key_for_citation(role, citation);
        if !seen_claims.insert(claim_key.clone()) {
            continue;
        }
        claims.push(PacketClaimDto {
            claim: packet_claim_for_role(role, citation, prompt, rank_terms),
            proof_status: None,
            required_evidence_role: None,
            citations: vec![citation.clone()],
            coverage_role: Some(role.as_str().to_string()),
            eligible_for_sufficiency: Some(true),
        });
        if claims.len() >= 18 {
            break;
        }
    }
    if claims.len() < 18 {
        packet_append_source_definition_claims(&ordered_citations, rank_terms, claims, seen_claims);
    }
}

pub(crate) fn packet_claim_for_role(
    role: PacketEvidenceRole,
    citation: &AgentCitationDto,
    prompt: &str,
    rank_terms: &[String],
) -> String {
    if let Some(shaped) = packet_citation_shaped_claim(citation, prompt) {
        return shaped;
    }
    if let Some(source_derived) = packet_source_derived_claim_for_role(role, citation, prompt) {
        return source_derived;
    }
    let symbol = citation.display_name.as_str();
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    match role {
        PacketEvidenceRole::CommandEntrypoint => format!(
            "The command or public entrypoint for this flow is `{symbol}`, which starts downstream coordination."
        ),
        PacketEvidenceRole::ClientFactory => {
            format!("`{symbol}` creates client instances or binds request methods for this flow.")
        }
        PacketEvidenceRole::InterceptorManagement => {
            format!("`{symbol}` is interceptor-related evidence for this request flow.")
        }
        PacketEvidenceRole::RequestDispatch => format!(
            "`{symbol}` dispatches requests by transforming config and handing off to an adapter or handler."
        ),
        PacketEvidenceRole::TransportAdapter => format!(
            "`{symbol}` is the transport adapter boundary for environment-specific sending."
        ),
        PacketEvidenceRole::EventLoop => format!(
            "`{symbol}` polls event-loop state and dispatches readable or writable file events."
        ),
        PacketEvidenceRole::NetworkCommandInput => {
            format!("`{symbol}` reads network or socket input into command-buffer processing.")
        }
        PacketEvidenceRole::CommandDispatch => format!(
            "`{symbol}` dispatches commands through lookup, validation, execution, or propagation."
        ),
        PacketEvidenceRole::ArgumentPlanning => format!(
            "`{symbol}` plans arguments by constructing walker, matcher, searcher, or printer behavior."
        ),
        PacketEvidenceRole::SearchDriver => format!(
            "`{symbol}` routes search entrypoint behavior into sequential or parallel execution."
        ),
        PacketEvidenceRole::SearchExecutionUnit => {
            format!("`{symbol}` executes per-candidate matcher, searcher, or printer work.")
        }
        PacketEvidenceRole::RuntimeOrchestration => format!(
            "`{symbol}` coordinates runtime state transitions and downstream service calls."
        ),
        PacketEvidenceRole::WorkspaceDiscoveryAndPlanning => format!(
            "`{symbol}` handles workspace file selection, manifests, or execution-plan behavior."
        ),
        PacketEvidenceRole::SourceGroupConfiguration => {
            format!("`{symbol}` maps project settings into source-group-specific indexing inputs.")
        }
        PacketEvidenceRole::IndexingWorkQueue => format!(
            "`{symbol}` turns build-index commands into parser handoff or source-file work items."
        ),
        PacketEvidenceRole::SymbolExtraction => {
            format!("`{symbol}` extracts nodes, edges, occurrences, or file-level symbol data.")
        }
        PacketEvidenceRole::PersistenceAndSearchProjection => {
            format!("`{symbol}` persists or projects durable graph/search state.")
        }
        PacketEvidenceRole::SnapshotRefresh => {
            format!("`{symbol}` refreshes post-write summaries or cache state.")
        }
        PacketEvidenceRole::RouteHandling => {
            format!("`{symbol}` handles route dispatch or handler ownership for the request path.")
        }
        PacketEvidenceRole::BufferedIo => {
            format!("`{symbol}` connects buffered read/write state with Source or Sink handoff.")
        }
        PacketEvidenceRole::CollectionConfiguration => {
            format!("`{symbol}` defines collection schema fields, hooks, or access rules.")
        }
        PacketEvidenceRole::EventOutputProcessing => {
            format!("`{symbol}` serializes typed runtime events for JSON/event output.")
        }
        PacketEvidenceRole::AppServerRequestProtocol => {
            format!("`{symbol}` defines app-server thread or turn start request protocol shape.")
        }
        PacketEvidenceRole::TestsAndRegressionCoverage => {
            format!("`{symbol}` covers regression behavior for focused verification choices.")
        }
        PacketEvidenceRole::SourceEvidence => {
            let flow_terms = packet_claim_flow_terms(rank_terms, citation);
            let focus = if flow_terms.is_empty() {
                "this flow".to_string()
            } else {
                flow_terms.join(", ")
            };
            format!(
                "`{symbol}` in `{path}` {}.",
                packet_source_evidence_flow_sentence(prompt, &focus)
            )
        }
        PacketEvidenceRole::SqlTableDefinition
        | PacketEvidenceRole::SqlRelationshipConstraint
        | PacketEvidenceRole::SqlSchemaFile
        | PacketEvidenceRole::CandidateFileConstruction => {
            format!("Schema or candidate-file evidence identifies `{symbol}` as part of this flow.")
        }
    }
}

fn packet_source_evidence_flow_sentence(prompt: &str, focus: &str) -> String {
    #[cfg(test)]
    {
        let normalized_prompt = normalize_identifier(prompt);
        if let Some(sentence) = eval_supporting_claim_flow_sentence(&normalized_prompt, focus) {
            return sentence;
        }
    }
    let _ = prompt;
    format!("ties {focus} in this flow to cited definitions and adjacent ownership")
}

fn packet_claim_flow_terms(rank_terms: &[String], citation: &AgentCitationDto) -> Vec<String> {
    let display = normalize_identifier(&citation.display_name);
    let path = normalize_identifier(citation.file_path.as_deref().unwrap_or_default());
    let mut terms = Vec::new();
    for term in rank_terms {
        if term.len() < 4 || packet_query_stop_term(term) || packet_adjacent_query_stop_term(term) {
            continue;
        }
        let normalized = normalize_identifier(term);
        if normalized.is_empty() {
            continue;
        }
        if (display.contains(&normalized) || path.contains(&normalized))
            && terms.iter().all(|existing| existing != &normalized)
        {
            terms.push(normalized);
        }
        if terms.len() >= 4 {
            break;
        }
    }
    terms
}

fn packet_citation_shaped_claim(citation: &AgentCitationDto, prompt: &str) -> Option<String> {
    #[cfg(test)]
    {
        let path = citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .unwrap_or_default();
        eval_citation_shaped_claim(citation, prompt, &path)
    }
    #[cfg(not(test))]
    {
        let _ = (citation, prompt);
        None
    }
}

fn packet_append_source_definition_claims(
    citations: &[AgentCitationDto],
    rank_terms: &[String],
    claims: &mut Vec<PacketClaimDto>,
    seen_claims: &mut HashSet<String>,
) {
    let normalized_terms = rank_terms
        .iter()
        .map(|term| normalize_identifier(term))
        .filter(|term| term.len() >= 6)
        .collect::<Vec<_>>();
    let rank_tokens = packet_definition_rank_tokens(rank_terms);
    if normalized_terms.is_empty() && rank_tokens.is_empty() {
        return;
    }

    let mut seen_definitions = HashSet::new();
    let mut appended = 0;
    for citation in citations.iter().take(24) {
        let Some(source) = packet_citation_source_text(citation) else {
            continue;
        };
        if source.len() > 400_000 {
            continue;
        }
        for line in source.lines().take(4_000) {
            let Some(definition) = packet_source_definition_name(line) else {
                continue;
            };
            let normalized_definition = normalize_identifier(&definition);
            if !packet_definition_matches_rank_terms(
                &definition,
                &normalized_definition,
                &normalized_terms,
                &rank_tokens,
            ) {
                continue;
            }
            let path = citation
                .file_path
                .as_deref()
                .map(packet_display_path)
                .unwrap_or_else(|| "<unknown path>".to_string());
            let definition_key = format!("{normalized_definition}:{path}");
            if !seen_definitions.insert(definition_key) {
                continue;
            }
            packet_push_claim(
                claims,
                seen_claims,
                &format!(
                    "`{definition}` is defined in cited source `{path}` and should be treated as an exact source anchor for this flow."
                ),
                Some(citation.clone()),
            );
            appended += 1;
            if claims.len() >= 18 {
                return;
            }
            if appended >= PACKET_SOURCE_DEFINITION_CLAIM_LIMIT {
                return;
            }
        }
    }
}

fn packet_push_claim(
    claims: &mut Vec<PacketClaimDto>,
    seen_claims: &mut HashSet<String>,
    claim_text: &str,
    citation: Option<AgentCitationDto>,
) {
    let key = normalize_identifier(claim_text);
    if key.is_empty() || !seen_claims.insert(key) {
        return;
    }
    claims.push(PacketClaimDto {
        claim: claim_text.to_string(),
        proof_status: None,
        required_evidence_role: None,
        citations: citation.map(|value| vec![value]).unwrap_or_default(),
        coverage_role: Some("source definition".to_string()),
        eligible_for_sufficiency: Some(false),
    });
}

fn packet_source_definition_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    for prefix in [
        "pub async fn ",
        "pub(crate) async fn ",
        "async fn ",
        "pub fn ",
        "pub(crate) fn ",
        "fn ",
        "pub struct ",
        "pub(crate) struct ",
        "struct ",
        "pub enum ",
        "pub(crate) enum ",
        "enum ",
        "pub trait ",
        "pub(crate) trait ",
        "trait ",
        "export class ",
        "class ",
        "export interface ",
        "interface ",
        "export function ",
        "function ",
        "export const ",
        "const ",
        "export type ",
        "type ",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return packet_take_definition_identifier(rest);
        }
    }
    None
}

fn packet_take_definition_identifier(rest: &str) -> Option<String> {
    let mut identifier = String::new();
    for ch in rest.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
            identifier.push(ch);
        } else {
            break;
        }
    }
    (identifier.len() >= 3).then_some(identifier)
}

fn packet_definition_matches_rank_terms(
    definition: &str,
    normalized_definition: &str,
    normalized_terms: &[String],
    rank_tokens: &HashSet<String>,
) -> bool {
    if normalized_definition.len() < 6 {
        return false;
    }
    if normalized_terms
        .iter()
        .any(|term| term == normalized_definition)
    {
        return true;
    }
    let definition_tokens = packet_identifier_tokens(definition);
    let overlap = definition_tokens
        .iter()
        .filter(|token| rank_tokens.contains(token.as_str()))
        .count();
    overlap >= 2 || (definition_tokens.iter().any(|token| token == "exec") && overlap >= 1)
}

fn packet_definition_rank_tokens(rank_terms: &[String]) -> HashSet<String> {
    rank_terms
        .iter()
        .flat_map(|term| packet_identifier_tokens(term))
        .filter(|term| {
            term.len() >= 3
                && !matches!(
                    term.as_str(),
                    "the" | "and" | "for" | "with" | "from" | "into" | "flow" | "flows"
                )
        })
        .collect()
}

fn packet_identifier_tokens(identifier: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_lower_or_digit = false;
    for ch in identifier.chars() {
        if ch == '_' || ch == '-' || ch == '$' || ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(current.clone());
                current.clear();
            }
            previous_lower_or_digit = false;
            continue;
        }
        if ch.is_ascii_uppercase() && previous_lower_or_digit && !current.is_empty() {
            tokens.push(current.clone());
            current.clear();
        }
        if ch.is_ascii_alphanumeric() {
            current.extend(ch.to_lowercase());
            previous_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else if !current.is_empty() {
            tokens.push(current.clone());
            current.clear();
            previous_lower_or_digit = false;
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalTraceDto, NodeId,
        NodeKind, PacketProofStatusDto, RetrievalScoreBreakdownDto, SearchHitOrigin,
    };

    fn test_answer(prompt: &str, citations: Vec<AgentCitationDto>) -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "packet-claims-test".to_string(),
            prompt: prompt.to_string(),
            summary: "test answer".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations,
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "packet-claims-test".to_string(),
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

    fn test_citation(display_name: &str, file_path: &str, score: f32) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(format!("test::{display_name}")),
            display_name: display_name.to_string(),
            kind: NodeKind::ANNOTATION,
            file_path: Some(file_path.to_string()),
            line: Some(1),
            score,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: score,
                semantic: 0.0,
                graph: 0.0,
                total: score,
                tier_cap: None,
                boosts: Vec::new(),
                dampening: Vec::new(),
                final_rank_reason: None,
                provenance: Vec::new(),
            }),
            evidence_tier: None,
            evidence_producer: Some("test".to_string()),
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
        }
    }

    #[test]
    fn generated_summary_and_dense_claims_need_backing_source_proof() {
        let mut generated = test_citation("generated summary", "target/generated/summary.md", 0.9);
        generated.evidence_tier = Some(PacketEvidenceTierDto::GeneratedSummary);
        generated.resolution_status = Some(PacketEvidenceResolutionDto::DiagnosticOnly);
        generated.eligible_for_sufficiency = None;

        let mut dense = test_citation("dense anchor", "src/runtime.rs", 0.8);
        dense.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
        dense.resolution_status = Some(PacketEvidenceResolutionDto::Resolved);
        dense.eligible_for_sufficiency = None;

        let mut claims = vec![PacketClaimDto {
            claim: "Runtime dispatch is covered.".to_string(),
            proof_status: None,
            required_evidence_role: None,
            citations: vec![generated, dense],
            coverage_role: Some("source evidence".to_string()),
            eligible_for_sufficiency: Some(true),
        }];

        decorate_packet_claims_proof_metadata(&mut claims);

        assert_eq!(
            claims[0].proof_status,
            Some(PacketProofStatusDto::Diagnostic)
        );
        assert_eq!(
            claims[0].required_evidence_role,
            Some(PacketEvidenceTierDto::ExactSource)
        );

        let mut exact_source = test_citation("dispatch", "src/runtime.rs", 1.0);
        exact_source.evidence_tier = Some(PacketEvidenceTierDto::ExactSource);
        exact_source.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);
        exact_source.eligible_for_sufficiency = Some(true);
        claims[0].citations.push(exact_source);

        decorate_packet_claims_proof_metadata(&mut claims);

        assert_eq!(claims[0].proof_status, Some(PacketProofStatusDto::Proven));
        assert_eq!(
            claims[0].required_evidence_role,
            Some(PacketEvidenceTierDto::ExactSource)
        );
    }

    fn write_sql_fixture(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "codestory-packet-claims-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create packet claims temp dir");
        let path = root.join("schema.sql");
        std::fs::write(
            &path,
            r#"
            CREATE TABLE Child
            (
                ChildId INTEGER NOT NULL,
                ParentId INTEGER NOT NULL,
                FOREIGN KEY (ParentId) REFERENCES Parent (ParentId)
            );
            CREATE TABLE Parent
            (
                ParentId INTEGER NOT NULL
            );
            "#,
        )
        .expect("write packet claims sql fixture");
        path
    }

    #[test]
    fn sql_relationship_claims_attach_to_retained_foreign_key_citations() {
        let path = write_sql_fixture("foreign-key");
        let path_text = path.to_string_lossy().to_string();
        let answer = test_answer(
            "Explain SQL schema relationships between child and parent rows.",
            vec![
                test_citation("CREATE TABLE Child", &path_text, 0.9),
                test_citation("FOREIGN KEY", &path_text, 0.8),
            ],
        );

        let claims = packet_supported_claims(&answer);
        let relationship_claim = claims
            .iter()
            .find(|claim| claim.claim == "Child rows reference Parent rows through ParentId.")
            .unwrap_or_else(|| panic!("expected relationship claim in {claims:?}"));
        assert!(
            relationship_claim
                .citations
                .iter()
                .any(|citation| citation.display_name == "FOREIGN KEY"),
            "relationship claim should cite retained FK evidence: {relationship_claim:?}"
        );
        assert!(
            !relationship_claim
                .citations
                .iter()
                .any(|citation| citation.display_name == "CREATE TABLE Child"),
            "relationship claim should not stay attached only to table evidence: {relationship_claim:?}"
        );

        let table_claim = claims
            .iter()
            .find(|claim| claim.claim == "SQL schema defines tables Child and Parent.")
            .unwrap_or_else(|| panic!("expected table claim in {claims:?}"));
        assert!(
            table_claim
                .citations
                .iter()
                .any(|citation| citation.display_name == "CREATE TABLE Child"),
            "table claim should keep table-definition evidence: {table_claim:?}"
        );

        let _ = std::fs::remove_dir_all(path.parent().expect("sql fixture parent"));
    }

    #[test]
    fn sql_relationship_claims_can_attach_to_retained_references_citations() {
        let path = write_sql_fixture("references");
        let path_text = path.to_string_lossy().to_string();
        let answer = test_answer(
            "Explain SQL schema relationships and references between child and parent rows.",
            vec![
                test_citation("CREATE TABLE Child", &path_text, 0.9),
                test_citation("REFERENCES", &path_text, 0.8),
            ],
        );

        let claims = packet_supported_claims(&answer);
        let relationship_claim = claims
            .iter()
            .find(|claim| claim.claim == "Child rows reference Parent rows through ParentId.")
            .unwrap_or_else(|| panic!("expected relationship claim in {claims:?}"));
        assert!(
            relationship_claim
                .citations
                .iter()
                .any(|citation| citation.display_name == "REFERENCES"),
            "relationship claim should cite retained REFERENCES evidence: {relationship_claim:?}"
        );

        let _ = std::fs::remove_dir_all(path.parent().expect("sql fixture parent"));
    }
}
