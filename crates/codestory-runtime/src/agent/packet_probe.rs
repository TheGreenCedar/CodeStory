use crate::AppController;
use crate::agent::citation::to_citation_from_hit;
use crate::agent::packet_candidate::{PacketAdmissionDecision, active_packet_proof_session};
use crate::agent::retrieval_primary::active_pinned_retrieval_publication;
use crate::target_resolution::{TargetResolution, TargetSelection};
use codestory_agent::{PinnedReader, admit_continuation_probe};
use codestory_contracts::api::{
    AgentCitationDto, NodeId, NodeKind, PacketEvidenceResolutionDto, PacketEvidenceTierDto,
    PacketProbeAmbiguityCandidateDto, PacketProbeDto, PacketProbeRejectionCodeDto,
    PacketProbeRejectionDto, PacketProbeResolutionDto, PacketProbeResolutionStatusDto,
    SearchHitOrigin,
};
use codestory_contracts::compilation::PacketContinuationSelectorV1;
use codestory_workspace::{
    ProjectRelativePathResolution, project_identity_v3, resolve_project_relative_path,
};
use std::path::Path;

pub(crate) fn normalize_packet_probe_request(probes: &[PacketProbeDto]) -> Vec<PacketProbeDto> {
    probes.to_vec()
}

pub(crate) fn resolve_packet_probes(
    controller: &AppController,
    probes: Vec<PacketProbeDto>,
) -> Vec<PacketProbeResolutionDto> {
    probes
        .into_iter()
        .enumerate()
        .map(|(index, probe)| {
            let input_index = index as u32;
            let reservation = packet_probe_reservation_identity(&probe);
            if let (Some(session), Some(identity)) =
                (active_packet_proof_session(), reservation.as_deref())
            {
                match session.admit_exact_selector(
                    identity,
                    codestory_contracts::compilation::INTERIM_SOURCE_ROW_UPPER_BOUND,
                    input_index,
                ) {
                    PacketAdmissionDecision::Admitted
                    | PacketAdmissionDecision::AlreadyAdmitted => {}
                    PacketAdmissionDecision::CountBudgetExceeded => {
                        return rejected_resolution(
                            input_index,
                            probe,
                            PacketProbeRejectionCodeDto::CandidateCountExceeded,
                            "packet candidate count budget was exhausted before probe resolution",
                        );
                    }
                    PacketAdmissionDecision::SourceBudgetExceeded => {
                        return rejected_resolution(
                            input_index,
                            probe,
                            PacketProbeRejectionCodeDto::SourceBudgetExceeded,
                            "packet source budget was exhausted before probe resolution",
                        );
                    }
                }
            }

            let mut resolution = resolve_packet_probe(controller, input_index, probe);
            if let (Some(session), Some(reserved)) =
                (active_packet_proof_session(), reservation.as_deref())
            {
                finalize_packet_probe_admission(&session, reserved, input_index, &mut resolution);
            }
            resolution
        })
        .collect()
}

fn packet_probe_reservation_identity(probe: &PacketProbeDto) -> Option<String> {
    match probe {
        PacketProbeDto::ExactPath { path } => Some(format!("path:{}", path.trim())),
        PacketProbeDto::SymbolId { id } => Some(format!("node:{}", id.trim())),
        PacketProbeDto::QualifiedSymbol { symbol } => {
            Some(format!("selector:qualified_symbol:{}", symbol.trim()))
        }
        PacketProbeDto::FileSymbol { path, symbol } => Some(format!(
            "selector:file_symbol:{}::{}",
            path.trim(),
            symbol.trim()
        )),
        PacketProbeDto::Continuation { selector, .. } => Some(selector.stable_identity.clone()),
        PacketProbeDto::FreeQuery { .. } => None,
    }
}

fn packet_probe_resolution_identity(resolution: &PacketProbeResolutionDto) -> Option<String> {
    resolution
        .symbol_id
        .as_deref()
        .map(|id| format!("node:{id}"))
        .or_else(|| {
            matches!(
                resolution.status,
                PacketProbeResolutionStatusDto::ExactPath
                    | PacketProbeResolutionStatusDto::ValidUncoveredPath
            )
            .then(|| {
                resolution
                    .path
                    .as_deref()
                    .map(|path| format!("path:{path}"))
            })
            .flatten()
        })
}

