//! The one wait every qualification operation that watches a server exit
//! shares: poll the owner until a completed observation finds it gone.
//!
//! The observer and the exit it observes are the same event seen from two
//! sides. `true_idle_respawn` freezes
//! `idle_connections_and_diagnostics_do_not_extend_idle`, so the server is
//! *required* to leave while observers are connected to it: `true_idle()`
//! ignores connection count, the accept loop breaks on the idle boundary at the
//! top of a tick, and the listener then closes with whatever the kernel had
//! already queued or a handler had half-answered. A client polling for that exit
//! therefore has a real third outcome besides "still there" and "gone": the
//! connection it was using went away underneath it. The product's own call paths
//! already treat that as the transient it is and reconnect (`is_server_loss` ->
//! replay); only these qualification waits treated it as fatal, which is how a
//! run measuring an exit could be failed *by* the exit it was measuring
//! (`embedding_qualification_measure_worker_failed:measure_true_idle:embedding_server_connection_lost`,
//! macOS calibration attempt 14, sample 2 of 3, sample 1 having measured the
//! same window cleanly).
//!
//! Tolerating the loss does not soften what the wait proves. A lost connection
//! is not absence -- a crashed server, a killed server and an idle exit are
//! indistinguishable at that instant -- so it never ends the wait and never
//! stamps the declared `engine_and_server_absent` instant. Only a completed
//! observation that finds no owner does, exactly as before. A loss just costs
//! one more poll, and the surrounding budget still bounds the whole wait, so a
//! server that keeps dropping connections without ever leaving still fails,
//! under its own name.

use super::super::gate::{POLL, elapsed};
use anyhow::{Result, bail};
use codestory_retrieval::{AwakeMonotonicClock, PerUserEmbeddingClient, embedding_retry_state};
use std::time::Duration;

/// The typed error code the transport raises when an authenticated connection
/// to the owner is lost mid-exchange, which is what an exiting server looks
/// like from a client that was mid-observation.
const CONNECTION_LOST_CODE: &str = "embedding_server_connection_lost";

/// One poll of the owner during a wait for its exit.
pub(in crate::embedding_qualification::worker) enum OwnerExitObservation {
    /// A completed snapshot exchange, carrying the answering server's instance
    /// id so the caller can reject a replacement owner.
    Present(String),
    /// The connection to the owner was lost mid-observation. Not absence: the
    /// next poll re-observes and either finds the owner still there or proves
    /// it gone.
    Lost,
    /// A completed observation found no owner. This is the only outcome that
    /// proves `engine_and_server_absent`.
    Absent,
}

/// Observe the owner once, separating a lost connection from proven absence.
/// Every other error is propagated untouched: an unresponsive owner, a protocol
/// mismatch or a rejected identity are not exits and must not be waited out.
pub(in crate::embedding_qualification::worker) fn observe_owner_exit(
    client: &PerUserEmbeddingClient,
) -> Result<OwnerExitObservation> {
    match client.observe() {
        Ok(None) => Ok(OwnerExitObservation::Absent),
        Ok(Some(snapshot)) => Ok(OwnerExitObservation::Present(
            snapshot.process.server_instance_id,
        )),
        Err(error) => classify_failed_observation(error),
    }
}

/// Decide whether a failed observation is the exit signature this wait exists
/// to see through. Only the transport's typed connection-loss code qualifies;
/// everything else stays an error, so an unresponsive owner or a rejected
/// identity can never be absorbed as "the server was probably leaving".
fn classify_failed_observation(error: anyhow::Error) -> Result<OwnerExitObservation> {
    if is_connection_lost(&error) {
        return Ok(OwnerExitObservation::Lost);
    }
    Err(error)
}

fn is_connection_lost(error: &anyhow::Error) -> bool {
    embedding_retry_state(error).is_some_and(|retry| retry.code == CONNECTION_LOST_CODE)
}

