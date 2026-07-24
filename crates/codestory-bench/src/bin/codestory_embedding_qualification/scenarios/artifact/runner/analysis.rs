use super::super::{
    MIXED_QUEUE_COUNT, MIXED_QUEUE_PROJECT_COUNT, QUALIFICATION_QUEUE_CAPACITY, RawServerIdentity,
    btree,
};
use super::process::existing_control_events;
use super::{ControlEvent, WorkerQueueOperation};
use crate::qualification::request::{read_private_request, sha256_bytes, validate_direct_child};
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingEngineIdentity, EmbeddingQualificationWatchdogMarker,
    EmbeddingServerSnapshot, PER_USER_EMBEDDING_HARD_NATIVE_NO_PROGRESS_MS,
    PER_USER_EMBEDDING_WATCHDOG_CADENCE_MS, SidecarRuntimeConfig,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::Path;
use std::time::Duration;

pub(super) fn scheduler_values(snapshot: &EmbeddingServerSnapshot) -> BTreeMap<String, Value> {
    btree([
        ("query_capacity", json!(snapshot.scheduler.query_capacity)),
        ("query_depth", json!(snapshot.scheduler.query_depth)),
        ("bulk_capacity", json!(snapshot.scheduler.bulk_capacity)),
        ("bulk_depth", json!(snapshot.scheduler.bulk_depth)),
        (
            "active_request_count",
            json!(snapshot.scheduler.active_request_count),
        ),
        ("lease_count", json!(snapshot.scheduler.lease_count)),
        (
            "active_request_class",
            json!(
                snapshot
                    .scheduler
                    .active_request
                    .as_ref()
                    .map(|active| active.class.as_str())
            ),
        ),
    ])
}

pub(super) struct QueueAnalysis {
    pub(super) capacity: BTreeMap<String, Value>,
    pub(super) class_orders: BTreeMap<String, Value>,
    pub(super) project_orders: BTreeMap<String, Value>,
    pub(super) query_preference: BTreeMap<String, Value>,
    pub(super) bulk_resumption: BTreeMap<String, Value>,
}

pub(super) fn require_pre_release_capacity_overflow(
    operations: &[WorkerQueueOperation],
) -> Result<()> {
    if operations.len() != 2 {
        bail!("embedding_qualification_pre_release_overflow_count_invalid");
    }
    for class in ["query", "bulk"] {
        let class_operations = operations
            .iter()
            .filter(|operation| operation.class == class)
            .collect::<Vec<_>>();
        if class_operations.len() != 1 {
            bail!("embedding_qualification_pre_release_overflow_class_invalid:{class}");
        }
        let operation = class_operations[0];
        let pressure = operation
            .error
            .as_ref()
            .and_then(|error| error.capacity.as_ref())
            .ok_or_else(|| {
                anyhow::anyhow!("embedding_qualification_pre_release_overflow_untyped:{class}")
            })?;
        if operation.status != "failed"
            || operation.response_payload_bytes != 0
            || operation
                .error
                .as_ref()
                .is_none_or(|error| error.code != "embedding_capacity")
            || pressure.reason != "queue_full"
            || pressure.queue_class != class
            || pressure.capacity != QUALIFICATION_QUEUE_CAPACITY
            || pressure.depth != pressure.capacity
            || pressure.retry_condition.trim().is_empty()
        {
            bail!("embedding_qualification_pre_release_overflow_invalid:{class}");
        }
    }
    Ok(())
}

