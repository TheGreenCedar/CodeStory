//! AppController batch search paths for packet retrieval.

use crate::AppController;
use crate::agent::packet_candidate::PacketSearchHit;
use crate::agent::retrieval_primary::{
    packet_batch_should_use_sidecar, search_sidecar_packet_batch,
    sidecar_retrieval_unavailable_error, sidecar_retrieval_unavailable_reason,
};
use codestory_contracts::api::{ApiError, PacketSidecarQueryDiagnosticDto};
use std::time::{Duration, Instant};

const PACKET_TRANSIENT_RETRY_MAX_DELAY_MS: u64 = 250;
const PACKET_TRANSIENT_RETRY_MIN_BUDGET_MS: u32 = 1_000;

#[derive(Debug)]
pub(crate) struct PacketFusedBatchOutcome {
    pub results: Vec<(String, Vec<PacketSearchHit>)>,
    pub retryable_queries: Vec<String>,
    pub sidecar_diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
}

fn packet_batch_error(controller: &AppController, error: ApiError, context: &str) -> ApiError {
    if matches!(
        error.code.as_str(),
        "embedding_capacity"
            | "embedding_retryable"
            | "cache_busy"
            | "publication_changed"
            | "cancelled"
    ) {
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

fn search_packet_batch_with_one_transient_retry<T>(
    latency_budget_ms: Option<u32>,
    mut search: impl FnMut(Option<u32>) -> Result<T, ApiError>,
    mut wait: impl FnMut(Duration),
) -> Result<T, ApiError> {
    // A packet owns one fresh-request retry for typed transient embedding pressure. The retry
    // remains inside the caller's deadline and never turns another failure into a repair loop.
    let started_at = Instant::now();
    let first_error = match search(latency_budget_ms) {
        Ok(outcome) => return Ok(outcome),
        Err(error) => error,
    };
    let Some(delay) = packet_batch_retry_delay(&first_error) else {
        return Err(first_error);
    };

    let projected_elapsed_ms =
        elapsed_ms(started_at).saturating_add(u32::try_from(delay.as_millis()).unwrap_or(u32::MAX));
    if latency_budget_ms.is_some_and(|budget| {
        budget.saturating_sub(projected_elapsed_ms) < PACKET_TRANSIENT_RETRY_MIN_BUDGET_MS
    }) {
        return Err(first_error);
    }

    wait(delay);
    let retry_budget_ms = latency_budget_ms
        .map(|budget| budget.saturating_sub(elapsed_ms(started_at).max(projected_elapsed_ms)));
    if retry_budget_ms.is_some_and(|budget| budget < PACKET_TRANSIENT_RETRY_MIN_BUDGET_MS) {
        return Err(first_error);
    }
    search(retry_budget_ms)
}

fn packet_batch_retry_delay(error: &ApiError) -> Option<Duration> {
    if !matches!(
        error.code.as_str(),
        "embedding_capacity" | "embedding_retryable"
    ) {
        return None;
    }
    let retry = error.details.as_ref()?.embedding_retry.as_ref()?;
    if !matches!(
        retry.retry_class.as_str(),
        "after_capacity_change" | "after_delay"
    ) || retry.retry_after_ms > PACKET_TRANSIENT_RETRY_MAX_DELAY_MS
    {
        return None;
    }
    Some(Duration::from_millis(retry.retry_after_ms))
}

fn elapsed_ms(started_at: Instant) -> u32 {
    u32::try_from(started_at.elapsed().as_millis()).unwrap_or(u32::MAX)
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
            match search_packet_batch_with_one_transient_retry(
                latency_budget_ms,
                |remaining_ms| search_sidecar_packet_batch(self, queries, remaining_ms),
                std::thread::sleep,
            ) {
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

    fn retryable_error(retry_class: &str, retry_after_ms: u64) -> ApiError {
        ApiError::embedding_retry(
            "embedding_retryable",
            "transient embedding failure",
            codestory_contracts::api::EmbeddingRetryStateDto {
                code: "embedding_deadline_exceeded".to_owned(),
                retry_class: retry_class.to_owned(),
                retry_after_ms,
                retry_condition: "a fresh bounded request".to_owned(),
                capacity: None,
            },
        )
    }

    #[test]
    fn packet_batch_retries_one_typed_transient_failure() {
        let mut attempts = 0;
        let mut waits = Vec::new();
        let result = search_packet_batch_with_one_transient_retry(
            Some(2_000),
            |_| {
                attempts += 1;
                if attempts == 1 {
                    Err(retryable_error("after_delay", 0))
                } else {
                    Ok("recovered")
                }
            },
            |delay| waits.push(delay),
        )
        .expect("fresh bounded request recovers");

        assert_eq!(result, "recovered");
        assert_eq!(attempts, 2);
        assert_eq!(waits, [std::time::Duration::ZERO]);
    }

    #[test]
    fn packet_batch_waits_for_typed_capacity_before_its_single_retry() {
        let mut attempts = 0;
        let mut waits = Vec::new();
        let result = search_packet_batch_with_one_transient_retry(
            Some(2_000),
            |_| {
                attempts += 1;
                if attempts == 1 {
                    Err(ApiError::embedding_capacity(
                        "embedding connection admission is full",
                        codestory_contracts::api::EmbeddingCapacityPressureDto {
                            reason: "pre_request_full".to_owned(),
                            queue_class: "connection".to_owned(),
                            capacity: 8,
                            depth: 8,
                            retry_after_ms: 40,
                            retry_condition: "an authenticated handler completes".to_owned(),
                            owner_state: "ready".to_owned(),
                            active_scope_id: None,
                            active_request_id: None,
                            active_request_class: None,
                        },
                    ))
                } else {
                    Ok("recovered")
                }
            },
            |delay| waits.push(delay),
        )
        .expect("capacity drains before the fresh bounded request");

        assert_eq!(result, "recovered");
        assert_eq!(attempts, 2);
        assert_eq!(waits, [std::time::Duration::from_millis(40)]);
    }

    #[test]
    fn packet_batch_does_not_retry_past_its_deadline_or_more_than_once() {
        let mut deadline_attempts = 0;
        let deadline_error = search_packet_batch_with_one_transient_retry(
            Some(100),
            |_| {
                deadline_attempts += 1;
                Err::<(), _>(retryable_error("after_capacity_change", 40))
            },
            |_| panic!("deadline-exhausted batch must not wait"),
        )
        .expect_err("insufficient deadline preserves the typed failure");
        assert_eq!(deadline_error.code, "embedding_retryable");
        assert_eq!(deadline_attempts, 1);

        let mut bounded_attempts = 0;
        let bounded_error = search_packet_batch_with_one_transient_retry(
            Some(2_000),
            |_| {
                bounded_attempts += 1;
                Err::<(), _>(retryable_error("after_capacity_change", 0))
            },
            |_| {},
        )
        .expect_err("the second typed failure is final");
        assert_eq!(bounded_error.code, "embedding_retryable");
        assert_eq!(bounded_attempts, 2);
    }

    #[test]
    fn packet_batch_does_not_retry_unapproved_retry_classes() {
        let mut attempts = 0;
        let error = search_packet_batch_with_one_transient_retry(
            Some(2_000),
            |_| {
                attempts += 1;
                Err::<(), _>(retryable_error("after_server_change", 0))
            },
            |_| panic!("unapproved retry class must not wait"),
        )
        .expect_err("unapproved retry remains public");

        assert_eq!(error.code, "embedding_retryable");
        assert_eq!(attempts, 1);
    }

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

    #[test]
    fn packet_batch_preserves_embedding_capacity_without_reindex_advice() {
        let error = packet_batch_error(
            &AppController::new(),
            ApiError::embedding_capacity(
                "embedding connection admission is full",
                codestory_contracts::api::EmbeddingCapacityPressureDto {
                    reason: "connection_limit".to_string(),
                    queue_class: "packet".to_string(),
                    capacity: 1,
                    depth: 1,
                    retry_after_ms: 25,
                    retry_condition: "after_capacity_change".to_string(),
                    owner_state: "busy".to_string(),
                    active_scope_id: Some("project-a".to_string()),
                    active_request_id: Some("packet-a".to_string()),
                    active_request_class: Some("packet".to_string()),
                },
            ),
            "packet batch",
        );

        assert_eq!(error.code, "embedding_capacity");
        let details = error.details.expect("typed capacity details");
        assert!(details.next_commands.is_empty());
        assert!(details.minimum_next.is_empty());
        assert!(details.full_repair.is_empty());
        assert_eq!(
            details
                .embedding_retry
                .as_ref()
                .map(|retry| retry.retry_after_ms),
            Some(25)
        );
        assert_eq!(
            details
                .embedding_capacity
                .as_ref()
                .map(|pressure| pressure.owner_state.as_str()),
            Some("busy")
        );
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
