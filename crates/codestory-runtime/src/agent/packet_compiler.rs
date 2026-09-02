//! Finalize the bounded Horizon A packet without answer-planning policy.
//!
//! Retrieval, typed-probe resolution, descriptor admission, and exact
//! hydration have already run. This interim boundary retains that evidence and
//! converts objective admission or ambiguity gaps into stable continuations.
//! Repository-derived selection belongs to Horizon B (`#2106`).

use crate::agent::packet_candidate::{PacketProofSession, active_packet_proof_session};
use crate::agent::packet_coverage::PacketCoverageInput;
use crate::agent::packet_freshness::PacketFreshnessInput;
use crate::agent::packet_scoring::packet_display_path;
use codestory_contracts::api::{
    AgentPacketDto, AgentPacketRequestDto, BoundedDrillPlanDto, DrillGapKindDto, DrillOptionDto,
    PACKET_DRILL_MAX_BYTES, PACKET_DRILL_MAX_DEPTH, PACKET_DRILL_MAX_HITS,
    PACKET_DRILL_MAX_OPTIONS, PacketDispositionDto, PacketProbeResolutionStatusDto,
    SourceCoverageStatusDto, SupportUnitDto, SupportUnitKindDto, decode_drill_option_id,
};
use codestory_contracts::compilation::{
    PacketAdmissionGapKindV1, PacketAdmissionReceiptV1, PacketContinuationSelectorV1,
    PacketStructuralGapReasonV1,
};
use codestory_contracts::graph::FileCoverageReason;
use std::collections::{BTreeSet, HashMap};

pub fn finalize_interim_packet_evidence(
    packet: &mut AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
    _project_id: &str,
) {
    let session = active_packet_proof_session();
    if let Some(session) = session.as_deref() {
        order_support_by_admission(&mut packet.support, &session.receipts());
    }
    let continuation = interim_continuations(packet, session.as_deref());
    let publication = packet.answer.retrieval_trace.retrieval_publication.as_ref();
    packet.disposition = classify_packet_disposition(
        packet,
        request,
        &continuation,
        publication
            .map(|publication| publication.core_generation_id.clone())
            .unwrap_or_default(),
        publication.map(|publication| publication.retrieval_generation.clone()),
    );
}

fn order_support_by_admission(
    support: &mut [SupportUnitDto],
    admissions: &[PacketAdmissionReceiptV1],
) {
    let ordinals = admissions
        .iter()
        .map(|admission| (admission.stable_identity.as_str(), admission.packet_ordinal))
        .collect::<HashMap<_, _>>();
    support.sort_by_key(|unit| {
        unit.symbol_id
            .as_deref()
            .and_then(|id| ordinals.get(format!("node:{id}").as_str()).copied())
            .or_else(|| {
                unit.path.as_deref().and_then(|path| {
                    let path = packet_display_path(path);
                    ordinals.get(format!("path:{path}").as_str()).copied()
                })
            })
            .unwrap_or(u32::MAX)
    });
}

fn interim_continuations(
    packet: &AgentPacketDto,
    session: Option<&PacketProofSession>,
) -> Vec<PacketContinuationSelectorV1> {
    let mut continuation = Vec::new();
    let mut seen = BTreeSet::new();

    if let Some(session) = session {
        for gap in session.gaps() {
            let Some(stable_identity) = gap.stable_identity else {
                continue;
            };
            let reason = match gap.kind {
                PacketAdmissionGapKindV1::CandidateCountExceeded => {
                    PacketStructuralGapReasonV1::CandidateCountExceeded
                }
                PacketAdmissionGapKindV1::SourceBudgetExceeded => {
                    PacketStructuralGapReasonV1::SourceBudgetExceeded
                }
                PacketAdmissionGapKindV1::StableIdentityMissing
                | PacketAdmissionGapKindV1::SourceBoundMissing
                | PacketAdmissionGapKindV1::SourceUnavailable => {
                    PacketStructuralGapReasonV1::SourceUnavailable
                }
            };
            push_continuation(&mut continuation, &mut seen, stable_identity, reason);
        }
    }

    for resolution in packet
        .plan
        .probe_resolutions
        .iter()
        .filter(|resolution| resolution.status == PacketProbeResolutionStatusDto::Ambiguous)
    {
        for candidate in &resolution.candidates {
            push_continuation(
                &mut continuation,
                &mut seen,
                format!("node:{}", candidate.symbol_id),
                PacketStructuralGapReasonV1::AmbiguousSelector,
            );
        }
    }

    continuation
}

