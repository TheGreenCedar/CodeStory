use crate::config::SidecarLayout;
use crate::lexical_index::{
    LexicalDocumentSource, LexicalHit, LexicalHitPayload, search_lexical_index_batch_with_cancel,
    search_lexical_index_batch_with_policy_cancel, search_lexical_index_descriptors_with_cancel,
    search_lexical_index_with_cancel, search_lexical_index_with_policy_cancel, shard_dir_for,
};
use crate::query_policy::LexicalAdmissionPolicy;
use anyhow::Result;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct LexicalClient;

impl LexicalClient {
    pub fn new(_layout: &SidecarLayout) -> Self {
        Self
    }

    pub(crate) fn enrich_packet_calls(
        &self,
        layout: &SidecarLayout,
        generation: &str,
        input_hash: &str,
        candidates: &mut [super::CandidateHit],
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<()> {
        let seeds = candidates
            .iter()
            .filter_map(|c| c.packet_descriptor())
            .take(codestory_contracts::compilation::INTERIM_MAX_ADMITTED_CANDIDATES)
            .map(|d| (d.stable_identity, d.path))
            .collect::<Vec<_>>();
        let metadata = crate::lexical_index::read_published_call_metadata(
            &shard_dir_for(&layout.lexical_data_dir, generation),
            input_hash,
            &seeds,
            cancelled,
        )?;
        for candidate in candidates {
            if candidate
                .provenance
                .iter()
                .any(|p| p == "published-call-descriptors-conflict")
            {
                continue;
            }
            let Some(annotation) = candidate
                .packet_stable_identity()
                .and_then(|id| metadata.get(&id))
            else {
                continue;
            };
            if candidate
                .published_calls
                .as_ref()
                .is_some_and(|m| m != annotation)
            {
                candidate.published_calls = None;
                candidate.add_provenance("published-call-descriptors-conflict");
            } else {
                candidate.published_calls = Some(annotation.clone());
            }
        }
        Ok(())
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
        .map(|hit| lexical_hit_to_candidate_bound(hit, sidecar_input_hash))
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
        .map(|hit| lexical_hit_to_candidate_bound(hit, sidecar_input_hash))
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
        .map(|hits| {
            hits.into_iter()
                .map(|hit| lexical_hit_to_candidate_bound(hit, sidecar_input_hash))
                .collect()
        })
        .collect()
    }
    pub(crate) fn search_with_policy_cancel<F>(
        &self,
        layout: &SidecarLayout,
        generation: &str,
        input_hash: &str,
        query: &str,
        limit: usize,
        policy: LexicalAdmissionPolicy,
        cancelled: F,
    ) -> Result<Vec<super::CandidateHit>>
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        search_lexical_index_with_policy_cancel(
            &shard_dir_for(&layout.lexical_data_dir, generation),
            input_hash,
            query,
            limit,
            LexicalHitPayload::Full,
            policy,
            cancelled,
        )?
        .into_iter()
        .map(|hit| lexical_hit_to_candidate_bound(hit, input_hash))
        .collect()
    }

    pub(crate) fn search_descriptors_with_policy_cancel<F>(
        &self,
        layout: &SidecarLayout,
        generation: &str,
        input_hash: &str,
        query: &str,
        limit: usize,
        policy: LexicalAdmissionPolicy,
        cancelled: F,
    ) -> Result<Vec<super::CandidateHit>>
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        search_lexical_index_with_policy_cancel(
            &shard_dir_for(&layout.lexical_data_dir, generation),
            input_hash,
            query,
            limit,
            LexicalHitPayload::DescriptorOnly,
            policy,
            cancelled,
        )?
        .into_iter()
        .map(|hit| lexical_hit_to_candidate_bound(hit, input_hash))
        .collect()
    }

    pub(crate) fn search_batch_with_policy_cancel(
        &self,
        layout: &SidecarLayout,
        generation: &str,
        input_hash: &str,
        queries: &[(String, usize, LexicalAdmissionPolicy)],
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Vec<Vec<super::CandidateHit>>> {
        search_lexical_index_batch_with_policy_cancel(
            &shard_dir_for(&layout.lexical_data_dir, generation),
            input_hash,
            queries,
            cancelled,
        )?
        .into_iter()
        .map(|hits| {
            hits.into_iter()
                .map(|hit| lexical_hit_to_candidate_bound(hit, input_hash))
                .collect()
        })
        .collect()
    }
}

pub(crate) fn lexical_hit_to_candidate_bound(
    mut hit: LexicalHit,
    input_hash: &str,
) -> Result<super::CandidateHit> {
    let neighborhood = hit.call_neighborhood.take();
    let mut candidate = lexical_hit_to_candidate(hit)?;
    candidate.published_calls =
        neighborhood.map(
            |neighborhood| codestory_contracts::compilation::PacketPublishedCallsV1 {
                sidecar_input_hash: input_hash.into(),
                neighborhood,
            },
        );
    Ok(candidate)
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
    // Only a located symbol or a real source-file row owns a source
    // reservation. Generated summaries remain navigation evidence.
    if (hit.source == LexicalDocumentSource::SymbolDoc
        && candidate.node_id.is_some()
        && hit.start_line.is_some_and(|line| line > 0))
        || (hit.source == LexicalDocumentSource::LexicalSource && candidate.node_id.is_none())
    {
        candidate.source_bytes_upper_bound =
            Some(codestory_contracts::compilation::INTERIM_SOURCE_ROW_UPPER_BOUND as u32);
    }
    candidate.start_line = hit.start_line;
    candidate.target = hit.target;
    candidate.source_excerpt = hit.source_excerpt;
    candidate.add_provenance(hit.source.provenance_label());
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_reservation_requires_source_owning_lexical_metadata() {
        let convert = |source, node_id: Option<&str>, start_line| {
            lexical_hit_to_candidate(LexicalHit {
                call_neighborhood: None,
                path: "src/service.rs".into(),
                source,
                node_id: node_id.map(str::to_string),
                symbol_name: Some("Service".into()),
                start_line,
                target: None,
                source_excerpt: None,
                score: 0.9,
            })
            .unwrap()
        };
        for line in [None, Some(0), Some(3)] {
            let summary = convert(LexicalDocumentSource::ComponentReport, Some("41"), line);
            assert!(summary.source_bytes_upper_bound.is_none());
            assert!(summary.packet_descriptor().is_none());
        }
        for line in [None, Some(0)] {
            assert!(
                convert(LexicalDocumentSource::SymbolDoc, Some("42"), line)
                    .packet_descriptor()
                    .is_none()
            );
        }
        assert!(
            convert(LexicalDocumentSource::SymbolDoc, Some("42"), Some(3))
                .packet_descriptor()
                .is_some()
        );
        assert!(
            convert(LexicalDocumentSource::LexicalSource, None, None)
                .packet_descriptor()
                .is_some()
        );
    }
}
