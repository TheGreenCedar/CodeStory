use crate::config::SidecarLayout;
use crate::lexical_index::{
    LexicalDocumentSource, LexicalHit, search_lexical_index_batch_with_cancel,
    search_lexical_index_descriptors_with_cancel, search_lexical_index_with_cancel, shard_dir_for,
};
use anyhow::Result;
use std::sync::Arc;

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
        search_lexical_index_with_cancel(
            &shard_dir_for(&layout.lexical_data_dir, generation),
            sidecar_input_hash,
            query,
            limit,
            cancelled,
        )?
        .into_iter()
        .map(lexical_hit_to_candidate)
        .collect()
    }

    pub fn search_descriptors_with_cancel<F>(
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
        search_lexical_index_descriptors_with_cancel(
            &shard_dir_for(&layout.lexical_data_dir, generation),
            sidecar_input_hash,
            query,
            limit,
            cancelled,
        )?
        .into_iter()
        .map(lexical_hit_to_candidate)
        .collect()
    }

    pub fn search_batch_with_cancel(
        &self,
        layout: &SidecarLayout,
        generation: &str,
        sidecar_input_hash: &str,
        queries: &[(String, usize)],
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Vec<Vec<super::CandidateHit>>> {
        search_lexical_index_batch_with_cancel(
            &shard_dir_for(&layout.lexical_data_dir, generation),
            sidecar_input_hash,
            queries,
            cancelled,
        )?
        .into_iter()
        .map(|hits| hits.into_iter().map(lexical_hit_to_candidate).collect())
        .collect()
    }
}

fn lexical_hit_to_candidate(hit: LexicalHit) -> Result<super::CandidateHit> {
    use super::candidate::{CandidateHit, CandidateSource};
    let mut candidate = CandidateHit::with_source(
        hit.path,
        hit.symbol_name,
        hit.score,
        CandidateSource::Lexical,
    );
    candidate.node_id = hit.node_id;
    if candidate.node_id.is_some() || hit.source == LexicalDocumentSource::LexicalSource {
        candidate.source_bytes_upper_bound =
            Some(codestory_contracts::compilation::INTERIM_SOURCE_ROW_UPPER_BOUND as u32);
    }
    candidate.start_line = hit.start_line;
    candidate.target = hit.target;
    candidate.source_excerpt = hit.source_excerpt;
    candidate.add_provenance(hit.source.provenance_label());
    Ok(candidate)
}
