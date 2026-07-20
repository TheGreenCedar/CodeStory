use crate::AppController;
use crate::agent::retrieval_primary::active_pinned_retrieval_publication;
use crate::target_resolution::{TargetResolution, TargetSelection};
use codestory_contracts::api::{
    NodeId, PACKET_PROBE_CONTRACT_VERSION, PacketEvidenceResolutionDto, PacketEvidenceTierDto,
    PacketProbeAmbiguityCandidateDto, PacketProbeDto, PacketProbeRejectionCodeDto,
    PacketProbeRejectionDto, PacketProbeResolutionDto, PacketProbeResolutionStatusDto,
};
use codestory_workspace::{
    ProjectRelativePathResolution, project_identity_v3, resolve_project_relative_path,
    same_workspace_path,
};
use std::path::Path;

const MAX_PACKET_PROBES: usize = 16;
const MAX_PACKET_PROBE_LEN: usize = 240;

pub(crate) fn normalize_packet_probe_request(
    probes: &[PacketProbeDto],
    legacy_probes: &[String],
) -> Vec<PacketProbeDto> {
    probes
        .iter()
        .cloned()
        .chain(legacy_probes.iter().filter_map(|probe| {
            let probe = probe.trim();
            if probe.is_empty() || probe.len() > MAX_PACKET_PROBE_LEN {
                return None;
            }
            serde_json::from_str::<PacketProbeDto>(probe)
                .ok()
                .or_else(|| Some(legacy_packet_probe(probe)))
        }))
        .take(MAX_PACKET_PROBES)
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
            !matches!(
                resolution.status,
                PacketProbeResolutionStatusDto::ExactPath
                    | PacketProbeResolutionStatusDto::ValidUncoveredPath
                    | PacketProbeResolutionStatusDto::Ambiguous
                    | PacketProbeResolutionStatusDto::Rejected
            )
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
    match controller.resolve_target(TargetSelection::Id(NodeId(id.to_string())), None) {
        Ok(TargetResolution::Resolved(resolved)) => {
            let mut resolution = base_resolution(
                input_index,
                probe,
                PacketProbeResolutionStatusDto::IndexedSymbol,
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
    match controller.resolve_target(
        TargetSelection::Query {
            query: symbol.to_string(),
            choose: None,
        },
        Some(&normalized_path),
    ) {
        Ok(TargetResolution::Resolved(resolved)) => {
            let status = if resolved.selected.evidence_tier
                == Some(PacketEvidenceTierDto::StructuralText)
                || resolved.selected.resolution_status
                    == Some(PacketEvidenceResolutionDto::SourceRangeOnly)
                || !resolved.selected.resolvable
            {
                PacketProbeResolutionStatusDto::TextHit
            } else {
                PacketProbeResolutionStatusDto::FileScopedSymbol
            };
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
                        hit.file_path.as_deref() == Some(normalized_path.as_str())
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
    if contract_version != PACKET_PROBE_CONTRACT_VERSION {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::IncompatibleContinuation,
            format!(
                "continuation contract {contract_version} is incompatible with {}",
                PACKET_PROBE_CONTRACT_VERSION
            ),
        );
    }
    let Ok(project_root) = controller.require_project_root() else {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::StaleContinuation,
            "continuation requires an open project",
        );
    };
    if project_id != project_identity_v3(&project_root).project_id {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::StaleContinuation,
            "continuation belongs to a different project",
        );
    }
    if controller
        .active_core_publication()
        .is_none_or(|publication| publication.generation_id != core_generation_id)
    {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::StaleContinuation,
            "continuation core generation is no longer selected",
        );
    }
    if let Some(expected) = retrieval_generation
        && active_pinned_retrieval_publication(controller)
            .is_none_or(|publication| publication.retrieval_generation != expected)
    {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::StaleContinuation,
            "continuation retrieval generation is no longer selected",
        );
    }
    let query = query.trim();
    if query.is_empty() {
        return rejected_resolution(
            input_index,
            probe,
            PacketProbeRejectionCodeDto::MalformedProbe,
            "continuation query must not be empty",
        );
    }
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
            "pub fn indexed_target() {}\n// textual_target\n",
        )
        .expect("write source");

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
                line_count: 2,
                file_role: FileRole::Source,
            })
            .expect("insert file");
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
                    start_line: Some(2),
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
}
