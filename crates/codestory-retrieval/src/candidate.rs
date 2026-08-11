use codestory_contracts::api::SearchTargetDto;
use codestory_contracts::graph::{EdgeKind, NodeKind};
use codestory_store::FileRole;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankFeatures {
    pub ranking_policy: String,
    pub lexical: f32,
    pub semantic: f32,
    pub scip_distance: f32,
    pub file_role_prior: f32,
    pub definition_quality: f32,
    pub token_overlap: f32,
    pub text_quality: f32,
    pub requested_role_agreement: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateGraphDirection {
    Anchor,
    Outgoing,
    Incoming,
}

/// Typed graph evidence retained independently from the fused score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateGraphEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_kind: Option<EdgeKind>,
    pub direction: CandidateGraphDirection,
    pub hop: u32,
    pub fanout: u32,
    pub edge_weight: f32,
    pub direction_weight: f32,
}

/// Lane-local evidence retained until reciprocal-rank fusion.
///
/// `raw_score` is meaningful only inside the producing lane. `rank` is the
/// one-based order assigned by that lane before candidates are merged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateLaneEvidence {
    pub raw_score: f32,
    pub rank: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<String>,
}

/// Independent retrieval-lane evidence for a candidate.
///
/// This is the candidate-v2 seam: lane scores never borrow the fused total or
/// another lane's score. The legacy scalar `CandidateHit::score` remains the
/// final public ranking score for compatibility.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CandidateLaneScores {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lexical: Option<CandidateLaneEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<CandidateLaneEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<CandidateLaneEvidence>,
}

impl CandidateLaneScores {
    pub fn is_empty(&self) -> bool {
        self.lexical.is_none() && self.semantic.is_none() && self.graph.is_none()
    }

