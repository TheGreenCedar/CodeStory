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
use codestory_contracts::api::{
    AgentAnswerDto, AgentPacketDto, AgentPacketRequestDto, BoundedDrillPlanDto, DrillOptionDto,
    EdgeKind, EmbeddingVectorPublicationIdentityDto, GraphArtifactDto, GraphResponse,
    PACKET_DRILL_MAX_BYTES, PACKET_DRILL_MAX_DEPTH, PACKET_DRILL_MAX_HITS,
    PACKET_DRILL_MAX_OPTIONS, PacketDispositionDto, PacketDispositionKindDto, PacketPlanDto,
    PacketProbeResolutionStatusDto, PacketQueryCompletionDto, SupportUnitDto, SupportUnitKindDto,
    decode_drill_option_id,
};
use std::collections::BTreeSet;

pub fn compile_packet_evidence(
    packet_id: &str,
    question: &str,
    plan: &PacketPlanDto,
    answer: &AgentAnswerDto,
    request: Option<&AgentPacketRequestDto>,
) -> (Vec<SupportUnitDto>, PacketDispositionDto) {
    let support = compile_support_units(answer);
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

pub fn compile_support_units(answer: &AgentAnswerDto) -> Vec<SupportUnitDto> {
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
    let labels = graph
        .nodes
        .iter()
        .map(|node| (node.id.0.as_str(), node.label.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    let mut units = Vec::new();
    for edge in &graph.edges {
        if !matches!(
            edge.kind,
            EdgeKind::CALL | EdgeKind::INHERITANCE | EdgeKind::IMPORT
        ) {
            continue;
        }
        let id = format!("edge:{}", edge.id.0);
        if !seen.insert(id.clone()) {
            continue;
        }
        let from = labels
            .get(edge.source.0.as_str())
            .copied()
            .unwrap_or(edge.source.0.as_str());
        let to = labels
            .get(edge.target.0.as_str())
            .copied()
            .unwrap_or(edge.target.0.as_str());
        let kind = match edge.kind {
            EdgeKind::CALL => "CALL",
            EdgeKind::INHERITANCE => "INHERITANCE",
            EdgeKind::IMPORT => "IMPORT",
            _ => continue,
        };
        units.push(SupportUnitDto {
            id,
            kind: SupportUnitKindDto::TypedGraphEdge,
            summary: format!("`{from}` {kind} `{to}`"),
            path: None,
            symbol_id: None,
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
    let coverage = PacketCoverageInput::from_observations(&input.answer.source_coverage);
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
    if input.already_drilled {
        return terminal_after_drill(input.support, drill_options);
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
    if has_positive_support(input.support) {
        PacketDispositionDto::supported()
    } else {
        PacketDispositionDto::not_established(
            "complete queries returned nothing that could support an answer".to_string(),
        )
    }
}

fn has_positive_support(support: &[SupportUnitDto]) -> bool {
    support
        .iter()
        .any(|unit| !matches!(unit.kind, SupportUnitKindDto::CompleteQueryNegative))
}

fn terminal_after_drill(
    support: &[SupportUnitDto],
    _unused_options: Vec<DrillOptionDto>,
) -> PacketDispositionDto {
    let disposition = if has_positive_support(support) {
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
        .filter(|obligation| obligation.material)
    {
        if let Some(edge_kind) = obligation.required_edge_kind
            && matches!(edge_kind, EdgeKind::CALL | EdgeKind::INHERITANCE)
            && obligation.carrier_edge_proofs.is_empty()
        {
            let target = obligation
                .carrier_node_ids
                .first()
                .map(|node| node.0.as_str())
                .unwrap_or(obligation.id.as_str());
            let option =
                DrillOptionDto::omitted_symbol(format!("omitted-edge:{}", obligation.id), target);
            if seen.insert(option.id.clone()) {
                options.push(option);
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

pub fn apply_compiled_evidence(
    packet: &mut AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
) {
    let (support, disposition) = compile_packet_evidence(
        &packet.packet_id,
        &packet.question,
        &packet.plan,
        &packet.answer,
        request,
    );
    packet.support = support;
    packet.disposition = disposition;
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
                    let symbol = target.strip_prefix("symbol:").unwrap_or(&target);
                    DrillOptionDto::omitted_symbol(format!("omitted-symbol:{symbol}"), symbol)
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
    use codestory_contracts::api::{
        AgentCitationDto, AgentRetrievalStepDto, AgentRetrievalStepKindDto,
        AgentRetrievalStepStatusDto, NodeId, NodeKind, PacketPlanDto, PacketProbeDto,
        PacketProbeResolutionDto, PacketProbeResolutionStatusDto, PacketTaskClassDto,
        SearchHitOrigin,
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
    fn merge_after_drill_cannot_emit_another_drill() {
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
            include_evidence: true,
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
        assert_eq!(disposition.kind, PacketDispositionKindDto::Supported);
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
}