fn finalize_packet_probe_admission(
    session: &crate::agent::packet_candidate::PacketProofSession,
    reserved: &str,
    selector_ordinal: u32,
    resolution: &mut PacketProbeResolutionDto,
) {
    let mut seen = std::collections::HashSet::new();
    let mut identities = packet_probe_resolution_identity(resolution)
        .into_iter()
        .chain(
            resolution
                .candidates
                .iter()
                .map(|candidate| format!("node:{}", candidate.symbol_id)),
        )
        .filter(|identity| seen.insert(identity.clone()))
        .collect::<Vec<_>>();
    if identities.is_empty() {
        session.consume_unresolved_reservation(reserved);
        return;
    }

    let first = identities.remove(0);
    session.canonicalize_identity(reserved, &first);
    let mut admitted = std::collections::HashSet::from([first]);
    for identity in identities {
        match session.admit_exact_selector(
            &identity,
            codestory_contracts::compilation::INTERIM_SOURCE_ROW_UPPER_BOUND,
            selector_ordinal,
        ) {
            PacketAdmissionDecision::Admitted | PacketAdmissionDecision::AlreadyAdmitted => {
                admitted.insert(identity);
            }
            PacketAdmissionDecision::CountBudgetExceeded
            | PacketAdmissionDecision::SourceBudgetExceeded => {}
        }
    }
    resolution
        .candidates
        .retain(|candidate| admitted.contains(&format!("node:{}", candidate.symbol_id)));
}

pub(crate) fn exact_packet_probe_citations(
    controller: &AppController,
    resolutions: &[PacketProbeResolutionDto],
    _question: &str,
    include_evidence: bool,
) -> Vec<AgentCitationDto> {
    let mut citations = Vec::new();
    for resolution in resolutions {
        let mut append = |citation: Option<AgentCitationDto>| {
            let Some(citation) = citation else {
                return;
            };
            if !citations.iter().any(|existing: &AgentCitationDto| {
                existing.node_id == citation.node_id && existing.file_path == citation.file_path
            }) {
                citations.push(citation);
            }
        };
        match resolution.status {
            PacketProbeResolutionStatusDto::ExactPath => {
                append(exact_path_probe_citation(controller, resolution));
            }
            PacketProbeResolutionStatusDto::ValidUncoveredPath => {
                append(exact_path_probe_citation(controller, resolution));
            }
            PacketProbeResolutionStatusDto::IndexedSymbol
            | PacketProbeResolutionStatusDto::FileScopedSymbol
            | PacketProbeResolutionStatusDto::TextHit
            | PacketProbeResolutionStatusDto::Continuation => {
                append(resolution.symbol_id.as_deref().and_then(|symbol_id| {
                    exact_symbol_probe_citation(controller, symbol_id, include_evidence)
                }));
            }
            PacketProbeResolutionStatusDto::FreeQuery
            | PacketProbeResolutionStatusDto::Ambiguous
            | PacketProbeResolutionStatusDto::Rejected => {}
        }
    }
    citations
}

fn exact_symbol_probe_citation(
    controller: &AppController,
    symbol_id: &str,
    include_evidence: bool,
) -> Option<AgentCitationDto> {
    let TargetResolution::Resolved(resolved) = controller
        .resolve_source_target(TargetSelection::Id(NodeId(symbol_id.to_string())), None)
        .ok()?
    else {
        return None;
    };
    let mut citation = to_citation_from_hit(&resolved.selected, None, None, include_evidence);
    citation.score = 100.0;
    citation.evidence_producer = Some("packet_exact_symbol_probe".to_string());
    citation.eligible_for_sufficiency = None;
    Some(citation)
}

fn exact_path_probe_citation(
    controller: &AppController,
    resolution: &PacketProbeResolutionDto,
) -> Option<AgentCitationDto> {
    let project_root = controller.require_project_root().ok()?;
    let relative = resolution.path.as_deref()?;
    let ProjectRelativePathResolution::Existing { relative, .. } =
        resolve_project_relative_path(&project_root, Path::new(relative)).ok()?
    else {
        return None;
    };
    let path = display_relative_path(&relative);
    Some(AgentCitationDto {
        node_id: NodeId(format!("packet::exact_path::{path}")),
        display_name: path.clone(),
        kind: NodeKind::FILE,
        file_path: Some(path),
        line: Some(1),
        score: 100.0,
        origin: SearchHitOrigin::TextMatch,
        target: None,
        resolvable: false,
        subgraph_id: None,
        evidence_edge_ids: Vec::new(),
        retrieval_score_breakdown: None,
        evidence_tier: Some(PacketEvidenceTierDto::ExactSource),
        evidence_producer: Some("packet_exact_path_probe".to_string()),
        resolution_status: Some(PacketEvidenceResolutionDto::SourceRangeOnly),
        loss_reason: None,
        eligible_for_sufficiency: None,
        source_excerpt: None,
    })
}

