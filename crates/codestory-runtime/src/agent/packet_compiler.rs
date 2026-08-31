//! Compile retained packet evidence into support units and a machine disposition.
//!
//! Runtime owns this classification. Budget may drop traces and duplicate ledgers
//! before support units; it must not re-run a fixpoint that changes disposition.

use crate::agent::packet_coverage::PacketCoverageInput;
use crate::agent::packet_degradation::packet_primary_retrieval_truncated;
use crate::agent::packet_evidence::citation_sufficiency_eligible;
use crate::agent::packet_freshness::PacketFreshnessInput;
use crate::agent::packet_probe::exact_packet_probe_paths;
use crate::agent::packet_scoring::packet_display_path;
use codestory_agent::packet_obligations::{
    PacketProofEvidenceExtras, reconcile_packet_proof_obligations_after_compile,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentPacketDto, AgentPacketRequestDto, BoundedDrillPlanDto, DrillOptionDto,
    EdgeKind, EmbeddingVectorPublicationIdentityDto, GraphArtifactDto, GraphResponse,
    PACKET_DRILL_MAX_BYTES, PACKET_DRILL_MAX_DEPTH, PACKET_DRILL_MAX_HITS,
    PACKET_DRILL_MAX_OPTIONS, PacketClaimObligationDto, PacketClaimObligationKindDto,
    PacketDispositionDto, PacketDispositionKindDto, PacketObligationProofStatusDto, PacketPlanDto,
    PacketProbeResolutionStatusDto, PacketQueryCompletionDto, SourceCoverageStatusDto,
    SupportUnitDto, SupportUnitKindDto, decode_drill_option_id, encode_drill_option_id,
};
use codestory_contracts::graph::FileCoverageReason;
use std::collections::BTreeSet;

#[cfg(test)]
pub fn compile_packet_evidence(
    packet_id: &str,
    question: &str,
    plan: &PacketPlanDto,
    answer: &AgentAnswerDto,
    request: Option<&AgentPacketRequestDto>,
) -> (Vec<SupportUnitDto>, PacketDispositionDto) {
    compile_packet_evidence_with_source_ranges(packet_id, question, plan, answer, &[], request)
}

fn compile_packet_evidence_with_source_ranges(
    packet_id: &str,
    question: &str,
    plan: &PacketPlanDto,
    answer: &AgentAnswerDto,
    source_ranges: &[SupportUnitDto],
    request: Option<&AgentPacketRequestDto>,
) -> (Vec<SupportUnitDto>, PacketDispositionDto) {
    let support = compile_support_units_with_source_ranges(answer, source_ranges);
    let publication = answer.retrieval_trace.retrieval_publication.as_ref();
    let already_drilled = request.is_some_and(|request| {
        request.parent_packet_id.is_some() || !request.option_ids.is_empty()
    });
    let disposition = classify_packet_disposition(ClassifyPacketDispositionInput {
        packet_id,
        question,
        plan,
        answer,
        support: &support,
        publication,
        already_drilled,
        request,
    });
    (support, disposition)
}

fn compile_support_units_with_source_ranges(
    answer: &AgentAnswerDto,
    source_ranges: &[SupportUnitDto],
) -> Vec<SupportUnitDto> {
    let mut units = Vec::new();
    let mut seen = BTreeSet::new();

    for citation in answer
        .citations
        .iter()
        .filter(|citation| citation_sufficiency_eligible(citation))
    {
        let id = format!("symbol:{}", citation.node_id.0);
        if !seen.insert(id.clone()) {
            continue;
        }
        let path = citation.file_path.as_deref().map(packet_display_path);
        let summary = match (path.as_deref(), citation.line) {
            (Some(path), Some(line)) => {
                format!("{} at {path}:{line}", citation.display_name)
            }
            (Some(path), None) => format!("{} at {path}", citation.display_name),
            _ => citation.display_name.clone(),
        };
        units.push(SupportUnitDto {
            id,
            kind: SupportUnitKindDto::SymbolLocation,
            summary,
            path,
            symbol_id: Some(citation.node_id.0.clone()),
            start_line: citation.line,
            end_line: None,
            snippet: None,
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        });
        for source_range in source_ranges.iter().filter(|unit| {
            unit.kind == SupportUnitKindDto::SourceRange
                && unit.symbol_id.as_deref() == Some(citation.node_id.0.as_str())
                && unit
                    .snippet
                    .as_deref()
                    .is_some_and(|snippet| !snippet.is_empty())
        }) {
            if seen.insert(source_range.id.clone()) {
                units.push(source_range.clone());
            }
        }
    }

    for artifact in &answer.graphs {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            continue;
        };
        units.extend(typed_edge_support_units(graph, &mut seen));
    }

    for diagnostic in &answer.retrieval_trace.packet_sidecar_diagnostics {
        if !matches!(diagnostic.completion, PacketQueryCompletionDto::Completed)
            || diagnostic.resolved_hit_count > 0
            || diagnostic.candidate_count > 0
        {
            continue;
        }
        let id = format!("negative:{}", diagnostic.query);
        if !seen.insert(id.clone()) {
            continue;
        }
        units.push(SupportUnitDto {
            id,
            kind: SupportUnitKindDto::CompleteQueryNegative,
            summary: format!("searched `{}`, zero hits", diagnostic.query),
            path: None,
            symbol_id: None,
            start_line: None,
            end_line: None,
            snippet: None,
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: Some(diagnostic.query.clone()),
        });
    }

    units
}

fn typed_edge_support_units(
    graph: &GraphResponse,
    seen: &mut BTreeSet<String>,
) -> Vec<SupportUnitDto> {
    let nodes = graph
        .nodes
        .iter()
        .map(|node| (node.id.0.as_str(), node))
        .collect::<std::collections::HashMap<_, _>>();
    let mut units = Vec::new();
    for edge in &graph.edges {
        // R2 visibility: TYPE_USAGE and USAGE join the public typed-support
        // allow-list so retained atom receipts appear in the scored payload
        // (landed in the same change as the budget-cap protection widening).
        if !matches!(
            edge.kind,
            EdgeKind::CALL
                | EdgeKind::INHERITANCE
                | EdgeKind::IMPORT
                | EdgeKind::TYPE_USAGE
                | EdgeKind::USAGE
        ) {
            continue;
        }
        let id = format!("edge:{}", edge.id.0);
        if !seen.insert(id.clone()) {
            continue;
        }
        let source_node = nodes.get(edge.source.0.as_str()).copied();
        let from = source_node
            .map(|node| node.label.as_str())
            .unwrap_or(edge.source.0.as_str());
        let to = nodes
            .get(edge.target.0.as_str())
            .map(|node| node.label.as_str())
            .unwrap_or(edge.target.0.as_str());
        let kind = match edge.kind {
            EdgeKind::CALL => "CALL",
            EdgeKind::INHERITANCE => "INHERITANCE",
            EdgeKind::IMPORT => "IMPORT",
            EdgeKind::TYPE_USAGE => "TYPE_USAGE",
            EdgeKind::USAGE => "USAGE",
            _ => continue,
        };
        units.push(SupportUnitDto {
            id,
            kind: SupportUnitKindDto::TypedGraphEdge,
            summary: format!("`{from}` {kind} `{to}`"),
            path: source_node
                .and_then(|node| node.file_path.as_deref())
                .map(packet_display_path),
            symbol_id: Some(edge.source.0.clone()),
            start_line: None,
            end_line: None,
            snippet: None,
            edge_kind: Some(kind.to_string()),
            from_symbol: Some(from.to_string()),
            to_symbol: Some(to.to_string()),
            query: None,
        });
    }
    units
}

