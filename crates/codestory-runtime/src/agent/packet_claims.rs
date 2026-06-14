use crate::agent::eval_probes::{
    eval_citation_shaped_claim, eval_flow_template_claims, eval_probes_enabled,
    eval_supporting_claim_flow_sentence,
};
use crate::agent::packet_citations::{
    packet_citation_matching_display, packet_citation_matching_display_contains,
    packet_citation_source_text,
};
use crate::agent::packet_claim_profiles::{
    packet_source_derived_claim_for_role, packet_source_derived_claims_for_citation,
};
use crate::agent::packet_command_profiles::packet_append_command_flow_template_claims;
use crate::agent::packet_evidence_roles::{
    PacketEvidenceRole, packet_claim_key_for_citation, packet_evidence_role,
};
use crate::agent::packet_scoring::{
    normalize_identifier, packet_adjacent_query_stop_term, packet_claim_carry_rank,
    packet_display_path, packet_query_stop_term,
};
use crate::agent::packet_terms::{packet_probe_terms, packet_terms_indicate_sql_schema_flow};
use codestory_contracts::api::{AgentCitationDto, PacketClaimDto};
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
    if !eval_probes_enabled() {
        return;
    }
    packet_append_indexing_storage_flow_template_claims(prompt, citations, claims, seen);
    for (claim, citation) in eval_flow_template_claims(&normalized_prompt, citations) {
        packet_push_flow_template_claim(claims, seen, &claim, Some(citation));
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

    let cli_entry = packet_citation_matching_display(citations, "run_index")
        .or_else(|| packet_citation_matching_display(citations, "Command::Index"))
        .or_else(|| packet_citation_matching_display(citations, "IndexCommand"))
        .or_else(|| packet_citation_matching_display(citations, "CliDirection"));
    let runtime_entry =
        packet_citation_matching_display_contains(citations, "IndexService::run_indexing")
            .or_else(|| packet_citation_matching_display(citations, "Runtime::index_service"));
    if let Some(runtime_entry) = runtime_entry {
        let mut claim_citations = Vec::new();
        if let Some(cli_entry) = cli_entry {
            claim_citations.push(cli_entry.clone());
        }
        claim_citations.push(runtime_entry.clone());
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "The CLI index command prepares command options and delegates indexing work into the runtime layer.",
            claim_citations,
        );
    }

    let workspace_plan =
        packet_citation_matching_display(citations, "WorkspaceManifest::build_execution_plan");
    if let Some(runtime_entry) = runtime_entry {
        let mut claim_citations = vec![runtime_entry.clone()];
        if let Some(workspace_plan) = workspace_plan {
            claim_citations.push(workspace_plan.clone());
        }
        packet_push_flow_template_claim_with_citations(
            claims,
            seen,
            "The runtime opens the workspace and store, chooses full or incremental indexing, and coordinates later refresh phases.",
            claim_citations,
        );
    }

    if let Some(workspace_plan) = workspace_plan {
        packet_push_flow_template_claim(
            claims,
            seen,
            "The workspace crate is responsible for source-file discovery and refresh-plan construction.",
            Some(workspace_plan.clone()),
        );
    }

    let workspace_indexer = packet_citation_matching_display(citations, "WorkspaceIndexer::run");
    let index_file = packet_citation_matching_display(citations, "index_file");
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
            "The indexer extracts nodes, edges, occurrences, and related symbol data from source files.",
            claim_citations,
        );
    }

    let storage_flush =
        packet_citation_matching_display(citations, "Storage::flush_projection_batch");
    let search_projection = packet_citation_matching_display(
        citations,
        "Storage::rebuild_search_symbol_projection_from_node_table",
    );
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
            "The store persists graph and file data to SQLite and rebuilds query/search projections from persisted data.",
            claim_citations,
        );
    }

    if let Some(snapshot_refresh) =
        packet_citation_matching_display(citations, "SnapshotStore::refresh_all_with_stats")
    {
        packet_push_flow_template_claim(
            claims,
            seen,
            "Snapshot refresh happens after persisted data changes so later grounding and summary reads see current indexed state.",
            Some(snapshot_refresh.clone()),
        );
    }
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
            packet_push_flow_template_claim(claims, seen, &claim, Some(citation.clone()));
            if claims.len() >= 18 {
                return;
            }
        }
    }
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
        citations,
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
                    citations: vec![citation.clone()],
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
            citations: vec![citation.clone()],
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
            "The command or public entrypoint for this flow is anchored by `{symbol}`; inspect it before following downstream coordination."
        ),
        PacketEvidenceRole::ClientFactory => format!(
            "Client factory behavior is anchored by `{symbol}`; inspect it for instance creation and request-method binding."
        ),
        PacketEvidenceRole::InterceptorManagement => format!(
            "Interceptor management is anchored by `{symbol}`; inspect it for fulfilled/rejected handler registration and iteration."
        ),
        PacketEvidenceRole::RequestDispatch => format!(
            "Request dispatch is anchored by `{symbol}`; inspect it for config transformation and adapter handoff."
        ),
        PacketEvidenceRole::TransportAdapter => format!(
            "Transport adapter selection is anchored by `{symbol}`; inspect it for environment-specific transport choice."
        ),
        PacketEvidenceRole::EventLoop => format!(
            "Event-loop polling is anchored by `{symbol}`; inspect it for readable/writable file-event dispatch."
        ),
        PacketEvidenceRole::NetworkCommandInput => format!(
            "Network command input is anchored by `{symbol}`; inspect it for socket reads and command-buffer processing."
        ),
        PacketEvidenceRole::CommandDispatch => format!(
            "Command dispatch is anchored by `{symbol}`; inspect it for command lookup, validation, execution, and propagation."
        ),
        PacketEvidenceRole::ArgumentPlanning => format!(
            "Argument planning is anchored by `{symbol}`; inspect it for walker, matcher, searcher, and printer construction."
        ),
        PacketEvidenceRole::SearchDriver => format!(
            "Search driver behavior is anchored by `{symbol}`; inspect it for entrypoint routing and sequential or parallel search selection."
        ),
        PacketEvidenceRole::SearchExecutionUnit => format!(
            "Search worker behavior is anchored by `{symbol}`; inspect it for per-candidate matcher/searcher/printer execution."
        ),
        PacketEvidenceRole::RuntimeOrchestration => format!(
            "Runtime orchestration is anchored by `{symbol}`; verify coordination, state transitions, and downstream service calls there."
        ),
        PacketEvidenceRole::WorkspaceDiscoveryAndPlanning => format!(
            "Workspace discovery or planning is anchored by `{symbol}`; inspect it for file selection, manifest, or execution-plan behavior."
        ),
        PacketEvidenceRole::SourceGroupConfiguration => format!(
            "Source-group configuration is anchored by `{symbol}`; inspect it for how project settings become source-group-specific indexing inputs."
        ),
        PacketEvidenceRole::IndexingWorkQueue => format!(
            "Indexing work queue behavior is anchored by `{symbol}`; inspect it for build-index commands, parser handoff, or source-file work items."
        ),
        PacketEvidenceRole::SymbolExtraction => format!(
            "Symbol extraction is anchored by `{symbol}`; inspect it for nodes, edges, occurrences, or file-level indexing."
        ),
        PacketEvidenceRole::PersistenceAndSearchProjection => format!(
            "Persistence or search projection is anchored by `{symbol}`; inspect it for durable graph/search state."
        ),
        PacketEvidenceRole::SnapshotRefresh => format!(
            "Snapshot refresh is anchored by `{symbol}`; inspect it for post-write summary or cache refresh behavior."
        ),
        PacketEvidenceRole::RouteHandling => format!(
            "Route handling is anchored by `{symbol}`; inspect it before tracing request dispatch or handler ownership."
        ),
        PacketEvidenceRole::CollectionConfiguration => format!(
            "Collection configuration is anchored by `{symbol}`; inspect schema fields, hooks, and access rules."
        ),
        PacketEvidenceRole::EventOutputProcessing => format!(
            "JSON/event output processing is anchored by `{symbol}`; inspect it for typed event serialization and stdout behavior."
        ),
        PacketEvidenceRole::AppServerRequestProtocol => format!(
            "App-server request protocol evidence is anchored by `{symbol}`; inspect it for thread or turn start request shape."
        ),
        PacketEvidenceRole::TestsAndRegressionCoverage => format!(
            "Regression coverage for this flow is anchored by `{symbol}`; use it to choose focused verification before broader suites."
        ),
        PacketEvidenceRole::SourceEvidence => {
            let flow_terms = packet_claim_flow_terms(rank_terms, citation);
            let focus = if flow_terms.is_empty() {
                "this flow".to_string()
            } else {
                flow_terms.join(", ")
            };
            format!(
                "`{symbol}` in `{path}` {}; inspect definitions and downstream handoff there.",
                packet_source_evidence_flow_sentence(prompt, &focus)
            )
        }
        PacketEvidenceRole::SqlTableDefinition
        | PacketEvidenceRole::SqlRelationshipConstraint
        | PacketEvidenceRole::SqlSchemaFile
        | PacketEvidenceRole::CandidateFileConstruction => {
            format!("Evidence for this flow is anchored by `{symbol}`.")
        }
    }
}

fn packet_source_evidence_flow_sentence(prompt: &str, focus: &str) -> String {
    let normalized_prompt = normalize_identifier(prompt);
    if let Some(sentence) = eval_supporting_claim_flow_sentence(&normalized_prompt, focus) {
        return sentence;
    }
    format!(
        "supports {focus} in this flow; inspect the cited source, local definitions, and adjacent ownership there"
    )
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
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    eval_citation_shaped_claim(citation, prompt, &path)
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
        citations: citation.map(|value| vec![value]).unwrap_or_default(),
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
