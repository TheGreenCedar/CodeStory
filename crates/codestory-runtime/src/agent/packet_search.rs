//! AppController batch search paths for packet retrieval.

use crate::AppController;
use crate::agent::retrieval_primary::{
    packet_batch_should_use_sidecar, search_sidecar_packet_batch,
    sidecar_retrieval_unavailable_error, sidecar_retrieval_unavailable_reason,
};
use codestory_contracts::api::{ApiError, PacketSidecarQueryDiagnosticDto, SearchHit};

#[derive(Debug)]
pub(crate) struct PacketFusedBatchOutcome {
    pub results: Vec<(String, Vec<SearchHit>)>,
    pub retryable_queries: Vec<String>,
    pub sidecar_diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
}

fn packet_batch_error(controller: &AppController, error: ApiError, context: &str) -> ApiError {
    if matches!(error.code.as_str(), "publication_changed" | "cancelled") {
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
    pub(crate) fn search_packet_fused_batch(
        &self,
        queries: &[(String, usize)],
        latency_budget_ms: Option<u32>,
    ) -> Result<PacketFusedBatchOutcome, ApiError> {
        if queries.is_empty() {
            return Ok(PacketFusedBatchOutcome {
                results: Vec::new(),
                retryable_queries: Vec::new(),
                sidecar_diagnostics: Vec::new(),
            });
        }
        if packet_batch_should_use_sidecar(self) {
            match search_sidecar_packet_batch(self, queries, latency_budget_ms) {
                Ok(outcome) => {
                    return Ok(PacketFusedBatchOutcome {
                        results: outcome.results,
                        retryable_queries: outcome.retryable_queries,
                        sidecar_diagnostics: outcome.diagnostics,
                    });
                }
                Err(error) => {
                    tracing::warn!(
                        "sidecar retrieval packet fused batch unavailable; fail-closed: {}",
                        error.message
                    );
                    return Err(packet_batch_error(
                        self,
                        error,
                        "sidecar retrieval packet fused batch unavailable",
                    ));
                }
            }
        } else if let Some(reason) = sidecar_retrieval_unavailable_reason(self) {
            return Err(sidecar_retrieval_unavailable_error(self, reason));
        }
        Err(sidecar_retrieval_unavailable_error(
            self,
            "full retrieval is mandatory for packet fused batch",
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

    #[test]
    fn packet_batch_preserves_public_cancellation() {
        let error = packet_batch_error(
            &AppController::new(),
            ApiError::new("cancelled", "request cancelled"),
            "packet batch",
        );

        assert_eq!(error.code, "cancelled");
        assert_eq!(error.message, "request cancelled");
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
    fn packet_fused_batch_fails_closed_without_sidecar_primary() {
        let _lock = crate::process_env_test_lock();
        let _retrieval_env = EnvVarGuard::cleared("CODESTORY_RETRIEVAL");
        let controller = AppController::new_with_config(crate::test_sidecar_runtime_from_env());

        let error = controller
            .search_packet_fused_batch(&[("run_exec_session".to_string(), 5)], None)
            .expect_err("packet fused batch must not fall back to legacy in-process search");

        assert!(
            error.message.contains("retrieval requires an open project"),
            "fused batch should report the mandatory retrieval gate, got: {}",
            error.message
        );
        assert_eq!(error.code, "retrieval_unavailable");
        let details = error.details.expect("retrieval error details");
        assert_eq!(details.failed_layer.as_deref(), Some("retrieval_engine"));
        assert!(
            !details.next_commands.is_empty(),
            "fused batch should include recovery commands"
        );
    }
}
