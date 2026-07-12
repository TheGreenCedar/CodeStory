use crate::config::SidecarLayout;
use crate::lexical_index::{search_lexical_index, shard_dir_for};
use anyhow::Result;

#[derive(Debug, Clone, Default)]
pub struct LexicalClient;

impl LexicalClient {
    pub fn new(_layout: &SidecarLayout) -> Self {
        Self
    }

    pub fn search(
        &self,
        layout: &SidecarLayout,
        generation: &str,
        sidecar_input_hash: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<super::CandidateHit>> {
        use super::candidate::{CandidateHit, CandidateSource};
        search_lexical_index(
            &shard_dir_for(&layout.lexical_data_dir, generation),
            sidecar_input_hash,
            query,
            limit,
        )?
        .into_iter()
        .map(|hit| {
            let mut candidate = CandidateHit::with_source(
                hit.path,
                hit.symbol_name,
                hit.score,
                CandidateSource::Lexical,
            );
            candidate.node_id = hit.node_id;
            candidate.start_line = hit.start_line;
            candidate.add_provenance(hit.source.provenance_label());
            Ok(candidate)
        })
        .collect()
    }
}