fn push_continuation(
    continuation: &mut Vec<PacketContinuationSelectorV1>,
    seen: &mut BTreeSet<String>,
    stable_identity: String,
    reason: PacketStructuralGapReasonV1,
) {
    let key = format!("{reason:?}:{stable_identity}");
    if !seen.insert(key) {
        return;
    }
    let path = stable_identity.strip_prefix("path:").map(str::to_string);
    let symbol_id = stable_identity.strip_prefix("node:").map(str::to_string);
    continuation.push(PacketContinuationSelectorV1 {
        stable_identity,
        path,
        symbol_id,
        reason,
    });
}

fn classify_packet_disposition(
    packet: &AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
    continuation: &[PacketContinuationSelectorV1],
    core_generation_id: String,
    retrieval_generation: Option<String>,
) -> PacketDispositionDto {
    if let Some(request) = request {
        if let Some(expected) = request.core_generation_id.as_deref()
            && expected != core_generation_id
        {
            return PacketDispositionDto::unavailable("pinned core publication changed");
        }
        if let Some(expected) = request.retrieval_generation.as_deref()
            && Some(expected) != retrieval_generation.as_deref()
        {
            return PacketDispositionDto::unavailable("pinned retrieval publication changed");
        }
    }

    let freshness = PacketFreshnessInput::from_observation(packet.answer.freshness.as_ref());
    if freshness.blocks_packet_availability() {
        return PacketDispositionDto::unavailable(
            freshness
                .gap()
                .unwrap_or_else(|| "publication freshness is not established".to_string()),
        );
    }
    let coverage = packet_coverage_for_disposition(&packet.answer.source_coverage, &packet.support);
    if coverage.blocks_packet_availability() {
        return PacketDispositionDto::unavailable(
            coverage
                .gaps()
                .into_iter()
                .next()
                .unwrap_or_else(|| "source coverage is not established".to_string()),
        );
    }
    if packet.answer.retrieval_trace.steps.iter().any(|step| {
        matches!(
            step.status,
            codestory_contracts::api::AgentRetrievalStepStatusDto::Error
        )
    }) {
        return PacketDispositionDto::unavailable("retrieval recorded a hard error");
    }

    let already_drilled = request.is_some_and(|request| {
        request.parent_packet_id.is_some() || !request.option_ids.is_empty()
    });
    if !already_drilled {
        let options = continuation
            .iter()
            .filter_map(drill_option_from_selector)
            .take(PACKET_DRILL_MAX_OPTIONS)
            .collect::<Vec<_>>();
        if !options.is_empty() {
            return PacketDispositionDto::drill_once(
                "bounded structural continuation available",
                BoundedDrillPlanDto {
                    parent_packet_id: packet.packet_id.clone(),
                    core_generation_id,
                    retrieval_generation,
                    gap_ids: options.iter().map(|option| option.gap_id.clone()).collect(),
                    options,
                    max_bytes: PACKET_DRILL_MAX_BYTES,
                    max_hits: PACKET_DRILL_MAX_HITS,
                    max_depth: PACKET_DRILL_MAX_DEPTH,
                    remaining_rounds: 1,
                },
            );
        }
    }

    if packet.support.is_empty() {
        PacketDispositionDto::not_established("no bounded repository evidence was retained")
    } else if already_drilled && !continuation.is_empty() {
        PacketDispositionDto::not_established("the bounded continuation left a structural gap")
    } else {
        // This legacy internal state means only that positive evidence exists.
        // Public v3 never projects it as answer sufficiency.
        PacketDispositionDto::supported()
    }
}