pub(super) fn analyze_queue_operations(
    operations: &[WorkerQueueOperation],
) -> Result<QueueAnalysis> {
    let first = operations
        .first()
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_queue_operations_missing"))?;
    if operations.iter().any(|operation| {
        !same_server_authority(&first.hello_snapshot, &operation.hello_snapshot)
            || (operation.status == "ok"
                && (operation.error.is_some()
                    || operation.response_payload_bytes == 0
                    || operation.native_completion_sequence.is_none()))
            || (operation.status == "failed"
                && (operation.error.is_none()
                    || operation.response_payload_bytes != 0
                    || operation.native_completion_sequence.is_some()))
            || !matches!(operation.status.as_str(), "ok" | "failed")
    }) {
        bail!("embedding_qualification_queue_operation_identity_invalid");
    }
    let mut observed_native_completion_sequences = BTreeSet::new();
    for operation in operations
        .iter()
        .filter(|operation| operation.status == "ok")
    {
        let sequence = operation.native_completion_sequence.unwrap_or_default();
        if sequence == 0 || !observed_native_completion_sequences.insert(sequence) {
            bail!("embedding_qualification_native_completion_sequence_invalid");
        }
    }
    let mut capacity = BTreeMap::new();
    let mut class_orders = BTreeMap::new();
    let mut project_orders = BTreeMap::new();
    let mut completed_by_class = BTreeMap::<&str, Vec<&WorkerQueueOperation>>::new();
    for class in ["query", "bulk"] {
        let class_operations = operations
            .iter()
            .filter(|operation| operation.class == class)
            .collect::<Vec<_>>();
        if class_operations.len() != MIXED_QUEUE_COUNT as usize {
            bail!("embedding_qualification_queue_operation_count_invalid:{class}");
        }
        let failures = class_operations
            .iter()
            .copied()
            .filter(|operation| operation.status == "failed")
            .collect::<Vec<_>>();
        if failures.len() != 1 {
            bail!("embedding_qualification_queue_capacity_failure_count:{class}");
        }
        let pressure = failures[0]
            .error
            .as_ref()
            .and_then(|error| error.capacity.as_ref())
            .ok_or_else(|| {
                anyhow::anyhow!("embedding_qualification_queue_capacity_untyped:{class}")
            })?;
        if pressure.queue_class != class
            || pressure.capacity != QUALIFICATION_QUEUE_CAPACITY
            || pressure.depth != pressure.capacity
            || pressure.retry_condition.trim().is_empty()
            || failures[0].submission_batch != 2
        {
            bail!("embedding_qualification_queue_capacity_contract_invalid:{class}");
        }
        capacity.insert(
            format!("{class}_65th"),
            json!({
                "correlation_id": failures[0].correlation_id,
                "error": failures[0].error,
                "submitted_ns": failures[0].submitted_ns,
                "completed_ns": failures[0].completed_ns,
            }),
        );
        let mut expected_queue_insertion = class_operations
            .iter()
            .copied()
            .filter(|operation| operation.status == "ok")
            .collect::<Vec<_>>();
        expected_queue_insertion.sort_by_key(|operation| {
            (
                operation.submission_batch,
                operation.ordinal,
                &operation.correlation_id,
            )
        });
        let mut expected_batch_projects = Vec::new();
        for submission_batch in 0..2 {
            let batch_operations = expected_queue_insertion
                .iter()
                .copied()
                .filter(|operation| operation.submission_batch == submission_batch)
                .collect::<Vec<_>>();
            if batch_operations.len() != MIXED_QUEUE_PROJECT_COUNT as usize
                || batch_operations
                    .iter()
                    .map(|operation| operation.ordinal)
                    .collect::<Vec<_>>()
                    != (0..MIXED_QUEUE_PROJECT_COUNT).collect::<Vec<_>>()
            {
                bail!("embedding_qualification_queue_insertion_order_invalid:{class}");
            }
            let projects = batch_operations
                .iter()
                .map(|operation| operation.project_identity_sha256.as_str())
                .collect::<BTreeSet<_>>();
            if projects.len() != 1 {
                bail!("embedding_qualification_queue_project_batch_invalid:{class}");
            }
            let project = projects.into_iter().next().ok_or_else(|| {
                anyhow::anyhow!("embedding_qualification_queue_project_batch_invalid:{class}")
            })?;
            expected_batch_projects.push(project);
        }
        if expected_batch_projects[0] == expected_batch_projects[1] {
            bail!("embedding_qualification_queue_project_batches_not_independent:{class}");
        }
        let mut completed = expected_queue_insertion.clone();
        completed.sort_by_key(|operation| {
            (
                operation.native_completion_sequence.unwrap_or_default(),
                &operation.correlation_id,
            )
        });
        let expected_queue_insertion_ids = expected_queue_insertion
            .iter()
            .map(|operation| operation.correlation_id.clone())
            .collect::<Vec<_>>();
        let native_completed_ids = completed
            .iter()
            .map(|operation| operation.correlation_id.clone())
            .collect::<Vec<_>>();
        if expected_queue_insertion_ids != native_completed_ids {
            bail!("embedding_qualification_queue_fifo_violation:{class}");
        }
        let expected_queue_insertion_projects = expected_queue_insertion
            .iter()
            .map(|operation| operation.project_identity_sha256.clone())
            .collect::<Vec<_>>();
        let native_completed_projects = completed
            .iter()
            .map(|operation| operation.project_identity_sha256.clone())
            .collect::<Vec<_>>();
        if expected_queue_insertion_projects != native_completed_projects
            || expected_queue_insertion_projects
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != 2
        {
            bail!("embedding_qualification_queue_scope_order_invalid:{class}");
        }
        let native_completion_sequences = completed
            .iter()
            .map(|operation| operation.native_completion_sequence.unwrap_or_default())
            .collect::<Vec<_>>();
        class_orders.insert(
            format!("{class}_expected_queue_insertion_request_ids"),
            json!(expected_queue_insertion_ids),
        );
        class_orders.insert(
            format!("{class}_native_completed_request_ids"),
            json!(native_completed_ids),
        );
        class_orders.insert(
            format!("{class}_native_completion_sequences"),
            json!(native_completion_sequences),
        );
        project_orders.insert(
            format!("{class}_expected_queue_insertion_project_identities"),
            json!(expected_queue_insertion_projects),
        );
        project_orders.insert(
            format!("{class}_native_completed_project_identities"),
            json!(native_completed_projects),
        );
        completed_by_class.insert(class, completed);
    }
    let queries = &completed_by_class["query"];
    let bulks = &completed_by_class["bulk"];
    let first_query = queries
        .first()
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_query_completion_missing"))?;
    let first_bulk = bulks
        .first()
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_bulk_completion_missing"))?;
    let first_query_native_completion_sequence =
        first_query.native_completion_sequence.unwrap_or_default();
    let first_bulk_native_completion_sequence =
        first_bulk.native_completion_sequence.unwrap_or_default();
    if first_query_native_completion_sequence >= first_bulk_native_completion_sequence {
        bail!("embedding_qualification_query_preference_missing");
    }
    let last_query = queries.last().expect("non-empty query completions");
    let last_bulk = bulks.last().expect("non-empty bulk completions");
    let last_query_native_completion_sequence =
        last_query.native_completion_sequence.unwrap_or_default();
    let last_bulk_native_completion_sequence =
        last_bulk.native_completion_sequence.unwrap_or_default();
    if last_bulk_native_completion_sequence <= last_query_native_completion_sequence {
        bail!("embedding_qualification_bulk_resumption_missing");
    }
    Ok(QueueAnalysis {
        capacity,
        class_orders,
        project_orders,
        query_preference: btree([
            ("first_query_request_id", json!(first_query.correlation_id)),
            (
                "first_query_native_completion_sequence",
                json!(first_query_native_completion_sequence),
            ),
            ("first_bulk_request_id", json!(first_bulk.correlation_id)),
            (
                "first_bulk_native_completion_sequence",
                json!(first_bulk_native_completion_sequence),
            ),
        ]),
        bulk_resumption: btree([
            ("last_query_request_id", json!(last_query.correlation_id)),
            (
                "last_query_native_completion_sequence",
                json!(last_query_native_completion_sequence),
            ),
            ("last_bulk_request_id", json!(last_bulk.correlation_id)),
            (
                "last_bulk_native_completion_sequence",
                json!(last_bulk_native_completion_sequence),
            ),
        ]),
    })
}