fn resolve_packet_probe(
    controller: &AppController,
    input_index: u32,
    probe: PacketProbeDto,
) -> PacketProbeResolutionDto {
    match probe.clone() {
        PacketProbeDto::ExactPath { path } => {
            resolve_exact_path_probe(controller, input_index, probe, &path)
        }
        PacketProbeDto::SymbolId { id } => {
            resolve_symbol_id_probe(controller, input_index, probe, &id)
        }
        PacketProbeDto::QualifiedSymbol { symbol } => {
            resolve_qualified_symbol_probe(controller, input_index, probe, &symbol)
        }
        PacketProbeDto::FileSymbol { path, symbol } => {
            resolve_file_symbol_probe(controller, input_index, probe, &path, &symbol)
        }
        PacketProbeDto::FreeQuery { query } => {
            let query = query.trim();
            if query.is_empty() {
                rejected_resolution(
                    input_index,
                    probe,
                    PacketProbeRejectionCodeDto::MalformedProbe,
                    "query probe must not be empty",
                )
            } else {
                base_resolution(
                    input_index,
                    probe,
                    PacketProbeResolutionStatusDto::FreeQuery,
                    Some(query.to_string()),
                )
            }
        }
        PacketProbeDto::Continuation {
            contract_version,
            project_id,
            core_generation_id,
            retrieval_generation,
            selector,
        } => resolve_continuation_probe(
            controller,
            input_index,
            probe,
            ContinuationPublication {
                contract_version,
                project_id: &project_id,
                core_generation_id: &core_generation_id,
                retrieval_generation: retrieval_generation.as_deref(),
            },
            &selector,
        ),
    }
}

fn resolve_qualified_symbol_probe(
    controller: &AppController,
    input_index: u32,
    probe: PacketProbeDto,
    symbol: &str,
) -> PacketProbeResolutionDto {
    let symbol = symbol.trim();
    if symbol.is_empty() {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MalformedProbe,
            "qualified-symbol probe must not be empty",
        );
    }
    let candidates = match controller.resolve_exact_indexed_symbol_identities(symbol) {
        Ok(candidates) => candidates,
        Err(error) => {
            return rejected_resolution(
                input_index,
                probe,
                PacketProbeRejectionCodeDto::MissingTarget,
                error.message,
            );
        }
    };
    match candidates.as_slice() {
        [] => rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MissingTarget,
            "qualified-symbol selector did not exactly match an indexed identity",
        ),
        [candidate] => {
            let mut resolution = base_resolution(
                input_index,
                probe,
                PacketProbeResolutionStatusDto::IndexedSymbol,
                Some(symbol.to_string()),
            );
            resolution.symbol_id = Some(candidate.node_id.0.clone());
            resolution
        }
        _ => PacketProbeResolutionDto {
            input_index,
            probe,
            status: PacketProbeResolutionStatusDto::Ambiguous,
            normalized_query: Some(symbol.to_string()),
            path: None,
            symbol_id: None,
            candidates: candidates
                .into_iter()
                .map(|candidate| PacketProbeAmbiguityCandidateDto {
                    symbol_id: candidate.node_id.0,
                    display_name: candidate.display_name,
                    path: None,
                    kind: NodeKind::UNKNOWN,
                })
                .collect(),
            rejection: None,
        },
    }
}

