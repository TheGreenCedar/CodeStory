use crate::config::SidecarLayout;
use crate::scip_index::{SCIP_SYMBOLS_FILE, ScipSymbolRecord, load_scip_symbols};
use std::cmp::Ordering;
use std::path::Path;

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
    pub fn health_probe(layout: &SidecarLayout, project_id: &str) -> ScipHealthProbe {
        let project_dir = layout.scip_project_dir(project_id);
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
        let has_graph_index = has_real_scip_artifact(&project_dir);
        let is_stub_revision = revision == "stub-v1" || !has_graph_index;
        ScipHealthProbe {
            availability: if is_stub_revision {
                ScipAvailability::Unavailable {
                    reason: "scip_stub".into(),
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
        project_id: &str,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<super::CandidateHit>> {
        let probe = Self::health_probe(layout, project_id);
        let ScipAvailability::Ready { .. } = probe.availability else {
            return Ok(Vec::new());
        };
        let project_dir = layout.scip_project_dir(project_id);
        let Some(index) = load_scip_symbols(&project_dir)? else {
            return Ok(Vec::new());
        };
        let profile = ScipQueryProfile::new(query);
        let mut hits = Vec::new();
        for symbol in index.symbols {
            if symbol_matches_query(&symbol, &profile) {
                let score = score_symbol_match(&symbol, &profile);
                hits.push(symbol_to_hit(&symbol, score, 0));
            }
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

    pub fn expand_graph(
        layout: &SidecarLayout,
        project_id: &str,
        anchors: &[super::CandidateHit],
        limit: usize,
    ) -> anyhow::Result<Vec<super::CandidateHit>> {
        let probe = Self::health_probe(layout, project_id);
        let ScipAvailability::Ready { .. } = probe.availability else {
            return Ok(Vec::new());
        };
        let project_dir = layout.scip_project_dir(project_id);
        let Some(index) = load_scip_symbols(&project_dir)? else {
            return Ok(Vec::new());
        };
        let mut hits = Vec::new();
        for anchor in anchors.iter().take(4) {
            let anchor_symbol = anchor.symbol_name.as_deref().unwrap_or("");
            for symbol in &index.symbols {
                if hits.len() >= limit {
                    break;
                }
                if symbol.path != anchor.file_path {
                    continue;
                }
                if anchor_symbol.is_empty() || symbol.symbol == anchor_symbol {
                    continue;
                }
                if symbol.symbol.contains(anchor_symbol) || anchor_symbol.contains(&symbol.symbol) {
                    hits.push(symbol_to_hit(
                        symbol,
                        0.65,
                        anchor.scip_hop_distance.unwrap_or(0) + 1,
                    ));
                }
            }
        }
        hits.truncate(limit);
        Ok(hits)
    }
}

fn symbol_to_hit(symbol: &ScipSymbolRecord, score: f32, hop: u32) -> super::CandidateHit {
    use super::candidate::{CandidateHit, CandidateSource};
    CandidateHit {
        node_id: None,
        file_path: symbol.path.clone(),
        symbol_name: Some(symbol.symbol.clone()),
        start_line: Some(symbol.start_line),
        score,
        source: CandidateSource::Scip,
        provenance: Vec::new(),
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

fn has_real_scip_artifact(project_dir: &Path) -> bool {
    if !project_dir.join(SCIP_SYMBOLS_FILE).is_file()
        || !project_dir
            .join(crate::scip_index::SCIP_INDEX_FILE)
            .is_file()
        || !project_dir.join("revision.txt").is_file()
        || project_dir.join("index.scip.stub").is_file()
    {
        return false;
    }
    load_scip_symbols(project_dir)
        .ok()
        .flatten()
        .is_some_and(|index| !index.symbols.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scip_index::{SCIP_INDEX_FILE, ScipSymbolsIndex};
    use tempfile::TempDir;

    #[test]
    fn anchor_search_scores_all_matches_before_truncating() {
        let root = TempDir::new().expect("root");
        let layout = SidecarLayout {
            zoekt_http_port: 1,
            qdrant_http_port: 2,
            qdrant_grpc_port: 3,
            zoekt_data_dir: root.path().join("zoekt"),
            qdrant_data_dir: root.path().join("qdrant"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("state.json"),
        };
        let project_id = "project";
        let project_dir = layout.scip_project_dir(project_id);
        std::fs::create_dir_all(&project_dir).expect("scip dir");

        let mut symbols = Vec::new();
        for index in 0..12 {
            symbols.push(ScipSymbolRecord {
                path: format!("src/needle/noise_{index}.ts"),
                symbol: format!("noise_{index}"),
                start_line: index + 1,
                end_line: index + 1,
            });
        }
        symbols.push(ScipSymbolRecord {
            path: "src/needle/target.ts".to_string(),
            symbol: "needle".to_string(),
            start_line: 99,
            end_line: 99,
        });
        let index = ScipSymbolsIndex {
            revision: "graph-test".to_string(),
            symbols,
        };
        std::fs::write(
            project_dir.join(SCIP_SYMBOLS_FILE),
            serde_json::to_string_pretty(&index).expect("serialize"),
        )
        .expect("write symbols");
        std::fs::write(project_dir.join("revision.txt"), "graph-test\n").expect("revision");
        std::fs::write(project_dir.join(SCIP_INDEX_FILE), "codestory-scip-v1\n").expect("index");

        let hits = ScipClient::anchor_search(&layout, project_id, "needle", 8).expect("search");

        assert!(
            hits.iter()
                .any(|hit| hit.file_path == "src/needle/target.ts"),
            "exact SCIP symbol match should survive top-k truncation even when many earlier path-only matches exist"
        );
        assert_eq!(hits[0].file_path, "src/needle/target.ts");
    }

    #[test]
    fn qualified_anchor_search_admits_crate_matching_terminal_definition() {
        let root = TempDir::new().expect("root");
        let layout = SidecarLayout {
            zoekt_http_port: 1,
            qdrant_http_port: 2,
            qdrant_grpc_port: 3,
            zoekt_data_dir: root.path().join("zoekt"),
            qdrant_data_dir: root.path().join("qdrant"),
            scip_artifacts_root: root.path().join("scip"),
            state_file: root.path().join("state.json"),
        };
        let project_id = "project";
        let project_dir = layout.scip_project_dir(project_id);
        std::fs::create_dir_all(&project_dir).expect("scip dir");

        let index = ScipSymbolsIndex {
            revision: "graph-test".to_string(),
            symbols: vec![
                ScipSymbolRecord {
                    path: "workspace/app/src/main.rs".to_string(),
                    symbol: "workspace_app::Cli".to_string(),
                    start_line: 15,
                    end_line: 15,
                },
                ScipSymbolRecord {
                    path: "workspace/tools/src/cli.rs".to_string(),
                    symbol: "Cli".to_string(),
                    start_line: 1,
                    end_line: 1,
                },
                ScipSymbolRecord {
                    path: "workspace/app/src/cli.rs".to_string(),
                    symbol: "Cli".to_string(),
                    start_line: 42,
                    end_line: 42,
                },
            ],
        };
        std::fs::write(
            project_dir.join(SCIP_SYMBOLS_FILE),
            serde_json::to_string_pretty(&index).expect("serialize"),
        )
        .expect("write symbols");
        std::fs::write(project_dir.join("revision.txt"), "graph-test\n").expect("revision");
        std::fs::write(project_dir.join(SCIP_INDEX_FILE), "codestory-scip-v1\n").expect("index");

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
            zoekt_http_port: 1,
            qdrant_http_port: 2,
            qdrant_grpc_port: 3,
            zoekt_data_dir: root.path().join("zoekt"),
            qdrant_data_dir: root.path().join("qdrant"),
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
}