pub(super) fn same_server_authority(
    first: &EmbeddingServerSnapshot,
    second: &EmbeddingServerSnapshot,
) -> bool {
    first.process.server_instance_id == second.process.server_instance_id
        && first.process.pid == second.process.pid
        && first.process.process_start_id == second.process.process_start_id
        && first.authority.lifetime_authority_id == second.authority.lifetime_authority_id
        && first.authority.listener_id == second.authority.listener_id
}

pub(super) fn control_key(action: &str, class: Option<&str>) -> String {
    class.map_or_else(|| action.into(), |class| format!("{action}:{class}"))
}

pub(super) fn validated_idle_epoch(
    event: &ControlEvent,
    snapshot: &EmbeddingServerSnapshot,
) -> Result<u64> {
    let details = event
        .details
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_idle_epoch_missing"))?;
    let expected = BTreeSet::from([
        "idle_epoch_ns",
        "true_idle",
        "clock_domain",
        "clock_boot_id",
        "server_instance_id",
    ]);
    if details.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected
        || details.get("true_idle").map(String::as_str) != Some("true")
        || details.get("clock_domain") != Some(&snapshot.clock.domain)
        || details.get("clock_boot_id") != Some(&snapshot.clock.boot_id)
        || details.get("server_instance_id") != Some(&snapshot.process.server_instance_id)
        || event.clock.domain != snapshot.clock.domain
        || event.clock.boot_id != snapshot.clock.boot_id
    {
        bail!("embedding_qualification_idle_epoch_invalid");
    }
    let idle_epoch_ns = details
        .get("idle_epoch_ns")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_idle_epoch_invalid"))?;
    if idle_epoch_ns > event.clock.observed_ns {
        bail!("embedding_qualification_idle_epoch_in_future");
    }
    Ok(idle_epoch_ns)
}

