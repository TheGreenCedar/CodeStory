use crate::config::SidecarLayout;
use crate::scip_index::{
    SCIP_GRAPH_PROJECTION_PROVENANCE, SCIP_INDEX_FILE, SCIP_SYMBOLS_FILE, ScipSymbolLookup,
    ScipSymbolRecord, load_fresh_scip_symbols, load_scip_symbols, reference_defect,
};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::Path;

/// Anchors expanded per stage-2 call, and the score a validated adjacency hit
/// enters fusion with.
const SCIP_ADJACENCY_ANCHOR_LIMIT: usize = 4;
const SCIP_ADJACENCY_SCORE: f32 = 0.65;

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
        if project_dir.join("index.scip.stub").is_file() {
            return ScipHealthProbe {
                availability: ScipAvailability::Unavailable {
                    reason: "scip_stub".into(),
                },
                artifact_count: artifacts,
                detail: "stub SCIP artifacts only (index.scip.stub present)".into(),
            };
        }
        let revision = read_scip_revision(&project_dir).unwrap_or_else(|| "stub-v1".into());
        let artifact_status = scip_artifact_status(&project_dir, &revision, generation);
        let is_stub_revision = revision == "stub-v1" || artifact_status == "scip_stub";
        ScipHealthProbe {
            availability: if is_stub_revision {
                ScipAvailability::Unavailable {
                    reason: "scip_stub".into(),
                }
            } else if artifact_status == "scip_stale"
                || artifact_status == "scip_imported_diagnostic_only"
            {
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
        let Some(index) = load_fresh_scip_symbols(&project_dir, &revision, generation)? else {
            return Ok(Vec::new());
        };
        let Some(provenance) = index.contract.provenance_label() else {
            return Ok(Vec::new());
        };
        let profile = ScipQueryProfile::new(query);
        let mut hits = Vec::new();
        for (index, symbol) in index.symbols.into_iter().enumerate() {
            if index % 64 == 0 && cancelled() {
                anyhow::bail!("SCIP anchor search cancelled");
            }
            if symbol_matches_query(&symbol, &profile) {
                let score = score_symbol_match(&symbol, &profile);
                hits.push(symbol_to_hit(&symbol, score, 0, provenance));
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
        let Some(index) = load_fresh_scip_symbols(&project_dir, &revision, generation)? else {
            return Ok(Vec::new());
        };
        let Some(evidence_source) = index.contract.evidence_source() else {
            return Ok(Vec::new());
        };
        let Some(provenance) = index.contract.provenance_label() else {
            return Ok(Vec::new());
        };

        let mut anchor_hops: Vec<(&str, u32)> = Vec::new();
        for anchor in anchors.iter().take(SCIP_ADJACENCY_ANCHOR_LIMIT) {
            let Some(node_id) = anchor.node_id.as_deref() else {
                continue;
            };
            if anchor_hops.iter().any(|(seen, _)| *seen == node_id) {
                continue;
            }
            anchor_hops.push((node_id, anchor.scip_hop_distance.unwrap_or(0)));
        }
        if anchor_hops.is_empty() {
            return Ok(Vec::new());
        }

        let lookup = ScipSymbolLookup::new(&index.symbols);
        let mut emitted: HashSet<&str> = anchor_hops.iter().map(|(node_id, _)| *node_id).collect();
        let mut hits = Vec::new();
        for (position, proof) in index.proofs.iter().enumerate() {
            if position % 64 == 0 && cancelled() {
                anyhow::bail!("SCIP reference adjacency cancelled");
            }
            if hits.len() >= limit {
                break;
            }
            if !proof.is_reference() {
                continue;
            }
            let (Some(node_id), Some(target_node_id)) =
                (proof.node_id.as_deref(), proof.target_node_id.as_deref())
            else {
                continue;
            };
            let Some((neighbor_node_id, anchor_hop)) =
                anchor_hops.iter().find_map(|(anchor_node_id, hop)| {
                    if *anchor_node_id == node_id {
                        Some((target_node_id, *hop))
                    } else if *anchor_node_id == target_node_id {
                        Some((node_id, *hop))
                    } else {
                        None
                    }
                })
            else {
                continue;
            };
            if emitted.contains(neighbor_node_id) {
                continue;
            }
            if reference_defect(proof, &lookup, evidence_source).is_some() {
                continue;
            }
            let Some(neighbor) = lookup.symbol_for_node(neighbor_node_id) else {
                continue;
            };
            hits.push(symbol_to_hit(
                neighbor,
                SCIP_ADJACENCY_SCORE,
                anchor_hop.saturating_add(1),
                provenance,
            ));
            emitted.insert(neighbor_node_id);
        }
        if cancelled() {
            anyhow::bail!("SCIP reference adjacency cancelled");
        }
        hits.truncate(limit);
        Ok(hits)
    }
}

fn symbol_to_hit(
    symbol: &ScipSymbolRecord,
    score: f32,
    hop: u32,
    provenance: &str,
) -> super::CandidateHit {
    use super::candidate::{CandidateHit, CandidateSource};
    CandidateHit {
        node_id: if provenance == SCIP_GRAPH_PROJECTION_PROVENANCE {
            symbol.node_id.clone()
        } else {
            None
        },
        file_path: symbol.path.clone(),
        symbol_name: Some(symbol.symbol.clone()),
        start_line: Some(symbol.start_line),
        target: None,
        source_excerpt: None,
        score,
        source: CandidateSource::Scip,
        provenance: vec![provenance.into()],
        file_role: None,
        scip_hop_distance: Some(hop),
        rank_features: None,
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

fn symbol_matches_query(symbol: &ScipSymbolRecord, profile: &ScipQueryProfile) -> bool {
    let symbol_lower = symbol.symbol.to_ascii_lowercase();
    let path_lower = symbol.path.to_ascii_lowercase();
    if profile.tokens.is_empty() {
        return symbol_lower.contains(&profile.query_lower)
            || path_lower.contains(&profile.query_lower);
    }
    if profile
        .tokens
        .iter()
        .all(|token| symbol_lower.contains(token) || path_lower.contains(token))
    {
        return true;
    }
    let Some(qualified) = profile.qualified.as_ref() else {
        return false;
    };
    symbol_terminal(&symbol_lower) == qualified.terminal_lower
        && qualified_prefix_path_score(&qualified.prefix_lower, &symbol.path) > 0
}

fn score_symbol_match(symbol: &ScipSymbolRecord, profile: &ScipQueryProfile) -> f32 {
    let symbol_lower = symbol.symbol.to_ascii_lowercase();
    let path_lower = symbol.path.to_ascii_lowercase();
    let mut score = 0.70_f32;
    if symbol_lower == profile.query_lower {
        score += 0.22;
    } else if symbol_lower.contains(&profile.query_lower) {
        score += 0.14;
    }
    if path_lower == profile.query_lower {
        score += 0.08;
    } else if path_lower.contains(&profile.query_lower) {
        score += 0.04;
    }
    for token in &profile.tokens {
        if symbol_lower == *token {
            score += 0.05;
        } else if symbol_lower.contains(token) {
            score += 0.03;
        }
        if path_lower.contains(token) {
            score += 0.01;
        }
    }
    if let Some(qualified) = profile.qualified.as_ref() {
        let terminal = symbol_terminal(&symbol_lower);
        let prefix_path_score = qualified_prefix_path_score(&qualified.prefix_lower, &symbol.path);
        if terminal == qualified.terminal_lower {
            score += 0.18;
        }
        score += match prefix_path_score {
            3 => 0.12,
            2 => 0.09,
            1 => 0.05,
            _ => 0.0,
        };
        if terminal == qualified.terminal_lower
            && file_stem_lower(&symbol.path).as_deref() == Some(qualified.terminal_lower.as_str())
        {
            score += 0.16;
        }
        if symbol_lower == profile.query_lower
            && file_stem_lower(&symbol.path).as_deref() != Some(qualified.terminal_lower.as_str())
        {
            score -= 0.12;
        }
    }
    score.min(1.20)
}

fn symbol_terminal(symbol: &str) -> String {
    symbol
        .rsplit("::")
        .next()
        .unwrap_or(symbol)
        .rsplit('.')
        .next()
        .unwrap_or(symbol)
        .to_ascii_lowercase()
}

fn qualified_prefix_path_score(prefix_lower: &str, path: &str) -> u8 {
    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let segments = normalized_path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return 0;
    }

    let hyphenated_prefix = prefix_lower.replace('_', "-");
    if !hyphenated_prefix.is_empty() && segments.iter().any(|segment| *segment == hyphenated_prefix)
    {
        return 3;
    }

    let trailing_prefix_segment = prefix_lower
        .rsplit('_')
        .next()
        .unwrap_or(prefix_lower)
        .replace('_', "-");
    if trailing_prefix_segment.len() >= 3
        && segments
            .iter()
            .any(|segment| *segment == trailing_prefix_segment)
    {
        return 2;
    }

    let compact_prefix = compact_alphanumeric(prefix_lower);
    if compact_prefix.len() >= 3
        && segments
            .iter()
            .any(|segment| compact_alphanumeric(segment) == compact_prefix)
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

fn file_stem_lower(path: &str) -> Option<String> {
    let file_name = path
        .rsplit(['/', '\\'])
        .next()
        .filter(|file_name| !file_name.is_empty())?;
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem);
    Some(stem.to_ascii_lowercase())
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
    if !project_dir.join(SCIP_SYMBOLS_FILE).is_file()
        || !project_dir.join(SCIP_INDEX_FILE).is_file()
        || !project_dir.join("revision.txt").is_file()
        || project_dir.join("index.scip.stub").is_file()
    {
        return "scip_stub";
    }
    load_scip_symbols(project_dir)
        .ok()
        .flatten()
        .filter(|index| !index.symbols.is_empty())
        .map_or("scip_stub", |index| {
            if !index.is_fresh_for(revision, generation) {
                return "scip_stale";
            }
            if index.contract.evidence_source == SCIP_GRAPH_PROJECTION_PROVENANCE {
                "ready"
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
        SCIP_DEFINITION_ROLE, SCIP_IMPORTED_PROOF_PROVENANCE,
        SCIP_PRECISE_SEMANTIC_IMPORT_PUBLIC_PROVENANCE, SCIP_REFERENCE_ROLE, ScipPackageIdentity,
        ScipProofAdapterContract, ScipProofRecord, ScipSymbolsIndex,
    };
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
            project_dir.join(SCIP_SYMBOLS_FILE),
            serde_json::to_string_pretty(&index).expect("serialize"),
        )
        .expect("write symbols");
        std::fs::write(project_dir.join("revision.txt"), format!("{revision}\n"))
            .expect("revision");
        std::fs::write(project_dir.join(SCIP_INDEX_FILE), "codestory-scip-v1\n").expect("index");
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
        std::fs::write(project_dir.join(SCIP_INDEX_FILE), "codestory-scip-v1\n").expect("index");

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
}