fn resolve_exact_path_probe(
    controller: &AppController,
    input_index: u32,
    probe: PacketProbeDto,
    path: &str,
) -> PacketProbeResolutionDto {
    let path = path.trim();
    if path.is_empty() {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MalformedProbe,
            "exact-path probe must not be empty",
        );
    }
    let Ok(project_root) = controller.require_project_root() else {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MalformedProbe,
            "exact-path probe requires an open project",
        );
    };
    let resolution = match resolve_project_relative_path(&project_root, Path::new(path)) {
        Ok(resolution) => resolution,
        Err(error) => {
            return rejected_resolution(
                input_index,
                probe,
                PacketProbeRejectionCodeDto::MalformedProbe,
                format!("exact-path probe could not be observed: {error}"),
            );
        }
    };
    match resolution {
        ProjectRelativePathResolution::Existing { absolute, relative } => {
            let normalized = display_relative_path(&relative);
            let indexed = controller
                .open_storage_read_only()
                .ok()
                .and_then(|storage| {
                    storage
                        .has_complete_indexed_file_path(&[absolute, relative])
                        .ok()
                })
                .unwrap_or(false);
            let mut resolution = base_resolution(
                input_index,
                probe,
                if indexed {
                    PacketProbeResolutionStatusDto::ExactPath
                } else {
                    PacketProbeResolutionStatusDto::ValidUncoveredPath
                },
                Some(normalized.clone()),
            );
            resolution.path = Some(normalized);
            resolution
        }
        ProjectRelativePathResolution::Missing { relative, .. } => rejected_resolution_with_path(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MissingTarget,
            "exact-path target does not exist",
            display_relative_path(&relative),
        ),
        ProjectRelativePathResolution::NotFile { relative, .. } => rejected_resolution_with_path(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MissingTarget,
            "exact-path target is not a file",
            display_relative_path(&relative),
        ),
        ProjectRelativePathResolution::OutOfProject => rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::OutOfProject,
            "exact-path target is outside the selected project",
        ),
    }
}

fn resolve_symbol_id_probe(
    controller: &AppController,
    input_index: u32,
    probe: PacketProbeDto,
    id: &str,
) -> PacketProbeResolutionDto {
    let id = id.trim();
    if id.is_empty() {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MalformedProbe,
            "symbol-id probe must not be empty",
        );
    }
    match controller.resolve_indexed_symbol_identity_by_id(&NodeId(id.to_string())) {
        Ok(Some(identity)) => {
            let mut resolution = base_resolution(
                input_index,
                probe,
                PacketProbeResolutionStatusDto::IndexedSymbol,
                Some(identity.display_name),
            );
            resolution.symbol_id = Some(identity.node_id.0);
            resolution
        }
        Ok(None) => rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::StaleSymbolId,
            "symbol-id selector did not match the pinned identity index",
        ),
        Err(error) => rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::StaleSymbolId,
            error.message,
        ),
    }
}

fn resolve_file_symbol_probe(
    controller: &AppController,
    input_index: u32,
    probe: PacketProbeDto,
    path: &str,
    symbol: &str,
) -> PacketProbeResolutionDto {
    let symbol = symbol.trim();
    if symbol.is_empty() {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MalformedProbe,
            "file-symbol probe symbol must not be empty",
        );
    }
    let path_resolution =
        resolve_exact_path_probe(controller, input_index, probe.clone(), path.trim());
    if !matches!(
        path_resolution.status,
        PacketProbeResolutionStatusDto::ExactPath
    ) {
        return path_resolution;
    }
    let normalized_path = path_resolution.path.clone().unwrap_or_default();
    let Ok(project_root) = controller.require_project_root() else {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MalformedProbe,
            "file-symbol probe requires an open project",
        );
    };
    let exact_path = project_root.join(&normalized_path);
    match controller.resolve_exact_indexed_symbol_identities_in_file(
        symbol,
        &project_root,
        &exact_path,
    ) {
        Ok(candidates) if candidates.len() == 1 => {
            let candidate = candidates.into_iter().next().expect("one candidate");
            let mut resolution = base_resolution(
                input_index,
                probe,
                PacketProbeResolutionStatusDto::FileScopedSymbol,
                Some(format!("{normalized_path}::{symbol}")),
            );
            resolution.path = Some(normalized_path);
            resolution.symbol_id = Some(candidate.node_id.0);
            resolution
        }
        Ok(candidates) if !candidates.is_empty() => PacketProbeResolutionDto {
            input_index,
            probe,
            status: PacketProbeResolutionStatusDto::Ambiguous,
            normalized_query: Some(format!("{normalized_path}::{symbol}")),
            path: Some(normalized_path.clone()),
            symbol_id: None,
            candidates: candidates
                .into_iter()
                .map(|candidate| PacketProbeAmbiguityCandidateDto {
                    symbol_id: candidate.node_id.0,
                    display_name: candidate.display_name,
                    path: Some(normalized_path.clone()),
                    kind: NodeKind::UNKNOWN,
                })
                .collect(),
            rejection: None,
        },
        Ok(_) => rejected_resolution_with_path(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MissingTarget,
            "file-symbol selector did not exactly match the pinned identity index",
            normalized_path,
        ),
        Err(error) => rejected_resolution_with_path(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MissingTarget,
            error.message,
            normalized_path,
        ),
    }
}