struct ClassifyPacketDispositionInput<'a> {
    packet_id: &'a str,
    question: &'a str,
    plan: &'a PacketPlanDto,
    answer: &'a AgentAnswerDto,
    support: &'a [SupportUnitDto],
    publication: Option<&'a EmbeddingVectorPublicationIdentityDto>,
    already_drilled: bool,
    request: Option<&'a AgentPacketRequestDto>,
}

fn classify_packet_disposition(input: ClassifyPacketDispositionInput<'_>) -> PacketDispositionDto {
    if let Some(request) = input.request
        && let Some(expected) = request.core_generation_id.as_deref()
    {
        let actual = input
            .publication
            .map(|publication| publication.core_generation_id.as_str());
        if actual != Some(expected) {
            return PacketDispositionDto::unavailable(format!(
                "pinned core generation `{expected}` is no longer current"
            ));
        }
        if let Some(expected_retrieval) = request.retrieval_generation.as_deref() {
            let actual_retrieval = input
                .publication
                .map(|publication| publication.retrieval_generation.as_str());
            if actual_retrieval != Some(expected_retrieval) {
                return PacketDispositionDto::unavailable(format!(
                    "pinned retrieval generation `{expected_retrieval}` is no longer current"
                ));
            }
        }
    }

    if let Some(ambiguous) = first_ambiguous_probe(input.plan) {
        return PacketDispositionDto::not_established(format!(
            "probe `{ambiguous}` is ambiguous and needs a user choice"
        ));
    }

    let freshness = PacketFreshnessInput::from_observation(input.answer.freshness.as_ref());
    if freshness.caps_sufficiency() {
        return PacketDispositionDto::unavailable(
            freshness
                .gap()
                .unwrap_or_else(|| "publication freshness is not established".to_string()),
        );
    }
    let coverage = packet_coverage_for_disposition(input.answer, input.support);
    if coverage.caps_sufficiency() {
        return PacketDispositionDto::unavailable(
            coverage
                .gaps()
                .into_iter()
                .next()
                .unwrap_or_else(|| "source coverage is not established".to_string()),
        );
    }
    if dead_required_sidecar(input.answer) {
        return PacketDispositionDto::unavailable(
            "a required retrieval sidecar did not complete".to_string(),
        );
    }
    if input.answer.retrieval_trace.steps.iter().any(|step| {
        matches!(
            step.status,
            codestory_contracts::api::AgentRetrievalStepStatusDto::Error
        )
    }) {
        return PacketDispositionDto::unavailable("retrieval recorded a hard error".to_string());
    }

    let drill_options = collect_drill_options(input.question, input.plan, input.answer);
    let has_unmet_material = packet_has_unmet_blocking_material(input.plan);
    if input.already_drilled {
        return terminal_after_drill(input.support, drill_options, has_unmet_material);
    }
    if !drill_options.is_empty() {
        let gap_ids = drill_options
            .iter()
            .map(|option| option.gap_id.clone())
            .collect();
        return PacketDispositionDto::drill_once(
            "evidence is objectively missing and closable by the listed options",
            BoundedDrillPlanDto {
                parent_packet_id: input.packet_id.to_string(),
                core_generation_id: input
                    .publication
                    .map(|publication| publication.core_generation_id.clone())
                    .unwrap_or_default(),
                retrieval_generation: input
                    .publication
                    .map(|publication| publication.retrieval_generation.clone()),
                gap_ids,
                options: drill_options,
                max_bytes: PACKET_DRILL_MAX_BYTES,
                max_hits: PACKET_DRILL_MAX_HITS,
                max_depth: PACKET_DRILL_MAX_DEPTH,
                remaining_rounds: 1,
            },
        );
    }
    if has_unmet_material {
        return PacketDispositionDto::not_established(
            "material packet obligations remain unproven after the bounded retrieval pass"
                .to_string(),
        );
    }
    if has_positive_support(input.support) {
        PacketDispositionDto::supported()
    } else {
        PacketDispositionDto::not_established(
            "complete queries returned nothing that could support an answer".to_string(),
        )
    }
}

/// Preserve exact positive evidence from a parser-partial file without claiming the file was
/// completely indexed. The runtime has reread every retained `SourceRange` from source after
/// retrieval and before disposition. That range can therefore support what it literally shows;
/// the index's parser-partial diagnostic remains serialized for the agent and still prevents any
/// unsupported range from passing. Complete-discovery and absence claims remain guarded by their
/// independently material obligations.
fn packet_coverage_for_disposition(
    answer: &AgentAnswerDto,
    support: &[SupportUnitDto],
) -> PacketCoverageInput {
    let verified_source_paths = support
        .iter()
        .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
        .filter(|unit| {
            unit.snippet
                .as_deref()
                .is_some_and(|snippet| !snippet.is_empty())
        })
        .filter_map(|unit| unit.path.as_deref().map(packet_display_path))
        .collect::<BTreeSet<_>>();
    let blocking_observations = answer
        .source_coverage
        .iter()
        .filter(|observation| {
            observation.status != SourceCoverageStatusDto::Incomplete
                || observation.reason != Some(FileCoverageReason::ParserPartial)
                || !verified_source_paths.contains(&packet_display_path(&observation.path))
        })
        .cloned()
        .collect::<Vec<_>>();
    PacketCoverageInput::from_observations(&blocking_observations)
}

fn has_positive_support(support: &[SupportUnitDto]) -> bool {
    support
        .iter()
        .any(|unit| !matches!(unit.kind, SupportUnitKindDto::CompleteQueryNegative))
}

fn terminal_after_drill(
    support: &[SupportUnitDto],
    remaining_options: Vec<DrillOptionDto>,
    has_unmet_material: bool,
) -> PacketDispositionDto {
    let disposition = if has_unmet_material || !remaining_options.is_empty() {
        PacketDispositionDto::not_established(
            "the bounded drill did not establish every material evidence obligation".to_string(),
        )
    } else if has_positive_support(support) {
        PacketDispositionDto::supported()
    } else {
        PacketDispositionDto::not_established(
            "the bounded drill did not establish additional support".to_string(),
        )
    };
    debug_assert_ne!(
        disposition.kind,
        PacketDispositionKindDto::DrillOnce,
        "merge cannot emit another drill"
    );
    disposition
}

