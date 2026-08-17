use crate::AppController;
use crate::agent::citation::to_citation_from_hit;
use crate::agent::packet_evidence_roles::{PacketEvidenceRole, packet_evidence_role};
use crate::agent::packet_scoring::normalize_identifier;
use crate::agent::packet_terms::prompt_search_terms;
use crate::agent::retrieval_primary::active_pinned_retrieval_publication;
use crate::target_resolution::{TargetResolution, TargetSelection, search_hit_matches_exact_file};
pub(crate) use codestory_agent::packet_probes::exact_packet_probe_paths;
use codestory_agent::{PinnedReader, admit_continuation_probe};
use codestory_contracts::api::{
    AgentCitationDto, NodeId, NodeKind, PacketEvidenceResolutionDto, PacketEvidenceTierDto,
    PacketProbeAmbiguityCandidateDto, PacketProbeDto, PacketProbeRejectionCodeDto,
    PacketProbeRejectionDto, PacketProbeResolutionDto, PacketProbeResolutionStatusDto, SearchHit,
    SearchHitOrigin,
};
use codestory_workspace::{
    ProjectRelativePathResolution, project_identity_v3, resolve_project_relative_path,
    same_workspace_path,
};
use std::path::Path;

pub(crate) fn normalize_packet_probe_request(
    probes: &[PacketProbeDto],
    legacy_probes: &[String],
) -> Vec<PacketProbeDto> {
    probes
        .iter()
        .cloned()
        .chain(legacy_probes.iter().map(|probe| {
            let probe = probe.trim();
            serde_json::from_str::<PacketProbeDto>(probe)
                .ok()
                .unwrap_or_else(|| legacy_packet_probe(probe))
        }))
        .collect()
}

fn legacy_packet_probe(probe: &str) -> PacketProbeDto {
    if probe.parse::<i64>().is_ok() {
        return PacketProbeDto::SymbolId {
            id: probe.to_string(),
        };
    }
    if let Some((path, symbol)) = probe.split_once(char::is_whitespace)
        && legacy_probe_path_like(path)
        && !symbol.trim().is_empty()
    {
        return PacketProbeDto::FileSymbol {
            path: path.to_string(),
            symbol: symbol.trim().to_string(),
        };
    }
    if legacy_probe_path_like(probe) {
        return PacketProbeDto::ExactPath {
            path: probe.to_string(),
        };
    }
    PacketProbeDto::FreeQuery {
        query: probe.to_string(),
    }
}

fn legacy_probe_path_like(value: &str) -> bool {
    !value.contains("://")
        && (value.contains('/') || value.contains('\\'))
        && Path::new(value).extension().is_some()
}

pub(crate) fn unresolved_packet_probe_queries(probes: &[PacketProbeDto]) -> Vec<String> {
    probes
        .iter()
        .filter_map(packet_probe_query)
        .filter(|query| !query.trim().is_empty())
        .collect()
}

pub(crate) fn resolved_packet_probe_queries(
    resolutions: &[PacketProbeResolutionDto],
) -> Vec<String> {
    resolutions
        .iter()
        .filter(|resolution| {
            resolution.status == PacketProbeResolutionStatusDto::FreeQuery
                || (resolution.status == PacketProbeResolutionStatusDto::Continuation
                    && resolution.symbol_id.is_none())
        })
        .filter_map(|resolution| resolution.normalized_query.clone())
        .collect()
}

pub(crate) fn resolve_packet_probes(
    controller: &AppController,
    probes: Vec<PacketProbeDto>,
) -> Vec<PacketProbeResolutionDto> {
    probes
        .into_iter()
        .enumerate()
        .map(|(index, probe)| resolve_packet_probe(controller, index as u32, probe))
        .collect()
}

