use crate::candidate::{CandidateGraphDirection, CandidateGraphEvidence};
use crate::config::SidecarLayout;
use crate::scip_index::{
    SCIP_GRAPH_PROJECTION_PROVENANCE, SCIP_STUB_MARKER_FILE, ScipAdjacencyDirection,
    ScipIndexMarkerError, ScipNormalizedSymbol, ScipSymbolRecord, load_fresh_scip_query_view,
    parse_scip_index_marker, scip_symbols_component_path,
};
use codestory_contracts::graph::EdgeKind;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[cfg(test)]
use crate::scip_index::load_scip_symbols;

/// Bound graph work while retaining independently ranked anchors from more
/// than one file and retrieval lane.
const SCIP_ADJACENCY_ANCHOR_LIMIT: usize = 8;
const SCIP_ADJACENCY_ANCHORS_PER_FILE: usize = 2;
const SCIP_ADJACENCY_FUSED_WINDOW: usize = 24;

/// Artifact status meaning "the graph lane is ready to serve".
const SCIP_READY_STATUS: &str = "ready";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScipAvailability {
    Ready { revision: String },
    Unavailable { reason: String },
}

#[derive(Debug, Clone)]
pub struct ScipHealthProbe {
    pub availability: ScipAvailability,
    pub artifact_count: u32,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct ScipClient;

impl ScipClient {
    /// `generation` is the retrieval sidecar generation. It names the artifact
    /// directory and must equal the generation stamped inside the artifact.
    pub fn health_probe(layout: &SidecarLayout, generation: &str) -> ScipHealthProbe {
        let project_dir = layout.scip_project_dir(generation);
        if !project_dir.exists() {
            return ScipHealthProbe {
                availability: ScipAvailability::Unavailable {
                    reason: "scip_unavailable".into(),
                },
                artifact_count: 0,
                detail: format!("no artifacts at {}", project_dir.display()),
            };
        }
        let artifacts = count_scip_artifacts(&project_dir);
        if artifacts == 0 {
            return ScipHealthProbe {
                availability: ScipAvailability::Unavailable {
                    reason: "scip_unavailable".into(),
                },
                artifact_count: 0,
                detail: "scip project dir exists but empty (indexers not run)".into(),
            };
        }
        if project_dir.join(SCIP_STUB_MARKER_FILE).is_file() {
            return ScipHealthProbe {
                availability: ScipAvailability::Unavailable {
                    reason: "scip_stub".into(),
                },
                artifact_count: artifacts,
                detail: format!("stub SCIP artifacts only ({SCIP_STUB_MARKER_FILE} present)"),
            };
        }
        let revision = read_scip_revision(&project_dir).unwrap_or_else(|| "stub-v1".into());
        let artifact_status = scip_artifact_status(&project_dir, &revision, generation);
        let is_stub_revision = revision == "stub-v1" || artifact_status == "scip_stub";
        ScipHealthProbe {
            // Only the ready status admits the lane. Every other status — the
            // ones that existed and the ones content validation added — is
            // reported as its own unavailable reason.
            availability: if is_stub_revision {
                ScipAvailability::Unavailable {
                    reason: "scip_stub".into(),
                }
            } else if artifact_status != SCIP_READY_STATUS {
                ScipAvailability::Unavailable {
                    reason: artifact_status.into(),
                }
            } else {
                ScipAvailability::Ready {
                    revision: revision.clone(),
                }
            },
            artifact_count: artifacts,
            detail: format!("{artifacts} artifact(s) under {}", project_dir.display()),
        }
    }

