//! Public evidence-only projection facade for CodeStory schema 3.

use std::{cmp::Reverse, collections::HashSet};

use codestory_contracts::{
    api::{
        AgentAnswerDto, AgentPacketDto, AgentPacketRequestDto, PacketDispositionKindDto, SearchHit,
        SearchResultsDto, SupportUnitDto, SupportUnitKindDto, decode_drill_option_id,
    },
    packet_projection_v3::{
        BoundedVecV3, ContextEvidenceRowV3Dto, ContextProjectionV3Dto, ContextTargetV3Dto,
        ContinuationStateV3Dto, DiagnosticCategoryV3Dto, DiagnosticCodeTextV3,
        DiagnosticsCapabilityV3Dto, EvidenceIdentityV3Dto, EvidenceKindV3Dto, GapIdentityV3Dto,
        GapKindV3Dto, IdentityTextV3, PacketEvidenceRowV3Dto, PacketProjectionV3Dto, PathTextV3,
        ProjectionGapRowV3Dto, RetrievalStateDescriptorV3Dto, RetrievalStateV3Dto,
        SearchEvidenceRowV3Dto, SearchProjectionV3Dto, Sha256DigestV3Dto, SummaryTextV3,
        SymbolIdTextV3,
    },
};
use sha2::{Digest, Sha256};

use crate::{
    agent::{
        packet_execution_record_v3::{
            FinalizedDiagnosticSourceRowV3, FinalizedPacketExecutionInputV3, PacketProfileV3,
            PacketRequestFingerprintV3, build_packet_execution_record_v3,
        },
        packet_projection_v3::{
            DiagnosticArtifactBuildV3, FinalizedContextProjectionInputV3,
            FinalizedSearchProjectionInputV3, build_context_projection_v3,
            build_diagnostic_artifact_v3, build_packet_projection_v3, build_search_projection_v3,
            finalize_packet_projection_v3,
        },
    },
    services::PublicOperationService,
};

/// Immutable bytes and identities the transport needs to mint one capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketDiagnosticProjectionV3 {
    pub bytes: Vec<u8>,
    pub packet_id: String,
    pub project_identity: String,
    pub core_generation: String,
    pub core_run: String,
    pub retrieval_generation: Option<String>,
    pub request_digest: String,
}

/// One finalized packet projection plus its immutable diagnostic artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketEvidenceProductV3 {
    pub projection: PacketProjectionV3Dto,
    pub diagnostics: PacketDiagnosticProjectionV3,
}

/// Re-apply the packet budget against an adapter's complete serialized
/// representation. This is required when the adapter adds publication
/// metadata or mirrors the root after the runtime projection was built.
pub fn finalize_packet_projection_v3_for_representation(
    projection: &mut PacketProjectionV3Dto,
    measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
) -> Result<usize, codestory_contracts::api::ApiError> {
    finalize_packet_projection_v3(projection, measure)
        .map_err(|error| projection_error("packet representation budget", error))
}

/// Convert the runtime's finalized packet execution into the only public v3
/// packet vocabulary. The legacy disposition is consumed only to retain gaps
/// and a bounded continuation; it is never serialized or treated as proof.
pub fn project_packet_v3(
    service: &PublicOperationService,
    caller_id: &str,
    request: &AgentPacketRequestDto,
    packet: &AgentPacketDto,
    mut measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
) -> Result<PacketEvidenceProductV3, codestory_contracts::api::ApiError> {
    let PacketEvidenceSelectionV3 {
        rows: evidence,
        was_bounded: evidence_was_bounded,
    } = packet_evidence_selection(
        &packet.support,
        &packet_evidence_ranking_terms(&request.question, &request.option_ids),
    );

    let mut gaps = packet_gaps(packet);
    if evidence_was_bounded {
        gaps.push(gap_row(
            "evidence-projection-bounded",
            GapKindV3Dto::OutputBudgetExceeded,
            Some(
                "Additional internal support rows were omitted from the bounded public projection.",
            ),
        )?);
    }
    canonicalize_gaps(&mut gaps);
    let continuation = packet_continuation(packet, &mut gaps)?;
    canonicalize_gaps(&mut gaps);
    let diagnostics = diagnostic_rows(&packet.answer, &gaps)?;
    let retrieval = retrieval_state(packet.answer.retrieval_trace.retrieval_publication.as_ref());
    let input = FinalizedPacketExecutionInputV3::new(
        identity(caller_id, "caller_id")?,
        identity(&packet.answer.retrieval_trace.request_id, "request_id")?,
        PacketRequestFingerprintV3::from_current_request(request, PacketProfileV3::Auto),
        evidence,
        gaps,
        continuation,
        retrieval,
        diagnostics,
    );
    let record = build_packet_execution_record_v3(service, &input)
        .map_err(|error| projection_error("packet record", error))?;
    let artifact = build_diagnostic_artifact_v3(&record)
        .map_err(|error| projection_error("packet diagnostics", error))?;
    let (bytes, reference) = match artifact {
        DiagnosticArtifactBuildV3::Complete {
            bytes, reference, ..
        } => (bytes.as_slice().to_vec(), reference),
        DiagnosticArtifactBuildV3::TooLarge { .. } => {
            return Err(codestory_contracts::api::ApiError::internal(
                "Packet diagnostic projection exceeded its closed v3 bound.",
            ));
        }
    };
    let projection = build_packet_projection_v3(
        &record,
        DiagnosticsCapabilityV3Dto::Available { reference },
        &mut measure,
    )
    .map_err(|error| projection_error("packet projection", error))?;
    Ok(PacketEvidenceProductV3 {
        projection,
        diagnostics: PacketDiagnosticProjectionV3 {
            bytes,
            packet_id: record.packet_id().as_str().to_owned(),
            project_identity: record.core_publication().project_id.as_str().to_owned(),
            core_generation: record.core_publication().generation_id.as_str().to_owned(),
            core_run: record.core_publication().run_id.as_str().to_owned(),
            retrieval_generation: record
                .retrieval_publication()
                .map(|publication| publication.retrieval_generation.as_str().to_owned()),
            request_digest: record.request_sha256().as_str().to_owned(),
        },
    })
}

