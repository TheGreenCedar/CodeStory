//! AppController batch search paths for packet retrieval.

use crate::agent::retrieval_primary::{
    packet_batch_should_use_sidecar, search_sidecar_packet_batch,
    sidecar_retrieval_unavailable_reason,
};
use crate::{AppController, HybridSearchScoredHit};
use codestory_contracts::api::{
    AgentHybridWeightsDto, ApiError, SearchHit, SemanticFallbackRecordDto,
};

pub(crate) struct SemanticHybridBatchOutcome {
    pub results: Vec<(String, Vec<HybridSearchScoredHit>)>,
    pub fallbacks: Vec<SemanticFallbackRecordDto>,
}

impl AppController {
    pub(crate) fn search_symbolic_packet_anchor_batch(
        &self,
        queries: &[String],
        max_results: usize,
    ) -> Result<Vec<(String, Vec<SearchHit>)>, ApiError> {
        let batched = queries
            .iter()
            .map(|query| (query.clone(), max_results))
            .collect::<Vec<_>>();
        self.search_lexical_hybrid_batch(&batched)
    }

    pub(crate) fn search_lexical_hybrid_batch(
        &self,
        queries: &[(String, usize)],
    ) -> Result<Vec<(String, Vec<SearchHit>)>, ApiError> {
        if queries.is_empty() {
            return Ok(Vec::new());
        }
        if packet_batch_should_use_sidecar(self) {
            match search_sidecar_packet_batch(self, queries, None) {
                Ok(results) => return Ok(results),
                Err(error) => {
                    tracing::warn!(
                        "sidecar retrieval packet lexical batch unavailable; fail-closed: {}",
                        error.message
                    );
                    return Err(ApiError::invalid_argument(format!(
                        "sidecar retrieval packet lexical batch unavailable: {}; sidecar retrieval is mandatory",
                        error.message
                    )));
                }
            }
        } else if let Some(reason) = sidecar_retrieval_unavailable_reason(self) {
            return Err(ApiError::invalid_argument(reason));
        }
        Err(ApiError::invalid_argument(
            "sidecar retrieval primary is mandatory for packet lexical batch",
        ))
    }

    pub(crate) fn search_semantic_hybrid_batch(
        &self,
        queries: &[(String, usize, Option<AgentHybridWeightsDto>)],
    ) -> Result<SemanticHybridBatchOutcome, ApiError> {
        if queries.is_empty() {
            return Ok(SemanticHybridBatchOutcome {
                results: Vec::new(),
                fallbacks: Vec::new(),
            });
        }
        if packet_batch_should_use_sidecar(self) {
            let batch = queries
                .iter()
                .map(|(query, max_results, _)| (query.clone(), *max_results))
                .collect::<Vec<_>>();
            match search_sidecar_packet_batch(self, &batch, None) {
                Ok(results) => {
                    return Ok(SemanticHybridBatchOutcome {
                        results: results
                            .into_iter()
                            .map(|(query, hits)| {
                                (
                                    query,
                                    hits.into_iter()
                                        .map(|hit| HybridSearchScoredHit {
                                            lexical_score: hit.score,
                                            semantic_score: 0.0,
                                            graph_score: 0.0,
                                            total_score: hit.score,
                                            hit,
                                        })
                                        .collect(),
                                )
                            })
                            .collect(),
                        fallbacks: Vec::new(),
                    });
                }
                Err(error) => {
                    tracing::warn!(
                        "sidecar retrieval packet semantic batch unavailable; fail-closed: {}",
                        error.message
                    );
                    return Err(ApiError::invalid_argument(format!(
                        "sidecar retrieval packet semantic batch unavailable: {}; sidecar retrieval is mandatory",
                        error.message
                    )));
                }
            }
        } else if let Some(reason) = sidecar_retrieval_unavailable_reason(self) {
            return Err(ApiError::invalid_argument(reason));
        }
        Err(ApiError::invalid_argument(
            "sidecar retrieval primary is mandatory for packet semantic batch",
        ))
    }

    pub(crate) fn warm_packet_subquery_embeddings(
        &self,
        queries: &[String],
    ) -> Result<(), ApiError> {
        if queries.is_empty() {
            return Ok(());
        }
        if packet_batch_should_use_sidecar(self) {
            return Ok(());
        } else if let Some(reason) = sidecar_retrieval_unavailable_reason(self) {
            return Err(ApiError::invalid_argument(reason));
        }
        Err(ApiError::invalid_argument(
            "sidecar retrieval primary is mandatory for packet subquery warmup",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_subquery_warmup_fails_closed_without_sidecar_primary() {
        let controller = AppController::new();

        let error = controller
            .warm_packet_subquery_embeddings(&["run_exec_session".to_string()])
            .expect_err("packet warmup must not fall back to the legacy in-process search engine");

        assert!(
            error
                .message
                .contains("sidecar retrieval primary requires an open project"),
            "warmup should report the mandatory sidecar gate, got: {}",
            error.message
        );
    }
}