/// The runtime's implementation of planning's read-only seam.
///
/// Every method is an owned read of an identity the current public operation
/// already pinned. Nothing here opens storage, activates a publication, or
/// retries one, and a missing pin is reported as `None` so the planning side
/// refuses rather than guesses.
struct ControllerPinnedReader<'a> {
    controller: &'a AppController,
}

impl PinnedReader for ControllerPinnedReader<'_> {
    fn pinned_project_id(&self) -> Option<String> {
        self.controller
            .require_project_root()
            .ok()
            .map(|root| project_identity_v3(&root).project_id)
    }

    fn pinned_core_generation_id(&self) -> Option<String> {
        self.controller
            .active_core_publication()
            .map(|publication| publication.generation_id)
    }

    fn pinned_retrieval_generation(&self) -> Option<String> {
        active_pinned_retrieval_publication(self.controller)
            .map(|publication| publication.retrieval_generation)
    }

    fn pinned_source_text(&self, citation: &AgentCitationDto) -> Option<String> {
        let _ = self.controller;
        citation
            .source_excerpt
            .as_deref()
            .map(str::trim)
            .filter(|excerpt| !excerpt.is_empty())
            .map(str::to_string)
    }
}

struct ContinuationPublication<'a> {
    contract_version: u32,
    project_id: &'a str,
    core_generation_id: &'a str,
    retrieval_generation: Option<&'a str>,
}

fn resolve_continuation_probe(
    controller: &AppController,
    input_index: u32,
    probe: PacketProbeDto,
    publication: ContinuationPublication<'_>,
    selector: &PacketContinuationSelectorV1,
) -> PacketProbeResolutionDto {
    if let Err(refusal) = admit_continuation_probe(
        &ControllerPinnedReader { controller },
        publication.contract_version,
        publication.project_id,
        publication.core_generation_id,
        publication.retrieval_generation,
    ) {
        return rejected_resolution(input_index, probe, refusal.code(), refusal.message());
    }
    if let Some(symbol_id) = selector.symbol_id.as_deref() {
        let symbol_id = symbol_id.strip_prefix("node:").unwrap_or(symbol_id);
        let mut resolution = resolve_symbol_id_probe(controller, input_index, probe, symbol_id);
        if resolution.status == PacketProbeResolutionStatusDto::IndexedSymbol {
            resolution.status = PacketProbeResolutionStatusDto::Continuation;
        }
        return resolution;
    }
    if let Some(path) = selector.path.as_deref() {
        let mut resolution = resolve_exact_path_probe(controller, input_index, probe, path);
        if matches!(
            resolution.status,
            PacketProbeResolutionStatusDto::ExactPath
                | PacketProbeResolutionStatusDto::ValidUncoveredPath
        ) {
            resolution.status = PacketProbeResolutionStatusDto::Continuation;
        }
        return resolution;
    }
    rejected_resolution(
        input_index,
        probe,
        PacketProbeRejectionCodeDto::MalformedProbe,
        "continuation selector requires a stable path or symbol identity",
    )
}

fn base_resolution(
    input_index: u32,
    probe: PacketProbeDto,
    status: PacketProbeResolutionStatusDto,
    normalized_query: Option<String>,
) -> PacketProbeResolutionDto {
    PacketProbeResolutionDto {
        input_index,
        probe,
        status,
        normalized_query,
        path: None,
        symbol_id: None,
        candidates: Vec::new(),
        rejection: None,
    }
}

fn rejected_resolution(
    input_index: u32,
    probe: PacketProbeDto,
    code: PacketProbeRejectionCodeDto,
    message: impl Into<String>,
) -> PacketProbeResolutionDto {
    PacketProbeResolutionDto {
        input_index,
        probe,
        status: PacketProbeResolutionStatusDto::Rejected,
        normalized_query: None,
        path: None,
        symbol_id: None,
        candidates: Vec::new(),
        rejection: Some(PacketProbeRejectionDto {
            code,
            message: message.into(),
        }),
    }
}

