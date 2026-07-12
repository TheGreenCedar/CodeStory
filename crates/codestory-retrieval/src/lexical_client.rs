use crate::config::SidecarLayout;
use crate::lexical_index::{search_lexical_index_with_cancel, shard_dir_for};
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
        self.search_with_cancel(layout, generation, sidecar_input_hash, query, limit, || {
            false
        })
    }

    pub fn search_with_cancel<F>(
        &self,
        layout: &SidecarLayout,
        generation: &str,
        sidecar_input_hash: &str,
        query: &str,
        limit: usize,
        cancelled: F,
    ) -> Result<Vec<super::CandidateHit>>
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        use super::candidate::{CandidateHit, CandidateSource};
        search_lexical_index_with_cancel(
            &shard_dir_for(&layout.lexical_data_dir, generation),
            sidecar_input_hash,
            query,
            limit,
            cancelled,
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