    pub fn evidence_count(&self) -> usize {
        usize::from(self.lexical.is_some())
            + usize::from(self.semantic.is_some())
            + usize::from(self.graph.is_some())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateLane {
    Lexical,
    Semantic,
    Graph,
}

/// Unified retrieval candidate from any sidecar lane.
///
/// Candidates are navigation evidence until the runtime resolves them back to indexed symbols.
/// Dense anchors, lexical hits, and graph neighbors should keep their `provenance` labels so
/// packet diagnostics can distinguish evidence lanes from unresolved sidecar output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateHit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub file_path: String,
    pub symbol_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structural_kind: Option<NodeKind>,
    pub start_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<SearchTargetDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_excerpt: Option<String>,
    pub score: f32,
    #[serde(default, skip_serializing_if = "CandidateLaneScores::is_empty")]
    pub lane_scores: CandidateLaneScores,
    pub source: CandidateSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_role: Option<FileRole>,
    /// SCIP graph hops from anchor (lower is better).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scip_hop_distance: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_evidence: Option<CandidateGraphEvidence>,
    /// Populated by the feature ranker after fusion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank_features: Option<RankFeatures>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Sidecar lane that produced a retrieval candidate.
pub enum CandidateSource {
    Lexical,
    #[serde(rename = "semantic")]
    Semantic,
    Scip,
    Legacy,
}

/// Dev-only synthetic hit prefix (`lexical:`, `semantic:`, `scip:`).
pub fn is_phantom_sidecar_hit(hit: &CandidateHit) -> bool {
    hit.file_path.starts_with("lexical:")
        || hit.file_path.starts_with("semantic:")
        || hit.file_path.starts_with("scip:")
}

pub fn phantom_sidecar_candidates_only(candidates: &[CandidateHit]) -> bool {
    !candidates.is_empty() && candidates.iter().all(is_phantom_sidecar_hit)
}

/// Identity two lane candidates must share before fusion may collapse them.
///
/// A resolved node id is the strongest identity a lane can offer, so two
/// candidates that both carry one are the same evidence only when the ids
/// match: the schema permits duplicate display names in one file, and
/// overloads or repeated local names are distinct nodes at distinct lines.
/// When at least one lane left the candidate unresolved there is no id to
/// compare, so identity falls back to the definition site itself.
pub fn fused_candidate_identity_matches(left: &CandidateHit, right: &CandidateHit) -> bool {
    match (left.node_id.as_deref(), right.node_id.as_deref()) {
        (Some(left_node_id), Some(right_node_id)) => left_node_id == right_node_id,
        _ => {
            left.file_path == right.file_path
                && left.symbol_name == right.symbol_name
                && left.start_line == right.start_line
        }
    }
}

impl CandidateHit {
    pub fn lexical_stub(file_path: impl Into<String>, score: f32) -> Self {
        Self {
            node_id: None,
            file_path: file_path.into(),
            symbol_name: None,
            qualified_name: None,
            structural_kind: None,
            start_line: None,
            target: None,
            source_excerpt: None,
            score,
            lane_scores: CandidateLaneScores {
                lexical: Some(CandidateLaneEvidence {
                    raw_score: score,
                    rank: 0,
                    provenance: vec!["lexical_source".into()],
                }),
                ..CandidateLaneScores::default()
            },
            source: CandidateSource::Lexical,
            provenance: vec!["lexical_source".into()],
            file_role: None,
            scip_hop_distance: None,
            graph_evidence: None,
            rank_features: None,
        }
    }

    pub fn with_source(
        file_path: impl Into<String>,
        symbol_name: Option<String>,
        score: f32,
        source: CandidateSource,
    ) -> Self {
        let mut hit = Self {
            node_id: None,
            file_path: file_path.into(),
            symbol_name,
            qualified_name: None,
            structural_kind: None,
            start_line: None,
            target: None,
            source_excerpt: None,
            score,
            lane_scores: CandidateLaneScores::default(),
            source,
            provenance: Vec::new(),
            file_role: None,
            scip_hop_distance: None,
            graph_evidence: None,
            rank_features: None,
        };
        hit.record_lane(source.lane(), score, 0, source.default_provenance());
        hit
    }

    pub fn add_provenance(&mut self, label: impl Into<String>) {
        let label = label.into();
        if !self.provenance.iter().any(|existing| existing == &label) {
            self.provenance.push(label);
        }
    }

    pub fn record_lane(
        &mut self,
        lane: CandidateLane,
        raw_score: f32,
        rank: u32,
        provenance: impl Into<String>,
    ) {
        let provenance = provenance.into();
        let slot = match lane {
            CandidateLane::Lexical => &mut self.lane_scores.lexical,
            CandidateLane::Semantic => &mut self.lane_scores.semantic,
            CandidateLane::Graph => &mut self.lane_scores.graph,
        };
        match slot {
            Some(evidence) => {
                if raw_score > evidence.raw_score {
                    evidence.raw_score = raw_score;
                }
                if rank > 0 && (evidence.rank == 0 || rank < evidence.rank) {
                    evidence.rank = rank;
                }
                if !provenance.is_empty()
                    && !evidence
                        .provenance
                        .iter()
                        .any(|existing| existing == &provenance)
                {
                    evidence.provenance.push(provenance);
                }
            }
            None => {
                *slot = Some(CandidateLaneEvidence {
                    raw_score,
                    rank,
                    provenance: if provenance.is_empty() {
                        Vec::new()
                    } else {
                        vec![provenance]
                    },
                });
            }
        }
    }

    pub fn merge_lane_scores(&mut self, incoming: &CandidateLaneScores) {
        for (lane, evidence) in [
            (CandidateLane::Lexical, incoming.lexical.as_ref()),
            (CandidateLane::Semantic, incoming.semantic.as_ref()),
            (CandidateLane::Graph, incoming.graph.as_ref()),
        ] {
            let Some(evidence) = evidence else {
                continue;
            };
            if evidence.provenance.is_empty() {
                self.record_lane(lane, evidence.raw_score, evidence.rank, "");
            } else {
                for provenance in &evidence.provenance {
                    self.record_lane(lane, evidence.raw_score, evidence.rank, provenance);
                }
            }
        }
    }

    pub fn ensure_source_lane(&mut self) {
        let lane = self.source.lane();
        let missing = match lane {
            CandidateLane::Lexical => self.lane_scores.lexical.is_none(),
            CandidateLane::Semantic => self.lane_scores.semantic.is_none(),
            CandidateLane::Graph => self.lane_scores.graph.is_none(),
        };
        if missing {
            self.record_lane(lane, self.score, 0, self.source.default_provenance());
        }
    }
}

impl CandidateSource {
    pub fn lane(self) -> CandidateLane {
        match self {
            Self::Lexical | Self::Legacy => CandidateLane::Lexical,
            Self::Semantic => CandidateLane::Semantic,
            Self::Scip => CandidateLane::Graph,
        }
    }

    fn default_provenance(self) -> &'static str {
        match self {
            Self::Lexical | Self::Legacy => "lexical_source",
            Self::Semantic => "dense_anchor",
            Self::Scip => "graph_neighbor",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overload(node_id: &str, start_line: u32) -> CandidateHit {
        let mut hit = CandidateHit::with_source(
            "src/search.rs",
            Some("search".into()),
            0.5,
            CandidateSource::Lexical,
        );
        hit.node_id = Some(node_id.into());
        hit.start_line = Some(start_line);
        hit
    }

    #[test]
    fn distinct_nodes_sharing_a_name_and_file_are_not_the_same_candidate() {
        assert!(!fused_candidate_identity_matches(
            &overload("11", 40),
            &overload("12", 120)
        ));
    }

    #[test]
    fn same_node_id_matches_across_lanes_that_spell_the_symbol_differently() {
        let mut lexical = overload("11", 40);
        let mut dense = overload("11", 40);
        dense.source = CandidateSource::Semantic;
        dense.symbol_name = Some("core::search".into());
        dense.start_line = None;
        lexical.symbol_name = Some("search".into());

        assert!(fused_candidate_identity_matches(&lexical, &dense));
    }

    #[test]
    fn unresolved_candidates_fall_back_to_the_definition_site() {
        let mut anchored = overload("11", 40);
        anchored.node_id = None;
        let mut same_site = anchored.clone();
        same_site.source = CandidateSource::Scip;
        let mut other_site = anchored.clone();
        other_site.start_line = Some(120);

        assert!(fused_candidate_identity_matches(&anchored, &same_site));
        assert!(!fused_candidate_identity_matches(&anchored, &other_site));
    }

    #[test]
    fn merging_lanes_never_relabels_one_raw_score_as_another() {
        let mut fused = CandidateHit::with_source(
            "src/search.rs",
            Some("search".into()),
            0.42,
            CandidateSource::Lexical,
        );
        let semantic = CandidateHit::with_source(
            "src/search.rs",
            Some("search".into()),
            0.87,
            CandidateSource::Semantic,
        );

        fused.merge_lane_scores(&semantic.lane_scores);

        assert_eq!(
            fused
                .lane_scores
                .lexical
                .as_ref()
                .expect("lexical")
                .raw_score,
            0.42
        );
        assert_eq!(
            fused
                .lane_scores
                .semantic
                .as_ref()
                .expect("semantic")
                .raw_score,
            0.87
        );
    }
}