/// A generated free-text identity is a useful retrieval lead, but it is not an explicit typed
/// probe and cannot turn an otherwise complete broad packet into a false negative. Behavioral and
/// structural flow rows remain blocking, as do exact probes the caller explicitly bound.
fn material_claim_blocks_supported(obligation: &PacketClaimObligationDto) -> bool {
    obligation.material
        && (obligation.kind != PacketClaimObligationKindDto::ExactProbe
            || obligation.probe_binding.is_some())
}

fn packet_has_unmet_blocking_material(plan: &PacketPlanDto) -> bool {
    plan.obligations.claim_obligations.iter().any(|obligation| {
        material_claim_blocks_supported(obligation)
            && obligation.proof_status != PacketObligationProofStatusDto::Proven
    }) || plan.obligations.query_obligations.iter().any(|obligation| {
        obligation.material && !material_query_obligation_is_satisfied(obligation)
    })
}

/// Bounded retrieval often skips sibling seeds once a flow step is already
/// carried. `not_dispatched` records that skip; it is not a missing search.
fn material_query_obligation_is_satisfied(
    obligation: &codestory_contracts::api::PacketQueryObligationDto,
) -> bool {
    match obligation.completion.as_ref() {
        Some(PacketQueryCompletionDto::Completed) => true,
        Some(PacketQueryCompletionDto::Cancelled { reason }) if reason == "not_dispatched" => true,
        _ => false,
    }
}

fn first_ambiguous_probe(plan: &PacketPlanDto) -> Option<String> {
    plan.probe_resolutions.iter().find_map(|resolution| {
        matches!(resolution.status, PacketProbeResolutionStatusDto::Ambiguous)
            .then(|| format!("probe-{}", resolution.input_index))
    })
}

fn dead_required_sidecar(answer: &AgentAnswerDto) -> bool {
    answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .iter()
        .any(|diagnostic| {
            matches!(
                diagnostic.completion,
                PacketQueryCompletionDto::Cancelled { .. }
            ) && diagnostic.blocking_unresolved_candidate_count > 0
        })
}

fn collect_drill_options(
    question: &str,
    plan: &PacketPlanDto,
    answer: &AgentAnswerDto,
) -> Vec<DrillOptionDto> {
    let mut options = Vec::new();
    let mut seen = BTreeSet::new();
    let cited_paths = answer
        .citations
        .iter()
        .filter(|citation| citation_sufficiency_eligible(citation))
        .filter_map(|citation| citation.file_path.as_deref().map(packet_display_path))
        .collect::<BTreeSet<_>>();

    if packet_primary_retrieval_truncated(answer) {
        for query in plan.queries.iter().take(PACKET_DRILL_MAX_OPTIONS) {
            let option = DrillOptionDto::deadline_lost_query(
                format!("deadline-lost:{}", query.query),
                query.query.clone(),
            );
            if seen.insert(option.id.clone()) {
                options.push(option);
            }
        }
    }

    for path in exact_packet_probe_paths(&plan.probe_resolutions) {
        let display = packet_display_path(&path);
        if cited_paths.contains(&display) {
            continue;
        }
        let option =
            DrillOptionDto::bounded_source_read(format!("named-path:{display}"), display.clone());
        if seen.insert(option.id.clone()) {
            options.push(option);
        }
    }

    for obligation in plan
        .obligations
        .claim_obligations
        .iter()
        .filter(|obligation| {
            material_claim_blocks_supported(obligation)
                && obligation.proof_status != PacketObligationProofStatusDto::Proven
        })
    {
        let named_schema_gap = obligation
            .reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("named_sql_table_carriers_missing:"));
        if obligation.kind != PacketClaimObligationKindDto::ExactProbe
            && !named_schema_gap
            && let Some(query) = obligation
                .open_next_candidates
                .iter()
                .find(|candidate| !candidate.trim().is_empty())
        {
            let option = omitted_support_query_option(
                format!("omitted-material:{}", obligation.id),
                query.trim(),
            );
            if seen.insert(option.id.clone()) {
                options.push(option);
            }
            continue;
        }
        if let Some(edge_kind) = obligation.required_edge_kind
            && matches!(edge_kind, EdgeKind::CALL | EdgeKind::INHERITANCE)
            && obligation.carrier_edge_proofs.is_empty()
            && let Some(target) = obligation.carrier_node_ids.first()
        {
            let option =
                omitted_support_option(answer, format!("omitted-edge:{}", obligation.id), target);
            if seen.insert(option.id.clone()) {
                options.push(option);
            }
        }
        if !named_schema_gap && let Some(node) = obligation.carrier_node_ids.first() {
            let option =
                omitted_support_option(answer, format!("omitted-material:{}", obligation.id), node);
            if seen.insert(option.id.clone()) {
                options.push(option);
            }
        } else if !named_schema_gap && let Some(path) = obligation.carrier_paths.first() {
            let display = packet_display_path(path);
            if !cited_paths.contains(&display) {
                let option = DrillOptionDto::bounded_source_read(
                    format!("omitted-material-path:{}", obligation.id),
                    display,
                );
                if seen.insert(option.id.clone()) {
                    options.push(option);
                }
            }
        }
        if obligation
            .kind
            .eq(&codestory_contracts::api::PacketClaimObligationKindDto::ExactProbe)
            && obligation.carrier_paths.is_empty()
            && let Some(path) = obligation
                .probe_binding
                .as_ref()
                .and_then(|binding| binding.path.clone())
        {
            let display = packet_display_path(&path);
            if !cited_paths.contains(&display) {
                let option =
                    DrillOptionDto::bounded_source_read(format!("omitted-path:{display}"), display);
                if seen.insert(option.id.clone()) {
                    options.push(option);
                }
            }
        }
    }

    for token in question
        .split(|c: char| c.is_whitespace() || matches!(c, '`' | '"' | '\'' | ',' | ';' | '(' | ')'))
    {
        let token = token.trim_matches(|c: char| {
            !c.is_ascii_alphanumeric() && !matches!(c, '/' | '\\' | '.' | '_' | '-')
        });
        if !(token.contains('/') && token.contains('.')) {
            continue;
        }
        let display = packet_display_path(token);
        if cited_paths.contains(&display) {
            continue;
        }
        let option = DrillOptionDto::bounded_source_read(format!("named-path:{display}"), display);
        if seen.insert(option.id.clone()) {
            options.push(option);
        }
    }

    options.truncate(PACKET_DRILL_MAX_OPTIONS);
    options
}

fn omitted_support_option(
    answer: &AgentAnswerDto,
    gap_id: impl Into<String>,
    node: &codestory_contracts::api::NodeId,
) -> DrillOptionDto {
    if node.0.starts_with("packet::")
        && let Some(path) = answer
            .citations
            .iter()
            .find(|citation| citation.node_id == *node)
            .and_then(|citation| citation.file_path.as_deref())
    {
        return DrillOptionDto::omitted_source_path(gap_id, packet_display_path(path));
    }
    DrillOptionDto::omitted_symbol(gap_id, &node.0)
}