pub(super) fn consume_watchdog_marker(
    directory: &Path,
    nonce_sha256: &str,
    expected: &EmbeddingServerSnapshot,
) -> Result<(EmbeddingQualificationWatchdogMarker, String)> {
    let filename = codestory_retrieval::embedding_qualification_watchdog_marker_filename(
        nonce_sha256,
        &expected.process.server_instance_id,
    )?;
    let path = directory.join(filename);
    validate_direct_child(&path, directory, true)?;
    let bytes = read_private_request(&path)?;
    let digest = sha256_bytes(&bytes);
    let marker: EmbeddingQualificationWatchdogMarker =
        serde_json::from_slice(&bytes).context("parse watchdog fail-stop marker")?;
    if marker.schema_version != 1
        || marker.nonce_sha256 != nonce_sha256
        || marker.server_instance_id != expected.process.server_instance_id
        || marker.pid != expected.process.pid
        || marker.process_start_id != expected.process.process_start_id
        || marker.executable_sha256 != expected.process.executable_sha256
        || marker.executable_version != expected.process.executable_version
        || marker.reason != "embedding_engine_stalled"
        || marker.clock.domain != "awake_monotonic"
        || marker.clock.boot_id != expected.clock.boot_id
        || marker.last_progress_ns > marker.clock.observed_ns
        || marker.clock.observed_ns - marker.last_progress_ns
            < marker.hard_native_no_progress_ms.saturating_mul(1_000_000)
        || marker.hard_native_no_progress_ms != PER_USER_EMBEDDING_HARD_NATIVE_NO_PROGRESS_MS
        || marker.watchdog_cadence_ms != PER_USER_EMBEDDING_WATCHDOG_CADENCE_MS
    {
        bail!("embedding_qualification_watchdog_marker_invalid");
    }
    fs::remove_file(&path).context("consume watchdog fail-stop marker")?;
    #[cfg(unix)]
    File::open(directory)
        .and_then(|parent| parent.sync_all())
        .context("sync consumed watchdog marker directory")?;
    Ok((marker, digest))
}

pub(super) fn project_identity_sha256(runtime: &SidecarRuntimeConfig) -> String {
    let seed = runtime
        .project_identity
        .as_ref()
        .map(|identity| format!("{}:{}", identity.project_id, identity.workspace_id))
        .unwrap_or_else(|| runtime.namespace.clone());
    sha256_bytes(seed.as_bytes())
}