pub(crate) fn exact_packet_probe_citations(
    controller: &AppController,
    resolutions: &[PacketProbeResolutionDto],
    question: &str,
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
                append(exact_path_probe_source_carrier_citation(
                    controller,
                    resolution,
                    question,
                    include_evidence,
                ));
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

fn exact_path_probe_source_carrier_citation(
    controller: &AppController,
    resolution: &PacketProbeResolutionDto,
    question: &str,
    include_evidence: bool,
) -> Option<AgentCitationDto> {
    let project_root = controller.require_project_root().ok()?;
    let requested = resolution.path.as_deref()?;
    let ProjectRelativePathResolution::Existing { absolute, relative } =
        resolve_project_relative_path(&project_root, Path::new(requested)).ok()?
    else {
        return None;
    };
    let storage = controller.open_storage_read_only().ok()?;
    let file_id = storage
        .get_files()
        .ok()?
        .into_iter()
        .find(|file| {
            file.indexed
                && file.complete
                && same_workspace_path(
                    &absolute,
                    &if file.path.is_absolute() {
                        file.path.clone()
                    } else {
                        project_root.join(&file.path)
                    },
                )
        })?
        .id;
    let question_terms = prompt_search_terms(question)
        .into_iter()
        .map(|term| normalize_identifier(&term))
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    let mut candidates = storage
        .get_grounding_root_symbols_for_files(&[file_id], 256)
        .ok()?
        .into_iter()
        .filter_map(|record| {
            let display = normalize_identifier(&record.display_name);
            let term_hits = question_terms
                .iter()
                .filter(|term| {
                    display.contains(term.as_str())
                        || (display.len() >= 4 && term.len() >= 4 && term.contains(&display))
                })
                .count();
            (!display.is_empty() && term_hits > 0).then_some((term_hits, record))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(left_hits, left), (right_hits, right)| {
        right_hits
            .cmp(left_hits)
            .then_with(|| left.node.start_line.cmp(&right.node.start_line))
            .then_with(|| left.display_name.cmp(&right.display_name))
            .then_with(|| left.node.id.cmp(&right.node.id))
    });

    candidates
        .into_iter()
        .filter_map(|(term_hits, record)| {
            let mut citation = exact_symbol_probe_citation(
                controller,
                &record.node.id.to_string(),
                include_evidence,
            )?;
            let cited_path = citation.file_path.as_deref()?;
            if !same_workspace_path(&absolute, &project_root.join(cited_path)) {
                return None;
            }
            if !matches!(
                citation.kind,
                NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
            ) {
                return None;
            }
            citation.file_path = Some(display_relative_path(&relative));
            citation.coverage_role = None;
            let role = packet_evidence_role(&citation)?;
            if matches!(
                role,
                PacketEvidenceRole::SourceEvidence | PacketEvidenceRole::TestsAndRegressionCoverage
            ) {
                return None;
            }
            let role_rank = match role {
                PacketEvidenceRole::CommandEntrypoint => 5,
                PacketEvidenceRole::RequestDispatch
                | PacketEvidenceRole::TransportAdapter
                | PacketEvidenceRole::BufferedIo => 4,
                PacketEvidenceRole::RuntimeOrchestration => 3,
                _ => 2,
            };
            citation.coverage_role = Some(role.as_str().to_string());
            citation.eligible_for_sufficiency = Some(true);
            citation.score = 99.0;
            Some((role_rank, term_hits, citation))
        })
        .max_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| right.2.display_name.cmp(&left.2.display_name))
        })
        .map(|(_, _, citation)| citation)
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
    citation.coverage_role = Some("explicit exact probe".to_string());
    citation.eligible_for_sufficiency = Some(false);
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
        coverage_role: Some("explicit exact probe".to_string()),
        eligible_for_sufficiency: Some(false),
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
            symbol_id,
            query,
        } => resolve_continuation_probe(
            controller,
            input_index,
            probe,
            contract_version,
            &project_id,
            &core_generation_id,
            retrieval_generation.as_deref(),
            symbol_id.as_deref(),
            &query,
        ),
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
                .and_then(|storage| storage.get_files().ok())
                .is_some_and(|files| {
                    files.into_iter().any(|file| {
                        if !file.indexed || !file.complete {
                            return false;
                        }
                        let candidate = if file.path.is_absolute() {
                            file.path
                        } else {
                            project_root.join(file.path)
                        };
                        same_workspace_path(&absolute, &candidate)
                    })
                });
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
    match controller.resolve_source_target(TargetSelection::Id(NodeId(id.to_string())), None) {
        Ok(TargetResolution::Resolved(resolved)) => {
            let mut resolution = base_resolution(
                input_index,
                probe,
                probe_status_for_hit(
                    &resolved.selected,
                    PacketProbeResolutionStatusDto::IndexedSymbol,
                ),
                Some(resolved.selected.display_name),
            );
            resolution.symbol_id = Some(resolved.selected.node_id.0);
            resolution.path = resolved.selected.file_path;
            resolution
        }
        Ok(TargetResolution::Ambiguous(ambiguous)) => {
            ambiguous_resolution(input_index, probe, id.to_string(), ambiguous.alternatives)
        }
        Ok(TargetResolution::Rejected(message)) => rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::StaleSymbolId,
            message,
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
    let exact_path_filter = exact_path.to_string_lossy();
    match controller.resolve_target(
        TargetSelection::Query {
            query: symbol.to_string(),
            choose: None,
        },
        Some(&exact_path_filter),
    ) {
        Ok(TargetResolution::Resolved(resolved)) => {
            let status = probe_status_for_hit(
                &resolved.selected,
                PacketProbeResolutionStatusDto::FileScopedSymbol,
            );
            let mut resolution = base_resolution(
                input_index,
                probe,
                status,
                Some(format!("{normalized_path}::{symbol}")),
            );
            resolution.path = Some(normalized_path);
            resolution.symbol_id = Some(resolved.selected.node_id.0);
            resolution
        }
        Ok(TargetResolution::Ambiguous(ambiguous)) => ambiguous_resolution(
            input_index,
            probe,
            format!("{normalized_path}::{symbol}"),
            ambiguous.alternatives,
        ),
        Ok(TargetResolution::Rejected(message)) => {
            let text_hit = controller
                .resolve_indexed_symbol_candidates(symbol, 50)
                .ok()
                .and_then(|hits| {
                    hits.into_iter().find(|hit| {
                        search_hit_matches_exact_file(&project_root, hit, &exact_path)
                            && (hit.evidence_tier == Some(PacketEvidenceTierDto::StructuralText)
                                || hit.resolution_status
                                    == Some(PacketEvidenceResolutionDto::SourceRangeOnly)
                                || !hit.resolvable)
                    })
                });
            if let Some(hit) = text_hit {
                let mut resolution = base_resolution(
                    input_index,
                    probe,
                    PacketProbeResolutionStatusDto::TextHit,
                    Some(format!("{normalized_path}::{symbol}")),
                );
                resolution.path = Some(normalized_path);
                resolution.symbol_id = Some(hit.node_id.0);
                resolution
            } else {
                rejected_resolution_with_path(
                    input_index,
                    probe,
                    PacketProbeRejectionCodeDto::MissingTarget,
                    message,
                    normalized_path,
                )
            }
        }
        Err(error) => rejected_resolution_with_path(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MissingTarget,
            error.message,
            normalized_path,
        ),
    }
}