fn omitted_support_query_option(
    gap_id: impl Into<String>,
    query: impl Into<String>,
) -> DrillOptionDto {
    let query = query.into();
    DrillOptionDto {
        id: encode_drill_option_id(
            codestory_contracts::api::DrillGapKindDto::OmittedMandatorySupport,
            &format!("query:{query}"),
        ),
        gap_id: gap_id.into(),
        kind: codestory_contracts::api::DrillGapKindDto::OmittedMandatorySupport,
        path: None,
        symbol_id: None,
        query: Some(query),
    }
}

pub fn apply_compiled_evidence(
    packet: &mut AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
) {
    let (support, disposition) = compile_packet_evidence_with_source_ranges(
        &packet.packet_id,
        &packet.question,
        &packet.plan,
        &packet.answer,
        &packet.support,
        request,
    );
    packet.support = support;
    packet.disposition = disposition;
}

/// [`apply_compiled_evidence`] followed by the R5 reconciliation: the compile
/// pass rewrites `packet.support` and `packet.disposition` but never touches
/// `plan.obligations`, so every formula-proven obligation is re-verified
/// against the COMPILED support and the live `answer.graphs`. On any missing
/// receipt the obligation is demoted fail-closed (reason recorded) and the
/// disposition is recomputed on the post-demotion state, so
/// `packet.disposition` and the obligations agree at return.
pub fn apply_compiled_evidence_with_proof_reconciliation(
    packet: &mut AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
    proof_evidence_extras: &PacketProofEvidenceExtras,
) {
    apply_compiled_evidence(packet, request);
    let demoted = reconcile_packet_proof_obligations_after_compile(
        &packet.question,
        packet.plan.task_class,
        &mut packet.plan.obligations,
        &packet.answer,
        &packet.support,
        proof_evidence_extras,
    );
    if demoted {
        apply_compiled_evidence(packet, request);
    }
}

