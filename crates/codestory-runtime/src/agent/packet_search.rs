//! AppController batch search paths for packet retrieval.

use crate::agent::retrieval_primary::{
    packet_batch_should_use_sidecar, search_sidecar_packet_batch,
    sidecar_retrieval_unavailable_error, sidecar_retrieval_unavailable_reason,
};
use crate::{AppController, HybridSearchScoredHit};
use codestory_contracts::api::{
    AgentHybridWeightsDto, ApiError, PacketSidecarQueryDiagnosticDto, SearchHit,
    SemanticFallbackRecordDto,
};

pub(crate) struct SemanticHybridBatchOutcome {
    pub results: Vec<(String, Vec<HybridSearchScoredHit>)>,
    pub fallbacks: Vec<SemanticFallbackRecordDto>,
    pub sidecar_diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
}

pub(crate) struct LexicalBatchOutcome {
    pub results: Vec<(String, Vec<SearchHit>)>,
    pub sidecar_diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
}

fn packet_batch_error(controller: &AppController, error: ApiError, context: &str) -> ApiError {
    if error.code == "publication_changed" {
        error
    } else {
        sidecar_retrieval_unavailable_error(
            controller,
            format!(
                "{context}: {}; sidecar retrieval is mandatory",
                error.message
            ),
        )
    }
}

impl AppController {
    pub(crate) fn search_symbolic_packet_anchor_batch(
        &self,
        queries: &[String],
        max_results: usize,
        latency_budget_ms: Option<u32>,
    ) -> Result<LexicalBatchOutcome, ApiError> {
        let batched = queries
            .iter()
            .map(|query| (query.clone(), max_results))
            .collect::<Vec<_>>();
        self.search_lexical_hybrid_batch(&batched, latency_budget_ms)
    }

    pub(crate) fn search_lexical_hybrid_batch(
        &self,
        queries: &[(String, usize)],
        latency_budget_ms: Option<u32>,
    ) -> Result<LexicalBatchOutcome, ApiError> {
        if queries.is_empty() {
            return Ok(LexicalBatchOutcome {
                results: Vec::new(),
                sidecar_diagnostics: Vec::new(),
            });
        }
        if packet_batch_should_use_sidecar(self) {
            match search_sidecar_packet_batch(self, queries, latency_budget_ms) {
                Ok(outcome) => {
                    return Ok(LexicalBatchOutcome {
                        results: outcome.results,
                        sidecar_diagnostics: outcome.diagnostics,
                    });
                }
                Err(error) => {
                    tracing::warn!(
                        "sidecar retrieval packet lexical batch unavailable; fail-closed: {}",
                        error.message
                    );
                    return Err(packet_batch_error(
                        self,
                        error,
                        "sidecar retrieval packet lexical batch unavailable",
                    ));
                }
            }
        } else if let Some(reason) = sidecar_retrieval_unavailable_reason(self) {
            return Err(sidecar_retrieval_unavailable_error(self, reason));
        }
        Err(sidecar_retrieval_unavailable_error(
            self,
            "full retrieval is mandatory for packet lexical batch",
        ))
    }

    pub(crate) fn search_semantic_hybrid_batch(
        &self,
        queries: &[(String, usize, Option<AgentHybridWeightsDto>)],
        latency_budget_ms: Option<u32>,
    ) -> Result<SemanticHybridBatchOutcome, ApiError> {
        if queries.is_empty() {
            return Ok(SemanticHybridBatchOutcome {
                results: Vec::new(),
                fallbacks: Vec::new(),
                sidecar_diagnostics: Vec::new(),
            });
        }
        if packet_batch_should_use_sidecar(self) {
            let batch = queries
                .iter()
                .map(|(query, max_results, _)| (query.clone(), *max_results))
                .collect::<Vec<_>>();
            match search_sidecar_packet_batch(self, &batch, latency_budget_ms) {
                Ok(outcome) => {
                    return Ok(SemanticHybridBatchOutcome {
                        results: outcome
                            .results
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
                        sidecar_diagnostics: outcome.diagnostics,
                    });
                }
                Err(error) => {
                    tracing::warn!(
                        "sidecar retrieval packet semantic batch unavailable; fail-closed: {}",
                        error.message
                    );
                    return Err(packet_batch_error(
                        self,
                        error,
                        "sidecar retrieval packet semantic batch unavailable",
                    ));
                }
            }
        } else if let Some(reason) = sidecar_retrieval_unavailable_reason(self) {
            return Err(sidecar_retrieval_unavailable_error(self, reason));
        }
        Err(sidecar_retrieval_unavailable_error(
            self,
            "full retrieval is mandatory for packet semantic batch",
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
            return Err(sidecar_retrieval_unavailable_error(self, reason));
        }
        Err(sidecar_retrieval_unavailable_error(
            self,
            "full retrieval is mandatory for packet subquery warmup",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_batch_preserves_publication_changed_for_operation_retry() {
        let error = packet_batch_error(
            &AppController::new(),
            ApiError::new("publication_changed", "generation drift"),
            "packet batch",
        );

        assert_eq!(error.code, "publication_changed");
        assert_eq!(error.message, "generation drift");
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn cleared(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: test-only env cleanup under the shared process env lock.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: restores the process-local env var captured by this guard.
            unsafe {
                if let Some(previous) = self.previous.take() {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[test]
    fn packet_subquery_warmup_fails_closed_without_sidecar_primary() {
        let _lock = crate::process_env_test_lock();
        let _retrieval_env = EnvVarGuard::cleared("CODESTORY_RETRIEVAL");
        let controller = AppController::new_with_config(crate::test_sidecar_runtime_from_env());

        let error = controller
            .warm_packet_subquery_embeddings(&["run_exec_session".to_string()])
            .expect_err("packet warmup must not fall back to the legacy in-process search engine");

        assert!(
            error.message.contains("retrieval requires an open project"),
            "warmup should report the mandatory retrieval gate, got: {}",
            error.message
        );
        assert_eq!(error.code, "retrieval_unavailable");
        let details = error.details.expect("retrieval error details");
        assert_eq!(details.failed_layer.as_deref(), Some("retrieval_engine"));
        assert!(
            !details.next_commands.is_empty(),
            "warmup should include recovery commands"
        );
    }
}