fn probe_status_for_hit(
    hit: &SearchHit,
    resolved_status: PacketProbeResolutionStatusDto,
) -> PacketProbeResolutionStatusDto {
    if hit.evidence_tier == Some(PacketEvidenceTierDto::StructuralText)
        || hit.resolution_status == Some(PacketEvidenceResolutionDto::SourceRangeOnly)
        || !hit.resolvable
    {
        PacketProbeResolutionStatusDto::TextHit
    } else {
        resolved_status
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
}

#[allow(clippy::too_many_arguments)]
fn resolve_continuation_probe(
    controller: &AppController,
    input_index: u32,
    probe: PacketProbeDto,
    contract_version: u32,
    project_id: &str,
    core_generation_id: &str,
    retrieval_generation: Option<&str>,
    symbol_id: Option<&str>,
    query: &str,
) -> PacketProbeResolutionDto {
    let query = match admit_continuation_probe(
        &ControllerPinnedReader { controller },
        contract_version,
        project_id,
        core_generation_id,
        retrieval_generation,
        query,
    ) {
        Ok(query) => query,
        Err(refusal) => {
            return rejected_resolution(input_index, probe, refusal.code(), refusal.message());
        }
    };
    let query = query.as_str();
    if let Some(symbol_id) = symbol_id {
        let mut resolution = resolve_symbol_id_probe(controller, input_index, probe, symbol_id);
        if resolution.status == PacketProbeResolutionStatusDto::IndexedSymbol {
            resolution.status = PacketProbeResolutionStatusDto::Continuation;
        }
        return resolution;
    }
    base_resolution(
        input_index,
        probe,
        PacketProbeResolutionStatusDto::Continuation,
        Some(query.to_string()),
    )
}

fn packet_probe_query(probe: &PacketProbeDto) -> Option<String> {
    match probe {
        PacketProbeDto::ExactPath { path } => Some(path.trim().to_string()),
        PacketProbeDto::SymbolId { id } => Some(id.trim().to_string()),
        PacketProbeDto::FileSymbol { path, symbol } => {
            Some(format!("{}::{}", path.trim(), symbol.trim()))
        }
        PacketProbeDto::FreeQuery { query } | PacketProbeDto::Continuation { query, .. } => {
            Some(query.trim().to_string())
        }
    }
}

fn ambiguous_resolution(
    input_index: u32,
    probe: PacketProbeDto,
    normalized_query: String,
    alternatives: Vec<codestory_contracts::api::SearchHit>,
) -> PacketProbeResolutionDto {
    let candidates = alternatives
        .into_iter()
        .map(|hit| PacketProbeAmbiguityCandidateDto {
            symbol_id: hit.node_id.0,
            display_name: hit.display_name,
            path: hit.file_path,
            kind: hit.kind,
        })
        .collect();
    PacketProbeResolutionDto {
        input_index,
        probe,
        status: PacketProbeResolutionStatusDto::Ambiguous,
        normalized_query: Some(normalized_query),
        path: None,
        symbol_id: None,
        candidates,
        rejection: None,
    }
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
    use codestory_contracts::api::PACKET_PROBE_CONTRACT_VERSION;
    use codestory_contracts::graph::{Node, NodeId as CoreNodeId, NodeKind as CoreNodeKind};
    use codestory_store::{FileInfo, FileRole, Store};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn controller_with_empty_store(project: &TempDir) -> AppController {
        let storage_path = project.path().join(".cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("storage parent"))
            .expect("create storage parent");
        drop(Store::open(&storage_path).expect("create store"));
        let controller = AppController::new();
        {
            let mut state = controller.state.lock();
            state.project_root = Some(project.path().to_path_buf());
            state.storage_path = Some(storage_path);
        }
        controller
    }

    fn controller_with_indexed_fixture(project: &TempDir) -> AppController {
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(
            &source_path,
            "pub fn indexed_target() {}\npub fn run_stdio_server() {}\n// textual_target\n",
        )
        .expect("write source");
        let duplicate_path = project.path().join("src").join("duplicate.rs");
        std::fs::write(&duplicate_path, "pub fn indexed_target() {}\n").expect("write duplicate");
        let script_path = project.path().join("scripts").join("entry.cjs");
        std::fs::create_dir_all(script_path.parent().expect("script parent"))
            .expect("create script parent");
        std::fs::write(&script_path, "module.exports = {};\n").expect("write script");

        let storage_path = project.path().join(".cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("storage parent"))
            .expect("create storage parent");
        let mut storage = Store::open(&storage_path).expect("create store");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: PathBuf::from("src/lib.rs"),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 3,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        storage
            .insert_file(&FileInfo {
                id: 10,
                path: PathBuf::from("src/duplicate.rs"),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            })
            .expect("insert duplicate file");
        storage
            .insert_file(&FileInfo {
                id: 20,
                path: script_path.clone(),
                language: "javascript".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            })
            .expect("insert symbol-free script file");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(1),
                    kind: CoreNodeKind::FILE,
                    serialized_name: "src/lib.rs".to_string(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(1),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(2),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "indexed_target".to_string(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(1),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(3),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "textual_target".to_string(),
                    canonical_id: Some("openapi:endpoint:get:/textual".to_string()),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(3),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(4),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "run_stdio_server".to_string(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(2),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(10),
                    kind: CoreNodeKind::FILE,
                    serialized_name: "src/duplicate.rs".to_string(),
                    file_node_id: Some(CoreNodeId(10)),
                    start_line: Some(1),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(11),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "indexed_target".to_string(),
                    file_node_id: Some(CoreNodeId(10)),
                    start_line: Some(1),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(20),
                    kind: CoreNodeKind::FILE,
                    serialized_name: script_path.to_string_lossy().to_string(),
                    file_node_id: Some(CoreNodeId(20)),
                    start_line: Some(1),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        drop(storage);

        let controller = AppController::new();
        {
            let mut state = controller.state.lock();
            state.project_root = Some(project.path().to_path_buf());
            state.storage_path = Some(storage_path);
        }
        controller
    }

    #[test]
    fn legacy_and_tagged_probes_share_one_normalization_path() {
        let tagged = PacketProbeDto::ExactPath {
            path: "assets/desk.svg".into(),
        };
        let legacy_json = serde_json::to_string(&tagged).expect("serialize tagged probe");
        let probes = normalize_packet_probe_request(
            std::slice::from_ref(&tagged),
            &[legacy_json, "WorkspaceIndexer".into()],
        );
        assert_eq!(probes[0], tagged);
        assert_eq!(probes[1], tagged);
        assert_eq!(
            probes[2],
            PacketProbeDto::FreeQuery {
                query: "WorkspaceIndexer".into()
            }
        );
    }

    #[test]
    fn legacy_probe_parser_preserves_exact_path_symbol_and_id_intent() {
        assert_eq!(
            legacy_packet_probe("assets/desk.svg"),
            PacketProbeDto::ExactPath {
                path: "assets/desk.svg".into()
            }
        );
        assert_eq!(
            legacy_packet_probe("src/lib.rs AppController::open"),
            PacketProbeDto::FileSymbol {
                path: "src/lib.rs".into(),
                symbol: "AppController::open".into()
            }
        );
        assert_eq!(
            legacy_packet_probe("-3816661223164617416"),
            PacketProbeDto::SymbolId {
                id: "-3816661223164617416".into()
            }
        );
    }

    #[test]
    fn rejected_and_ambiguous_probes_do_not_become_packet_queries() {
        let rejected = rejected_resolution(
            0,
            PacketProbeDto::ExactPath {
                path: "../outside".into(),
            },
            PacketProbeRejectionCodeDto::OutOfProject,
            "outside",
        );
        let ambiguous = PacketProbeResolutionDto {
            input_index: 1,
            probe: PacketProbeDto::FreeQuery {
                query: "run".into(),
            },
            status: PacketProbeResolutionStatusDto::Ambiguous,
            normalized_query: Some("run".into()),
            path: None,
            symbol_id: None,
            candidates: Vec::new(),
            rejection: None,
        };
        assert!(resolved_packet_probe_queries(&[rejected, ambiguous]).is_empty());
    }

    #[test]
    fn exact_path_resolves_without_broad_retrieval_and_preserves_uncovered_state() {
        let project = TempDir::new().expect("project");
        std::fs::create_dir_all(project.path().join("assets")).expect("assets");
        std::fs::write(project.path().join("assets").join("desk.svg"), "<svg/>\n").expect("asset");
        let controller = controller_with_empty_store(&project);

        let resolutions = resolve_packet_probes(
            &controller,
            vec![
                PacketProbeDto::ExactPath {
                    path: "assets/desk.svg".into(),
                },
                PacketProbeDto::ExactPath {
                    path: "../outside.svg".into(),
                },
            ],
        );
        assert_eq!(
            resolutions[0].status,
            PacketProbeResolutionStatusDto::ValidUncoveredPath
        );
        assert_eq!(resolutions[0].path.as_deref(), Some("assets/desk.svg"));
        assert_eq!(
            resolutions[1]
                .rejection
                .as_ref()
                .map(|rejection| rejection.code),
            Some(PacketProbeRejectionCodeDto::OutOfProject)
        );
        assert!(
            resolved_packet_probe_queries(&resolutions).is_empty(),
            "exact and valid-uncovered paths must not be replaced by broad fuzzy retrieval"
        );
        assert_eq!(
            exact_packet_probe_paths(&resolutions),
            vec!["assets/desk.svg".to_string()],
            "only resolved in-project exact paths should constrain architecture sufficiency"
        );
        let citations = exact_packet_probe_citations(
            &controller,
            &resolutions,
            "Explain this exact asset.",
            true,
        );
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].file_path.as_deref(), Some("assets/desk.svg"));
        assert_eq!(
            citations[0].evidence_producer.as_deref(),
            Some("packet_exact_path_probe")
        );
        assert_eq!(citations[0].eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn indexed_exact_path_keeps_diagnostic_and_adds_distinct_source_carrier() {
        let project = TempDir::new().expect("project");
        let controller = controller_with_indexed_fixture(&project);
        let resolutions = resolve_packet_probes(
            &controller,
            vec![PacketProbeDto::ExactPath {
                path: "src/lib.rs".into(),
            }],
        );

        let citations = exact_packet_probe_citations(
            &controller,
            &resolutions,
            "Explain the stdio server architecture.",
            true,
        );

        assert_eq!(citations.len(), 2);
        assert_eq!(citations[0].file_path.as_deref(), Some("src/lib.rs"));
        assert_eq!(citations[0].eligible_for_sufficiency, Some(false));
        assert_eq!(citations[1].file_path.as_deref(), Some("src/lib.rs"));
        assert_eq!(citations[1].eligible_for_sufficiency, Some(true));
        assert_eq!(
            citations[1].coverage_role.as_deref(),
            Some("command entrypoint")
        );
        assert_eq!(citations[1].display_name, "run_stdio_server");
        assert_ne!(citations[0].node_id, citations[1].node_id);
    }

    #[test]
    fn exact_path_carrier_selection_filters_non_semantic_matches_before_bounding() {
        let project = TempDir::new().expect("project");
        let controller = controller_with_indexed_fixture(&project);
        let storage_path = project.path().join(".cache").join("codestory.db");
        let mut storage = Store::open(&storage_path).expect("open store");
        let decoys = (0..32)
            .map(|index| Node {
                id: CoreNodeId(1_000 + index),
                kind: CoreNodeKind::FUNCTION,
                serialized_name: format!("request_retrieval_publication_{index}"),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(1),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        storage
            .insert_nodes_batch(&decoys)
            .expect("insert lexical decoys");
        drop(storage);
        let resolutions = resolve_packet_probes(
            &controller,
            vec![PacketProbeDto::ExactPath {
                path: "src/lib.rs".into(),
            }],
        );

        let citations = exact_packet_probe_citations(
            &controller,
            &resolutions,
            "Explain the request through stdio retrieval publication.",
            true,
        );

        assert_eq!(citations.len(), 2);
        assert_eq!(citations[1].display_name, "run_stdio_server");
        assert_eq!(
            citations[1].coverage_role.as_deref(),
            Some("command entrypoint")
        );
        assert_eq!(citations[1].eligible_for_sufficiency, Some(true));
    }

    #[test]
    fn indexed_symbol_free_path_remains_diagnostic_only() {
        let project = TempDir::new().expect("project");
        let controller = controller_with_indexed_fixture(&project);
        let resolutions = resolve_packet_probes(
            &controller,
            vec![PacketProbeDto::ExactPath {
                path: "scripts/entry.cjs".into(),
            }],
        );

        let citations = exact_packet_probe_citations(
            &controller,
            &resolutions,
            "Explain the plugin stdio server architecture.",
            true,
        );

        assert_eq!(citations.len(), 1);
        assert_eq!(
            citations[0].node_id.0,
            "packet::exact_path::scripts/entry.cjs"
        );
        assert_eq!(citations[0].kind, NodeKind::FILE);
        assert_eq!(citations[0].file_path.as_deref(), Some("scripts/entry.cjs"));
        assert_eq!(citations[0].eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn indexed_exact_path_does_not_promote_an_unrelated_symbol() {
        let project = TempDir::new().expect("project");
        let controller = controller_with_indexed_fixture(&project);
        let resolutions = resolve_packet_probes(
            &controller,
            vec![PacketProbeDto::ExactPath {
                path: "src/lib.rs".into(),
            }],
        );

        let citations = exact_packet_probe_citations(
            &controller,
            &resolutions,
            "Explain the frobnicator ownership boundary.",
            true,
        );

        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].file_path.as_deref(), Some("src/lib.rs"));
        assert_eq!(citations[0].eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn exact_path_requires_complete_indexed_file_state() {
        for (indexed, complete, expected) in [
            (true, true, PacketProbeResolutionStatusDto::ExactPath),
            (
                true,
                false,
                PacketProbeResolutionStatusDto::ValidUncoveredPath,
            ),
            (
                false,
                true,
                PacketProbeResolutionStatusDto::ValidUncoveredPath,
            ),
            (
                false,
                false,
                PacketProbeResolutionStatusDto::ValidUncoveredPath,
            ),
        ] {
            let project = TempDir::new().expect("project");
            let source_path = project.path().join("src/lib.rs");
            std::fs::create_dir_all(source_path.parent().expect("source parent"))
                .expect("create source parent");
            std::fs::write(&source_path, "pub fn target() {}\n").expect("write source");
            let controller = controller_with_empty_store(&project);
            let storage_path = project.path().join(".cache/codestory.db");
            let storage = Store::open(&storage_path).expect("open store");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: PathBuf::from("src/lib.rs"),
                    language: "rust".to_string(),
                    modification_time: 1,
                    indexed,
                    complete,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert file state");
            drop(storage);

            let resolution = resolve_packet_probes(
                &controller,
                vec![PacketProbeDto::ExactPath {
                    path: "src/lib.rs".into(),
                }],
            )
            .remove(0);

            assert_eq!(
                resolution.status, expected,
                "indexed={indexed} complete={complete}"
            );
        }
    }

    #[test]
    fn indexed_text_missing_malformed_and_stale_targets_remain_distinct() {
        let project = TempDir::new().expect("project");
        let controller = controller_with_indexed_fixture(&project);
        let resolutions = resolve_packet_probes(
            &controller,
            vec![
                PacketProbeDto::ExactPath {
                    path: "src/lib.rs".into(),
                },
                PacketProbeDto::FileSymbol {
                    path: "src/lib.rs".into(),
                    symbol: "indexed_target".into(),
                },
                PacketProbeDto::FileSymbol {
                    path: "src/lib.rs".into(),
                    symbol: "textual_target".into(),
                },
                PacketProbeDto::ExactPath {
                    path: "src/missing.rs".into(),
                },
                PacketProbeDto::FreeQuery {
                    query: "   ".into(),
                },
                PacketProbeDto::SymbolId {
                    id: "999999".into(),
                },
            ],
        );

        assert_eq!(
            resolutions[0].status,
            PacketProbeResolutionStatusDto::ExactPath
        );
        assert_eq!(
            resolutions[1].status,
            PacketProbeResolutionStatusDto::FileScopedSymbol
        );
        assert_eq!(
            resolutions[2].status,
            PacketProbeResolutionStatusDto::TextHit
        );
        assert_eq!(
            resolutions[3]
                .rejection
                .as_ref()
                .map(|rejection| rejection.code),
            Some(PacketProbeRejectionCodeDto::MissingTarget)
        );
        assert_eq!(
            resolutions[4]
                .rejection
                .as_ref()
                .map(|rejection| rejection.code),
            Some(PacketProbeRejectionCodeDto::MalformedProbe)
        );
        assert_eq!(
            resolutions[5]
                .rejection
                .as_ref()
                .map(|rejection| rejection.code),
            Some(PacketProbeRejectionCodeDto::StaleSymbolId)
        );
    }

    #[test]
    fn duplicate_name_symbol_and_continuation_anchors_keep_stable_node_identity() {
        let project = TempDir::new().expect("project");
        let controller = controller_with_indexed_fixture(&project);
        let resolutions = vec![
            PacketProbeResolutionDto {
                input_index: 0,
                probe: PacketProbeDto::SymbolId { id: "2".into() },
                status: PacketProbeResolutionStatusDto::IndexedSymbol,
                normalized_query: Some("indexed_target".into()),
                path: Some("src/lib.rs".into()),
                symbol_id: Some("2".into()),
                candidates: Vec::new(),
                rejection: None,
            },
            PacketProbeResolutionDto {
                input_index: 1,
                probe: PacketProbeDto::Continuation {
                    contract_version: PACKET_PROBE_CONTRACT_VERSION,
                    project_id: "project".into(),
                    core_generation_id: "generation".into(),
                    retrieval_generation: None,
                    symbol_id: Some("11".into()),
                    query: "indexed_target".into(),
                },
                status: PacketProbeResolutionStatusDto::Continuation,
                normalized_query: Some("indexed_target".into()),
                path: Some("src/duplicate.rs".into()),
                symbol_id: Some("11".into()),
                candidates: Vec::new(),
                rejection: None,
            },
        ];

        let citations = exact_packet_probe_citations(
            &controller,
            &resolutions,
            "Find the exact indexed targets.",
            true,
        );
        assert_eq!(
            citations
                .iter()
                .map(|citation| citation.node_id.0.as_str())
                .collect::<Vec<_>>(),
            ["2", "11"]
        );
        assert_eq!(
            citations
                .iter()
                .filter_map(|citation| citation.file_path.as_deref())
                .collect::<Vec<_>>(),
            ["src/lib.rs", "src/duplicate.rs"]
        );
        assert!(
            citations
                .iter()
                .all(|citation| citation.eligible_for_sufficiency == Some(false))
        );
        assert!(
            resolved_packet_probe_queries(&resolutions).is_empty(),
            "stable node identities must not be reduced back to display-name retrieval"
        );
    }

    #[test]
    fn continuation_fails_closed_on_project_and_generation_mismatch() {
        let project = TempDir::new().expect("project");
        let controller = controller_with_empty_store(&project);
        let project_id = project_identity_v3(project.path()).project_id;

        let resolutions = resolve_packet_probes(
            &controller,
            vec![
                PacketProbeDto::Continuation {
                    contract_version: PACKET_PROBE_CONTRACT_VERSION + 1,
                    project_id: project_id.clone(),
                    core_generation_id: "generation".into(),
                    retrieval_generation: None,
                    symbol_id: None,
                    query: "AppController".into(),
                },
                PacketProbeDto::Continuation {
                    contract_version: PACKET_PROBE_CONTRACT_VERSION,
                    project_id: "different-project".into(),
                    core_generation_id: "generation".into(),
                    retrieval_generation: None,
                    symbol_id: None,
                    query: "AppController".into(),
                },
                PacketProbeDto::Continuation {
                    contract_version: PACKET_PROBE_CONTRACT_VERSION,
                    project_id,
                    core_generation_id: "stale-generation".into(),
                    retrieval_generation: None,
                    symbol_id: None,
                    query: "AppController".into(),
                },
            ],
        );
        assert_eq!(
            resolutions[0]
                .rejection
                .as_ref()
                .map(|rejection| rejection.code),
            Some(PacketProbeRejectionCodeDto::IncompatibleContinuation)
        );
        for resolution in &resolutions[1..] {
            assert_eq!(
                resolution
                    .rejection
                    .as_ref()
                    .map(|rejection| rejection.code),
                Some(PacketProbeRejectionCodeDto::StaleContinuation)
            );
        }
    }

    /// Continuation admission moved into `codestory-agent` behind `PinnedReader`,
    /// so the runtime now *renders* a refusal the planning crate decided. This
    /// pins the rendered wire pair — code and message — at the layer a caller
    /// actually receives it, for every refusal a real controller can reach.
    #[test]
    fn continuation_refusals_render_the_same_wire_code_and_message_as_before_extraction() {
        let project = TempDir::new().expect("project");
        let controller = controller_with_empty_store(&project);
        let project_id = project_identity_v3(project.path()).project_id;
        let rootless = AppController::new();

        let continuation = |contract_version: u32,
                            project_id: &str,
                            core_generation_id: &str,
                            retrieval_generation: Option<&str>,
                            query: &str| {
            PacketProbeDto::Continuation {
                contract_version,
                project_id: project_id.to_string(),
                core_generation_id: core_generation_id.to_string(),
                retrieval_generation: retrieval_generation.map(str::to_string),
                symbol_id: None,
                query: query.to_string(),
            }
        };

        let resolutions = resolve_packet_probes(
            &controller,
            vec![
                continuation(
                    PACKET_PROBE_CONTRACT_VERSION + 1,
                    &project_id,
                    "generation",
                    None,
                    "AppController",
                ),
                continuation(
                    PACKET_PROBE_CONTRACT_VERSION,
                    "different-project",
                    "generation",
                    None,
                    "AppController",
                ),
                continuation(
                    PACKET_PROBE_CONTRACT_VERSION,
                    &project_id,
                    "stale-generation",
                    Some("retrieval-generation"),
                    "AppController",
                ),
            ],
        );
        let rootless_resolutions = resolve_packet_probes(
            &rootless,
            vec![continuation(
                PACKET_PROBE_CONTRACT_VERSION,
                &project_id,
                "generation",
                None,
                "AppController",
            )],
        );

        let rendered = |resolution: &PacketProbeResolutionDto| {
            let rejection = resolution
                .rejection
                .as_ref()
                .expect("a refused continuation carries a rejection");
            (rejection.code, rejection.message.clone())
        };

        assert_eq!(
            rendered(&resolutions[0]),
            (
                PacketProbeRejectionCodeDto::IncompatibleContinuation,
                format!(
                    "continuation contract {} is incompatible with {PACKET_PROBE_CONTRACT_VERSION}",
                    PACKET_PROBE_CONTRACT_VERSION + 1
                )
            )
        );
        assert_eq!(
            rendered(&resolutions[1]),
            (
                PacketProbeRejectionCodeDto::StaleContinuation,
                "continuation belongs to a different project".to_string()
            )
        );
        // No core publication is pinned here, so the core check refuses before
        // the retrieval generation this probe also names is ever consulted.
        assert_eq!(
            rendered(&resolutions[2]),
            (
                PacketProbeRejectionCodeDto::StaleContinuation,
                "continuation core generation is no longer selected".to_string()
            )
        );
        assert_eq!(
            rendered(&rootless_resolutions[0]),
            (
                PacketProbeRejectionCodeDto::StaleContinuation,
                "continuation requires an open project".to_string()
            )
        );
        for resolution in resolutions.iter().chain(rootless_resolutions.iter()) {
            assert_eq!(
                resolution.status,
                PacketProbeResolutionStatusDto::Rejected,
                "a refused continuation must not resolve to a reusable probe"
            );
            assert!(
                resolution.normalized_query.is_none() && resolution.symbol_id.is_none(),
                "a refused continuation must not carry a query or symbol forward"
            );
        }
    }
}