struct PacketEvidenceSelectionV3 {
    rows: Vec<PacketEvidenceRowV3Dto>,
    was_bounded: bool,
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct PacketEvidenceContentKeyV3 {
    kind: u8,
    path: Option<String>,
    symbol_id: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
    summary: Option<String>,
}

fn packet_evidence_selection(
    support: &[SupportUnitDto],
    ranking_terms: &HashSet<String>,
) -> PacketEvidenceSelectionV3 {
    let source_symbols = support
        .iter()
        .filter(|unit| {
            unit.kind == SupportUnitKindDto::SourceRange
                && unit
                    .snippet
                    .as_deref()
                    .is_some_and(|snippet| !snippet.trim().is_empty())
        })
        .filter_map(|unit| Some((unit.path.as_deref()?, unit.symbol_id.as_deref()?)))
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut distinct = Vec::new();
    for unit in support
        .iter()
        .filter(|unit| unit.kind != SupportUnitKindDto::CompleteQueryNegative)
    {
        if unit.kind == SupportUnitKindDto::SymbolLocation
            && unit
                .path
                .as_deref()
                .zip(unit.symbol_id.as_deref())
                .is_some_and(|identity| source_symbols.contains(&identity))
        {
            continue;
        }
        let Some(row) = packet_evidence_row(0, unit) else {
            continue;
        };
        if seen.insert(packet_evidence_content_key(&row)) {
            let relevance = packet_evidence_relevance(&row, ranking_terms);
            distinct.push((unit.kind, row, relevance));
        }
    }

    let distinct_rows = distinct.len();
    let selected = select_packet_evidence_indices(&distinct);
    let mut rows = Vec::with_capacity(selected.len());
    for index in selected {
        let mut row = distinct[index].1.clone();
        row.identity = evidence_identity(&format!("packet-evidence-{:03}", rows.len()));
        rows.push(row);
    }

    PacketEvidenceSelectionV3 {
        rows,
        was_bounded: distinct_rows > PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3,
    }
}

fn select_packet_evidence_indices(
    distinct: &[(SupportUnitKindDto, PacketEvidenceRowV3Dto, usize)],
) -> Vec<usize> {
    // Close the mandatory identity envelope before transport compaction. Preserve the runtime's
    // native rank inside each class while reserving useful context for paths, symbols, and edges.
    let mut selected = Vec::with_capacity(distinct.len().min(PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3));
    let mut selected_indices = HashSet::new();
    let source_order = packet_evidence_kind_order(distinct, SupportUnitKindDto::SourceRange);
    let location_order = packet_evidence_kind_order(distinct, SupportUnitKindDto::SymbolLocation);
    let relation_order = packet_evidence_kind_order(distinct, SupportUnitKindDto::TypedGraphEdge);

    let mut source_paths = HashSet::new();
    for index in &source_order {
        let row = &distinct[*index].1;
        if source_paths.insert(row.path.as_ref().map(|path| path.as_str())) {
            selected.push(*index);
            selected_indices.insert(*index);
            if selected.len() == PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3 {
                break;
            }
        }
    }

    let mut source_symbols = selected
        .iter()
        .filter_map(|index| distinct[*index].1.symbol_id.as_ref())
        .map(|symbol| symbol.as_str())
        .collect::<HashSet<_>>();
    for index in &source_order {
        if selected.len() == PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3 {
            break;
        }
        let row = &distinct[*index].1;
        if !selected_indices.contains(index)
            && row
                .symbol_id
                .as_ref()
                .is_some_and(|symbol| source_symbols.insert(symbol.as_str()))
        {
            selected.push(*index);
            selected_indices.insert(*index);
        }
    }
    select_packet_evidence_kind(
        &source_order,
        PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3,
        &mut selected,
        &mut selected_indices,
    );
    let location_target = selected
        .len()
        .saturating_add(PACKET_PUBLIC_LOCATION_ROWS_TARGET_V3)
        .min(PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
    select_packet_evidence_kind(
        &location_order,
        location_target,
        &mut selected,
        &mut selected_indices,
    );
    let relation_target = selected
        .len()
        .saturating_add(PACKET_PUBLIC_RELATION_ROWS_TARGET_V3)
        .min(PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
    let mut relation_callers = HashSet::new();
    for index in &relation_order {
        if selected.len() == relation_target {
            break;
        }
        let caller = distinct[*index]
            .1
            .summary
            .as_ref()
            .and_then(|summary| summary.as_str().split_once(" -[").map(|(caller, _)| caller));
        if !selected_indices.contains(index) && relation_callers.insert(caller) {
            selected.push(*index);
            selected_indices.insert(*index);
        }
    }
    select_packet_evidence_kind(
        &relation_order,
        relation_target,
        &mut selected,
        &mut selected_indices,
    );

    let mut remainder = (0..distinct.len())
        .filter(|index| !selected_indices.contains(index))
        .collect::<Vec<_>>();
    remainder.sort_by_key(|index| {
        (
            Reverse(distinct[*index].2),
            packet_evidence_kind_priority(distinct[*index].0),
            *index,
        )
    });
    for index in remainder {
        if selected.len() == PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3 {
            break;
        }
        selected.push(index);
        selected_indices.insert(index);
    }
    selected
}

fn packet_evidence_kind_priority(kind: SupportUnitKindDto) -> u8 {
    match kind {
        SupportUnitKindDto::SourceRange => 0,
        SupportUnitKindDto::SymbolLocation => 1,
        SupportUnitKindDto::TypedGraphEdge => 2,
        SupportUnitKindDto::CompleteQueryNegative => 3,
    }
}

fn select_packet_evidence_kind(
    ordered_indices: &[usize],
    maximum_total: usize,
    selected: &mut Vec<usize>,
    selected_indices: &mut HashSet<usize>,
) {
    for index in ordered_indices {
        if selected.len() == maximum_total {
            break;
        }
        if selected_indices.insert(*index) {
            selected.push(*index);
        }
    }
}

fn packet_evidence_kind_order(
    distinct: &[(SupportUnitKindDto, PacketEvidenceRowV3Dto, usize)],
    selected_kind: SupportUnitKindDto,
) -> Vec<usize> {
    let mut indices = distinct
        .iter()
        .enumerate()
        .filter_map(|(index, (kind, _, _))| (*kind == selected_kind).then_some(index))
        .collect::<Vec<_>>();
    indices.sort_by_key(|index| (Reverse(distinct[*index].2), *index));
    indices
}

fn packet_evidence_ranking_terms(question: &str, option_ids: &[String]) -> HashSet<String> {
    let mut terms = packet_evidence_terms(question);
    for option_id in option_ids {
        if let Some((_, target)) = decode_drill_option_id(option_id) {
            terms.extend(packet_evidence_terms(&target));
        }
    }
    terms
}

fn packet_evidence_relevance(
    row: &PacketEvidenceRowV3Dto,
    ranking_terms: &HashSet<String>,
) -> usize {
    let mut text = String::new();
    for value in [
        row.path.as_ref().map(|value| value.as_str()),
        row.symbol_id.as_ref().map(|value| value.as_str()),
        row.summary.as_ref().map(|value| value.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        text.push_str(value);
        text.push(' ');
    }
    let evidence_terms = packet_evidence_terms(&text);
    ranking_terms
        .iter()
        .filter(|expected| {
            evidence_terms
                .iter()
                .any(|observed| packet_evidence_term_matches(expected, observed))
        })
        .count()
}

fn packet_evidence_terms(value: &str) -> HashSet<String> {
    let mut normalized = String::with_capacity(value.len());
    let mut previous_was_lower_or_digit = false;
    for ch in value.chars() {
        if ch.is_alphanumeric() {
            if ch.is_uppercase() && previous_was_lower_or_digit {
                normalized.push(' ');
            }
            for lowercase in ch.to_lowercase() {
                normalized.push(lowercase);
            }
            previous_was_lower_or_digit = ch.is_lowercase() || ch.is_ascii_digit();
        } else {
            normalized.push(' ');
            previous_was_lower_or_digit = false;
        }
    }
    normalized
        .split_whitespace()
        .filter(|term| term.len() >= 4 && !packet_evidence_stopword(term))
        .map(str::to_owned)
        .collect()
}

fn packet_evidence_stopword(term: &str) -> bool {
    matches!(
        term,
        "cite"
            | "cited"
            | "cites"
            | "explain"
            | "file"
            | "files"
            | "from"
            | "into"
            | "name"
            | "named"
            | "source"
            | "sources"
            | "supporting"
            | "symbol"
            | "symbols"
            | "that"
            | "them"
            | "then"
            | "this"
            | "through"
            | "with"
    )
}

fn packet_evidence_term_matches(expected: &str, observed: &str) -> bool {
    expected == observed
        || (expected.chars().count() >= 4
            && observed.chars().count() >= 4
            && expected.chars().take(4).eq(observed.chars().take(4)))
}

fn packet_evidence_content_key(row: &PacketEvidenceRowV3Dto) -> PacketEvidenceContentKeyV3 {
    PacketEvidenceContentKeyV3 {
        kind: match row.kind {
            EvidenceKindV3Dto::ExactSource => 0,
            EvidenceKindV3Dto::StructuralSource => 1,
            EvidenceKindV3Dto::GraphRelation => 2,
            EvidenceKindV3Dto::RetrievalExcerpt => 3,
        },
        path: row.path.as_ref().map(|value| value.as_str().to_owned()),
        symbol_id: row
            .symbol_id
            .as_ref()
            .map(|value| value.as_str().to_owned()),
        start_line: row.start_line,
        end_line: row.end_line,
        summary: row.summary.as_ref().map(|value| value.as_str().to_owned()),
    }
}

#[cfg(test)]
fn packet_evidence_rows(support: &[SupportUnitDto]) -> Vec<PacketEvidenceRowV3Dto> {
    packet_evidence_selection(support, &HashSet::new()).rows
}

#[cfg(test)]
fn packet_evidence_rows_for_request(
    support: &[SupportUnitDto],
    question: &str,
    option_ids: &[String],
) -> Vec<PacketEvidenceRowV3Dto> {
    packet_evidence_selection(
        support,
        &packet_evidence_ranking_terms(question, option_ids),
    )
    .rows
}

#[cfg(test)]
fn packet_evidence_was_bounded(support: &[SupportUnitDto]) -> bool {
    packet_evidence_selection(support, &HashSet::new()).was_bounded
}

const PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3: usize = 8;
const PACKET_PUBLIC_LOCATION_ROWS_TARGET_V3: usize = 2;
const PACKET_PUBLIC_RELATION_ROWS_TARGET_V3: usize = 4;
const PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3: usize = 16;

/// Project a context answer without exposing answer confidence or claim state.
pub fn project_context_v3(
    service: &PublicOperationService,
    caller_id: &str,
    target_path: Option<&str>,
    target_symbol_id: Option<&str>,
    answer: &AgentAnswerDto,
) -> Result<ContextProjectionV3Dto, codestory_contracts::api::ApiError> {
    let (identity, publication, retrieval) = projection_envelope(
        service,
        caller_id,
        &answer.prompt,
        &answer.retrieval_trace.request_id,
        answer.retrieval_trace.retrieval_publication.as_ref(),
    )?;
    let evidence = answer
        .citations
        .iter()
        .enumerate()
        .filter_map(|(index, citation)| {
            let path = citation.file_path.as_deref()?;
            let line = citation.line.filter(|line| *line > 0);
            Some(ContextEvidenceRowV3Dto {
                identity: evidence_identity(&format!("context-{index}-{}", citation.node_id.0)),
                path: path_text(path),
                symbol_id: citation
                    .resolvable
                    .then(|| symbol_text(Some(citation.node_id.0.as_str())))
                    .flatten(),
                start_line: line,
                end_line: line,
                excerpt: citation.source_excerpt.as_deref().map(bounded_excerpt),
            })
        })
        .take(codestory_contracts::packet_projection_v3::EVIDENCE_ROWS_MAX_V3)
        .collect();
    let gaps = answer_gap_rows(answer)?;
    build_context_projection_v3(&FinalizedContextProjectionInputV3::new(
        identity,
        publication,
        retrieval,
        ContextTargetV3Dto {
            path: target_path.map(path_text),
            symbol_id: symbol_text(target_symbol_id),
        },
        evidence,
        gaps,
        None,
        DiagnosticsCapabilityV3Dto::Unavailable,
        Vec::new(),
    ))
    .map_err(|error| projection_error("context projection", error))
}

/// Project ranked search rows as evidence locations without exposing scores,
/// ranking rationale, or sufficiency fields.
pub fn project_search_v3(
    service: &PublicOperationService,
    caller_id: &str,
    results: &SearchResultsDto,
) -> Result<SearchProjectionV3Dto, codestory_contracts::api::ApiError> {
    let request_id = format!("search-{}", digest_hex(results.query.as_bytes()));
    let (identity, publication, retrieval) = projection_envelope(
        service,
        caller_id,
        &results.query,
        &request_id,
        results.retrieval_publication.as_ref(),
    )?;
    let evidence = results
        .hits
        .iter()
        .enumerate()
        .filter_map(|(index, hit)| search_evidence_row(index, hit))
        .take(codestory_contracts::packet_projection_v3::EVIDENCE_ROWS_MAX_V3)
        .collect();
    let mut gaps = Vec::new();
    if results.hits.len() > codestory_contracts::packet_projection_v3::EVIDENCE_ROWS_MAX_V3 {
        gaps.push(gap_row(
            "search-projection-bounded",
            GapKindV3Dto::OutputBudgetExceeded,
            Some("Additional search rows were omitted from the bounded public projection."),
        )?);
    }
    if results.retrieval_publication.is_none() {
        gaps.push(gap_row(
            "search-retrieval-unavailable",
            GapKindV3Dto::RetrievalUnavailable,
            Some("No complete retrieval publication was available for this search."),
        )?);
    }
    build_search_projection_v3(&FinalizedSearchProjectionInputV3::new(
        identity,
        publication,
        retrieval,
        evidence,
        gaps,
        None,
        DiagnosticsCapabilityV3Dto::Unavailable,
        Vec::new(),
    ))
    .map_err(|error| projection_error("search projection", error))
}

fn projection_envelope(
    service: &PublicOperationService,
    caller_id: &str,
    question: &str,
    request_id: &str,
    retrieval_publication: Option<&codestory_contracts::api::EmbeddingVectorPublicationIdentityDto>,
) -> Result<
    (
        codestory_contracts::packet_projection_v3::PacketRequestIdentityV3Dto,
        codestory_contracts::packet_projection_v3::PublicationIdentityV3Dto,
        RetrievalStateDescriptorV3Dto,
    ),
    codestory_contracts::api::ApiError,
> {
    let request = AgentPacketRequestDto {
        question: question.to_owned(),
        budget: Default::default(),
        task_class: None,
        probes: Vec::new(),
        extra_probes: Vec::new(),
        latency_budget_ms: None,
        parent_packet_id: None,
        option_ids: Vec::new(),
        core_generation_id: None,
        retrieval_generation: None,
    };
    let retrieval = retrieval_state(retrieval_publication);
    let input = FinalizedPacketExecutionInputV3::new(
        identity(caller_id, "caller_id")?,
        identity(request_id, "request_id")?,
        PacketRequestFingerprintV3::from_current_request(&request, PacketProfileV3::Auto),
        Vec::new(),
        Vec::new(),
        None,
        retrieval.clone(),
        Vec::new(),
    );
    let record = build_packet_execution_record_v3(service, &input)
        .map_err(|error| projection_error("projection envelope", error))?;
    let packet =
        build_packet_projection_v3(&record, DiagnosticsCapabilityV3Dto::Unavailable, |_| Ok(0))
            .map_err(|error| projection_error("projection envelope", error))?;
    let (identity, publication) = match packet {
        PacketProjectionV3Dto::Complete {
            identity,
            publication,
            ..
        }
        | PacketProjectionV3Dto::BudgetExceeded {
            identity,
            publication,
            ..
        } => (identity, publication),
    };
    Ok((identity, publication, retrieval))
}

fn packet_evidence_row(index: usize, unit: &SupportUnitDto) -> Option<PacketEvidenceRowV3Dto> {
    if unit.kind == SupportUnitKindDto::CompleteQueryNegative {
        return None;
    }
    Some(PacketEvidenceRowV3Dto {
        // The execution record canonicalizes by identity. A fixed-width rank
        // prefix keeps that canonical order equal to the projection's useful-
        // context order while avoiding repository-shaped identifiers.
        identity: evidence_identity(&format!("packet-evidence-{index:03}")),
        kind: match unit.kind {
            SupportUnitKindDto::SymbolLocation | SupportUnitKindDto::SourceRange => {
                EvidenceKindV3Dto::ExactSource
            }
            SupportUnitKindDto::TypedGraphEdge => EvidenceKindV3Dto::GraphRelation,
            SupportUnitKindDto::CompleteQueryNegative => return None,
        },
        path: unit.path.as_deref().map(path_text),
        symbol_id: symbol_text(unit.symbol_id.as_deref()),
        start_line: unit.start_line,
        end_line: unit.end_line,
        summary: packet_evidence_summary(unit),
    })
}

const PACKET_EVIDENCE_SUMMARY_MAX_BYTES_V3: usize = 512;

fn packet_evidence_summary(unit: &SupportUnitDto) -> Option<SummaryTextV3> {
    let text = match unit.kind {
        SupportUnitKindDto::SourceRange => unit
            .snippet
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&unit.summary)
            .to_owned(),
        SupportUnitKindDto::TypedGraphEdge => {
            match (
                unit.from_symbol.as_deref(),
                unit.edge_kind.as_deref(),
                unit.to_symbol.as_deref(),
            ) {
                (Some(from), Some(edge), Some(to)) => format!("{from} -[{edge}]-> {to}"),
                _ => unit.summary.clone(),
            }
        }
        SupportUnitKindDto::SymbolLocation => packet_symbol_location_summary(unit),
        SupportUnitKindDto::CompleteQueryNegative => return None,
    };
    Some(summary_text_bounded(
        &text,
        PACKET_EVIDENCE_SUMMARY_MAX_BYTES_V3,
    ))
}

fn packet_symbol_location_summary(unit: &SupportUnitDto) -> String {
    let Some((label, _)) = unit.summary.rsplit_once(" at ") else {
        return unit.summary.clone();
    };
    let label_is_absolute = label.starts_with('/')
        || label
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
    if !label_is_absolute {
        return unit.summary.clone();
    }
    match (unit.path.as_deref(), unit.start_line) {
        (Some(path), Some(line)) => format!("{path}:{line}"),
        (Some(path), None) => path.to_owned(),
        _ => "project source location".to_owned(),
    }
}

fn packet_gaps(packet: &AgentPacketDto) -> Vec<ProjectionGapRowV3Dto> {
    let mut gaps = packet
        .support
        .iter()
        .filter(|unit| unit.kind == SupportUnitKindDto::CompleteQueryNegative)
        .map(|unit| {
            gap_row(
                &format!("negative-not-authority-{}", unit.id),
                GapKindV3Dto::EvidenceMissing,
                Some("A bounded negative query was retained as a gap, not proof of absence."),
            )
            .expect("static bounded gap")
        })
        .collect::<Vec<_>>();
    match packet.disposition.kind {
        PacketDispositionKindDto::Supported => {}
        PacketDispositionKindDto::DrillOnce => {
            if let Some(drill) = &packet.disposition.drill {
                gaps.extend(drill.options.iter().map(|option| {
                    gap_row(
                        &option.id,
                        GapKindV3Dto::ContinuationRequired,
                        packet.disposition.reason.as_deref(),
                    )
                    .expect("bounded disposition gap")
                }));
            }
        }
        PacketDispositionKindDto::NotEstablished => gaps.push(
            gap_row(
                "packet-evidence-not-established",
                GapKindV3Dto::EvidenceMissing,
                packet.disposition.reason.as_deref(),
            )
            .expect("bounded disposition gap"),
        ),
        PacketDispositionKindDto::Unavailable => gaps.push(
            gap_row(
                "packet-evidence-unavailable",
                GapKindV3Dto::RetrievalUnavailable,
                packet.disposition.reason.as_deref(),
            )
            .expect("bounded disposition gap"),
        ),
    }
    gaps
}

fn packet_continuation(
    packet: &AgentPacketDto,
    gaps: &mut Vec<ProjectionGapRowV3Dto>,
) -> Result<Option<ContinuationStateV3Dto>, codestory_contracts::api::ApiError> {
    let Some(drill) = packet.disposition.drill.as_ref() else {
        return Ok(None);
    };
    if drill.remaining_rounds == 0 {
        return Ok(None);
    }
    if drill.gap_ids.is_empty() {
        gaps.push(gap_row(
            "packet-continuation-required",
            GapKindV3Dto::ContinuationRequired,
            packet.disposition.reason.as_deref(),
        )?);
    }
    canonicalize_gaps(gaps);
    let references = gaps
        .iter()
        .filter(|gap| gap.kind == GapKindV3Dto::ContinuationRequired)
        .map(|gap| gap.identity.clone())
        .take(codestory_contracts::packet_projection_v3::REFERENCE_ROWS_MAX_V3)
        .collect();
    ContinuationStateV3Dto::new(
        identity(&drill.parent_packet_id, "continuation_id")?,
        u16::try_from(drill.remaining_rounds).unwrap_or(u16::MAX),
        BoundedVecV3::new(references).expect("bounded continuation references"),
    )
    .map(Some)
    .map_err(|error| projection_error("packet continuation", error))
}

fn diagnostic_rows(
    answer: &AgentAnswerDto,
    gaps: &[ProjectionGapRowV3Dto],
) -> Result<Vec<FinalizedDiagnosticSourceRowV3>, codestory_contracts::api::ApiError> {
    let mut rows = Vec::new();
    for (index, annotation) in answer.retrieval_trace.annotations.iter().enumerate() {
        let gap_ids = if annotation.is_gap() {
            gaps.iter().map(|gap| gap.identity.clone()).collect()
        } else {
            Vec::new()
        };
        rows.push(FinalizedDiagnosticSourceRowV3::new(
            identity(&format!("diagnostic-{index}"), "diagnostic_id")?,
            DiagnosticCategoryV3Dto::Retrieval,
            DiagnosticCodeTextV3::new(if annotation.is_gap() {
                "retrieval_gap"
            } else {
                "retrieval_observation"
            })
            .expect("static diagnostic code"),
            Vec::new(),
            gap_ids,
        ));
    }
    Ok(rows)
}

fn answer_gap_rows(
    answer: &AgentAnswerDto,
) -> Result<Vec<ProjectionGapRowV3Dto>, codestory_contracts::api::ApiError> {
    answer
        .retrieval_trace
        .annotations
        .iter()
        .enumerate()
        .filter(|(_, annotation)| annotation.is_gap())
        .map(|(index, _)| {
            gap_row(
                &format!("context-gap-{index}"),
                GapKindV3Dto::EvidenceMissing,
                Some("Context retrieval reported an evidence gap; details are diagnostic-only."),
            )
        })
        .collect()
}

fn search_evidence_row(index: usize, hit: &SearchHit) -> Option<SearchEvidenceRowV3Dto> {
    let path = hit.file_path.as_deref()?;
    Some(SearchEvidenceRowV3Dto {
        identity: evidence_identity(&format!("search-{index}-{}", hit.node_id.0)),
        path: path_text(path),
        symbol_id: hit.resolvable.then(|| symbol_id_text(&hit.node_id.0)),
        start_line: hit.line,
        end_line: hit.line,
        excerpt: hit.source_excerpt.as_deref().map(bounded_excerpt),
    })
}

fn retrieval_state(
    publication: Option<&codestory_contracts::api::EmbeddingVectorPublicationIdentityDto>,
) -> RetrievalStateDescriptorV3Dto {
    match publication {
        Some(publication) => RetrievalStateDescriptorV3Dto {
            state: RetrievalStateV3Dto::Full,
            generation_id: Some(identity_text(&publication.retrieval_generation)),
        },
        None => RetrievalStateDescriptorV3Dto {
            state: RetrievalStateV3Dto::Degraded,
            generation_id: None,
        },
    }
}

fn canonicalize_gaps(gaps: &mut Vec<ProjectionGapRowV3Dto>) {
    gaps.sort_by(|left, right| left.identity.cmp(&right.identity));
    gaps.dedup_by(|left, right| left.identity == right.identity);
    gaps.truncate(codestory_contracts::packet_projection_v3::GAP_ROWS_MAX_V3);
}

fn gap_row(
    id: &str,
    kind: GapKindV3Dto,
    message: Option<&str>,
) -> Result<ProjectionGapRowV3Dto, codestory_contracts::api::ApiError> {
    Ok(ProjectionGapRowV3Dto {
        identity: GapIdentityV3Dto {
            gap_id: identity(id, "gap_id")?,
        },
        kind,
        message: message.map(|value| {
            codestory_contracts::packet_projection_v3::MessageTextV3::new(truncate_utf8(
                value,
                codestory_contracts::packet_projection_v3::MESSAGE_MAX_BYTES_V3,
            ))
            .expect("truncated gap message is bounded")
        }),
    })
}

fn evidence_identity(value: &str) -> EvidenceIdentityV3Dto {
    EvidenceIdentityV3Dto {
        evidence_id: identity_text(value),
    }
}

fn identity(
    value: &str,
    field: &str,
) -> Result<IdentityTextV3, codestory_contracts::api::ApiError> {
    if value.trim().is_empty() {
        return Err(codestory_contracts::api::ApiError::internal(format!(
            "The finalized v3 {field} was empty."
        )));
    }
    Ok(identity_text(value))
}

fn identity_text(value: &str) -> IdentityTextV3 {
    let bounded = if value.len() <= codestory_contracts::packet_projection_v3::IDENTITY_MAX_BYTES_V3
    {
        value.to_owned()
    } else {
        format!("sha256-{}", digest_hex(value.as_bytes()))
    };
    IdentityTextV3::new(bounded).expect("bounded identity")
}

fn path_text(value: &str) -> PathTextV3 {
    PathTextV3::new(truncate_utf8(
        value,
        codestory_contracts::packet_projection_v3::PATH_MAX_BYTES_V3,
    ))
    .expect("truncated path is bounded")
}

fn symbol_text(value: Option<&str>) -> Option<SymbolIdTextV3> {
    value.map(symbol_id_text)
}

fn symbol_id_text(value: &str) -> SymbolIdTextV3 {
    SymbolIdTextV3::new(truncate_utf8(
        value,
        codestory_contracts::packet_projection_v3::SYMBOL_ID_MAX_BYTES_V3,
    ))
    .expect("truncated symbol id is bounded")
}

fn summary_text_bounded(value: &str, maximum: usize) -> SummaryTextV3 {
    SummaryTextV3::new(truncate_utf8(value, maximum)).expect("truncated summary is bounded")
}

fn bounded_excerpt(value: &str) -> codestory_contracts::packet_projection_v3::ExcerptTextV3 {
    codestory_contracts::packet_projection_v3::ExcerptTextV3::new(truncate_utf8(
        value,
        codestory_contracts::packet_projection_v3::EXCERPT_MAX_BYTES_V3,
    ))
    .expect("truncated excerpt is bounded")
}

fn truncate_utf8(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn digest_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn projection_error(
    stage: &str,
    error: impl std::fmt::Debug,
) -> codestory_contracts::api::ApiError {
    codestory_contracts::api::ApiError::internal(format!(
        "Failed to build the closed v3 {stage}: {error:?}"
    ))
}

#[allow(dead_code)]
fn _sha_type_is_public(_: Sha256DigestV3Dto) {}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, atomic::AtomicBool};

    use codestory_contracts::api::IndexMode;
    use serde_json::json;

    use super::*;

    fn support_unit(kind: SupportUnitKindDto) -> SupportUnitDto {
        SupportUnitDto {
            id: "unit-1".to_string(),
            kind,
            summary: "fixture".to_string(),
            path: None,
            symbol_id: None,
            start_line: None,
            end_line: None,
            snippet: None,
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        }
    }

    #[test]
    fn complete_query_negative_is_a_gap_not_a_bounded_evidence_row() {
        let support = vec![support_unit(SupportUnitKindDto::CompleteQueryNegative)];

        assert!(!packet_evidence_was_bounded(&support));
        assert!(!packet_evidence_was_bounded(&[support_unit(
            SupportUnitKindDto::SourceRange
        )]));
    }

    #[test]
    fn packet_evidence_keeps_ranked_source_and_relation_context_in_compact_identities() {
        let mut source = support_unit(SupportUnitKindDto::SourceRange);
        source.id = "repository-shaped-source-id".to_owned();
        source.summary = "source range".to_owned();
        source.snippet = Some(format!("fn useful() {{}}\n{}", "x".repeat(700)));
        source.path = Some("src/useful.rs".to_owned());
        let source = packet_evidence_row(7, &source).expect("source evidence");
        assert_eq!(source.identity.evidence_id.as_str(), "packet-evidence-007");
        assert_eq!(source.summary.as_ref().unwrap().as_str().len(), 512);
        assert!(
            source
                .summary
                .as_ref()
                .unwrap()
                .as_str()
                .starts_with("fn useful()")
        );

        let mut relation = support_unit(SupportUnitKindDto::TypedGraphEdge);
        relation.from_symbol = Some("caller".to_owned());
        relation.edge_kind = Some("CALL".to_owned());
        relation.to_symbol = Some("callee".to_owned());
        let relation = packet_evidence_row(8, &relation).expect("relation evidence");
        assert_eq!(
            relation.summary.as_ref().unwrap().as_str(),
            "caller -[CALL]-> callee"
        );
    }

    #[test]
    fn packet_evidence_prioritizes_source_excerpts_locations_and_relations() {
        let mut location = support_unit(SupportUnitKindDto::SymbolLocation);
        location.summary = "location".to_owned();
        let mut relation = support_unit(SupportUnitKindDto::TypedGraphEdge);
        relation.from_symbol = Some("caller".to_owned());
        relation.edge_kind = Some("CALL".to_owned());
        relation.to_symbol = Some("callee".to_owned());
        let mut source = support_unit(SupportUnitKindDto::SourceRange);
        source.snippet = Some("fn useful() {}".to_owned());

        let rows = packet_evidence_rows(&[location, relation, source]);
        assert_eq!(
            rows.iter()
                .map(|row| row.summary.as_ref().unwrap().as_str())
                .collect::<Vec<_>>(),
            ["fn useful() {}", "location", "caller -[CALL]-> callee"]
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.identity.evidence_id.as_str())
                .collect::<Vec<_>>(),
            [
                "packet-evidence-000",
                "packet-evidence-001",
                "packet-evidence-002"
            ]
        );
    }

    #[test]
    fn packet_evidence_deduplicates_identical_public_rows_before_applying_the_bound() {
        let duplicate = || {
            let mut unit = support_unit(SupportUnitKindDto::SourceRange);
            unit.path = Some("src/repeated.rs".to_owned());
            unit.symbol_id = Some("repeated".to_owned());
            unit.start_line = Some(7);
            unit.end_line = Some(9);
            unit.snippet = Some("fn repeated() {}".to_owned());
            unit
        };
        let mut support = (0..40)
            .map(|index| {
                let mut unit = duplicate();
                unit.id = format!("duplicate-{index}");
                unit
            })
            .collect::<Vec<_>>();
        let mut distinct = duplicate();
        distinct.id = "distinct".to_owned();
        distinct.path = Some("src/distinct.rs".to_owned());
        support.push(distinct);

        let evidence = packet_evidence_rows(&support);

        assert_eq!(evidence.len(), 2);
        assert_eq!(
            evidence
                .iter()
                .map(|row| row.path.as_ref().unwrap().as_str())
                .collect::<Vec<_>>(),
            ["src/repeated.rs", "src/distinct.rs"]
        );
        assert!(
            !packet_evidence_was_bounded(&support),
            "discarding duplicate renderings must not report missing evidence"
        );
    }

    #[test]
    fn packet_evidence_drops_a_location_when_the_same_symbol_has_source_content() {
        let mut location = support_unit(SupportUnitKindDto::SymbolLocation);
        location.path = Some("src/mapper.cs".to_owned());
        location.symbol_id = Some("Mapper.Map".to_owned());
        location.start_line = Some(12);
        let mut source = support_unit(SupportUnitKindDto::SourceRange);
        source.path = location.path.clone();
        source.symbol_id = location.symbol_id.clone();
        source.start_line = location.start_line;
        source.end_line = Some(18);
        source.snippet = Some("public object Map(...) => MapCore(...);".to_owned());

        let evidence = packet_evidence_rows(&[location, source]);

        assert_eq!(evidence.len(), 1);
        assert_eq!(
            evidence[0].summary.as_ref().unwrap().as_str(),
            "public object Map(...) => MapCore(...);"
        );
    }

    #[test]
    fn packet_evidence_relation_floor_spans_distinct_callers_before_repeats() {
        let repeated = (0..8).map(|index| {
            let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
            unit.id = format!("repeated-{index}");
            unit.from_symbol = Some("one_caller".to_owned());
            unit.edge_kind = Some("CALL".to_owned());
            unit.to_symbol = Some(format!("callee-{index}"));
            unit
        });
        let distinct = (0..4).map(|index| {
            let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
            unit.id = format!("distinct-{index}");
            unit.from_symbol = Some(format!("caller-{index}"));
            unit.edge_kind = Some("CALL".to_owned());
            unit.to_symbol = Some(format!("target-{index}"));
            unit
        });

        let evidence = packet_evidence_rows(&repeated.chain(distinct).collect::<Vec<_>>());
        let first_callers = evidence
            .iter()
            .take(PACKET_PUBLIC_RELATION_ROWS_TARGET_V3)
            .filter_map(|row| {
                row.summary
                    .as_ref()
                    .and_then(|summary| summary.as_str().split(" -[").next())
            })
            .collect::<HashSet<_>>();

        assert_eq!(first_callers.len(), PACKET_PUBLIC_RELATION_ROWS_TARGET_V3);
    }

    #[test]
    fn packet_evidence_closes_a_diverse_content_bearing_identity_envelope() {
        let support = (0..15)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("source-{index}");
                unit.path = Some(format!("src/source-{index}.rs"));
                unit.snippet = Some(format!("fn source_{index}() {{}}"));
                unit
            })
            .chain((0..24).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
                unit.id = format!("edge-{index}");
                unit.from_symbol = Some(format!("caller-{index}"));
                unit.edge_kind = Some("CALL".to_owned());
                unit.to_symbol = Some(format!("callee-{index}"));
                unit
            }))
            .chain((0..14).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SymbolLocation);
                unit.id = format!("location-{index}");
                unit.path = Some(format!("src/location-{index}.rs"));
                unit
            }))
            .collect::<Vec<_>>();

        let evidence = packet_evidence_rows(&support);

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert_eq!(
            evidence
                .iter()
                .filter(|row| row.kind == EvidenceKindV3Dto::ExactSource)
                .count(),
            12,
            "the closed envelope keeps ten excerpts and two symbol locations"
        );
        assert_eq!(
            evidence
                .iter()
                .filter(|row| row.kind == EvidenceKindV3Dto::GraphRelation)
                .count(),
            4,
            "the closed envelope reserves four relation receipts"
        );
        assert_eq!(
            evidence
                .iter()
                .filter_map(|row| row.summary.as_ref().map(|summary| summary.as_str()))
                .collect::<Vec<_>>(),
            [
                "fn source_0() {}",
                "fn source_1() {}",
                "fn source_2() {}",
                "fn source_3() {}",
                "fn source_4() {}",
                "fn source_5() {}",
                "fn source_6() {}",
                "fn source_7() {}",
                "fixture",
                "fixture",
                "caller-0 -[CALL]-> callee-0",
                "caller-1 -[CALL]-> callee-1",
                "caller-2 -[CALL]-> callee-2",
                "caller-3 -[CALL]-> callee-3",
                "fn source_8() {}",
                "fn source_9() {}",
            ]
        );
        assert!(packet_evidence_was_bounded(&support));
    }

    #[test]
    fn packet_evidence_uses_distinct_source_paths_before_repeated_ranges() {
        let support = (0..12)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("source-{index}");
                unit.path = Some(if index < 5 {
                    "src/repeated.rs".to_owned()
                } else {
                    format!("src/distinct-{index}.rs")
                });
                unit.start_line = Some(index + 1);
                unit.snippet = Some(format!("source {index}"));
                unit
            })
            .collect::<Vec<_>>();

        let evidence = packet_evidence_rows(&support);
        let first_eight_paths = evidence
            .iter()
            .take(8)
            .filter_map(|row| row.path.as_ref().map(|path| path.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(first_eight_paths.len(), 8);
        assert_eq!(
            first_eight_paths
                .iter()
                .copied()
                .collect::<HashSet<_>>()
                .len(),
            8,
            "each available source path gets one content-bearing row before repeats"
        );
        assert_eq!(first_eight_paths[0], "src/repeated.rs");
    }

    #[test]
    fn packet_evidence_prioritizes_question_terms_inside_each_evidence_class() {
        let mut support = (0..10)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("unrelated-{index}");
                unit.path = Some(format!("src/unrelated-{index}.rs"));
                unit.symbol_id = Some(format!("unrelated-{index}"));
                unit.snippet = Some(format!("fn unrelated_{index}() {{}}"));
                unit
            })
            .collect::<Vec<_>>();
        let mut relevant = support_unit(SupportUnitKindDto::SourceRange);
        relevant.id = "process-command".to_owned();
        relevant.path = Some("src/server.c".to_owned());
        relevant.symbol_id = Some("process-command".to_owned());
        relevant.summary = "source for processCommand".to_owned();
        relevant.snippet = Some("int processCommand(client *c) { return execute(c); }".to_owned());
        support.push(relevant);

        let evidence = packet_evidence_rows_for_request(
            &support,
            "Trace how the server routes a command for execution.",
            &[],
        );

        assert_eq!(evidence[0].path.as_ref().unwrap().as_str(), "src/server.c");
    }

    #[test]
    fn packet_evidence_spends_unreserved_rows_on_the_most_relevant_evidence() {
        let support = (0..12)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("source-{index}");
                unit.path = Some(format!("src/source-{index}.rs"));
                unit.symbol_id = Some(format!("format_stage_{index}"));
                unit.snippet = Some(format!("fn format_stage_{index}() {{}}"));
                unit
            })
            .chain((0..8).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SymbolLocation);
                unit.id = format!("location-{index}");
                unit.path = Some(format!("src/location-{index}.rs"));
                unit.summary = format!("unrelated location {index}");
                unit
            }))
            .chain((0..8).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
                unit.id = format!("edge-{index}");
                unit.from_symbol = Some(format!("caller-{index}"));
                unit.edge_kind = Some("CALL".to_owned());
                unit.to_symbol = Some(format!("callee-{index}"));
                unit
            }))
            .collect::<Vec<_>>();

        let evidence =
            packet_evidence_rows_for_request(&support, "Explain the format stages.", &[]);

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert!(
            evidence
                .iter()
                .filter(|row| {
                    row.path
                        .as_ref()
                        .is_some_and(|path| path.as_str().starts_with("src/source-"))
                })
                .count()
                >= 10,
            "unreserved rows should prefer relevant source evidence over low-value duplicate locations"
        );
    }

    #[test]
    fn packet_evidence_prioritizes_the_selected_continuation_gap() {
        let mut support = (0..10)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("base-{index}");
                unit.path = Some(format!("source/base-{index}.css"));
                unit.symbol_id = Some(format!("base-{index}"));
                unit.snippet = Some(format!(".base-{index} {{ animation-duration: 1s; }}"));
                unit
            })
            .collect::<Vec<_>>();
        let mut bounce = support_unit(SupportUnitKindDto::SourceRange);
        bounce.id = "bounce-keyframes".to_owned();
        bounce.path = Some("source/attention_seekers/bounce.css".to_owned());
        bounce.symbol_id = Some("bounce-keyframes".to_owned());
        bounce.summary = "source for @keyframes bounce".to_owned();
        bounce.snippet = Some("@keyframes bounce { from { transform: none; } }".to_owned());
        support.push(bounce);
        let option_id = codestory_contracts::api::encode_drill_option_id(
            codestory_contracts::api::DrillGapKindDto::OmittedMandatorySupport,
            "symbol:packet::css_import::source/attention_seekers/bounce.css::@keyframes bounce",
        );

        let evidence = packet_evidence_rows_for_request(
            &support,
            "Explain how the animation classes connect.",
            &[option_id],
        );

        assert_eq!(
            evidence[0].path.as_ref().unwrap().as_str(),
            "source/attention_seekers/bounce.css"
        );
    }

    #[test]
    fn packet_symbol_locations_do_not_repeat_absolute_project_paths_in_summary_text() {
        let mut location = support_unit(SupportUnitKindDto::SymbolLocation);
        location.summary = "/private/project/src/lib.rs at src/lib.rs:7".to_owned();
        location.path = Some("src/lib.rs".to_owned());
        location.start_line = Some(7);

        let row = packet_evidence_row(0, &location).expect("location evidence");
        assert_eq!(row.summary.as_ref().unwrap().as_str(), "src/lib.rs:7");
    }

    #[test]
    fn context_projection_keeps_the_resolved_target_with_or_without_citations() {
        let project = tempfile::tempdir().expect("project");
        let source = project.path().join("resolved.rs");
        fs::write(&source, "pub fn resolved_target() {}\n").expect("source");
        let storage = project.path().join("codestory.db");
        let controller = crate::AppController::new();
        controller
            .open_project_summary_with_storage_path(project.path().to_path_buf(), storage)
            .expect("open project");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index project");
        let service = crate::services::PublicOperationService::new(controller);

        for citations in [
            json!([]),
            json!([{
                "node_id":"citation-node",
                "display_name":"different_citation",
                "kind":"FUNCTION",
                "file_path":"different/citation.rs",
                "line":7,
                "score":1.0,
                "origin":"indexed_symbol",
                "resolvable":true,
                "evidence_edge_ids":[]
            }]),
        ] {
            let answer: AgentAnswerDto = serde_json::from_value(json!({
                "answer_id":"context-answer",
                "prompt":"resolved_target",
                "summary":"fixture",
                "source_coverage":[],
                "sections":[],
                "citations":citations,
                "subgraph_ids":[],
                "retrieval_version":"fixture",
                "graphs":[],
                "retrieval_trace":{
                    "request_id":"context-request",
                    "resolved_profile":"investigate",
                    "policy_mode":"latency_first",
                    "total_latency_ms":0,
                    "sla_missed":false,
                    "semantic_fallback_count":0,
                    "semantic_fallbacks":[],
                    "semantic_stage_timeout_zero_hits":0,
                    "semantic_abstained_count":0,
                    "annotations":[],
                    "steps":[],
                    "packet_sidecar_diagnostics":[]
                }
            }))
            .expect("answer fixture");
            let projection = service
                .run_observational_with_cancel("context", Arc::new(AtomicBool::new(false)), || {
                    project_context_v3(
                        &service,
                        "test",
                        Some("resolved.rs"),
                        Some("resolved-node"),
                        &answer,
                    )
                })
                .expect("project context")
                .value;

            assert_eq!(
                projection.target.path.as_ref().map(PathTextV3::as_str),
                Some("resolved.rs"),
                "a citation cannot replace the already resolved request target"
            );
            assert_eq!(
                projection
                    .target
                    .symbol_id
                    .as_ref()
                    .map(SymbolIdTextV3::as_str),
                Some("resolved-node")
            );
        }
    }

    #[test]
    fn context_projection_preserves_path_only_unresolved_uncertainty() {
        let project = tempfile::tempdir().expect("project");
        let source = project.path().join("unresolved.rs");
        fs::write(&source, "pub fn unresolved_target() {}\n").expect("source");
        let storage = project.path().join("codestory.db");
        let controller = crate::AppController::new();
        controller
            .open_project_summary_with_storage_path(project.path().to_path_buf(), storage)
            .expect("open project");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index project");
        let service = crate::services::PublicOperationService::new(controller);
        let answer: AgentAnswerDto = serde_json::from_value(json!({
            "answer_id":"context-answer",
            "prompt":"unresolved_target",
            "summary":"fixture",
            "source_coverage":[],
            "sections":[],
            "citations":[{
                "node_id":"unresolved-node",
                "display_name":"unresolved_target",
                "kind":"FUNCTION",
                "file_path":"unresolved.rs",
                "line":null,
                "score":1.0,
                "origin":"indexed_symbol",
                "resolvable":false,
                "evidence_edge_ids":[]
            }],
            "subgraph_ids":[],
            "retrieval_version":"fixture",
            "graphs":[],
            "retrieval_trace":{
                "request_id":"context-request",
                "resolved_profile":"investigate",
                "policy_mode":"latency_first",
                "total_latency_ms":0,
                "sla_missed":false,
                "semantic_fallback_count":0,
                "semantic_fallbacks":[],
                "semantic_stage_timeout_zero_hits":0,
                "semantic_abstained_count":0,
                "annotations":[],
                "steps":[],
                "packet_sidecar_diagnostics":[]
            }
        }))
        .expect("answer fixture");
        let projection = service
            .run_observational_with_cancel("context", Arc::new(AtomicBool::new(false)), || {
                project_context_v3(&service, "test", Some("unresolved.rs"), None, &answer)
            })
            .expect("project context")
            .value;
        let serialized = serde_json::to_value(projection).expect("serialize context");
        let evidence = &serialized["evidence"][0];

        assert_eq!(evidence["path"], json!("unresolved.rs"));
        assert_eq!(evidence["start_line"], serde_json::Value::Null);
        assert_eq!(evidence["end_line"], serde_json::Value::Null);
        assert_eq!(evidence["symbol_id"], serde_json::Value::Null);
    }
}
