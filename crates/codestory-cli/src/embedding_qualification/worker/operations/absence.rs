use super::super::gate::{POLL, elapsed, qualification_request_id};
use anyhow::{Result, bail};
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingQualificationOperationResult, EmbeddingQualificationResult,
    PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS, PerUserEmbeddingClient, SidecarRuntimeConfig,
};
use std::time::Duration;

const OWNER_ABSENCE_GRACE: Duration = Duration::from_secs(30);

pub(in crate::embedding_qualification::worker) fn wait_for_owner_absence(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
) -> Result<EmbeddingQualificationResult> {
    let client = PerUserEmbeddingClient::for_runtime(runtime)?;
    let started_ns = clock.now_ns();
    let initial_snapshot = client.observe()?;
    let timeout = Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
        .saturating_add(OWNER_ABSENCE_GRACE);
    if let Some(initial) = initial_snapshot.as_ref() {
        loop {
            match client.observe()? {
                None => break,
                Some(snapshot)
                    if snapshot.process.server_instance_id
                        != initial.process.server_instance_id =>
                {
                    bail!("embedding_qualification_owner_changed_before_absence")
                }
                Some(_) => {}
            }
            if elapsed(clock, started_ns) >= timeout {
                bail!("embedding_qualification_owner_exit_timeout");
            }
            clock.sleep(POLL);
        }
    }
    let completed_ns = clock.now_ns();
    Ok(EmbeddingQualificationResult {
        schema_version: 1,
        scenario: "wait_for_absence".into(),
        started_ns,
        finished_ns: completed_ns,
        operations: vec![EmbeddingQualificationOperationResult {
            correlation_id: qualification_request_id("wait-for-absence", started_ns),
            class: "observe".into(),
            submitted_ns: started_ns,
            completed_ns,
            status: "ok".into(),
            error_code: None,
            server_instance_id: initial_snapshot
                .as_ref()
                .map(|snapshot| snapshot.process.server_instance_id.clone()),
            load_generation: initial_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.engine.as_ref())
                .map(|engine| engine.load_generation),
            attempts: Vec::new(),
        }],
        initial_snapshot,
        final_snapshot: None,
    })
}