pub(super) fn elapsed(clock: &dyn AwakeMonotonicClock, started_ns: u64) -> Duration {
    Duration::from_nanos(clock.now_ns().saturating_sub(started_ns))
}

pub(super) fn attach_native_completion_sequences(
    directory: &Path,
    operations: &mut [WorkerQueueOperation],
) -> Result<()> {
    let expected_request_ids = operations
        .iter()
        .filter(|operation| operation.status == "ok")
        .map(|operation| operation.correlation_id.clone())
        .collect::<BTreeSet<_>>();
    let mut sequences_by_request = BTreeMap::new();
    for event in existing_control_events(directory)? {
        if event.action != "completed_tokens" || event.status != "completed" {
            continue;
        }
        let Some(details) = event.details else {
            continue;
        };
        let Some(request_id) = details.get("request_id") else {
            continue;
        };
        if !expected_request_ids.contains(request_id) {
            continue;
        }
        let sequence = details
            .get("native_completion_sequence")
            .ok_or_else(|| {
                anyhow::anyhow!("embedding_qualification_native_completion_sequence_missing")
            })?
            .parse::<u64>()
            .map_err(|_| {
                anyhow::anyhow!("embedding_qualification_native_completion_sequence_invalid")
            })?;
        if sequence == 0 {
            bail!("embedding_qualification_native_completion_sequence_invalid");
        }
        if sequences_by_request
            .insert(request_id.clone(), sequence)
            .is_some()
        {
            bail!("embedding_qualification_native_completion_sequence_duplicate_request");
        }
    }
    let mut observed_sequences = BTreeSet::new();
    for operation in operations {
        if operation.status == "ok" {
            let sequence = sequences_by_request
                .remove(&operation.correlation_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("embedding_qualification_native_completion_sequence_missing")
                })?;
            if !observed_sequences.insert(sequence) {
                bail!("embedding_qualification_native_completion_sequence_duplicate");
            }
            operation.native_completion_sequence = Some(sequence);
        } else if operation.native_completion_sequence.is_some() {
            bail!("embedding_qualification_native_completion_sequence_unexpected");
        }
    }
    Ok(())
}

pub(super) fn accelerator_operands(identity: &EmbeddingEngineIdentity) -> BTreeMap<String, Value> {
    btree([
        ("policy", json!(identity.policy)),
        ("backend", json!(identity.backend)),
        (
            "accelerator_execution_verified",
            json!(identity.accelerator_execution_verified),
        ),
        (
            "resident_accelerator_tensor_count",
            json!(identity.resident_accelerator_tensor_count),
        ),
        (
            "resident_accelerator_tensor_bytes",
            json!(identity.resident_accelerator_tensor_bytes),
        ),
        (
            "offloaded_layer_count",
            json!(identity.offloaded_layer_count),
        ),
        ("model_layer_count", json!(identity.model_layer_count)),
    ])
}

pub(super) fn raw_server_identity(snapshot: &EmbeddingServerSnapshot) -> Result<RawServerIdentity> {
    let load_generation = snapshot
        .engine
        .as_ref()
        .map(|engine| engine.load_generation)
        .filter(|generation| *generation > 0)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_server_generation_missing"))?;
    Ok(RawServerIdentity {
        server_instance_id: snapshot.process.server_instance_id.clone(),
        process_start_id: snapshot.process.process_start_id.clone(),
        load_generation,
    })
}

pub(super) fn snapshot_has_resident_generation(snapshot: &EmbeddingServerSnapshot) -> bool {
    resident_generation_is_valid(
        &snapshot.lifecycle,
        snapshot
            .engine
            .as_ref()
            .map(|engine| engine.load_generation),
    )
}

pub(super) fn resident_generation_is_valid(lifecycle: &str, load_generation: Option<u64>) -> bool {
    lifecycle == "resident" && load_generation.is_some_and(|generation| generation > 0)
}