fn packet_coverage_for_disposition(
    observations: &[codestory_contracts::api::SourceCoverageObservationDto],
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
    let blocking = observations
        .iter()
        .filter(|observation| {
            observation.status != SourceCoverageStatusDto::Incomplete
                || observation.reason != Some(FileCoverageReason::ParserPartial)
                || !verified_source_paths.contains(&packet_display_path(&observation.path))
        })
        .cloned()
        .collect::<Vec<_>>();
    PacketCoverageInput::from_observations(&blocking)
}

fn drill_option_from_selector(selector: &PacketContinuationSelectorV1) -> Option<DrillOptionDto> {
    let gap_id = format!(
        "{}:{}",
        structural_reason_label(selector.reason),
        selector.stable_identity
    );
    let mut option = if let Some(path) = selector
        .path
        .as_deref()
        .or_else(|| selector.stable_identity.strip_prefix("path:"))
    {
        DrillOptionDto::bounded_source_read(gap_id, path)
    } else {
        let symbol_id = selector
            .symbol_id
            .as_deref()
            .or_else(|| selector.stable_identity.strip_prefix("node:"))?;
        DrillOptionDto::omitted_symbol(gap_id, symbol_id)
    };
    option.structural_reason = Some(selector.reason);
    Some(option)
}

fn structural_reason_label(reason: PacketStructuralGapReasonV1) -> &'static str {
    match reason {
        PacketStructuralGapReasonV1::CandidateCountExceeded => "candidate_count_exceeded",
        PacketStructuralGapReasonV1::SourceBudgetExceeded => "source_budget_exceeded",
        PacketStructuralGapReasonV1::SourceUnavailable => "source_unavailable",
        PacketStructuralGapReasonV1::AmbiguousSelector => "ambiguous_selector",
        PacketStructuralGapReasonV1::DisconnectedSeed => "disconnected_seed",
    }
}

/// Decode only stable path or symbol continuations. Historical query-text
/// options are deliberately not reintroduced as retrieval policy.
pub fn drill_options_from_ids(option_ids: &[String]) -> Vec<DrillOptionDto> {
    option_ids
        .iter()
        .filter_map(|id| {
            let (kind, target) = decode_drill_option_id(id)?;
            match kind {
                DrillGapKindDto::BoundedSourceRead => Some(DrillOptionDto::bounded_source_read(
                    format!("source_unavailable:{target}"),
                    target,
                )),
                DrillGapKindDto::OmittedMandatorySupport => {
                    if let Some(path) = target.strip_prefix("path:") {
                        Some(DrillOptionDto::omitted_source_path(
                            format!("disconnected_seed:{path}"),
                            path,
                        ))
                    } else {
                        let symbol = target.strip_prefix("symbol:")?;
                        Some(DrillOptionDto::omitted_symbol(
                            format!("disconnected_seed:{symbol}"),
                            symbol,
                        ))
                    }
                }
            }
        })
        .take(PACKET_DRILL_MAX_OPTIONS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_query_continuations_are_not_decoded() {
        assert!(drill_options_from_ids(&["deadline_lost_candidate:diagnostic".into()]).is_empty());
    }

    #[test]
    fn stable_symbol_continuation_round_trips_without_query_text() {
        let original = DrillOptionDto::omitted_symbol("gap", "node-1");
        let decoded = drill_options_from_ids(&[original.id]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].symbol_id.as_deref(), Some("node-1"));
        assert!(decoded[0].structural_reason.is_some());
    }

    #[test]
    fn admission_gaps_map_to_typed_structural_reasons() {
        let mut continuation = Vec::new();
        let mut seen = BTreeSet::new();
        push_continuation(
            &mut continuation,
            &mut seen,
            "path:src/lib.rs".into(),
            PacketStructuralGapReasonV1::SourceBudgetExceeded,
        );
        assert_eq!(continuation.len(), 1);
        assert_eq!(continuation[0].path.as_deref(), Some("src/lib.rs"));
        assert_eq!(
            continuation[0].reason,
            PacketStructuralGapReasonV1::SourceBudgetExceeded
        );
    }
}
