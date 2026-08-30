//! Public evidence-only projection facade for CodeStory schema 3.

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

use codestory_contracts::{
    api::{
        AgentAnswerDto, AgentPacketDto, AgentPacketRequestDto, PacketClaimObligationDto,
        PacketDispositionKindDto, SearchHit, SearchResultsDto, SupportUnitDto, SupportUnitKindDto,
        decode_drill_option_id,
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
        &packet.plan.obligations.claim_obligations,
    );
    let _ = crate::agent::packet_accuracy_stage_ledger::record_public_projection_stage_from_env(
        &evidence,
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
    claim_obligations: &[PacketClaimObligationDto],
) -> PacketEvidenceSelectionV3 {
    let mut planned_stage_terms = ranking_terms.clone();
    for candidate in claim_obligations
        .iter()
        .filter(|obligation| obligation.material)
        .flat_map(|obligation| &obligation.open_next_candidates)
    {
        planned_stage_terms.extend(packet_evidence_terms(candidate));
    }
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
    let mut seen: HashMap<PacketEvidenceContentKeyV3, usize> = HashMap::new();
    let mut distinct: Vec<(
        SupportUnitKindDto,
        PacketEvidenceRowV3Dto,
        usize,
        Vec<usize>,
    )> = Vec::new();
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
        let material_carrier_ranks = packet_material_carrier_ranks(unit, claim_obligations);
        let Some(row) = packet_evidence_row(0, unit, !material_carrier_ranks.is_empty()) else {
            continue;
        };
        let content_key = packet_evidence_content_key(&row);
        if let Some(existing_index) = seen.get(&content_key).copied() {
            distinct[existing_index].3.extend(material_carrier_ranks);
            distinct[existing_index].3.sort_unstable();
            distinct[existing_index].3.dedup();
        } else {
            let relevance = packet_evidence_relevance(&row, &planned_stage_terms);
            seen.insert(content_key, distinct.len());
            distinct.push((unit.kind, row, relevance, material_carrier_ranks));
        }
    }
    merge_contained_packet_source_rows(&mut distinct);

    let distinct_rows = distinct.len();
    let selected = select_packet_evidence_indices(
        &distinct,
        claim_obligations
            .iter()
            .any(|obligation| obligation.material),
    );
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
    distinct: &[(
        SupportUnitKindDto,
        PacketEvidenceRowV3Dto,
        usize,
        Vec<usize>,
    )],
    has_material_obligations: bool,
) -> Vec<usize> {
    // Close the mandatory identity envelope before transport compaction. One carrier covers each
    // material obligation before any repeated carrier can consume the bound. Within the source
    // envelope, distinct native paths precede same-path repeats. Existing relevance/native order
    // remains the tie-breaker inside those priorities.
    let mut selected = Vec::with_capacity(distinct.len().min(PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3));
    let mut selected_indices = HashSet::new();
    let source_order = packet_evidence_kind_order(distinct, SupportUnitKindDto::SourceRange);
    let location_order = packet_evidence_kind_order(distinct, SupportUnitKindDto::SymbolLocation);
    let relation_order = packet_evidence_kind_order(distinct, SupportUnitKindDto::TypedGraphEdge);

    let mandatory_indices = packet_material_carrier_indices(distinct);
    let mandatory_relation_indices = packet_material_relation_indices(distinct);
    for index in &source_order {
        if mandatory_indices.contains(index) && selected_indices.insert(*index) {
            selected.push(*index);
        }
    }

    // Material callables can need more than one disjoint source window: the declaration, a
    // prompt-relevant internal callsite, and the terminal behavior may be far apart. Keep every
    // selected material window ahead of optional path diversity so byte compaction preserves the
    // behavior-bearing summaries rather than an unrelated row that happened to use a new path.
    for index in &source_order {
        if selected.len() == PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3 {
            break;
        }
        if !distinct[*index].3.is_empty() && selected_indices.insert(*index) {
            selected.push(*index);
        }
    }

    let mut source_paths = selected
        .iter()
        .filter_map(|index| {
            (distinct[*index].0 == SupportUnitKindDto::SourceRange)
                .then(|| distinct[*index].1.path.as_ref().map(|path| path.as_str()))
                .flatten()
        })
        .collect::<HashSet<_>>();
    let mut source_count = selected.len();
    for index in &source_order {
        if source_count >= PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3 {
            break;
        }
        let remaining_mandatory = mandatory_indices
            .iter()
            .filter(|required| !selected_indices.contains(required))
            .count();
        if selected.len().saturating_add(remaining_mandatory) >= PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3
        {
            break;
        }
        if selected_indices.contains(index) {
            continue;
        }
        let row = &distinct[*index].1;
        if row
            .path
            .as_ref()
            .is_some_and(|path| source_paths.insert(path.as_str()))
        {
            selected.push(*index);
            selected_indices.insert(*index);
            source_count += 1;
        }
    }

    for index in &source_order {
        if source_count >= PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3 {
            break;
        }
        let remaining_mandatory = mandatory_indices
            .iter()
            .filter(|required| !selected_indices.contains(required))
            .count();
        if selected.len().saturating_add(remaining_mandatory) >= PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3
        {
            break;
        }
        if selected_indices.insert(*index) {
            selected.push(*index);
            source_count += 1;
        }
    }

    for index in &location_order {
        if mandatory_indices.contains(index) && selected_indices.insert(*index) {
            selected.push(*index);
        }
    }
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

    let qualified_relation_order = relation_order
        .iter()
        .copied()
        .filter(|index| {
            !has_material_obligations
                || mandatory_relation_indices.contains(index)
                || packet_relation_connects_selected_material_source(distinct, *index, &selected)
        })
        .collect::<Vec<_>>();
    let relation_floor = selected
        .len()
        .saturating_add(
            qualified_relation_order
                .len()
                .min(PACKET_PUBLIC_RELATION_ROWS_TARGET_V3),
        )
        .min(PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
    for index in &qualified_relation_order {
        if selected.len() == PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3 {
            break;
        }
        if mandatory_relation_indices.contains(index) && selected_indices.insert(*index) {
            selected.push(*index);
        }
    }
    let relation_target = relation_floor.max(selected.len());
    let mut relation_callers = HashSet::new();
    for index in &selected {
        if distinct[*index].0 != SupportUnitKindDto::TypedGraphEdge {
            continue;
        }
        let caller = distinct[*index]
            .1
            .summary
            .as_ref()
            .and_then(|summary| summary.as_str().split_once(" -[").map(|(caller, _)| caller));
        relation_callers.insert(caller);
    }
    for index in &qualified_relation_order {
        if selected.len() >= relation_target {
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
        &qualified_relation_order,
        relation_target,
        &mut selected,
        &mut selected_indices,
    );

    let mut remainder = (0..distinct.len())
        .filter(|index| {
            !selected_indices.contains(index)
                && (!has_material_obligations
                    || distinct[*index].0 != SupportUnitKindDto::TypedGraphEdge
                    || qualified_relation_order.contains(index))
        })
        .collect::<Vec<_>>();
    remainder.sort_by_key(|index| {
        (
            distinct[*index].3.is_empty(),
            distinct[*index].3.first().copied().unwrap_or(usize::MAX),
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

type PacketEvidenceCandidateV3 = (
    SupportUnitKindDto,
    PacketEvidenceRowV3Dto,
    usize,
    Vec<usize>,
);

fn merge_contained_packet_source_rows(distinct: &mut Vec<PacketEvidenceCandidateV3>) {
    let mut left = 0;
    while left < distinct.len() {
        let mut right = left + 1;
        while right < distinct.len() {
            let left_contains = packet_source_row_contains(&distinct[left].1, &distinct[right].1);
            let right_contains = packet_source_row_contains(&distinct[right].1, &distinct[left].1);
            if !left_contains && !right_contains {
                right += 1;
                continue;
            }
            let carrier_compatible = distinct[left].1.symbol_id == distinct[right].1.symbol_id
                || distinct[left].3.is_empty()
                || distinct[right].3.is_empty()
                || distinct[left]
                    .3
                    .iter()
                    .any(|rank| distinct[right].3.contains(rank));
            if !carrier_compatible {
                right += 1;
                continue;
            }

            let prefer_right = match (distinct[left].3.is_empty(), distinct[right].3.is_empty()) {
                (true, false) => true,
                (false, true) => false,
                _ => {
                    right_contains
                        && (!left_contains
                            || packet_evidence_summary_len(&distinct[right].1)
                                > packet_evidence_summary_len(&distinct[left].1))
                }
            };
            if prefer_right {
                let mut containing = distinct.remove(right);
                merge_packet_evidence_candidate(&mut containing, &distinct[left]);
                distinct[left] = containing;
            } else {
                let contained = distinct.remove(right);
                merge_packet_evidence_candidate(&mut distinct[left], &contained);
            }
        }
        left += 1;
    }
}

fn packet_source_row_contains(
    containing: &PacketEvidenceRowV3Dto,
    contained: &PacketEvidenceRowV3Dto,
) -> bool {
    containing.kind == EvidenceKindV3Dto::ExactSource
        && contained.kind == EvidenceKindV3Dto::ExactSource
        && containing.path == contained.path
        && containing
            .start_line
            .zip(containing.end_line)
            .zip(contained.start_line.zip(contained.end_line))
            .is_some_and(|((outer_start, outer_end), (inner_start, inner_end))| {
                outer_start <= inner_start && outer_end >= inner_end
            })
}

fn packet_relation_connects_selected_material_source(
    distinct: &[PacketEvidenceCandidateV3],
    relation_index: usize,
    selected: &[usize],
) -> bool {
    let relation = &distinct[relation_index].1;
    let Some((caller, target)) = packet_relation_endpoints(relation) else {
        return false;
    };
    selected.iter().any(|source_index| {
        let (_, source, _, carrier_ranks) = &distinct[*source_index];
        if carrier_ranks.is_empty()
            || distinct[*source_index].0 != SupportUnitKindDto::SourceRange
            || relation.path != source.path
        {
            return false;
        }
        let source_words: HashSet<String> = source
            .summary
            .as_ref()
            .map(|summary| {
                packet_relation_words(summary.as_str())
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default();
        !caller.is_empty()
            && !target.is_empty()
            && caller.iter().all(|word| source_words.contains(word))
            && target.iter().all(|word| source_words.contains(word))
    })
}

fn packet_relation_endpoints(row: &PacketEvidenceRowV3Dto) -> Option<(Vec<String>, Vec<String>)> {
    let summary = row.summary.as_ref()?.as_str();
    let (caller, relation) = summary.split_once(" -[")?;
    let (_, target) = relation.split_once("]-> ")?;
    Some((packet_relation_words(caller), packet_relation_words(target)))
}

fn packet_relation_words(value: &str) -> Vec<String> {
    let mut normalized = String::with_capacity(value.len());
    let mut previous_was_lower_or_digit = false;
    for character in value.chars() {
        if character.is_alphanumeric() {
            if character.is_uppercase() && previous_was_lower_or_digit {
                normalized.push(' ');
            }
            for lowercase in character.to_lowercase() {
                normalized.push(lowercase);
            }
            previous_was_lower_or_digit = character.is_lowercase() || character.is_ascii_digit();
        } else {
            normalized.push(' ');
            previous_was_lower_or_digit = false;
        }
    }
    normalized.split_whitespace().map(str::to_owned).collect()
}

fn packet_evidence_summary_len(row: &PacketEvidenceRowV3Dto) -> usize {
    row.summary
        .as_ref()
        .map_or(0, |summary| summary.as_str().len())
}

fn merge_packet_evidence_candidate(
    retained: &mut PacketEvidenceCandidateV3,
    removed: &PacketEvidenceCandidateV3,
) {
    retained.2 = retained.2.max(removed.2);
    retained.3.extend(removed.3.iter().copied());
    retained.3.sort_unstable();
    retained.3.dedup();
}

fn packet_evidence_kind_priority(kind: SupportUnitKindDto) -> u8 {
    match kind {
        SupportUnitKindDto::SourceRange => 0,
        SupportUnitKindDto::TypedGraphEdge => 1,
        SupportUnitKindDto::SymbolLocation => 2,
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
    distinct: &[(
        SupportUnitKindDto,
        PacketEvidenceRowV3Dto,
        usize,
        Vec<usize>,
    )],
    selected_kind: SupportUnitKindDto,
) -> Vec<usize> {
    let mut indices = distinct
        .iter()
        .enumerate()
        .filter_map(|(index, (kind, _, _, _))| (*kind == selected_kind).then_some(index))
        .collect::<Vec<_>>();
    indices.sort_by_key(|index| {
        (
            distinct[*index].3.is_empty(),
            distinct[*index].3.first().copied().unwrap_or(usize::MAX),
            Reverse(distinct[*index].2),
            *index,
        )
    });
    indices
}

fn packet_material_carrier_ranks(
    unit: &SupportUnitDto,
    claim_obligations: &[PacketClaimObligationDto],
) -> Vec<usize> {
    claim_obligations
        .iter()
        .enumerate()
        .filter(|(_, obligation)| obligation.material)
        .filter_map(|(index, obligation)| {
            let matches = match unit.kind {
                SupportUnitKindDto::SourceRange | SupportUnitKindDto::SymbolLocation => {
                    let node_matches = unit.symbol_id.as_deref().is_some_and(|symbol_id| {
                        obligation
                            .carrier_node_ids
                            .iter()
                            .any(|node_id| node_id.0 == symbol_id)
                    });
                    let path_is_only_available_identity = obligation.carrier_node_ids.is_empty()
                        || unit.symbol_id.as_deref().is_none();
                    node_matches
                        || (path_is_only_available_identity
                            && unit.path.as_deref().is_some_and(|path| {
                                obligation
                                    .carrier_paths
                                    .iter()
                                    .any(|carrier_path| carrier_path == path)
                            }))
                }
                SupportUnitKindDto::TypedGraphEdge => {
                    unit.id.strip_prefix("edge:").is_some_and(|edge_id| {
                        let primary_edge_carrier = obligation
                            .carrier_node_ids
                            .iter()
                            .find(|node_id| {
                                obligation
                                    .carrier_edge_proofs
                                    .iter()
                                    .any(|proof| &proof.carrier_node_id == *node_id)
                            })
                            .or_else(|| {
                                obligation
                                    .carrier_edge_proofs
                                    .first()
                                    .map(|proof| &proof.carrier_node_id)
                            });
                        primary_edge_carrier.is_some_and(|primary_edge_carrier| {
                            obligation.carrier_edge_proofs.iter().any(|proof| {
                                proof.carrier_node_id == *primary_edge_carrier
                                    && proof.edge_id.0 == edge_id
                            })
                        })
                    })
                }
                SupportUnitKindDto::CompleteQueryNegative => false,
            };
            matches.then_some(index)
        })
        .collect()
}

fn packet_material_carrier_indices(
    distinct: &[(
        SupportUnitKindDto,
        PacketEvidenceRowV3Dto,
        usize,
        Vec<usize>,
    )],
) -> HashSet<usize> {
    let mut obligation_order = Vec::new();
    let mut best_candidate_by_obligation = HashMap::new();
    for (index, (kind, _, relevance, obligations)) in distinct.iter().enumerate() {
        let candidate = (
            packet_evidence_kind_priority(*kind),
            Reverse(*relevance),
            index,
        );
        for obligation in obligations {
            obligation_order.push(*obligation);
            best_candidate_by_obligation
                .entry(*obligation)
                .and_modify(|best| {
                    if candidate < *best {
                        *best = candidate;
                    }
                })
                .or_insert(candidate);
        }
    }
    obligation_order.sort_unstable();
    obligation_order.dedup();

    let mut covered_obligations = HashSet::new();
    let mut required = HashSet::new();
    for obligation in obligation_order {
        if required.len() == PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3 {
            break;
        }
        if covered_obligations.contains(&obligation) {
            continue;
        }
        if let Some((_, _, index)) = best_candidate_by_obligation.get(&obligation).copied() {
            required.insert(index);
            covered_obligations.extend(distinct[index].3.iter().copied());
        }
    }
    required
}

/// Keep one exact relation witness for each material obligation before repeated witnesses from an
/// earlier obligation consume the relation envelope. Source and relation rows are complementary:
/// the source says what the carrier contains, while the typed edge records the claimed boundary.
fn packet_material_relation_indices(
    distinct: &[(
        SupportUnitKindDto,
        PacketEvidenceRowV3Dto,
        usize,
        Vec<usize>,
    )],
) -> HashSet<usize> {
    let mut obligation_order = Vec::new();
    let mut best_candidate_by_obligation = HashMap::new();
    for (index, (kind, _, relevance, obligations)) in distinct.iter().enumerate() {
        if *kind != SupportUnitKindDto::TypedGraphEdge {
            continue;
        }
        let candidate = (Reverse(*relevance), index);
        for obligation in obligations {
            obligation_order.push(*obligation);
            best_candidate_by_obligation
                .entry(*obligation)
                .and_modify(|best| {
                    if candidate < *best {
                        *best = candidate;
                    }
                })
                .or_insert(candidate);
        }
    }
    obligation_order.sort_unstable();
    obligation_order.dedup();

    let mut covered_obligations = HashSet::new();
    let mut required = HashSet::new();
    for obligation in obligation_order {
        if required.len() == PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3 {
            break;
        }
        if covered_obligations.contains(&obligation) {
            continue;
        }
        if let Some((_, index)) = best_candidate_by_obligation.get(&obligation).copied() {
            required.insert(index);
            covered_obligations.extend(distinct[index].3.iter().copied());
        }
    }
    required
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
    packet_evidence_selection(support, &HashSet::new(), &[]).rows
}

#[cfg(test)]
fn packet_evidence_rows_with_obligations(
    support: &[SupportUnitDto],
    question: &str,
    claim_obligations: &[PacketClaimObligationDto],
) -> Vec<PacketEvidenceRowV3Dto> {
    packet_evidence_selection(
        support,
        &packet_evidence_ranking_terms(question, &[]),
        claim_obligations,
    )
    .rows
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
        &[],
    )
    .rows
}

#[cfg(test)]
fn packet_evidence_was_bounded(support: &[SupportUnitDto]) -> bool {
    packet_evidence_selection(support, &HashSet::new(), &[]).was_bounded
}

const PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3: usize = 8;
const PACKET_PUBLIC_LOCATION_ROWS_TARGET_V3: usize = 0;
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

fn packet_evidence_row(
    index: usize,
    unit: &SupportUnitDto,
    material_carrier: bool,
) -> Option<PacketEvidenceRowV3Dto> {
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
        summary: packet_evidence_summary(unit, material_carrier),
    })
}

const PACKET_EVIDENCE_SUMMARY_MAX_BYTES_V3: usize = 512;
const PACKET_MATERIAL_CARRIER_SUMMARY_MAX_BYTES_V3: usize = 2 * 1024;

fn packet_evidence_summary(unit: &SupportUnitDto, material_carrier: bool) -> Option<SummaryTextV3> {
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
    let maximum = if material_carrier && unit.kind == SupportUnitKindDto::SourceRange {
        PACKET_MATERIAL_CARRIER_SUMMARY_MAX_BYTES_V3
    } else {
        PACKET_EVIDENCE_SUMMARY_MAX_BYTES_V3
    };
    Some(summary_text_bounded(&text, maximum))
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

    use codestory_contracts::api::{
        EdgeId, EdgeKind, IndexMode, NodeId, PacketClaimObligationKindDto,
        PacketObligationCarrierEdgeProofDto, PacketObligationProofStatusDto,
    };
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

    fn material_obligation(id: &str) -> PacketClaimObligationDto {
        PacketClaimObligationDto {
            id: id.to_owned(),
            kind: PacketClaimObligationKindDto::Dispatch,
            binding_terms: Vec::new(),
            probe_binding: None,
            material: true,
            allowed_node_kinds: Vec::new(),
            required_edge_kind: None,
            requires_complete_discovery: false,
            proof_status: PacketObligationProofStatusDto::Proven,
            reason: None,
            carrier_node_ids: Vec::new(),
            carrier_paths: Vec::new(),
            carrier_edge_proofs: Vec::new(),
            open_next_candidates: Vec::new(),
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
        let source = packet_evidence_row(7, &source, false).expect("source evidence");
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
        let relation = packet_evidence_row(8, &relation, false).expect("relation evidence");
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
            ["fn useful() {}", "caller -[CALL]-> callee", "location"]
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
    fn packet_evidence_deduplicates_contained_ranges_for_the_same_source_carrier() {
        let mut inner = support_unit(SupportUnitKindDto::SourceRange);
        inner.id = "inner".to_owned();
        inner.path = Some("src/request.rs".to_owned());
        inner.symbol_id = Some("Request.finalize".to_owned());
        inner.start_line = Some(140);
        inner.end_line = Some(150);
        inner.snippet = Some("return body;".to_owned());

        let mut outer = inner.clone();
        outer.id = "outer".to_owned();
        outer.start_line = Some(127);
        outer.snippet = Some("fn finalize() { prepare(); return body; }".to_owned());

        let evidence = packet_evidence_rows(&[inner, outer]);

        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].start_line, Some(127));
        assert_eq!(evidence[0].end_line, Some(150));
        assert_eq!(
            evidence[0].summary.as_ref().map(|summary| summary.as_str()),
            Some("fn finalize() { prepare(); return body; }")
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
            "the closed envelope keeps twelve source excerpts"
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
                "caller-0 -[CALL]-> callee-0",
                "caller-1 -[CALL]-> callee-1",
                "caller-2 -[CALL]-> callee-2",
                "caller-3 -[CALL]-> callee-3",
                "fn source_8() {}",
                "fn source_9() {}",
                "fn source_10() {}",
                "fn source_11() {}",
            ]
        );
        assert!(packet_evidence_was_bounded(&support));
    }

    #[test]
    fn packet_evidence_keeps_relevance_as_the_tie_breaker_inside_the_path_envelope() {
        let support = (0..8)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("route-stage-{index}");
                unit.path = Some("src/router.rs".to_owned());
                unit.symbol_id = Some(format!("route-stage-{index}"));
                unit.start_line = Some(index + 1);
                unit.snippet = Some(format!("fn route_stage_{index}() {{}}"));
                unit
            })
            .chain((0..8).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("noise-{index}");
                unit.path = Some(format!("src/noise-{index}.rs"));
                unit.symbol_id = Some(format!("noise-{index}"));
                unit.start_line = Some(index + 1);
                unit.snippet = Some(format!("fn noise_{index}() {{}}"));
                unit
            }))
            .collect::<Vec<_>>();

        let evidence =
            packet_evidence_rows_for_request(&support, "Explain the route stages in order.", &[]);
        let first_eight = evidence
            .iter()
            .take(8)
            .map(|row| {
                (
                    row.path.as_ref().map(|path| path.as_str()),
                    row.symbol_id.as_ref().map(|symbol| symbol.as_str()),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(first_eight[0].1, Some("route-stage-0"));
        assert_eq!(
            first_eight
                .iter()
                .filter_map(|(path, _)| *path)
                .collect::<HashSet<_>>()
                .len(),
            PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3,
            "path diversity owns the reserved envelope while relevance selects the best row for a repeated path: {first_eight:?}"
        );
    }

    #[test]
    fn packet_evidence_reserves_material_node_path_and_edge_carriers_before_relevance_fill() {
        let mut support = (0..10)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("relevant-{index}");
                unit.path = Some(format!("src/relevant-{index}.rs"));
                unit.symbol_id = Some(format!("relevant-stage-{index}"));
                unit.snippet = Some(format!("fn relevant_stage_{index}() {{}}"));
                unit
            })
            .collect::<Vec<_>>();

        let mut node_carrier = support_unit(SupportUnitKindDto::SourceRange);
        node_carrier.id = "node-carrier".to_owned();
        node_carrier.path = Some("src/flow.rs".to_owned());
        node_carrier.symbol_id = Some("Flow.dispatch".to_owned());
        node_carrier.snippet = Some(format!(
            "fn dispatch() {{\n{}\nthis.router.handle(request);\n}}",
            "x".repeat(700)
        ));
        support.push(node_carrier);

        let mut path_carrier = support_unit(SupportUnitKindDto::SourceRange);
        path_carrier.id = "path-carrier".to_owned();
        path_carrier.path = Some("src/material-path.rs".to_owned());
        path_carrier.start_line = Some(4);
        path_carrier.end_line = Some(8);
        path_carrier.snippet = Some("material path source".to_owned());
        support.push(path_carrier);

        for index in 0..5 {
            let mut relation = support_unit(SupportUnitKindDto::TypedGraphEdge);
            relation.id = format!("edge:ordinary-{index}");
            relation.from_symbol = Some(format!("ordinary-caller-{index}"));
            relation.edge_kind = Some("CALL".to_owned());
            relation.to_symbol = Some(format!("ordinary-target-{index}"));
            support.push(relation);
        }
        let mut edge_carrier = support_unit(SupportUnitKindDto::TypedGraphEdge);
        edge_carrier.id = "edge:material-edge".to_owned();
        edge_carrier.from_symbol = Some("Flow.dispatch".to_owned());
        edge_carrier.edge_kind = Some("CALL".to_owned());
        edge_carrier.to_symbol = Some("Router.handle".to_owned());
        support.push(edge_carrier);

        let mut node_obligation = material_obligation("node-obligation");
        node_obligation.carrier_node_ids = vec![NodeId("Flow.dispatch".to_owned())];
        let mut path_obligation = material_obligation("path-obligation");
        path_obligation.carrier_paths = vec!["src/material-path.rs".to_owned()];
        let mut edge_obligation = material_obligation("edge-obligation");
        edge_obligation.carrier_edge_proofs = vec![PacketObligationCarrierEdgeProofDto {
            carrier_node_id: NodeId("Flow.dispatch".to_owned()),
            edge_id: EdgeId("material-edge".to_owned()),
            edge_kind: EdgeKind::CALL,
        }];

        let evidence = packet_evidence_rows_with_obligations(
            &support,
            "Explain the relevant stages.",
            &[node_obligation, path_obligation, edge_obligation],
        );

        assert_eq!(
            evidence[0].symbol_id.as_ref().unwrap().as_str(),
            "Flow.dispatch"
        );
        assert_eq!(
            evidence[1].path.as_ref().unwrap().as_str(),
            "src/material-path.rs"
        );
        assert!(
            evidence[0]
                .summary
                .as_ref()
                .unwrap()
                .as_str()
                .contains("this.router.handle(request)"),
            "a material source carrier must retain more than the optional 512-byte prefix"
        );
        assert_eq!(
            evidence[8].summary.as_ref().unwrap().as_str(),
            "Flow.dispatch -[CALL]-> Router.handle"
        );
        assert_eq!(
            evidence.len(),
            13,
            "unrelated relations must not be added merely to fill the public row budget"
        );
    }

    #[test]
    fn packet_evidence_orders_every_material_callable_window_before_optional_context() {
        let mut support = (0..10)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("optional-{index}");
                unit.path = Some(format!("src/optional-{index}.rs"));
                unit.symbol_id = Some(format!("Optional.stage{index}"));
                unit.start_line = Some(1);
                unit.end_line = Some(4);
                unit.snippet = Some(format!("fn optional_{index}() {{}}"));
                unit
            })
            .collect::<Vec<_>>();
        for (id, start, end, snippet) in [
            ("declaration", 105, 115, "send declaration and open"),
            ("callsite", 147, 157, "await request stream pipe"),
            ("terminal", 206, 216, "return streamed response"),
        ] {
            let mut unit = support_unit(SupportUnitKindDto::SourceRange);
            unit.id = id.to_owned();
            unit.path = Some("src/io_client.rs".to_owned());
            unit.symbol_id = Some("IOClient.send".to_owned());
            unit.start_line = Some(start);
            unit.end_line = Some(end);
            unit.snippet = Some(snippet.to_owned());
            support.push(unit);
        }

        let mut obligation = material_obligation("transport-send");
        obligation.carrier_node_ids = vec![NodeId("IOClient.send".to_owned())];
        let evidence = packet_evidence_rows_with_obligations(
            &support,
            "Explain the transport send behavior.",
            &[obligation],
        );

        assert_eq!(
            evidence
                .iter()
                .take(3)
                .map(|row| (row.symbol_id.as_ref().map(|id| id.as_str()), row.start_line))
                .collect::<Vec<_>>(),
            [
                (Some("IOClient.send"), Some(105)),
                (Some("IOClient.send"), Some(147)),
                (Some("IOClient.send"), Some(206)),
            ]
        );
    }

    #[test]
    fn packet_evidence_admits_only_relations_that_connect_retained_material_source() {
        let mut source = support_unit(SupportUnitKindDto::SourceRange);
        source.id = "client-get".to_owned();
        source.path = Some("src/client.dart".to_owned());
        source.symbol_id = Some("Client.get".to_owned());
        source.start_line = Some(20);
        source.end_line = Some(30);
        source.snippet =
            Some("get(uri, {headers}) => _withClient(client => client.get(uri));".to_owned());

        let relation = |id: &str, path: &str, caller: &str, target: &str| {
            let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
            unit.id = id.to_owned();
            unit.path = Some(path.to_owned());
            unit.from_symbol = Some(caller.to_owned());
            unit.edge_kind = Some("CALL".to_owned());
            unit.to_symbol = Some(target.to_owned());
            unit
        };
        let support = vec![
            source,
            relation("get-edge", "src/client.dart", "get", "_withClient"),
            relation("head-edge", "src/client.dart", "head", "_withClient"),
            relation("patch-edge", "src/client.dart", "patch", "_sendUnstreamed"),
            relation("adapter-edge", "src/adapter.dart", "Adapter.send", "method"),
        ];
        let mut obligation = material_obligation("client-public-interface");
        obligation.carrier_node_ids = vec![NodeId("Client.get".to_owned())];

        let evidence =
            packet_evidence_rows_with_obligations(&support, "Explain Client.get.", &[obligation]);
        let relations = evidence
            .iter()
            .filter(|row| row.kind == EvidenceKindV3Dto::GraphRelation)
            .filter_map(|row| row.summary.as_ref().map(|summary| summary.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(relations, ["get -[CALL]-> _withClient"]);
    }

    #[test]
    fn packet_evidence_keeps_each_material_source_with_an_exact_call_witness_under_the_cap() {
        let mut support = (0..8)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("source-{index}");
                unit.path = Some(format!("src/flow-{index}.rs"));
                unit.symbol_id = Some(format!("Flow.stage{index}"));
                unit.snippet = Some(format!("fn stage_{index}() {{}}"));
                unit
            })
            .collect::<Vec<_>>();
        support[0].symbol_id = Some("Loop.drive".to_owned());
        support[1].symbol_id = Some("Command.route".to_owned());

        let relation = |id: &str, caller: &str, target: &str| {
            let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
            unit.id = format!("edge:{id}");
            unit.from_symbol = Some(caller.to_owned());
            unit.edge_kind = Some("CALL".to_owned());
            unit.to_symbol = Some(target.to_owned());
            unit
        };
        for index in 0..5 {
            support.push(relation(
                &format!("loop-{index}"),
                "Loop.drive",
                &format!("Loop.tick{index}"),
            ));
        }
        support.push(relation("command-route", "Command.route", "Command.check"));
        for index in 0..4 {
            support.push(relation(
                &format!("ordinary-{index}"),
                &format!("Ordinary{index}"),
                &format!("OrdinaryTarget{index}"),
            ));
        }

        let proof = |carrier: &str, edge: &str| PacketObligationCarrierEdgeProofDto {
            carrier_node_id: NodeId(carrier.to_owned()),
            edge_id: EdgeId(edge.to_owned()),
            edge_kind: EdgeKind::CALL,
        };
        let mut loop_obligation = material_obligation("event-loop");
        loop_obligation.carrier_node_ids = vec![NodeId("Loop.drive".to_owned())];
        loop_obligation.carrier_edge_proofs = (0..5)
            .map(|index| proof("Loop.drive", &format!("loop-{index}")))
            .collect();
        let mut command_obligation = material_obligation("command-router");
        command_obligation.carrier_node_ids = vec![NodeId("Command.route".to_owned())];
        command_obligation.carrier_edge_proofs = vec![proof("Command.route", "command-route")];

        let evidence = packet_evidence_rows_with_obligations(
            &support,
            "Trace the loop and command route.",
            &[loop_obligation, command_obligation],
        );
        let summaries = evidence
            .iter()
            .filter_map(|row| row.summary.as_ref().map(|summary| summary.as_str()))
            .collect::<HashSet<_>>();
        let symbols = evidence
            .iter()
            .filter_map(|row| row.symbol_id.as_ref().map(|symbol| symbol.as_str()))
            .collect::<HashSet<_>>();

        assert!(symbols.contains("Loop.drive"));
        assert!(symbols.contains("Command.route"));
        assert!(summaries.contains("Loop.drive -[CALL]-> Loop.tick0"));
        assert!(summaries.contains("Command.route -[CALL]-> Command.check"));
        assert_eq!(
            evidence.len(),
            10,
            "only the two material relation witnesses may accompany the eight source rows"
        );
    }

    #[test]
    fn packet_evidence_binds_a_material_relation_to_the_primary_exact_carrier() {
        let relation = |id: &str, caller: &str, target: &str| {
            let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
            unit.id = format!("edge:{id}");
            unit.from_symbol = Some(caller.to_owned());
            unit.edge_kind = Some("CALL".to_owned());
            unit.to_symbol = Some(target.to_owned());
            unit
        };
        let mut support = (0..12)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("source-{index}");
                unit.path = Some(format!("src/filler-{index}.rs"));
                unit.symbol_id = Some(format!("Filler{index}"));
                unit.snippet = Some(format!("fn filler_{index}() {{}}"));
                unit
            })
            .collect::<Vec<_>>();
        support[0].symbol_id = Some("Runtime.main".to_owned());
        support[1].symbol_id = Some("EventLoop.processEvents".to_owned());
        support.push(relation(
            "main-loop",
            "Runtime.main",
            "EventLoop.processEvents",
        ));
        support.push(relation(
            "iteration",
            "EventLoop.processEvents",
            "EventLoop.processTimeEvents",
        ));
        let mut obligation = material_obligation("event-loop");
        obligation.carrier_node_ids = vec![
            NodeId("Runtime.main".to_owned()),
            NodeId("EventLoop.processEvents".to_owned()),
        ];
        obligation.carrier_edge_proofs = vec![
            PacketObligationCarrierEdgeProofDto {
                carrier_node_id: NodeId("Runtime.main".to_owned()),
                edge_id: EdgeId("main-loop".to_owned()),
                edge_kind: EdgeKind::CALL,
            },
            PacketObligationCarrierEdgeProofDto {
                carrier_node_id: NodeId("EventLoop.processEvents".to_owned()),
                edge_id: EdgeId("iteration".to_owned()),
                edge_kind: EdgeKind::CALL,
            },
        ];

        let evidence = packet_evidence_rows_with_obligations(
            &support,
            "Trace how the process events iteration handles events and time events.",
            &[obligation],
        );
        let summaries = evidence
            .iter()
            .filter_map(|row| row.summary.as_ref().map(|summary| summary.as_str()))
            .collect::<HashSet<_>>();

        assert!(summaries.contains("Runtime.main -[CALL]-> EventLoop.processEvents"));
    }

    #[test]
    fn packet_evidence_never_overflows_when_material_sources_fill_the_closed_envelope() {
        let mut support = Vec::new();
        let mut obligations = Vec::new();
        for index in 0..PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3 {
            let symbol = format!("Flow.stage{index}");
            let edge_id = format!("edge-{index}");
            let mut source = support_unit(SupportUnitKindDto::SourceRange);
            source.id = format!("source-{index}");
            source.path = Some(format!("src/stage-{index}.rs"));
            source.symbol_id = Some(symbol.clone());
            source.snippet = Some(format!("fn stage_{index}() {{}}"));
            support.push(source);

            let mut relation = support_unit(SupportUnitKindDto::TypedGraphEdge);
            relation.id = format!("edge:{edge_id}");
            relation.from_symbol = Some(symbol.clone());
            relation.edge_kind = Some("CALL".to_owned());
            relation.to_symbol = Some(format!("Flow.target{index}"));
            support.push(relation);

            let mut obligation = material_obligation(&format!("stage-{index}"));
            obligation.carrier_node_ids = vec![NodeId(symbol.clone())];
            obligation.carrier_edge_proofs = vec![PacketObligationCarrierEdgeProofDto {
                carrier_node_id: NodeId(symbol),
                edge_id: EdgeId(edge_id),
                edge_kind: EdgeKind::CALL,
            }];
            obligations.push(obligation);
        }

        let evidence = packet_evidence_rows_with_obligations(
            &support,
            "Trace every material stage.",
            &obligations,
        );
        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert!(
            evidence
                .iter()
                .all(|row| row.kind == EvidenceKindV3Dto::ExactSource),
            "source receipts that fill the closed envelope leave no lawful room for relation rows"
        );
    }

    #[test]
    fn packet_evidence_covers_each_material_obligation_before_repeating_a_carrier() {
        let mut support = (0..24)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("repeated-stage-{index}");
                unit.path = Some("src/repeated-flow.rs".to_owned());
                unit.symbol_id = Some(format!("RepeatedFlow.stage{index}"));
                unit.start_line = Some(index + 1);
                unit.snippet = Some(format!("fn dispatch_route_stage_{index}() {{}}"));
                unit
            })
            .collect::<Vec<_>>();
        let mut unique_stage = support_unit(SupportUnitKindDto::SourceRange);
        unique_stage.id = "unique-material-stage".to_owned();
        unique_stage.path = Some("src/unique-material-stage.rs".to_owned());
        unique_stage.symbol_id = Some("UniqueMaterialStage.run".to_owned());
        unique_stage.start_line = Some(1);
        unique_stage.snippet = Some("fn run() {}".to_owned());
        support.push(unique_stage);

        let mut repeated_obligation = material_obligation("repeated-flow");
        repeated_obligation.carrier_paths = vec!["src/repeated-flow.rs".to_owned()];
        let mut unique_obligation = material_obligation("unique-stage");
        unique_obligation.carrier_paths = vec!["src/unique-material-stage.rs".to_owned()];

        let evidence = packet_evidence_rows_with_obligations(
            &support,
            "Explain the dispatch route stages.",
            &[repeated_obligation, unique_obligation],
        );

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert_eq!(
            evidence
                .iter()
                .take(2)
                .filter_map(|row| row.path.as_ref().map(|path| path.as_str()))
                .collect::<HashSet<_>>(),
            HashSet::from(["src/repeated-flow.rs", "src/unique-material-stage.rs"]),
            "a repeated high-relevance carrier must not displace another material obligation's only carrier"
        );
    }

    #[test]
    fn packet_evidence_chooses_the_best_carrier_for_each_native_obligation_rank() {
        let relation = |id: &str, caller: &str, target: &str| {
            let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
            unit.id = format!("edge:{id}");
            unit.from_symbol = Some(caller.to_owned());
            unit.edge_kind = Some("CALL".to_owned());
            unit.to_symbol = Some(target.to_owned());
            unit
        };
        let carrier = |edge_id: &str| PacketObligationCarrierEdgeProofDto {
            carrier_node_id: NodeId("Fixture.caller".to_owned()),
            edge_id: EdgeId(edge_id.to_owned()),
            edge_kind: EdgeKind::CALL,
        };

        let mut support = vec![
            relation("a", "zirconium_plutonium", "a_target"),
            relation("shared-o0", "unrelated_shared", "shared_target"),
            // These two rows intentionally collapse to one public relation. Their distinct edge
            // identities must merge O0 and O1 onto the shared carrier before selection.
            relation("shared-o1", "unrelated_shared", "shared_target"),
            relation("c", "zirconium_plutonium_manganese", "c_target"),
        ];
        let mut obligations = Vec::new();
        let mut obligation_zero = material_obligation("obligation-zero");
        obligation_zero.carrier_edge_proofs = vec![carrier("a"), carrier("shared-o0")];
        obligations.push(obligation_zero);
        let mut obligation_one = material_obligation("obligation-one");
        obligation_one.carrier_edge_proofs = vec![carrier("shared-o1"), carrier("c")];
        obligations.push(obligation_one);

        for index in 0..14 {
            let edge_id = format!("mandatory-{index}");
            support.push(relation(
                &edge_id,
                &format!("mandatory_caller_{index}"),
                &format!("mandatory_target_{index}"),
            ));
            let mut obligation = material_obligation(&format!("mandatory-{index}"));
            obligation.carrier_edge_proofs = vec![carrier(&edge_id)];
            obligations.push(obligation);
        }

        let evidence = packet_evidence_rows_with_obligations(
            &support,
            "Trace zirconium plutonium manganese.",
            &obligations,
        );
        let summaries = evidence
            .iter()
            .filter_map(|row| row.summary.as_ref().map(|summary| summary.as_str()))
            .collect::<HashSet<_>>();

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert!(summaries.contains("zirconium_plutonium -[CALL]-> a_target"));
        assert!(summaries.contains("zirconium_plutonium_manganese -[CALL]-> c_target"));
        assert!(
            !summaries.contains("unrelated_shared -[CALL]-> shared_target"),
            "the merged shared carrier must not substitute for each obligation's better native-rank candidate"
        );
    }

    #[test]
    fn packet_evidence_maximizes_distinct_source_paths_before_same_path_repeats() {
        let support = (0..24)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("repeated-path-{index}");
                unit.path = Some("src/repeated.rs".to_owned());
                unit.symbol_id = Some(format!("Repeated.stage{index}"));
                unit.start_line = Some(index + 1);
                unit.snippet = Some(format!("fn repeated_route_stage_{index}() {{}}"));
                unit
            })
            .chain((0..7).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("distinct-path-{index}");
                unit.path = Some(format!("src/distinct-{index}.rs"));
                unit.symbol_id = Some(format!("Distinct{index}.run"));
                unit.start_line = Some(1);
                unit.snippet = Some(format!("fn run_{index}() {{}}"));
                unit
            }))
            .collect::<Vec<_>>();

        let evidence =
            packet_evidence_rows_for_request(&support, "Explain the repeated route stages.", &[]);
        let paths = evidence
            .iter()
            .filter_map(|row| row.path.as_ref().map(|path| path.as_str()))
            .collect::<HashSet<_>>();

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert_eq!(
            paths.len(),
            PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3,
            "same-path repeats must not consume the reserved distinct-path envelope"
        );
        for index in 0..7 {
            assert!(paths.contains(format!("src/distinct-{index}.rs").as_str()));
        }
    }

    #[test]
    fn packet_evidence_selection_is_deterministic_bounded_and_keeps_unaffected_rank_order() {
        let support = (0..20)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("source-{index}");
                unit.path = Some(format!("src/source-{index}.rs"));
                unit.symbol_id = Some(format!("stage-{index}"));
                unit.start_line = Some(index + 1);
                unit.snippet = Some(if index == 17 {
                    "fn selected_route_target() {}".to_owned()
                } else {
                    format!("fn unrelated_{index}() {{}}")
                });
                unit
            })
            .collect::<Vec<_>>();

        let first =
            packet_evidence_rows_for_request(&support, "Explain the selected route target.", &[]);
        let second =
            packet_evidence_rows_for_request(&support, "Explain the selected route target.", &[]);

        assert_eq!(first, second);
        assert_eq!(first.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert_eq!(first[0].path.as_ref().unwrap().as_str(), "src/source-17.rs");
        assert_eq!(first[1].path.as_ref().unwrap().as_str(), "src/source-0.rs");
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
                >= 12,
            "unreserved rows should prefer relevant source evidence over low-value duplicate locations"
        );
    }

    #[test]
    fn packet_evidence_ranks_the_planned_upstream_stage_before_downstream_repeats() {
        let mut support = (0..PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("downstream-{index}");
                unit.path = Some(format!("src/downstream-{index}.rs"));
                unit.symbol_id = Some(format!("Processor.handle{index}"));
                unit.snippet = Some(format!("fn handle_{index}() {{ process(); }}"));
                unit
            })
            .collect::<Vec<_>>();
        let mut upstream = support_unit(SupportUnitKindDto::SourceRange);
        upstream.id = "public-registration".to_owned();
        upstream.path = Some("src/public-facade.rs".to_owned());
        upstream.symbol_id = Some("PublicFacade.registerRequest".to_owned());
        upstream.snippet = Some("fn register_request() { route.store(); }".to_owned());
        support.push(upstream);

        let mut obligation = material_obligation("public-upstream-stage");
        obligation.open_next_candidates = vec!["public request registration".to_owned()];

        let evidence = packet_evidence_rows_with_obligations(
            &support,
            "Explain the complete lifecycle.",
            &[obligation],
        );

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert_eq!(
            evidence[0].symbol_id.as_ref().map(|symbol| symbol.as_str()),
            Some("PublicFacade.registerRequest"),
            "the public envelope must spend a row on the planner's missing upstream stage before repeating downstream internals"
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

        let row = packet_evidence_row(0, &location, false).expect("location evidence");
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
