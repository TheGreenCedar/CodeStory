use codestory_contracts::api::SearchTargetDto;
use codestory_store::FileRole;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankFeatures {
    pub lexical: f32,
    pub semantic: f32,
    pub scip_distance: f32,
    pub file_role_prior: f32,
    pub definition_quality: f32,
    pub token_overlap: f32,
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
    pub start_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<SearchTargetDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_excerpt: Option<String>,
    pub score: f32,
    pub source: CandidateSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_role: Option<FileRole>,
    /// SCIP graph hops from anchor (lower is better).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scip_hop_distance: Option<u32>,
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
            start_line: None,
            target: None,
            source_excerpt: None,
            score,
            source: CandidateSource::Lexical,
            provenance: vec!["lexical_source".into()],
            file_role: None,
            scip_hop_distance: None,
            rank_features: None,
        }
    }

    pub fn with_source(
        file_path: impl Into<String>,
        symbol_name: Option<String>,
        score: f32,
        source: CandidateSource,
    ) -> Self {
        Self {
            node_id: None,
            file_path: file_path.into(),
            symbol_name,
            start_line: None,
            target: None,
            source_excerpt: None,
            score,
            source,
            provenance: Vec::new(),
            file_role: None,
            scip_hop_distance: None,
            rank_features: None,
        }
    }

    pub fn add_provenance(&mut self, label: impl Into<String>) {
        let label = label.into();
        if !self.provenance.iter().any(|existing| existing == &label) {
            self.provenance.push(label);
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
}