/// Poll `observe` until the pinned owner is proven gone.
///
/// Returns only on a completed observation that found no owner. A replacement
/// owner fails the wait, because an exit that has already been followed by a
/// respawn can no longer be attributed to the pinned server. The timeout is the
/// caller's and is checked every poll, including the polls a lost connection
/// costs, and it names whether any loss was tolerated so a server that drops
/// connections without leaving cannot be reported as a silent slow exit.
pub(in crate::embedding_qualification::worker) fn wait_for_owner_exit(
    clock: &dyn AwakeMonotonicClock,
    started_ns: u64,
    timeout: Duration,
    owner_instance_id: &str,
    mut observe: impl FnMut() -> Result<OwnerExitObservation>,
) -> Result<()> {
    let mut tolerated_connection_loss = false;
    loop {
        match observe()? {
            OwnerExitObservation::Absent => return Ok(()),
            OwnerExitObservation::Lost => tolerated_connection_loss = true,
            OwnerExitObservation::Present(instance_id) if instance_id != owner_instance_id => {
                bail!("embedding_qualification_owner_changed_before_absence")
            }
            OwnerExitObservation::Present(_) => {}
        }
        if elapsed(clock, started_ns) >= timeout {
            if tolerated_connection_loss {
                bail!("embedding_qualification_owner_exit_timeout_after_connection_loss");
            }
            bail!("embedding_qualification_owner_exit_timeout");
        }
        clock.sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_retrieval::{EmbeddingServerClockSnapshot, PerUserEmbeddingError};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A clock whose only motion is the sleep the wait itself performs, so a
    /// test spends the declared budget without spending wall time.
    struct StepClock {
        now_ns: AtomicU64,
    }

    impl StepClock {
        fn new() -> Self {
            Self {
                now_ns: AtomicU64::new(0),
            }
        }
    }

    impl AwakeMonotonicClock for StepClock {
        fn now_ns(&self) -> u64 {
            self.now_ns.load(Ordering::Acquire)
        }

        fn sleep(&self, duration: Duration) {
            self.now_ns
                .fetch_add(duration.as_nanos() as u64, Ordering::AcqRel);
        }

        fn snapshot(&self) -> EmbeddingServerClockSnapshot {
            EmbeddingServerClockSnapshot {
                domain: "awake_monotonic".into(),
                api: "test_step_clock".into(),
                boot_id: "test-boot".into(),
                resolution_ns: 1,
            }
        }
    }

    fn connection_lost_error() -> anyhow::Error {
        anyhow::anyhow!("read frame").context(PerUserEmbeddingError {
            code: CONNECTION_LOST_CODE.into(),
            message: "the authenticated embedding server connection was lost".into(),
            retry_class: "same_rpc_once".into(),
            retry_after_ms: 0,
            retry_condition: "the server instance changes".into(),
            capacity: None,
        })
    }

    fn unresponsive_error() -> anyhow::Error {
        anyhow::anyhow!("read frame").context(PerUserEmbeddingError {
            code: "embedding_server_owner_unresponsive".into(),
            message: "the embedding server did not complete a bounded exchange".into(),
            retry_class: "after_server_change".into(),
            retry_after_ms: 0,
            retry_condition: "the lifetime authority or server instance changes".into(),
            capacity: None,
        })
    }

    fn scripted(
        steps: Vec<Result<OwnerExitObservation>>,
    ) -> impl FnMut() -> Result<OwnerExitObservation> {
        let steps = RefCell::new(steps.into_iter());
        move || {
            steps
                .borrow_mut()
                .next()
                .unwrap_or_else(|| panic!("the wait polled past its script"))
        }
    }

    fn wait(
        clock: &StepClock,
        steps: Vec<Result<OwnerExitObservation>>,
        timeout: Duration,
    ) -> Result<()> {
        wait_for_owner_exit(clock, clock.now_ns(), timeout, "owner-1", scripted(steps))
    }

    /// The regression: the exit this wait exists to observe reaches the client
    /// as a lost connection, and the wait must keep polling until the exit is
    /// actually proven rather than failing on the loss.
    #[test]
    fn a_connection_lost_at_the_exit_does_not_fail_the_wait() {
        let clock = StepClock::new();
        let result = wait(
            &clock,
            vec![
                Ok(OwnerExitObservation::Present("owner-1".into())),
                Ok(OwnerExitObservation::Lost),
                Ok(OwnerExitObservation::Absent),
            ],
            Duration::from_secs(90),
        );
        assert!(result.is_ok(), "{result:?}");
    }

    /// A lost connection is not absence. The wait must end on the completed
    /// observation that found no owner, never on the loss itself.
    #[test]
    fn a_lost_connection_alone_never_proves_absence() {
        let clock = StepClock::new();
        let error = wait(
            &clock,
            vec![
                Ok(OwnerExitObservation::Lost),
                Ok(OwnerExitObservation::Lost),
                Ok(OwnerExitObservation::Lost),
                Ok(OwnerExitObservation::Lost),
            ],
            // Three polls of headroom, so the fourth script entry is only
            // reachable if a loss were mistaken for absence-with-slack.
            POLL.saturating_mul(3),
        );
        assert_eq!(
            error.unwrap_err().to_string(),
            "embedding_qualification_owner_exit_timeout_after_connection_loss"
        );
    }

    /// A server that never leaves still fails, and the timeout keeps its
    /// original name when nothing was tolerated on the way there.
    #[test]
    fn a_resident_owner_still_fails_the_wait_under_its_own_name() {
        let clock = StepClock::new();
        let error = wait(
            &clock,
            vec![
                Ok(OwnerExitObservation::Present("owner-1".into())),
                Ok(OwnerExitObservation::Present("owner-1".into())),
            ],
            POLL,
        );
        assert_eq!(
            error.unwrap_err().to_string(),
            "embedding_qualification_owner_exit_timeout"
        );
    }

    /// Tolerating the loss must not tolerate a respawn: an exit already
    /// followed by a replacement owner is no longer attributable to the pinned
    /// server.
    #[test]
    fn a_replacement_owner_after_a_loss_still_fails_the_wait() {
        let clock = StepClock::new();
        let error = wait(
            &clock,
            vec![
                Ok(OwnerExitObservation::Lost),
                Ok(OwnerExitObservation::Present("owner-2".into())),
            ],
            Duration::from_secs(90),
        );
        assert_eq!(
            error.unwrap_err().to_string(),
            "embedding_qualification_owner_changed_before_absence"
        );
    }

    /// Only the exit signature is waited out. Anything else the observation can
    /// fail with is still terminal.
    #[test]
    fn a_non_exit_observation_failure_is_still_terminal() {
        let clock = StepClock::new();
        let error = wait(
            &clock,
            vec![Err(unresponsive_error())],
            Duration::from_secs(90),
        );
        assert_eq!(
            embedding_retry_state(&error.unwrap_err())
                .expect("typed transport error")
                .code,
            "embedding_server_owner_unresponsive"
        );
    }

    /// The other half of the fix: a failed observation carrying the transport's
    /// connection-loss code becomes a poll outcome rather than an error, and
    /// nothing else does.
    #[test]
    fn a_failed_observation_is_a_lost_connection_only_when_the_transport_says_so() {
        assert!(matches!(
            classify_failed_observation(connection_lost_error()),
            Ok(OwnerExitObservation::Lost)
        ));
        assert!(classify_failed_observation(unresponsive_error()).is_err());
    }

    /// The classifier reads the transport's typed code, not error prose, so a
    /// message that merely mentions the code cannot buy tolerance.
    #[test]
    fn only_the_typed_code_classifies_as_a_lost_connection() {
        assert!(is_connection_lost(&connection_lost_error()));
        assert!(!is_connection_lost(&unresponsive_error()));
        assert!(!is_connection_lost(&anyhow::anyhow!(
            "embedding_server_connection_lost"
        )));
    }
}