fn rejected_resolution_with_path(
    input_index: u32,
    probe: PacketProbeDto,
    code: PacketProbeRejectionCodeDto,
    message: impl Into<String>,
    path: String,
) -> PacketProbeResolutionDto {
    let mut resolution = rejected_resolution(input_index, probe, code, message);
    resolution.path = Some(path);
    resolution
}

fn display_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::packet_candidate::{
        PacketAdmissionDecision, PacketProofSession, install_packet_proof_session,
    };
    use codestory_contracts::compilation::{
        PACKET_RETRIEVAL_SCORE_VERSION_V1, PacketCandidateDescriptorV1, PacketRetrievalLaneV1,
        VersionedRetrievalScoreV1,
    };
    use std::rc::Rc;

    fn retrieval_descriptor(index: usize) -> PacketCandidateDescriptorV1 {
        PacketCandidateDescriptorV1 {
            stable_identity: format!("node:retrieval-{index}"),
            path: format!("src/retrieval-{index}.rs"),
            symbol: Some(format!("retrieval_{index}")),
            retrieval_lane: PacketRetrievalLaneV1::Lexical,
            retrieval_score: VersionedRetrievalScoreV1 {
                version: PACKET_RETRIEVAL_SCORE_VERSION_V1.to_string(),
                value: 1.0,
            },
            source_bytes_upper_bound: Some(1),
            exact_selector_ordinal: None,
        }
    }

    #[test]
    fn unresolved_exact_probe_keeps_its_packet_wide_reservation_charged() {
        let controller = AppController::new();
        let session = Rc::new(PacketProofSession::new());
        let _guard = install_packet_proof_session(Rc::clone(&session));

        let resolutions = resolve_packet_probes(
            &controller,
            vec![PacketProbeDto::ExactPath {
                path: "src/missing.rs".into(),
            }],
        );
        assert_eq!(
            resolutions[0].status,
            PacketProbeResolutionStatusDto::Rejected
        );
        assert!(session.receipts().is_empty());
        assert_eq!(*session.hydrated_admissions.borrow(), 1);

        for index in 0..15 {
            assert_eq!(
                session.admit_descriptor(&retrieval_descriptor(index)),
                PacketAdmissionDecision::Admitted
            );
        }
        assert_eq!(
            session.admit_descriptor(&retrieval_descriptor(15)),
            PacketAdmissionDecision::CountBudgetExceeded
        );
        assert_eq!(*session.hydrated_admissions.borrow(), 16);
        assert_eq!(session.receipts().len(), 15);
    }

    #[test]
    fn ambiguous_exact_probe_retains_only_candidates_admitted_by_the_shared_session() {
        let session = PacketProofSession::new();
        let reserved = "selector:qualified_symbol:shared";
        assert_eq!(
            session.admit_exact_selector(
                reserved,
                codestory_contracts::compilation::INTERIM_SOURCE_ROW_UPPER_BOUND,
                0,
            ),
            PacketAdmissionDecision::Admitted
        );
        let mut resolution = PacketProbeResolutionDto {
            input_index: 0,
            probe: PacketProbeDto::QualifiedSymbol {
                symbol: "shared".into(),
            },
            status: PacketProbeResolutionStatusDto::Ambiguous,
            normalized_query: Some("shared".into()),
            path: None,
            symbol_id: None,
            candidates: (0..20)
                .map(|index| PacketProbeAmbiguityCandidateDto {
                    symbol_id: index.to_string(),
                    display_name: "shared".into(),
                    path: None,
                    kind: NodeKind::FUNCTION,
                })
                .collect(),
            rejection: None,
        };

        finalize_packet_probe_admission(&session, reserved, 0, &mut resolution);

        assert_eq!(resolution.candidates.len(), 16);
        assert_eq!(
            resolution
                .candidates
                .iter()
                .map(|candidate| candidate.symbol_id.clone())
                .collect::<Vec<_>>(),
            (0..16).map(|index| index.to_string()).collect::<Vec<_>>()
        );
        assert_eq!(*session.hydrated_admissions.borrow(), 16);
        assert_eq!(session.receipts().len(), 16);
        assert_eq!(
            session.admit_descriptor(&retrieval_descriptor(99)),
            PacketAdmissionDecision::CountBudgetExceeded
        );
    }
}