    pub fn anchor_search(
        layout: &SidecarLayout,
        generation: &str,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<super::CandidateHit>> {
        Self::anchor_search_with_cancel(layout, generation, query, limit, &|| false)
    }

    pub fn anchor_search_with_cancel(
        layout: &SidecarLayout,
        generation: &str,
        query: &str,
        limit: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> anyhow::Result<Vec<super::CandidateHit>> {
        if cancelled() {
            anyhow::bail!("SCIP anchor search cancelled");
        }
        let probe = Self::health_probe(layout, generation);
        let ScipAvailability::Ready { revision } = probe.availability else {
            return Ok(Vec::new());
        };
        let project_dir = layout.scip_project_dir(generation);
        let Some(view) = load_fresh_scip_query_view(&project_dir, &revision, generation)? else {
            return Ok(Vec::new());
        };
        let Some(provenance) = view.contract().provenance_label() else {
            return Ok(Vec::new());
        };
        let profile = ScipQueryProfile::new(query);
        let mut hits = Vec::new();
        for (index, (symbol, normalized)) in view.symbols().enumerate() {
            if index % 64 == 0 && cancelled() {
                anyhow::bail!("SCIP anchor search cancelled");
            }
            if symbol_matches_query(normalized, &profile) {
                let score = score_symbol_match(normalized, &profile);
                let mut hit = symbol_to_hit(
                    symbol,
                    score,
                    0,
                    provenance,
                    Some(CandidateGraphEvidence {
                        edge_kind: None,
                        direction: CandidateGraphDirection::Anchor,
                        hop: 0,
                        fanout: 0,
                        edge_weight: 1.0,
                        direction_weight: 1.0,
                    }),
                );
                if symbol_is_exact_query_match(normalized, &profile) {
                    hit.add_provenance("exact");
                    hit.record_lane(crate::candidate::CandidateLane::Graph, score, 0, "exact");
                }
                hits.push(hit);
            }
        }
        if cancelled() {
            anyhow::bail!("SCIP anchor search cancelled");
        }
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.file_path.cmp(&right.file_path))
                .then_with(|| left.symbol_name.cmp(&right.symbol_name))
                .then_with(|| left.start_line.cmp(&right.start_line))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    pub fn expand_reference_adjacency(
        layout: &SidecarLayout,
        generation: &str,
        anchors: &[super::CandidateHit],
        limit: usize,
    ) -> anyhow::Result<Vec<super::CandidateHit>> {
        Self::expand_reference_adjacency_with_cancel(layout, generation, anchors, limit, &|| false)
    }

    /// Expand anchors along validated SCIP reference adjacency.
    ///
    /// Only reference records bound to graph node identity on both ends, whose
    /// endpoints resolve inside the same generation-bound artifact and whose
    /// named symbols agree with those endpoints, produce a neighbour. Symbols
    /// that merely share a file or a name substring produce nothing.
    pub fn expand_reference_adjacency_with_cancel(
        layout: &SidecarLayout,
        generation: &str,
        anchors: &[super::CandidateHit],
        limit: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> anyhow::Result<Vec<super::CandidateHit>> {
        if cancelled() {
            anyhow::bail!("SCIP reference adjacency cancelled");
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let probe = Self::health_probe(layout, generation);
        let ScipAvailability::Ready { revision } = probe.availability else {
            return Ok(Vec::new());
        };
        let project_dir = layout.scip_project_dir(generation);
        let Some(view) = load_fresh_scip_query_view(&project_dir, &revision, generation)? else {
            return Ok(Vec::new());
        };
        let Some(provenance) = view.contract().provenance_label() else {
            return Ok(Vec::new());
        };

        let selected_anchors = selected_adjacency_anchors(anchors);
        let mut anchor_hops: Vec<(&str, u32, f32)> = Vec::new();
        for anchor in selected_anchors {
            let Some(node_id) = anchor.node_id.as_deref() else {
                continue;
            };
            if anchor_hops.iter().any(|(seen, _, _)| *seen == node_id) {
                continue;
            }
            anchor_hops.push((
                node_id,
                anchor.scip_hop_distance.unwrap_or(0),
                adjacency_anchor_relevance(anchor),
            ));
        }
        if anchor_hops.is_empty() {
            return Ok(Vec::new());
        }

        let emitted: HashSet<&str> = anchor_hops.iter().map(|(node_id, _, _)| *node_id).collect();
        let mut expansions = Vec::new();
        let mut fanout_by_anchor = HashMap::<&str, u32>::new();
        for (anchor_position, (anchor_node_id, anchor_hop, anchor_relevance)) in
            anchor_hops.iter().enumerate()
        {
            if anchor_position % 8 == 0 && cancelled() {
                anyhow::bail!("SCIP reference adjacency cancelled");
            }
            if view.symbol_for_node(anchor_node_id).is_none() {
                continue;
            }
            let mut distinct_neighbors = HashSet::new();
            for adjacency in view.adjacency(anchor_node_id) {
                let Some(neighbor) = view.symbol_at(adjacency.neighbor_symbol_index) else {
                    continue;
                };
                let Some(neighbor_node_id) = neighbor.node_id.as_deref() else {
                    continue;
                };
                if emitted.contains(neighbor_node_id) {
                    continue;
                }
                distinct_neighbors.insert(neighbor_node_id);
                let direction = match adjacency.direction {
                    ScipAdjacencyDirection::Outgoing => CandidateGraphDirection::Outgoing,
                    ScipAdjacencyDirection::Incoming => CandidateGraphDirection::Incoming,
                };
                expansions.push((
                    adjacency.proof_ordinal,
                    neighbor_node_id,
                    neighbor,
                    *anchor_node_id,
                    *anchor_hop,
                    *anchor_relevance,
                    direction,
                    adjacency.edge_kind,
                ));
            }
            fanout_by_anchor.insert(
                *anchor_node_id,
                u32::try_from(distinct_neighbors.len()).unwrap_or(u32::MAX),
            );
        }
        expansions.sort_by_key(|(proof_ordinal, ..)| *proof_ordinal);
        let mut hits_by_node = HashMap::<String, super::CandidateHit>::new();
        for (
            position,
            (
                _,
                neighbor_node_id,
                neighbor,
                anchor_node_id,
                anchor_hop,
                anchor_relevance,
                direction,
                edge_kind,
            ),
        ) in expansions.into_iter().enumerate()
        {
            if position % 64 == 0 && cancelled() {
                anyhow::bail!("SCIP reference adjacency cancelled");
            }
            let hop = anchor_hop.saturating_add(1);
            let fanout = fanout_by_anchor
                .get(anchor_node_id)
                .copied()
                .unwrap_or(1)
                .max(1);
            let edge_weight = scip_edge_weight(edge_kind);
            let direction_weight = match direction {
                CandidateGraphDirection::Outgoing => 1.0,
                CandidateGraphDirection::Incoming => 0.9,
                CandidateGraphDirection::Anchor => 1.0,
            };
            let score = (anchor_relevance * edge_weight * direction_weight
                / ((1 + hop) as f32 * (1.0 + fanout as f32).sqrt()))
            .clamp(0.0, 1.0);
            let mut hit = symbol_to_hit(
                neighbor,
                score,
                hop,
                provenance,
                Some(CandidateGraphEvidence {
                    edge_kind: Some(edge_kind),
                    direction,
                    hop,
                    fanout,
                    edge_weight,
                    direction_weight,
                }),
            );
            hit.add_provenance(format!("scip_edge:{edge_kind:?}"));
            hit.add_provenance(format!("scip_direction:{direction:?}"));
            match hits_by_node.entry(neighbor_node_id.to_string()) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if hit.score > entry.get().score
                        || (hit.score == entry.get().score
                            && hit.scip_hop_distance < entry.get().scip_hop_distance)
                    {
                        entry.insert(hit);
                    }
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(hit);
                }
            }
        }
        if cancelled() {
            anyhow::bail!("SCIP reference adjacency cancelled");
        }
        let mut hits = hits_by_node.into_values().collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.file_path.cmp(&right.file_path))
                .then_with(|| left.symbol_name.cmp(&right.symbol_name))
                .then_with(|| left.start_line.cmp(&right.start_line))
        });
        hits.truncate(limit);
        Ok(hits)
    }
}

fn selected_adjacency_anchors(anchors: &[super::CandidateHit]) -> Vec<&super::CandidateHit> {
    let fused_window = anchors
        .iter()
        .take(SCIP_ADJACENCY_FUSED_WINDOW)
        .filter(|anchor| anchor.node_id.is_some())
        .collect::<Vec<_>>();
    let mut selected: Vec<&super::CandidateHit> = Vec::with_capacity(SCIP_ADJACENCY_ANCHOR_LIMIT);
    for exact_pass in [true, false] {
        for anchor in &fused_window {
            let exact = anchor.provenance.iter().any(|label| label == "exact");
            if exact != exact_pass
                || selected.len() == SCIP_ADJACENCY_ANCHOR_LIMIT
                || selected
                    .iter()
                    .filter(|selected_anchor| {
                        same_anchor_file(&selected_anchor.file_path, &anchor.file_path)
                    })
                    .count()
                    >= SCIP_ADJACENCY_ANCHORS_PER_FILE
            {
                continue;
            }
            selected.push(*anchor);
        }
    }
    selected
}