pub fn drill_options_from_ids(option_ids: &[String]) -> Vec<DrillOptionDto> {
    option_ids
        .iter()
        .filter_map(|id| {
            let (kind, target) = decode_drill_option_id(id)?;
            Some(match kind {
                codestory_contracts::api::DrillGapKindDto::BoundedSourceRead => {
                    DrillOptionDto::bounded_source_read(format!("named-path:{target}"), target)
                }
                codestory_contracts::api::DrillGapKindDto::OmittedMandatorySupport => {
                    if let Some(path) = target.strip_prefix("path:") {
                        DrillOptionDto::omitted_source_path(
                            format!("omitted-source-path:{path}"),
                            path,
                        )
                    } else if let Some(query) = target.strip_prefix("query:") {
                        omitted_support_query_option(format!("omitted-query:{query}"), query)
                    } else {
                        let symbol = target.strip_prefix("symbol:").unwrap_or(&target);
                        DrillOptionDto::omitted_symbol(format!("omitted-symbol:{symbol}"), symbol)
                    }
                }
                codestory_contracts::api::DrillGapKindDto::DeadlineLostCandidate => {
                    DrillOptionDto::deadline_lost_query(format!("deadline-lost:{target}"), target)
                }
            })
        })
        .take(PACKET_DRILL_MAX_OPTIONS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::packet_budget::tests::test_packet;
    use crate::agent::packet_freshness::fresh_index_observation;
    use codestory_agent::packet_command::packet_follow_up_argv;
    use codestory_contracts::api::{
        AgentCitationDto, AgentRetrievalStepDto, AgentRetrievalStepKindDto,
        AgentRetrievalStepStatusDto, DrillGapKindDto, NodeId, NodeKind, PacketBudgetModeDto,
        PacketEvidenceResolutionDto, PacketEvidenceTierDto, PacketPlanDto, PacketProbeDto,
        PacketProbeResolutionDto, PacketProbeResolutionStatusDto, PacketQueryObligationDto,
        PacketQueryObligationKindDto, PacketTaskClassDto, SearchHitOrigin,
        SourceCoverageObservationDto,
    };
    use std::path::Path;

    fn eligible_citation(name: &str, path: &str) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(name.to_string()),
            display_name: name.to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some(path.to_string()),
            line: Some(10),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::ExactSource),
            evidence_producer: None,
            resolution_status: Some(
                codestory_contracts::api::PacketEvidenceResolutionDto::Resolved,
            ),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
            source_excerpt: None,
        }
    }

    fn retained_source_range(symbol_id: &str, path: &str) -> SupportUnitDto {
        SupportUnitDto {
            id: format!("source:{symbol_id}:10"),
            kind: SupportUnitKindDto::SourceRange,
            summary: format!("source for {symbol_id} at {path}:10-11"),
            path: Some(path.to_string()),
            symbol_id: Some(symbol_id.to_string()),
            start_line: Some(10),
            end_line: Some(11),
            snippet: Some("fn verified_from_source() {}".to_string()),
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        }
    }

    fn incomplete_observation(
        path: &str,
        reason: FileCoverageReason,
    ) -> SourceCoverageObservationDto {
        SourceCoverageObservationDto {
            path: path.to_string(),
            status: SourceCoverageStatusDto::Incomplete,
            reason: Some(reason),
            not_established_cause: None,
            observed_size: None,
            byte_cap: None,
        }
    }

    fn empty_plan() -> PacketPlanDto {
        PacketPlanDto {
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            inferred_task_class: true,
            queries: Vec::new(),
            probe_resolutions: Vec::new(),
            obligations: Default::default(),
            trace: Vec::new(),
        }
    }

    fn claim_obligation(
        kind: PacketClaimObligationKindDto,
        status: PacketObligationProofStatusDto,
    ) -> PacketClaimObligationDto {
        PacketClaimObligationDto {
            id: "material-flow".to_string(),
            kind,
            binding_terms: Vec::new(),
            probe_binding: None,
            material: true,
            allowed_node_kinds: Vec::new(),
            required_edge_kind: None,
            requires_complete_discovery: false,
            proof_status: status,
            reason: None,
            carrier_node_ids: Vec::new(),
            carrier_paths: Vec::new(),
            carrier_edge_proofs: Vec::new(),
            open_next_candidates: Vec::new(),
        }
    }

    #[test]
    fn positive_support_cannot_hide_an_unproven_material_flow() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation("Router.use", "src/router.rs")];
        let mut obligation = claim_obligation(
            PacketClaimObligationKindDto::Dispatch,
            PacketObligationProofStatusDto::Reported,
        );
        obligation.carrier_node_ids = vec![NodeId("Router.use".to_string())];
        packet.plan = empty_plan();
        packet.plan.obligations.claim_obligations = vec![obligation];

        let (_support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );

        assert_eq!(disposition.kind, PacketDispositionKindDto::DrillOnce);
        let options = disposition.drill.expect("bounded drill").options;
        assert!(
            options
                .iter()
                .any(|option| { option.symbol_id.as_deref() == Some("Router.use") })
        );
    }

    #[test]
    fn typed_drill_continuation_proves_omitted_http_handle_after_one_round() {
        let mut packet = test_packet(
            "Trace how an HTTP server dispatches an incoming request to a handler.",
            98_304,
        );
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation("Router.use", "src/router.rs")];
        let mut obligation = claim_obligation(
            PacketClaimObligationKindDto::Dispatch,
            PacketObligationProofStatusDto::Reported,
        );
        obligation.id = "request_dispatch".to_string();
        obligation.carrier_node_ids = vec![NodeId("Router.use".to_string())];
        packet.plan = empty_plan();
        packet.plan.obligations.claim_obligations = vec![obligation];

        let (_support, first) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );
        assert_eq!(first.kind, PacketDispositionKindDto::DrillOnce);
        let drill = first.drill.expect("bounded drill");
        let argv = packet_follow_up_argv(
            Path::new("/tmp/project"),
            &packet.question,
            PacketBudgetModeDto::Standard,
            Some(&drill),
        )
        .expect("drill_once must publish a typed continuation");
        assert!(argv.contains(&"--parent-packet-id".to_string()));
        assert!(argv.contains(&packet.packet_id));
        assert!(argv.contains(&"--option-id".to_string()));
        assert!(argv.iter().any(|argument| argument.contains("Router.use")));
        assert!(!argv.iter().any(|argument| argument == "deep"));
        assert!(
            !argv.contains(&"--core-generation-id".to_string()),
            "empty generation pins must not be forwarded"
        );

        packet.answer.citations.push(eligible_citation(
            "ServerEngine.handleHTTPRequest",
            "src/router.rs",
        ));
        packet.plan.obligations.claim_obligations[0].proof_status =
            PacketObligationProofStatusDto::Proven;
        packet.plan.obligations.claim_obligations[0]
            .carrier_node_ids
            .push(NodeId("ServerEngine.handleHTTPRequest".to_string()));
        let request = AgentPacketRequestDto {
            question: packet.question.clone(),
            budget: Default::default(),
            task_class: None,
            probes: Vec::new(),
            extra_probes: Vec::new(),
            latency_budget_ms: None,
            parent_packet_id: Some(packet.packet_id.clone()),
            option_ids: drill
                .options
                .iter()
                .map(|option| option.id.clone())
                .collect(),
            core_generation_id: None,
            retrieval_generation: None,
        };
        let (_support, continuation) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            Some(&request),
        );
        assert_eq!(continuation.kind, PacketDispositionKindDto::Supported);
        assert!(continuation.is_terminal());
    }

    #[test]
    fn unmet_flow_stage_drills_by_its_planned_query_with_or_without_a_current_carrier() {
        for current_carrier in [None, Some("Downstream.handle")] {
            let mut packet = test_packet("Explain the complete request lifecycle.", 98_304);
            packet.answer.freshness = Some(fresh_index_observation());
            let mut obligation = claim_obligation(
                PacketClaimObligationKindDto::Dispatch,
                PacketObligationProofStatusDto::Reported,
            );
            obligation.id = "upstream_public_stage".to_string();
            obligation.open_next_candidates = vec!["public request registration".to_string()];
            if let Some(carrier) = current_carrier {
                packet.answer.citations = vec![eligible_citation(carrier, "src/downstream.rs")];
                obligation.carrier_node_ids = vec![NodeId(carrier.to_string())];
            }
            packet.plan = empty_plan();
            packet.plan.obligations.claim_obligations = vec![obligation];

            let (_support, disposition) = compile_packet_evidence(
                &packet.packet_id,
                &packet.question,
                &packet.plan,
                &packet.answer,
                None,
            );
            let options = disposition
                .drill
                .expect("an unmet planned stage must remain closable")
                .options;

            assert_eq!(options.len(), 1, "current carrier: {current_carrier:?}");
            assert_eq!(
                options[0].query.as_deref(),
                Some("public request registration"),
                "the continuation must not replay a downstream carrier: {current_carrier:?}"
            );
            assert!(options[0].symbol_id.is_none());
            assert!(options[0].path.is_none());
            let decoded = drill_options_from_ids(&[options[0].id.clone()]);
            assert_eq!(decoded.len(), 1);
            assert_eq!(decoded[0].kind, options[0].kind);
            assert_eq!(decoded[0].query, options[0].query);
            assert!(decoded[0].symbol_id.is_none());
            assert!(decoded[0].path.is_none());
        }
    }

    #[test]
    fn an_obligation_name_cannot_be_published_as_a_drill_symbol_id() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation("Router.use", "src/router.rs")];
        let mut obligation = claim_obligation(
            PacketClaimObligationKindDto::Dispatch,
            PacketObligationProofStatusDto::Unsupported,
        );
        obligation.id = "request_dispatch".to_string();
        obligation.required_edge_kind = Some(EdgeKind::CALL);
        packet.plan = empty_plan();
        packet.plan.obligations.claim_obligations = vec![obligation];

        let (_support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );

        assert_eq!(disposition.kind, PacketDispositionKindDto::NotEstablished);
        assert!(disposition.drill.is_none());
    }

    #[test]
    fn synthetic_source_carriers_drill_by_exact_path_instead_of_opaque_symbol_id() {
        let mut packet = test_packet("explain the imported animation keyframe", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        let mut citation = eligible_citation(
            "@keyframes bounce",
            "/private/tmp/product-value/repos/animate-css-animate-css/source/attention_seekers/bounce.css",
        );
        citation.node_id = NodeId(
            "packet::css_import::source/attention_seekers/bounce.css::@keyframes bounce"
                .to_string(),
        );
        citation.resolvable = false;
        citation.evidence_tier = Some(PacketEvidenceTierDto::SyntheticSourceScan);
        citation.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);
        packet.answer.citations = vec![citation.clone()];

        let mut obligation = claim_obligation(
            PacketClaimObligationKindDto::Dispatch,
            PacketObligationProofStatusDto::Reported,
        );
        obligation.carrier_node_ids = vec![citation.node_id];
        packet.plan = empty_plan();
        packet.plan.obligations.claim_obligations = vec![obligation];

        let (_support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );
        let option = disposition
            .drill
            .expect("synthetic source omission should offer one bounded continuation")
            .options
            .into_iter()
            .next()
            .expect("bounded continuation option");

        assert_eq!(option.kind, DrillGapKindDto::OmittedMandatorySupport);
        assert_eq!(
            option.path.as_deref(),
            Some("source/attention_seekers/bounce.css")
        );
        assert!(option.symbol_id.is_none());
        let decoded = drill_options_from_ids(&[option.id]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(
            decoded[0].path.as_deref(),
            Some("source/attention_seekers/bounce.css")
        );
        assert!(decoded[0].symbol_id.is_none());
    }

    #[test]
    fn a_material_flow_still_unproven_after_drill_is_terminal_not_established() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation("Router.use", "src/router.rs")];
        let mut obligation = claim_obligation(
            PacketClaimObligationKindDto::Dispatch,
            PacketObligationProofStatusDto::Reported,
        );
        obligation.carrier_node_ids = vec![NodeId("Router.use".to_string())];
        packet.plan = empty_plan();
        packet.plan.obligations.claim_obligations = vec![obligation];
        let request = AgentPacketRequestDto {
            question: packet.question.clone(),
            budget: Default::default(),
            task_class: None,
            probes: Vec::new(),
            extra_probes: Vec::new(),
            latency_budget_ms: None,
            parent_packet_id: Some(packet.packet_id.clone()),
            option_ids: vec!["omitted_mandatory_support:symbol%3ARouter.use".to_string()],
            core_generation_id: None,
            retrieval_generation: None,
        };

        let (_support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            Some(&request),
        );

        assert_eq!(disposition.kind, PacketDispositionKindDto::NotEstablished);
        assert!(disposition.is_terminal());
    }

    #[test]
    fn generated_free_text_exact_lead_does_not_block_a_complete_broad_packet() {
        let mut packet = test_packet("explain the AutoMapper APIs", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation(
            "MapperConfiguration.BuildExecutionPlan",
            "src/mapper_configuration.cs",
        )];
        packet.plan = empty_plan();
        packet.plan.obligations.claim_obligations = vec![claim_obligation(
            PacketClaimObligationKindDto::ExactProbe,
            PacketObligationProofStatusDto::Unsupported,
        )];

        let (_support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );

        assert_eq!(disposition.kind, PacketDispositionKindDto::Supported);
    }

    #[test]
    fn proven_material_flow_with_positive_support_is_supported() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation("Router.dispatch", "src/router.rs")];
        packet.plan = empty_plan();
        packet.plan.obligations.claim_obligations = vec![claim_obligation(
            PacketClaimObligationKindDto::Dispatch,
            PacketObligationProofStatusDto::Proven,
        )];

        let (_support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );

        assert_eq!(disposition.kind, PacketDispositionKindDto::Supported);
    }

    #[test]
    fn skipped_sibling_queries_do_not_block_a_proven_material_flow() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation("Router.dispatch", "src/router.rs")];
        packet.plan = empty_plan();
        packet.plan.obligations.claim_obligations = vec![claim_obligation(
            PacketClaimObligationKindDto::Dispatch,
            PacketObligationProofStatusDto::Proven,
        )];
        packet.plan.obligations.query_obligations = vec![PacketQueryObligationDto {
            id: "query:0".to_string(),
            kind: PacketQueryObligationKindDto::RequiredFlow,
            query: "transport send".to_string(),
            material: true,
            completion: Some(PacketQueryCompletionDto::Cancelled {
                reason: "not_dispatched".to_string(),
            }),
        }];

        let (_support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );

        assert_eq!(disposition.kind, PacketDispositionKindDto::Supported);
    }

    #[test]
    fn a_hard_cancelled_material_query_still_blocks_supported() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation("Router.dispatch", "src/router.rs")];
        packet.plan = empty_plan();
        packet.plan.obligations.claim_obligations = vec![claim_obligation(
            PacketClaimObligationKindDto::Dispatch,
            PacketObligationProofStatusDto::Proven,
        )];
        packet.plan.obligations.query_obligations = vec![PacketQueryObligationDto {
            id: "query:0".to_string(),
            kind: PacketQueryObligationKindDto::RequiredFlow,
            query: "transport send".to_string(),
            material: true,
            completion: Some(PacketQueryCompletionDto::Cancelled {
                reason: "stage_deadline".to_string(),
            }),
        }];

        let (_support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );

        assert_eq!(disposition.kind, PacketDispositionKindDto::NotEstablished);
    }

    #[test]
    fn exact_source_range_preserves_positive_support_from_a_parser_partial_file() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation("Router.dispatch", "src/router.ts")];
        packet.answer.source_coverage = vec![incomplete_observation(
            "/checkout/repos/example/src/router.ts",
            FileCoverageReason::ParserPartial,
        )];
        packet.plan = empty_plan();
        packet.support = vec![retained_source_range("Router.dispatch", "src/router.ts")];

        apply_compiled_evidence(&mut packet, None);

        assert_eq!(packet.disposition.kind, PacketDispositionKindDto::Supported);
        assert!(packet.support.iter().any(|unit| {
            unit.kind == SupportUnitKindDto::SourceRange
                && unit.path.as_deref() == Some("src/router.ts")
        }));
        assert_eq!(
            packet.answer.source_coverage[0].status,
            SourceCoverageStatusDto::Incomplete,
            "the parser-partial diagnostic stays visible"
        );
    }

    #[test]
    fn parser_partial_file_without_its_exact_source_range_remains_unavailable() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation("Router.dispatch", "src/router.ts")];
        packet.answer.source_coverage = vec![incomplete_observation(
            "src/router.ts",
            FileCoverageReason::ParserPartial,
        )];
        packet.plan = empty_plan();

        let (_support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );

        assert_eq!(disposition.kind, PacketDispositionKindDto::Unavailable);
    }

    #[test]
    fn exact_source_range_does_not_excuse_an_unreadable_file() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation("Router.dispatch", "src/router.ts")];
        packet.answer.source_coverage = vec![incomplete_observation(
            "src/router.ts",
            FileCoverageReason::Unreadable,
        )];
        packet.plan = empty_plan();
        packet.support = vec![retained_source_range("Router.dispatch", "src/router.ts")];

        apply_compiled_evidence(&mut packet, None);

        assert_eq!(
            packet.disposition.kind,
            PacketDispositionKindDto::Unavailable
        );
    }

    #[test]
    fn exact_source_range_does_not_prove_complete_discovery() {
        let mut packet = test_packet("is this route unused?", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation("Router.dispatch", "src/router.ts")];
        packet.answer.source_coverage = vec![incomplete_observation(
            "src/router.ts",
            FileCoverageReason::ParserPartial,
        )];
        packet.plan = empty_plan();
        let mut obligation = claim_obligation(
            PacketClaimObligationKindDto::Dispatch,
            PacketObligationProofStatusDto::Reported,
        );
        obligation.requires_complete_discovery = true;
        packet.plan.obligations.claim_obligations = vec![obligation];
        packet.support = vec![retained_source_range("Router.dispatch", "src/router.ts")];

        apply_compiled_evidence(&mut packet, None);

        assert_ne!(packet.disposition.kind, PacketDispositionKindDto::Supported);
        assert!(matches!(
            packet.disposition.kind,
            PacketDispositionKindDto::DrillOnce | PacketDispositionKindDto::NotEstablished
        ));
    }

    #[test]
    fn source_range_support_stays_bound_to_its_retained_citation() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.citations = vec![eligible_citation("Router.dispatch", "src/router.rs")];
        let source_range = |id: &str, symbol_id: &str, snippet: &str| SupportUnitDto {
            id: id.to_string(),
            kind: SupportUnitKindDto::SourceRange,
            summary: "source for Router.dispatch at src/router.rs:10".to_string(),
            path: Some("src/router.rs".to_string()),
            symbol_id: Some(symbol_id.to_string()),
            start_line: Some(10),
            end_line: None,
            snippet: Some(snippet.to_string()),
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        };

        let support = compile_support_units_with_source_ranges(
            &packet.answer,
            &[
                source_range("source:retained", "Router.dispatch", "fn dispatch() {}"),
                source_range("source:dropped", "Dropped.symbol", "fn dropped() {}"),
                source_range("source:empty", "Router.dispatch", ""),
            ],
        );

        assert_eq!(support.len(), 2);
        assert_eq!(support[0].kind, SupportUnitKindDto::SymbolLocation);
        assert_eq!(support[1].kind, SupportUnitKindDto::SourceRange);
        assert_eq!(support[1].id, "source:retained");
    }

    #[test]
    fn one_citation_is_not_automatically_supported_when_a_named_path_is_unread() {
        let mut packet = test_packet("explain src/unread.rs", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation(
            "OnlyHit",
            "crates/codestory-runtime/src/agent/packet_budget.rs",
        )];
        packet.plan = PacketPlanDto {
            probe_resolutions: vec![PacketProbeResolutionDto {
                input_index: 0,
                probe: PacketProbeDto::ExactPath {
                    path: "src/unread.rs".to_string(),
                },
                status: PacketProbeResolutionStatusDto::ExactPath,
                normalized_query: None,
                path: Some("src/unread.rs".to_string()),
                symbol_id: None,
                candidates: Vec::new(),
                rejection: None,
            }],
            ..empty_plan()
        };

        let (support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );
        assert_eq!(support.len(), 1, "one citation still compiles as support");
        assert_eq!(disposition.kind, PacketDispositionKindDto::DrillOnce);
        let drill = disposition.drill.expect("drill plan");
        assert_eq!(drill.remaining_rounds, 1);
        assert!(
            drill
                .options
                .iter()
                .any(|option| option.path.as_deref() == Some("src/unread.rs")),
            "{drill:?}"
        );
    }

    #[test]
    fn unresolved_named_path_after_drill_is_terminal_not_established() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation(
            "OnlyHit",
            "crates/codestory-runtime/src/agent/packet_budget.rs",
        )];
        packet.plan = PacketPlanDto {
            probe_resolutions: vec![PacketProbeResolutionDto {
                input_index: 0,
                probe: PacketProbeDto::ExactPath {
                    path: "src/unread.rs".to_string(),
                },
                status: PacketProbeResolutionStatusDto::ExactPath,
                normalized_query: None,
                path: Some("src/unread.rs".to_string()),
                symbol_id: None,
                candidates: Vec::new(),
                rejection: None,
            }],
            ..empty_plan()
        };
        let request = AgentPacketRequestDto {
            question: packet.question.clone(),
            budget: Default::default(),
            task_class: None,
            probes: Vec::new(),
            extra_probes: Vec::new(),
            latency_budget_ms: None,
            parent_packet_id: Some(packet.packet_id.clone()),
            option_ids: vec!["bounded_source_read:src%2Funread.rs".to_string()],
            core_generation_id: None,
            retrieval_generation: None,
        };

        let (_support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            Some(&request),
        );
        assert_ne!(disposition.kind, PacketDispositionKindDto::DrillOnce);
        assert!(disposition.is_terminal());
        assert_eq!(disposition.kind, PacketDispositionKindDto::NotEstablished);
    }

    #[test]
    fn complete_zero_hit_is_not_established_not_a_search_loop() {
        let mut packet = test_packet("no such symbol xyzzy", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations.clear();
        packet.answer.retrieval_trace.packet_sidecar_diagnostics =
            vec![codestory_contracts::api::PacketSidecarQueryDiagnosticDto {
                query: "xyzzy".to_string(),
                completion: PacketQueryCompletionDto::Completed,
                retrieval_mode: "full".to_string(),
                sidecar_query_ms: None,
                candidate_resolution_ms: None,
                total_elapsed_ms: None,
                sidecar_stage_count: 1,
                sidecar_stage_total_ms: None,
                batch_query_wall_ms: None,
                candidate_count: 0,
                resolved_hit_count: 0,
                unresolved_candidate_count: 0,
                blocking_unresolved_candidate_count: 0,
                semantic_stage_timeout_zero_hits: false,
                semantic_abstained: false,
                diagnostic: None,
            }];

        let (support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );
        assert!(
            support
                .iter()
                .any(|unit| unit.kind == SupportUnitKindDto::CompleteQueryNegative)
        );
        assert_eq!(disposition.kind, PacketDispositionKindDto::NotEstablished);
    }

    #[test]
    fn retrieval_error_is_unavailable() {
        let mut packet = test_packet("explain routing", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.retrieval_trace.steps = vec![AgentRetrievalStepDto {
            kind: AgentRetrievalStepKindDto::Search,
            status: AgentRetrievalStepStatusDto::Error,
            duration_ms: 1,
            input: Vec::new(),
            output: Vec::new(),
            message: Some("sidecar crashed".to_string()),
        }];

        let (_support, disposition) = compile_packet_evidence(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            None,
        );
        assert_eq!(disposition.kind, PacketDispositionKindDto::Unavailable);
    }

    #[test]
    fn budget_cannot_drop_drill_options_or_change_disposition() {
        let mut packet = test_packet("explain src/unread.rs", 98_304);
        packet.answer.freshness = Some(fresh_index_observation());
        packet.answer.citations = vec![eligible_citation(
            "OnlyHit",
            "crates/codestory-runtime/src/agent/packet_budget.rs",
        )];
        packet.plan = PacketPlanDto {
            probe_resolutions: vec![PacketProbeResolutionDto {
                input_index: 0,
                probe: PacketProbeDto::ExactPath {
                    path: "src/unread.rs".to_string(),
                },
                status: PacketProbeResolutionStatusDto::ExactPath,
                normalized_query: None,
                path: Some("src/unread.rs".to_string()),
                symbol_id: None,
                candidates: Vec::new(),
                rejection: None,
            }],
            ..empty_plan()
        };
        apply_compiled_evidence(&mut packet, None);
        assert_eq!(packet.disposition.kind, PacketDispositionKindDto::DrillOnce);
        let option_ids = packet
            .disposition
            .drill
            .as_ref()
            .expect("drill plan")
            .options
            .iter()
            .map(|option| option.id.clone())
            .collect::<Vec<_>>();
        assert!(!option_ids.is_empty());
        crate::agent::packet_budget::enforce_packet_output_budget(
            Path::new("/workspace/CodeStory"),
            &mut packet,
        );
        assert_eq!(packet.disposition.kind, PacketDispositionKindDto::DrillOnce);
        let retained = packet
            .disposition
            .drill
            .as_ref()
            .expect("drill plan after budget")
            .options
            .iter()
            .map(|option| option.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(retained, option_ids);
    }

    // -----------------------------------------------------------------------
    // Stage 2: R5 reconciliation after compile
    // -----------------------------------------------------------------------

    /// Finalizes the mapper fixture so `mapper_config` is formula-proven
    /// through its atom receipts (a certain TYPE_USAGE edge, the builder's
    /// MEMBER-onto-METHOD edge, and a reread configuration source range).
    fn finalized_mapper_proof_packet() -> AgentPacketDto {
        let mut packet = crate::agent::packet_budget::tests::mapper_proof_packet();
        codestory_agent::packet_obligations::finalize_packet_obligation_plan(
            &packet.question.clone(),
            packet.plan.task_class,
            &mut packet.plan.obligations,
            &packet.answer,
            &packet.budget,
            &packet.support.clone(),
            &PacketProofEvidenceExtras::default(),
        );
        assert_eq!(
            mapper_config_obligation(&packet).proof_status,
            PacketObligationProofStatusDto::Proven,
            "fixture must start formula-proven"
        );
        packet
    }

    fn mapper_config_obligation(packet: &AgentPacketDto) -> &PacketClaimObligationDto {
        packet
            .plan
            .obligations
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == "mapper_config")
            .expect("mapper_config obligation")
    }

    /// R5 control: when every receipt survives compile, nothing is demoted
    /// and the compiled disposition stands.
    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn reconciliation_keeps_formula_proof_whose_receipts_survive_compile() {
        let mut packet = finalized_mapper_proof_packet();

        apply_compiled_evidence_with_proof_reconciliation(
            &mut packet,
            None,
            &PacketProofEvidenceExtras::default(),
        );

        let obligation = mapper_config_obligation(&packet);
        assert_eq!(
            obligation.proof_status,
            PacketObligationProofStatusDto::Proven,
            "{obligation:?}"
        );
        assert!(
            packet.support.iter().any(|unit| {
                unit.kind == SupportUnitKindDto::SourceRange
                    && unit.symbol_id.as_deref() == Some("MapperConfiguration")
            }),
            "the A2 receipt must survive compile for this control to be meaningful"
        );
    }

    /// R5: a formula-proven obligation whose receipt is absent from the
    /// compiled support is demoted fail-closed with the recorded reason, and
    /// the disposition is recomputed on the post-demotion state — a fresh
    /// compile of the returned packet yields the same disposition.
    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn reconciliation_demotes_formula_proof_and_recomputes_disposition() {
        let mut packet = finalized_mapper_proof_packet();
        // A budget-style loss between finalize and compile: the configuration
        // citation is gone, so compile drops the A2 source-range receipt.
        packet
            .answer
            .citations
            .retain(|citation| citation.node_id.0 != "MapperConfiguration");

        apply_compiled_evidence_with_proof_reconciliation(
            &mut packet,
            None,
            &PacketProofEvidenceExtras::default(),
        );

        let obligation = mapper_config_obligation(&packet);
        assert_ne!(
            obligation.proof_status,
            PacketObligationProofStatusDto::Proven,
            "missing receipts must demote fail-closed: {obligation:?}"
        );
        assert_eq!(
            obligation.reason.as_deref(),
            Some("flow_proof_receipts_missing_after_compile")
        );
        assert!(
            !packet.support.iter().any(|unit| {
                unit.kind == SupportUnitKindDto::SourceRange
                    && unit.symbol_id.as_deref() == Some("MapperConfiguration")
            }),
            "compile must actually have dropped the receipt for this test to bite"
        );
        // Disposition and obligations agree at return: recompiling the
        // post-demotion state reproduces the returned disposition exactly.
        let (_support, recompiled) = compile_packet_evidence_with_source_ranges(
            &packet.packet_id,
            &packet.question,
            &packet.plan,
            &packet.answer,
            &packet.support,
            None,
        );
        assert_eq!(
            packet.disposition, recompiled,
            "packet.disposition must be the disposition of the post-demotion state"
        );
    }

    /// Pins the property R5's single-recompile argument rests on: compiling
    /// support is idempotent on its own output. If a future support-side
    /// change breaks `compile(answer, S1) == S1` for `S1 = compile(answer,
    /// S0)`, the one-pass reconciliation in
    /// `apply_compiled_evidence_with_proof_reconciliation` would no longer be
    /// provably sufficient — this test makes that break loud.
    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn compile_support_units_are_idempotent_on_their_own_output() {
        let packet = finalized_mapper_proof_packet();

        let first = compile_support_units_with_source_ranges(&packet.answer, &packet.support);
        let second = compile_support_units_with_source_ranges(&packet.answer, &first);

        assert!(
            first
                .iter()
                .any(|unit| unit.kind == SupportUnitKindDto::SourceRange),
            "the fixture must retain a SourceRange unit for the property to bite"
        );
        assert_eq!(
            second, first,
            "compiling support must be idempotent on its own output"
        );
    }

    /// R2 visibility (landed together with the budget-cap protection
    /// widening): retained TYPE_USAGE and USAGE atom receipts appear in the
    /// scored typed-support payload, while kinds outside the allow-list stay
    /// excluded.
    #[test]
    fn typed_support_allow_list_carries_type_usage_and_usage_edges() {
        let mut seen = std::collections::BTreeSet::new();
        let node = |id: &str| codestory_contracts::api::GraphNodeDto {
            id: codestory_contracts::api::NodeId(id.to_string()),
            label: id.to_string(),
            kind: codestory_contracts::api::NodeKind::CLASS,
            depth: 1,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: (id == "builder").then(|| "src/builder.rs".to_string()),
            qualified_name: None,
            member_access: None,
        };
        let edge = |id: &str, kind: EdgeKind| codestory_contracts::api::GraphEdgeDto {
            id: codestory_contracts::api::EdgeId(id.to_string()),
            source: codestory_contracts::api::NodeId("builder".to_string()),
            target: codestory_contracts::api::NodeId("config".to_string()),
            kind,
            confidence: None,
            certainty: Some("certain".to_string()),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        };
        let graph = GraphResponse {
            center_id: codestory_contracts::api::NodeId("builder".to_string()),
            nodes: vec![node("builder"), node("config")],
            edges: vec![
                edge("uses-config", EdgeKind::TYPE_USAGE),
                edge("uses-var", EdgeKind::USAGE),
                edge("overrides", EdgeKind::OVERRIDE),
            ],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };
        let units = typed_edge_support_units(&graph, &mut seen);
        let kinds = units
            .iter()
            .filter_map(|unit| unit.edge_kind.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            ["TYPE_USAGE", "USAGE"],
            "atom receipt kinds must be visible and OVERRIDE must stay excluded"
        );
        assert!(
            units
                .iter()
                .all(|unit| unit.kind == SupportUnitKindDto::TypedGraphEdge)
        );
        assert_eq!(units[0].summary, "`builder` TYPE_USAGE `config`");
        assert_eq!(units[0].path.as_deref(), Some("src/builder.rs"));
        assert_eq!(units[0].symbol_id.as_deref(), Some("builder"));
    }
}
