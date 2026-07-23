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

impl CandidateHit {
    pub fn lexical_stub(file_path: impl Into<String>, score: f32) -> Self {
        Self {
            node_id: None,
            file_path: file_path.into(),
            symbol_name: None,
            start_line: None,
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