fn same_anchor_file(left: &str, right: &str) -> bool {
    if left == right || codestory_workspace::same_workspace_path(Path::new(left), Path::new(right))
    {
        return true;
    }
    match (
        codestory_workspace::workspace_path_lexical_identity(Path::new(left)),
        codestory_workspace::workspace_path_lexical_identity(Path::new(right)),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn adjacency_anchor_relevance(candidate: &super::CandidateHit) -> f32 {
    candidate.score.clamp(0.0, 1.0)
}

fn symbol_to_hit(
    symbol: &ScipSymbolRecord,
    score: f32,
    hop: u32,
    provenance: &str,
    graph_evidence: Option<CandidateGraphEvidence>,
) -> super::CandidateHit {
    use super::candidate::{CandidateHit, CandidateSource};
    let mut hit = CandidateHit {
        node_id: if provenance == SCIP_GRAPH_PROJECTION_PROVENANCE {
            symbol.node_id.clone()
        } else {
            None
        },
        file_path: symbol.path.clone(),
        symbol_name: Some(symbol.symbol.clone()),
        qualified_name: None,
        structural_kind: None,
        start_line: Some(symbol.start_line),
        target: None,
        source_excerpt: None,
        score,
        lane_scores: Default::default(),
        source: CandidateSource::Scip,
        provenance: vec![provenance.into()],
        file_role: None,
        scip_hop_distance: Some(hop),
        graph_evidence,
        rank_features: None,
    };
    hit.record_lane(
        CandidateSource::Scip.lane(),
        score,
        0,
        if hop == 0 {
            "scip_anchor"
        } else {
            "graph_neighbor"
        },
    );
    hit
}

fn scip_edge_weight(kind: EdgeKind) -> f32 {
    match kind {
        EdgeKind::CALL => 1.0,
        EdgeKind::OVERRIDE => 0.95,
        EdgeKind::INHERITANCE => 0.90,
        EdgeKind::ANNOTATION_USAGE => 0.85,
        EdgeKind::TYPE_USAGE => 0.80,
        EdgeKind::USAGE => 0.75,
        EdgeKind::TYPE_ARGUMENT | EdgeKind::TEMPLATE_SPECIALIZATION => 0.70,
        EdgeKind::MACRO_USAGE => 0.65,
        EdgeKind::IMPORT | EdgeKind::INCLUDE => 0.60,
        EdgeKind::MEMBER | EdgeKind::UNKNOWN => 0.0,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScipQueryProfile {
    query_lower: String,
    tokens: Vec<String>,
    qualified: Option<QualifiedSymbolQuery>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QualifiedSymbolQuery {
    prefix_lower: String,
    terminal_lower: String,
}

impl ScipQueryProfile {
    fn new(query: &str) -> Self {
        let query_lower = query.to_ascii_lowercase();
        let tokens = query_lower
            .split_whitespace()
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .collect();
        Self {
            query_lower,
            tokens,
            qualified: qualified_symbol_query(query),
        }
    }
}

fn qualified_symbol_query(query: &str) -> Option<QualifiedSymbolQuery> {
    let trimmed = query.trim();
    let index = trimmed.rfind("::")?;
    let prefix = trimmed[..index].trim();
    let terminal = trimmed[index + 2..].trim();
    if prefix.is_empty()
        || terminal.is_empty()
        || terminal
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'))
    {
        return None;
    }
    Some(QualifiedSymbolQuery {
        prefix_lower: prefix.to_ascii_lowercase(),
        terminal_lower: terminal.to_ascii_lowercase(),
    })
}

fn symbol_matches_query(symbol: &ScipNormalizedSymbol, profile: &ScipQueryProfile) -> bool {
    if profile.tokens.is_empty() {
        return symbol.symbol_lower.contains(&profile.query_lower)
            || symbol.path_lower.contains(&profile.query_lower);
    }
    if profile
        .tokens
        .iter()
        .all(|token| symbol.symbol_lower.contains(token) || symbol.path_lower.contains(token))
    {
        return true;
    }
    let Some(qualified) = profile.qualified.as_ref() else {
        return false;
    };
    symbol.terminal_lower == qualified.terminal_lower
        && qualified_prefix_path_score(&qualified.prefix_lower, symbol) > 0
}

fn symbol_is_exact_query_match(symbol: &ScipNormalizedSymbol, profile: &ScipQueryProfile) -> bool {
    if symbol.symbol_lower == profile.query_lower
        || (profile.tokens.len() == 1
            && !profile.query_lower.contains('/')
            && !profile.query_lower.contains('\\')
            && symbol.terminal_lower == profile.query_lower)
    {
        return true;
    }
    profile.qualified.as_ref().is_some_and(|qualified| {
        symbol.terminal_lower == qualified.terminal_lower
            && qualified_prefix_path_score(&qualified.prefix_lower, symbol) > 0
    })
}

fn score_symbol_match(symbol: &ScipNormalizedSymbol, profile: &ScipQueryProfile) -> f32 {
    let mut score = 0.70_f32;
    if symbol.symbol_lower == profile.query_lower {
        score += 0.22;
    } else if symbol.symbol_lower.contains(&profile.query_lower) {
        score += 0.14;
    }
    if symbol.path_lower == profile.query_lower {
        score += 0.08;
    } else if symbol.path_lower.contains(&profile.query_lower) {
        score += 0.04;
    }
    for token in &profile.tokens {
        if symbol.symbol_lower == *token {
            score += 0.05;
        } else if symbol.symbol_lower.contains(token) {
            score += 0.03;
        }
        if symbol.path_lower.contains(token) {
            score += 0.01;
        }
    }
    if let Some(qualified) = profile.qualified.as_ref() {
        let prefix_path_score = qualified_prefix_path_score(&qualified.prefix_lower, symbol);
        if symbol.terminal_lower == qualified.terminal_lower {
            score += 0.18;
        }
        score += match prefix_path_score {
            3 => 0.12,
            2 => 0.09,
            1 => 0.05,
            _ => 0.0,
        };
        if symbol.terminal_lower == qualified.terminal_lower
            && symbol.file_stem_lower.as_deref() == Some(qualified.terminal_lower.as_str())
        {
            score += 0.16;
        }
        if symbol.symbol_lower == profile.query_lower
            && symbol.file_stem_lower.as_deref() != Some(qualified.terminal_lower.as_str())
        {
            score -= 0.12;
        }
    }
    score.min(1.20)
}

fn qualified_prefix_path_score(prefix_lower: &str, symbol: &ScipNormalizedSymbol) -> u8 {
    if symbol.path_segments_lower.is_empty() {
        return 0;
    }

    let hyphenated_prefix = prefix_lower.replace('_', "-");
    if !hyphenated_prefix.is_empty()
        && symbol
            .path_segments_lower
            .iter()
            .any(|segment| segment == &hyphenated_prefix)
    {
        return 3;
    }

    let trailing_prefix_segment = prefix_lower
        .rsplit('_')
        .next()
        .unwrap_or(prefix_lower)
        .replace('_', "-");
    if trailing_prefix_segment.len() >= 3
        && symbol
            .path_segments_lower
            .iter()
            .any(|segment| segment == &trailing_prefix_segment)
    {
        return 2;
    }

    let compact_prefix = compact_alphanumeric(prefix_lower);
    if compact_prefix.len() >= 3
        && symbol
            .path_segments_compact
            .iter()
            .any(|segment| segment == &compact_prefix)
    {
        return 1;
    }

    0
}

fn compact_alphanumeric(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn count_scip_artifacts(dir: &Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .count() as u32
}

fn read_scip_revision(dir: &Path) -> Option<String> {
    let revision_path = dir.join("revision.txt");
    std::fs::read_to_string(revision_path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn scip_artifact_status(project_dir: &Path, revision: &str, generation: &str) -> &'static str {
    if !scip_symbols_component_path(project_dir).is_file()
        || !project_dir.join("revision.txt").is_file()
        || project_dir.join(SCIP_STUB_MARKER_FILE).is_file()
    {
        return "scip_stub";
    }
    // A present `index.scip` is not evidence until it parses and names this
    // revision. Each defect reports its own typed code so the generation falls
    // through to a rebuild carrying why, instead of publishing as a healthy
    // graph lane. Absence keeps its existing stub reason.
    if let Err(error) = parse_scip_index_marker(project_dir, revision) {
        return match error {
            ScipIndexMarkerError::Missing => "scip_stub",
            damaged => damaged.code(),
        };
    }
    load_fresh_scip_query_view(project_dir, revision, generation)
        .ok()
        .flatten()
        .map_or("scip_stale", |view| {
            if view.contract().evidence_source == SCIP_GRAPH_PROJECTION_PROVENANCE {
                SCIP_READY_STATUS
            } else {
                "scip_imported_diagnostic_only"
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::{CandidateHit, CandidateSource};
    use crate::scip_index::{
        SCIP_DEFINITION_ROLE, SCIP_IMPORTED_PROOF_PROVENANCE, SCIP_INDEX_FILE,
        SCIP_PRECISE_SEMANTIC_IMPORT_PUBLIC_PROVENANCE, SCIP_REFERENCE_ROLE, ScipPackageIdentity,
        ScipProofAdapterContract, ScipProofRecord, ScipSymbolsIndex,
        emit_scip_artifacts_from_store, write_scip_index_marker,
    };
    use codestory_contracts::graph::{
        Edge, EdgeId, EdgeKind, Node, NodeId, NodeKind, ResolutionCertainty,
    };
    use codestory_store::{FileInfo, FileRole, Store};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use tempfile::TempDir;

    fn write_scip_index(
        project_dir: &Path,
        generation: &str,
        revision: &str,
        contract: ScipProofAdapterContract,
        symbols: Vec<ScipSymbolRecord>,
        proofs: Vec<ScipProofRecord>,
    ) {
        let index = ScipSymbolsIndex {
            generation: generation.to_string(),
            revision: revision.to_string(),
            contract,
            symbols,
            proofs,
        };
        std::fs::write(
            scip_symbols_component_path(project_dir),
            serde_json::to_string_pretty(&index).expect("serialize"),
        )
        .expect("write symbols");
        std::fs::write(project_dir.join("revision.txt"), format!("{revision}\n"))
            .expect("revision");
        write_scip_index_marker(project_dir, revision).expect("index marker");
    }

    fn graph_definition_proofs(symbols: &[ScipSymbolRecord]) -> Vec<ScipProofRecord> {
        symbols
            .iter()
            .map(|symbol| ScipProofRecord {
                role: SCIP_DEFINITION_ROLE.into(),
                path: symbol.path.clone(),
                symbol: symbol.symbol.clone(),
                start_line: symbol.start_line,
                start_character_utf16: 0,
                end_line: symbol.end_line,
                end_character_utf16: 0,
                target_symbol: None,
                node_id: symbol.node_id.clone(),
                target_node_id: None,
                edge_kind: None,
            })
            .collect()
    }

    fn graph_reference_proof(
        source: &ScipSymbolRecord,
        target: &ScipSymbolRecord,
    ) -> ScipProofRecord {
        ScipProofRecord {
            role: SCIP_REFERENCE_ROLE.into(),
            path: source.path.clone(),
            symbol: source.symbol.clone(),
            start_line: source.start_line,
            start_character_utf16: 0,
            end_line: source.end_line,
            end_character_utf16: 0,
            target_symbol: Some(target.symbol.clone()),
            node_id: source.node_id.clone(),
            target_node_id: target.node_id.clone(),
            edge_kind: Some(codestory_contracts::graph::EdgeKind::CALL),
        }
    }

    fn imported_contract(revision: &str) -> ScipProofAdapterContract {
        ScipProofAdapterContract {
            evidence_source: SCIP_IMPORTED_PROOF_PROVENANCE.into(),
            producer: "scip-fixture".into(),
            producer_version: "0.1.0".into(),
            producer_args: vec!["scip".into(), "index".into(), "--cwd=.".into()],
            producer_config: "fixture-config-v1".into(),
            revision: revision.into(),
            package: ScipPackageIdentity {
                manager: "cargo".into(),
                name: "fixture_package".into(),
                version: Some("1.2.3".into()),
            },
            position_encoding: "line_one_based_utf16_column_zero_based".into(),
            freshness: "fresh".into(),
        }
    }

    fn imported_symbol() -> ScipSymbolRecord {
        ScipSymbolRecord {
            node_id: Some("forged-graph-node".into()),
            path: "src/lib.rs".into(),
            symbol: "fixture_package::run".into(),
            start_line: 3,
            end_line: 3,
        }
    }

    fn valid_imported_proofs() -> Vec<ScipProofRecord> {
        vec![
            ScipProofRecord {
                role: SCIP_DEFINITION_ROLE.into(),
                path: "src/lib.rs".into(),
                symbol: "fixture_package::run".into(),
                start_line: 3,
                start_character_utf16: 4,
                end_line: 3,
                end_character_utf16: 7,
                target_symbol: None,
                node_id: None,
                target_node_id: None,
                edge_kind: None,
            },
            ScipProofRecord {
                role: SCIP_REFERENCE_ROLE.into(),
                path: "src/main.rs".into(),
                symbol: "fixture_package::main".into(),
                start_line: 8,
                start_character_utf16: 9,
                end_line: 8,
                end_character_utf16: 12,
                target_symbol: Some("fixture_package::run".into()),
                node_id: None,
                target_node_id: None,
                edge_kind: None,
            },
        ]
    }

    #[test]
    fn anchor_search_scores_all_matches_before_truncating() {
        let root = TempDir::new().expect("root");
        let layout = SidecarLayout {
            lexical_data_dir: root.path().join("lexical"),
            semantic_data_dir: root.path().join("semantic"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("state.json"),
        };
        let project_id = "project";
        let project_dir = layout.scip_project_dir(project_id);
        std::fs::create_dir_all(&project_dir).expect("scip dir");

        let mut symbols = Vec::new();
        for index in 0..12 {
            symbols.push(ScipSymbolRecord {
                node_id: None,
                path: format!("src/needle/noise_{index}.ts"),
                symbol: format!("noise_{index}"),
                start_line: index + 1,
                end_line: index + 1,
            });
        }
        symbols.push(ScipSymbolRecord {
            node_id: None,
            path: "src/needle/target.ts".to_string(),
            symbol: "needle".to_string(),
            start_line: 99,
            end_line: 99,
        });
        let proofs = graph_definition_proofs(&symbols);
        write_scip_index(
            &project_dir,
            project_id,
            "graph-test",
            ScipProofAdapterContract::graph_projection("graph-test"),
            symbols,
            proofs,
        );

        let hits = ScipClient::anchor_search(&layout, project_id, "needle", 8).expect("search");

        assert!(
            hits.iter()
                .any(|hit| hit.file_path == "src/needle/target.ts"),
            "exact SCIP symbol match should survive top-k truncation even when many earlier path-only matches exist"
        );
        assert_eq!(hits[0].file_path, "src/needle/target.ts");
    }

    #[test]
    fn anchor_search_polls_cancellation_while_scanning_symbols() {
        let root = TempDir::new().expect("root");
        let layout = SidecarLayout {
            lexical_data_dir: root.path().join("lexical"),
            semantic_data_dir: root.path().join("semantic"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("state.json"),
        };
        let project_dir = layout.scip_project_dir("project");
        std::fs::create_dir_all(&project_dir).expect("scip dir");
        let symbols = (0..256)
            .map(|index| ScipSymbolRecord {
                node_id: None,
                path: format!("src/{index}.rs"),
                symbol: format!("symbol_{index}"),
                start_line: index + 1,
                end_line: index + 1,
            })
            .collect::<Vec<_>>();
        let proofs = graph_definition_proofs(&symbols);
        write_scip_index(
            &project_dir,
            "project",
            "graph-test",
            ScipProofAdapterContract::graph_projection("graph-test"),
            symbols,
            proofs,
        );
        let polls = AtomicUsize::new(0);

        let error = ScipClient::anchor_search_with_cancel(&layout, "project", "symbol", 8, &|| {
            polls.fetch_add(1, AtomicOrdering::Relaxed) > 0
        })
        .expect_err("scan should observe cancellation");

        assert!(error.to_string().contains("cancelled"));
        assert!(polls.load(AtomicOrdering::Relaxed) >= 2);
    }

    #[test]
    fn qualified_anchor_search_admits_crate_matching_terminal_definition() {
        let root = TempDir::new().expect("root");
        let layout = SidecarLayout {
            lexical_data_dir: root.path().join("lexical"),
            semantic_data_dir: root.path().join("semantic"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("state.json"),
        };
        let project_id = "project";
        let project_dir = layout.scip_project_dir(project_id);
        std::fs::create_dir_all(&project_dir).expect("scip dir");

        let symbols = vec![
            ScipSymbolRecord {
                node_id: None,
                path: "workspace/app/src/main.rs".to_string(),
                symbol: "workspace_app::Cli".to_string(),
                start_line: 15,
                end_line: 15,
            },
            ScipSymbolRecord {
                node_id: None,
                path: "workspace/tools/src/cli.rs".to_string(),
                symbol: "Cli".to_string(),
                start_line: 1,
                end_line: 1,
            },
            ScipSymbolRecord {
                node_id: None,
                path: "workspace/app/src/cli.rs".to_string(),
                symbol: "Cli".to_string(),
                start_line: 42,
                end_line: 42,
            },
        ];
        let proofs = graph_definition_proofs(&symbols);
        write_scip_index(
            &project_dir,
            project_id,
            "graph-test",
            ScipProofAdapterContract::graph_projection("graph-test"),
            symbols,
            proofs,
        );

        let hits = ScipClient::anchor_search(&layout, project_id, "workspace_app::Cli", 8)
            .expect("search");

        assert_eq!(
            hits.first().map(|hit| hit.file_path.as_str()),
            Some("workspace/app/src/cli.rs"),
            "crate-qualified terminal definition should outrank import aliases and unrelated Cli definitions: {hits:#?}"
        );
        assert!(
            hits.iter()
                .all(|hit| hit.file_path != "workspace/tools/src/cli.rs"),
            "qualified terminal expansion should require a matching prefix path: {hits:#?}"
        );
    }

    #[test]
    fn health_rejects_marker_without_symbol_index() {
        let root = TempDir::new().expect("root");
        let layout = SidecarLayout {
            lexical_data_dir: root.path().join("lexical"),
            semantic_data_dir: root.path().join("semantic"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("state.json"),
        };
        let project_id = "project";
        let project_dir = layout.scip_project_dir(project_id);
        std::fs::create_dir_all(&project_dir).expect("scip dir");
        std::fs::write(project_dir.join("revision.txt"), "graph-test\n").expect("revision");
        write_scip_index_marker(&project_dir, "graph-test").expect("index marker");

        let probe = ScipClient::health_probe(&layout, project_id);

        assert_eq!(
            probe.availability,
            ScipAvailability::Unavailable {
                reason: "scip_stub".into()
            }
        );
    }

    #[test]
    fn imported_proof_contract_is_diagnostic_not_graph_health() {
        let root = TempDir::new().expect("root");
        let layout = SidecarLayout {
            lexical_data_dir: root.path().join("lexical"),
            semantic_data_dir: root.path().join("semantic"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("state.json"),
        };
        let project_id = "project";
        let project_dir = layout.scip_project_dir(project_id);
        std::fs::create_dir_all(&project_dir).expect("scip dir");
        let revision = "imported-a";
        write_scip_index(
            &project_dir,
            project_id,
            revision,
            imported_contract(revision),
            vec![imported_symbol()],
            valid_imported_proofs(),
        );

        let loaded = load_scip_symbols(&project_dir)
            .expect("load")
            .expect("index");
        assert_eq!(loaded.contract.producer, "scip-fixture");
        assert_eq!(loaded.contract.producer_version, "0.1.0");
        assert_eq!(loaded.contract.producer_args, ["scip", "index", "--cwd=."]);
        assert_eq!(loaded.contract.producer_config, "fixture-config-v1");
        assert_eq!(loaded.contract.revision, revision);
        assert_eq!(loaded.contract.package.manager, "cargo");
        assert_eq!(loaded.contract.package.name, "fixture_package");
        assert_eq!(loaded.contract.package.version.as_deref(), Some("1.2.3"));
        assert_eq!(
            loaded.contract.position_encoding,
            "line_one_based_utf16_column_zero_based"
        );
        assert_eq!(loaded.contract.freshness, "fresh");
        assert_eq!(loaded.proofs.len(), 2);
        let hit = symbol_to_hit(
            &loaded.symbols[0],
            1.0,
            0,
            loaded.contract.provenance_label().expect("provenance"),
            None,
        );
        assert_eq!(hit.node_id, None);

        assert_eq!(
            loaded.contract.provenance_label(),
            Some(SCIP_PRECISE_SEMANTIC_IMPORT_PUBLIC_PROVENANCE)
        );
        let probe = ScipClient::health_probe(&layout, project_id);
        assert_eq!(
            probe.availability,
            ScipAvailability::Unavailable {
                reason: "scip_imported_diagnostic_only".into()
            }
        );
        let hits = ScipClient::anchor_search(&layout, project_id, "fixture_package::run", 4)
            .expect("search");
        assert!(hits.is_empty());
    }

    #[test]
    fn imported_contract_without_proofs_fails_closed() {
        let root = TempDir::new().expect("root");
        let layout = SidecarLayout {
            lexical_data_dir: root.path().join("lexical"),
            semantic_data_dir: root.path().join("semantic"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("state.json"),
        };
        let project_id = "project";
        let project_dir = layout.scip_project_dir(project_id);
        std::fs::create_dir_all(&project_dir).expect("scip dir");
        let revision = "imported-no-proofs";
        write_scip_index(
            &project_dir,
            project_id,
            revision,
            imported_contract(revision),
            vec![imported_symbol()],
            Vec::new(),
        );

        let probe = ScipClient::health_probe(&layout, project_id);
        assert_eq!(
            probe.availability,
            ScipAvailability::Unavailable {
                reason: "scip_stale".into()
            }
        );
        let hits = ScipClient::anchor_search(&layout, project_id, "fixture_package::run", 4)
            .expect("search");
        assert!(hits.is_empty());
    }

    #[test]
    fn unknown_evidence_source_fails_closed() {
        let root = TempDir::new().expect("root");
        let layout = SidecarLayout {
            lexical_data_dir: root.path().join("lexical"),
            semantic_data_dir: root.path().join("semantic"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("state.json"),
        };
        let project_id = "project";
        let project_dir = layout.scip_project_dir(project_id);
        std::fs::create_dir_all(&project_dir).expect("scip dir");
        let revision = "imported-unknown-source";
        let mut contract = imported_contract(revision);
        contract.evidence_source = "imported-scip-proof".into();
        write_scip_index(
            &project_dir,
            project_id,
            revision,
            contract,
            vec![imported_symbol()],
            valid_imported_proofs(),
        );

        let probe = ScipClient::health_probe(&layout, project_id);
        assert_eq!(
            probe.availability,
            ScipAvailability::Unavailable {
                reason: "scip_stale".into()
            }
        );
        let hits = ScipClient::anchor_search(&layout, project_id, "fixture_package::run", 4)
            .expect("search");
        assert!(hits.is_empty());
    }

    #[test]
    fn stale_scip_import_fails_closed_without_candidates() {
        let root = TempDir::new().expect("root");
        let layout = SidecarLayout {
            lexical_data_dir: root.path().join("lexical"),
            semantic_data_dir: root.path().join("semantic"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("state.json"),
        };
        let project_id = "project";
        let project_dir = layout.scip_project_dir(project_id);
        std::fs::create_dir_all(&project_dir).expect("scip dir");
        let mut contract = imported_contract("old-import");
        contract.freshness = "stale".into();
        write_scip_index(
            &project_dir,
            project_id,
            "current-import",
            contract,
            vec![imported_symbol()],
            valid_imported_proofs(),
        );

        let probe = ScipClient::health_probe(&layout, project_id);
        assert_eq!(
            probe.availability,
            ScipAvailability::Unavailable {
                reason: "scip_stale".into()
            }
        );
        let hits = ScipClient::anchor_search(&layout, project_id, "fixture_package::run", 4)
            .expect("search");
        assert!(hits.is_empty());
    }

    fn adjacency_layout(root: &TempDir) -> SidecarLayout {
        SidecarLayout {
            lexical_data_dir: root.path().join("lexical"),
            semantic_data_dir: root.path().join("semantic"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("state.json"),
        }
    }

    /// `Client` (node 2) really references `parse_client` (node 4).
    /// `ClientConfig` (node 3) shares the file and a name substring with the
    /// anchor and has no edge to anything.
    fn adjacency_symbols() -> Vec<ScipSymbolRecord> {
        vec![
            ScipSymbolRecord {
                node_id: Some("2".into()),
                path: "src/client.rs".into(),
                symbol: "Client".into(),
                start_line: 10,
                end_line: 15,
            },
            ScipSymbolRecord {
                node_id: Some("3".into()),
                path: "src/client.rs".into(),
                symbol: "ClientConfig".into(),
                start_line: 30,
                end_line: 35,
            },
            ScipSymbolRecord {
                node_id: Some("4".into()),
                path: "src/client.rs".into(),
                symbol: "parse_client".into(),
                start_line: 50,
                end_line: 55,
            },
        ]
    }

    fn client_anchor() -> CandidateHit {
        let mut anchor = CandidateHit::with_source(
            "src/client.rs",
            Some("Client".into()),
            0.9,
            CandidateSource::Lexical,
        );
        anchor.node_id = Some("2".into());
        anchor
    }

    #[test]
    fn adjacency_anchor_selection_preserves_fused_order_and_file_breadth() {
        let mut anchors = (0..10)
            .map(|index| {
                let path = if index < 3 {
                    "src/shared.rs".to_string()
                } else {
                    format!("src/file_{index}.rs")
                };
                let mut anchor = CandidateHit::with_source(
                    path,
                    Some(format!("symbol_{index}")),
                    1.0 - index as f32 / 20.0,
                    CandidateSource::Lexical,
                );
                anchor.node_id = Some(format!("node_{index}"));
                anchor
            })
            .collect::<Vec<_>>();
        anchors[9].add_provenance("exact");

        let selected = selected_adjacency_anchors(&anchors);
        let node_ids = selected
            .iter()
            .filter_map(|anchor| anchor.node_id.as_deref())
            .collect::<Vec<_>>();

        assert_eq!(node_ids[0], "node_9", "exact anchor remains mandatory");
        assert_eq!(
            node_ids[1..],
            [
                "node_0", "node_1", "node_3", "node_4", "node_5", "node_6", "node_7"
            ]
        );
        assert!(
            !node_ids.contains(&"node_2"),
            "third same-file anchor is dropped"
        );
    }

    fn write_adjacency_artifact(
        layout: &SidecarLayout,
        generation: &str,
        artifact_generation: &str,
        references: Vec<ScipProofRecord>,
    ) {
        let project_dir = layout.scip_project_dir(generation);
        std::fs::create_dir_all(&project_dir).expect("scip dir");
        let symbols = adjacency_symbols();
        let mut proofs = graph_definition_proofs(&symbols);
        proofs.extend(references);
        write_scip_index(
            &project_dir,
            artifact_generation,
            "graph-adjacency",
            ScipProofAdapterContract::graph_projection("graph-adjacency"),
            symbols,
            proofs,
        );
    }

    #[test]
    fn stage_two_expands_validated_reference_adjacency_and_not_same_file_name_affinity() {
        let root = TempDir::new().expect("root");
        let layout = adjacency_layout(&root);
        let symbols = adjacency_symbols();
        write_adjacency_artifact(
            &layout,
            "generation-a",
            "generation-a",
            vec![graph_reference_proof(&symbols[0], &symbols[2])],
        );

        let hits =
            ScipClient::expand_reference_adjacency(&layout, "generation-a", &[client_anchor()], 8)
                .expect("expand");

        assert_eq!(
            hits.iter()
                .map(|hit| hit.symbol_name.clone().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["parse_client".to_string()],
            "only the symbol reached by a validated reference is a neighbour: {hits:#?}"
        );
        assert_eq!(hits[0].node_id.as_deref(), Some("4"));
        assert_eq!(hits[0].scip_hop_distance, Some(1));
        assert_eq!(
            hits[0].graph_evidence.as_ref().map(|evidence| (
                evidence.edge_kind,
                evidence.direction,
                evidence.fanout
            )),
            Some((Some(EdgeKind::CALL), CandidateGraphDirection::Outgoing, 1))
        );
    }

    #[test]
    fn stage_two_order_is_independent_of_serialized_proof_order() {
        let root = TempDir::new().expect("root");
        let layout = adjacency_layout(&root);
        let symbols = adjacency_symbols();
        let first = graph_reference_proof(&symbols[0], &symbols[1]);
        let second = graph_reference_proof(&symbols[0], &symbols[2]);
        write_adjacency_artifact(
            &layout,
            "generation-a",
            "generation-a",
            vec![first.clone(), second.clone()],
        );
        write_adjacency_artifact(&layout, "generation-b", "generation-b", vec![second, first]);

        let first_order =
            ScipClient::expand_reference_adjacency(&layout, "generation-a", &[client_anchor()], 1)
                .expect("first order");
        let reverse_order =
            ScipClient::expand_reference_adjacency(&layout, "generation-b", &[client_anchor()], 1)
                .expect("reverse order");

        assert_eq!(
            first_order
                .iter()
                .map(|hit| (&hit.file_path, &hit.symbol_name, hit.score))
                .collect::<Vec<_>>(),
            reverse_order
                .iter()
                .map(|hit| (&hit.file_path, &hit.symbol_name, hit.score))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn stage_two_penalizes_high_fanout_before_truncation() {
        let root = TempDir::new().expect("root");
        let layout = adjacency_layout(&root);
        let symbols = adjacency_symbols();
        let target = graph_reference_proof(&symbols[0], &symbols[2]);
        write_adjacency_artifact(
            &layout,
            "generation-a",
            "generation-a",
            vec![target.clone()],
        );
        write_adjacency_artifact(
            &layout,
            "generation-b",
            "generation-b",
            vec![target, graph_reference_proof(&symbols[0], &symbols[1])],
        );

        let narrow =
            ScipClient::expand_reference_adjacency(&layout, "generation-a", &[client_anchor()], 8)
                .expect("narrow fanout");
        let wide =
            ScipClient::expand_reference_adjacency(&layout, "generation-b", &[client_anchor()], 8)
                .expect("wide fanout");
        let narrow_target = narrow
            .iter()
            .find(|hit| hit.node_id.as_deref() == Some("4"))
            .expect("narrow target");
        let wide_target = wide
            .iter()
            .find(|hit| hit.node_id.as_deref() == Some("4"))
            .expect("wide target");

        assert!(wide_target.score < narrow_target.score);
        assert_eq!(wide_target.graph_evidence.as_ref().unwrap().fanout, 2);
    }

    #[test]
    fn stage_two_fanout_counts_distinct_eligible_neighbors() {
        let root = TempDir::new().expect("root");
        let layout = adjacency_layout(&root);
        let symbols = adjacency_symbols();
        let duplicate = graph_reference_proof(&symbols[0], &symbols[2]);
        write_adjacency_artifact(
            &layout,
            "generation-a",
            "generation-a",
            vec![duplicate.clone(), duplicate],
        );

        let hits =
            ScipClient::expand_reference_adjacency(&layout, "generation-a", &[client_anchor()], 8)
                .expect("expand");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].node_id.as_deref(), Some("4"));
        assert_eq!(hits[0].graph_evidence.as_ref().unwrap().fanout, 1);
    }

    #[test]
    fn stage_two_yields_nothing_when_the_only_same_file_pair_has_no_reference() {
        let root = TempDir::new().expect("root");
        let layout = adjacency_layout(&root);
        write_adjacency_artifact(&layout, "generation-a", "generation-a", Vec::new());

        let hits =
            ScipClient::expand_reference_adjacency(&layout, "generation-a", &[client_anchor()], 8)
                .expect("expand");

        assert!(
            hits.is_empty(),
            "`Client`/`ClientConfig` share a file and a name substring but no edge: {hits:#?}"
        );
    }

    #[test]
    fn stage_two_refuses_a_cross_generation_artifact() {
        let root = TempDir::new().expect("root");
        let layout = adjacency_layout(&root);
        let symbols = adjacency_symbols();
        write_adjacency_artifact(
            &layout,
            "generation-a",
            "generation-b",
            vec![graph_reference_proof(&symbols[0], &symbols[2])],
        );

        let probe = ScipClient::health_probe(&layout, "generation-a");
        let hits =
            ScipClient::expand_reference_adjacency(&layout, "generation-a", &[client_anchor()], 8)
                .expect("expand");

        assert_eq!(
            probe.availability,
            ScipAvailability::Unavailable {
                reason: "scip_stale".into()
            },
            "an artifact stamped with another generation is not admissible evidence"
        );
        assert!(hits.is_empty(), "{hits:#?}");
    }

    #[test]
    fn stage_two_refuses_a_reference_record_that_disagrees_with_its_node_identity() {
        let root = TempDir::new().expect("root");
        let layout = adjacency_layout(&root);
        let symbols = adjacency_symbols();
        let mut forged = graph_reference_proof(&symbols[0], &symbols[2]);
        forged.target_symbol = Some("ClientConfig".into());
        write_adjacency_artifact(&layout, "generation-a", "generation-a", vec![forged]);

        let hits =
            ScipClient::expand_reference_adjacency(&layout, "generation-a", &[client_anchor()], 8)
                .expect("expand");

        assert!(
            hits.is_empty(),
            "a reference naming a symbol its target node does not carry is refused: {hits:#?}"
        );
    }

    #[test]
    fn stage_two_refuses_a_reference_record_without_graph_node_identity() {
        let root = TempDir::new().expect("root");
        let layout = adjacency_layout(&root);
        let symbols = adjacency_symbols();
        let mut unbound = graph_reference_proof(&symbols[0], &symbols[2]);
        unbound.target_node_id = None;
        write_adjacency_artifact(&layout, "generation-a", "generation-a", vec![unbound]);

        let hits =
            ScipClient::expand_reference_adjacency(&layout, "generation-a", &[client_anchor()], 8)
                .expect("expand");

        assert!(
            hits.is_empty(),
            "graph-lane adjacency must be bound to node identity on both ends: {hits:#?}"
        );
    }

    #[test]
    fn stage_two_expands_incoming_references_and_polls_cancellation() {
        let root = TempDir::new().expect("root");
        let layout = adjacency_layout(&root);
        let symbols = adjacency_symbols();
        write_adjacency_artifact(
            &layout,
            "generation-a",
            "generation-a",
            vec![graph_reference_proof(&symbols[2], &symbols[0])],
        );

        let hits =
            ScipClient::expand_reference_adjacency(&layout, "generation-a", &[client_anchor()], 8)
                .expect("expand");
        assert_eq!(
            hits.iter()
                .map(|hit| hit.symbol_name.clone().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["parse_client".to_string()],
            "a symbol that references the anchor is adjacent to it: {hits:#?}"
        );
        assert_eq!(
            hits[0]
                .graph_evidence
                .as_ref()
                .map(|evidence| evidence.direction),
            Some(CandidateGraphDirection::Incoming)
        );

        let polls = AtomicUsize::new(0);
        let error = ScipClient::expand_reference_adjacency_with_cancel(
            &layout,
            "generation-a",
            &[client_anchor()],
            8,
            &|| polls.fetch_add(1, AtomicOrdering::Relaxed) > 0,
        )
        .expect_err("adjacency scan should observe cancellation");
        assert!(error.to_string().contains("cancelled"), "{error}");
    }

    fn graph_projection_fixture(root: &TempDir, revision: &str) -> (SidecarLayout, PathBuf) {
        let layout = adjacency_layout(root);
        let project_dir = layout.scip_project_dir("project");
        std::fs::create_dir_all(&project_dir).expect("scip dir");
        let symbols = vec![ScipSymbolRecord {
            node_id: Some("1".into()),
            path: "src/lib.rs".into(),
            symbol: "alpha".into(),
            start_line: 1,
            end_line: 1,
        }];
        let proofs = graph_definition_proofs(&symbols);
        write_scip_index(
            &project_dir,
            "project",
            revision,
            ScipProofAdapterContract::graph_projection(revision),
            symbols,
            proofs,
        );
        (layout, project_dir)
    }

    #[test]
    fn a_truncated_index_scip_marker_cannot_report_a_ready_graph_lane() {
        let root = TempDir::new().expect("root");
        let (layout, project_dir) = graph_projection_fixture(&root, "graph-test");
        assert_eq!(
            ScipClient::health_probe(&layout, "project").availability,
            ScipAvailability::Ready {
                revision: "graph-test".into()
            },
            "the intact fixture must be ready, or the corruption below proves nothing"
        );

        // Present, non-empty, and completely useless: exactly the artifact the
        // old `.is_file()` check published as healthy.
        std::fs::write(project_dir.join(SCIP_INDEX_FILE), "\0\0\0\0").expect("truncate marker");

        assert_eq!(
            ScipClient::health_probe(&layout, "project").availability,
            ScipAvailability::Unavailable {
                reason: "scip_index_marker_header_unrecognized".into()
            }
        );
    }

    #[test]
    fn an_index_scip_marker_from_another_generation_cannot_report_a_ready_graph_lane() {
        let root = TempDir::new().expect("root");
        let (layout, project_dir) = graph_projection_fixture(&root, "graph-test");

        write_scip_index_marker(&project_dir, "graph-someone-else").expect("stale marker");

        assert_eq!(
            ScipClient::health_probe(&layout, "project").availability,
            ScipAvailability::Unavailable {
                reason: "scip_index_marker_revision_mismatch".into()
            }
        );
    }

    #[test]
    fn an_index_scip_marker_without_a_revision_line_cannot_report_a_ready_graph_lane() {
        let root = TempDir::new().expect("root");
        let (layout, project_dir) = graph_projection_fixture(&root, "graph-test");

        std::fs::write(project_dir.join(SCIP_INDEX_FILE), "codestory-scip-v1\n")
            .expect("header-only marker");

        assert_eq!(
            ScipClient::health_probe(&layout, "project").availability,
            ScipAvailability::Unavailable {
                reason: "scip_index_marker_revision_missing".into()
            }
        );
    }

    #[test]
    fn a_deleted_index_scip_marker_still_reports_as_a_stub() {
        let root = TempDir::new().expect("root");
        let (layout, project_dir) = graph_projection_fixture(&root, "graph-test");

        std::fs::remove_file(project_dir.join(SCIP_INDEX_FILE)).expect("remove marker");

        assert_eq!(
            ScipClient::health_probe(&layout, "project").availability,
            ScipAvailability::Unavailable {
                reason: "scip_stub".into()
            },
            "absence keeps its existing reason; only present-but-damaged is new"
        );
    }

    /// Two unrelated `Handler` symbols, in different files and on different
    /// graph nodes, each with its own outgoing and incoming call.
    ///
    /// `src/alpha/handler.rs`: `Handler` (node 2) calls `alpha_route`
    /// (node 3); `alpha_caller` (node 4) calls `Handler`; `alpha_route` also
    /// calls `alpha_leaf` (node 9), an edge in the anchor's own file that the
    /// anchor node is not an endpoint of.
    /// `src/beta/handler.rs`: `Handler` (node 6) calls `beta_route`
    /// (node 7); `beta_caller` (node 8) calls `Handler`.
    ///
    /// The display-name collision is real, not hand-written: the emitter
    /// projects `node.qualified_name` when it is set and `node.serialized_name`
    /// otherwise, so two same-named nodes without qualified names publish one
    /// display name across two files.
    fn shared_display_name_store(project: &TempDir) -> std::path::PathBuf {
        let storage_path = project.path().join("codestory.db");
        let mut storage = Store::open(&storage_path).expect("open store");
        let mut nodes = Vec::new();
        for (file_id, relative_path) in
            [(1_i64, "src/alpha/handler.rs"), (5, "src/beta/handler.rs")]
        {
            storage
                .insert_file(&FileInfo {
                    id: file_id,
                    path: project.path().join(relative_path),
                    language: "rust".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 90,
                    file_role: FileRole::Source,
                })
                .expect("insert file");
            nodes.push(Node {
                id: NodeId(file_id),
                kind: NodeKind::FILE,
                serialized_name: relative_path.to_string(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: None,
                start_line: Some(1),
                start_col: Some(0),
                end_line: Some(90),
                end_col: Some(0),
            });
        }
        for (id, file_id, name, line) in [
            (2_i64, 1_i64, "Handler", 10_u32),
            (3, 1, "alpha_route", 30),
            (4, 1, "alpha_caller", 50),
            (6, 5, "Handler", 10),
            (7, 5, "beta_route", 30),
            (8, 5, "beta_caller", 50),
            (9, 1, "alpha_leaf", 70),
        ] {
            nodes.push(Node {
                id: NodeId(id),
                kind: NodeKind::FUNCTION,
                serialized_name: name.to_string(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: Some(NodeId(file_id)),
                start_line: Some(line),
                start_col: Some(0),
                end_line: Some(line + 5),
                end_col: Some(0),
            });
        }
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        let edges = [
            (1_i64, 2_i64, 3_i64, 1_i64),
            (2, 4, 2, 1),
            (3, 6, 7, 5),
            (4, 8, 6, 5),
            (5, 3, 9, 1),
        ]
        .into_iter()
        .map(|(edge_id, source, target, file_id)| Edge {
            id: EdgeId(edge_id),
            source: NodeId(source),
            target: NodeId(target),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(file_id)),
            line: Some(12),
            resolved_source: Some(NodeId(source)),
            resolved_target: Some(NodeId(target)),
            confidence: Some(1.0),
            certainty: Some(ResolutionCertainty::Certain),
            callsite_identity: Some(format!("call:{source}->{target}")),
            candidate_targets: Vec::new(),
        })
        .collect::<Vec<_>>();
        storage.insert_edges_batch(&edges).expect("insert edges");
        drop(storage);
        storage_path
    }

    #[test]
    fn stage_two_adjacency_follows_the_anchor_node_not_a_shared_display_name() {
        let project = TempDir::new().expect("project");
        let storage_path = shared_display_name_store(&project);
        let root = TempDir::new().expect("root");
        let layout = adjacency_layout(&root);
        let project_dir = layout.scip_project_dir("generation-a");
        emit_scip_artifacts_from_store(&storage_path, &project_dir, "generation-a")
            .expect("emit scip")
            .expect("revision");

        let index = load_scip_symbols(&project_dir)
            .expect("load scip")
            .expect("index");
        let handlers = index
            .symbols
            .iter()
            .filter(|symbol| symbol.symbol == "Handler")
            .collect::<Vec<_>>();
        assert_eq!(
            handlers
                .iter()
                .map(|symbol| (
                    symbol.node_id.clone().unwrap_or_default(),
                    symbol.path.clone()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("2".to_string(), "src/alpha/handler.rs".to_string()),
                ("6".to_string(), "src/beta/handler.rs".to_string()),
            ],
            "the fixture only pins node identity if the artifact really carries \
             one display name on two nodes in two files: {:#?}",
            index.symbols
        );
        assert_eq!(
            index
                .proofs
                .iter()
                .filter(|proof| proof.is_reference())
                .map(|proof| (
                    proof.node_id.clone().unwrap_or_default(),
                    proof.target_node_id.clone().unwrap_or_default()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("2".to_string(), "3".to_string()),
                ("3".to_string(), "9".to_string()),
                ("4".to_string(), "2".to_string()),
                ("6".to_string(), "7".to_string()),
                ("8".to_string(), "6".to_string()),
            ],
            "every reference the anchor must not pick up has to be admissible \
             on its own, so only the anchor's node identity can separate them: \
             {:#?}",
            index.proofs
        );

        let mut anchor = CandidateHit::with_source(
            "src/alpha/handler.rs",
            Some("Handler".into()),
            0.9,
            CandidateSource::Lexical,
        );
        anchor.node_id = Some("2".into());

        let hits = ScipClient::expand_reference_adjacency(&layout, "generation-a", &[anchor], 8)
            .expect("expand");

        assert_eq!(
            hits.iter()
                .map(|hit| (
                    hit.symbol_name.clone().unwrap_or_default(),
                    hit.node_id.clone().unwrap_or_default(),
                    hit.file_path.clone(),
                    hit.scip_hop_distance
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    "alpha_route".to_string(),
                    "3".to_string(),
                    "src/alpha/handler.rs".to_string(),
                    Some(1)
                ),
                (
                    "alpha_caller".to_string(),
                    "4".to_string(),
                    "src/alpha/handler.rs".to_string(),
                    Some(1)
                ),
            ],
            "only references the anchor node is an endpoint of are adjacent: not \
             those of the homonymous `Handler` on node 6, and not `alpha_route`'s \
             own call inside the anchor's file: {hits:#?}"
        );
    }
}
