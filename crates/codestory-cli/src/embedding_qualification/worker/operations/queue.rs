use super::super::gate::{POLL, elapsed, project_identity_sha256, sha256_bytes};
use super::super::protocol::{
    authenticate_snapshot, client_hello_operation, read_protocol_frame, write_protocol_frame,
};
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingCompatibility, EmbeddingOperation, EmbeddingProtocolRequest,
    EmbeddingProtocolResponse, EmbeddingQualificationParameters,
    EmbeddingQualificationWorkerQueueOperation as WorkerQueueOperation, EmbeddingResult,
    EmbeddingServerStream, PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
    PER_USER_EMBEDDING_PROTOCOL_V1, PerUserEmbeddingClient, SidecarRuntimeConfig,
};
use std::sync::Arc;
use std::time::Duration;

const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const QUEUE_SETUP_TIMEOUT: Duration = Duration::from_secs(60);
const QUALIFICATION_QUEUE_CAPACITY: u64 = 64;
const MIXED_QUEUE_COUNT: u32 = QUALIFICATION_QUEUE_CAPACITY as u32 + 1;

pub(in crate::embedding_qualification::worker) fn run_queue_load(
    runtime: &SidecarRuntimeConfig,
    parameters: EmbeddingQualificationParameters,
    clock: Arc<dyn AwakeMonotonicClock>,
) -> Result<Vec<WorkerQueueOperation>> {
    if parameters.query_count == 0
        || parameters.bulk_count == 0
        || parameters.query_count > MIXED_QUEUE_COUNT
        || parameters.bulk_count > MIXED_QUEUE_COUNT
    {
        bail!("embedding_qualification_queue_load_parameters_invalid");
    }
    let runtime = runtime.clone();
    let project_identity = project_identity_sha256(&runtime);
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let observer = PerUserEmbeddingClient::for_runtime(&runtime)?;
    let baseline = observer
        .observe()?
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_queue_owner_absent"))?;
    if baseline.scheduler.query_capacity != QUALIFICATION_QUEUE_CAPACITY
        || baseline.scheduler.bulk_capacity != QUALIFICATION_QUEUE_CAPACITY
    {
        bail!("embedding_qualification_queue_capacity_invalid");
    }
    let mut expected_query_depth = baseline.scheduler.query_depth;
    let mut expected_bulk_depth = baseline.scheduler.bulk_depth;
    let query_success_limit = QUALIFICATION_QUEUE_CAPACITY.saturating_sub(expected_query_depth);
    let bulk_success_limit = QUALIFICATION_QUEUE_CAPACITY.saturating_sub(expected_bulk_depth);
    let mut query_attempts = 0_u64;
    let mut bulk_attempts = 0_u64;
    let mut workers = Vec::new();
    let maximum = parameters.query_count.max(parameters.bulk_count);
    for ordinal in 0..maximum {
        for class in ["bulk", "query"] {
            let count = if class == "bulk" {
                parameters.bulk_count
            } else {
                parameters.query_count
            };
            if ordinal >= count {
                continue;
            }
            let runtime = runtime.clone();
            let worker_clock = Arc::clone(&clock);
            let transport = transport.clone();
            let project_identity = project_identity.clone();
            let correlation_id = format!(
                "queue-{}-{class}-{ordinal}-{}",
                std::process::id(),
                &project_identity[..12]
            );
            let (submitted_tx, submitted_rx) = std::sync::mpsc::sync_channel(1);
            let worker = std::thread::Builder::new()
                .name(format!("codestory-queue-{class}-{ordinal}"))
                .spawn(move || {
                    run_queue_operation(QueueOperation {
                        runtime,
                        transport,
                        clock: worker_clock,
                        project_identity_sha256: project_identity,
                        class,
                        ordinal,
                        correlation_id,
                        measured_input: None,
                        submitted_tx,
                    })
                })?;
            submitted_rx
                .recv_timeout(CONTROL_TIMEOUT)
                .context("wait for qualification queue submission")?;
            let expected_depth = if class == "query" {
                query_attempts = query_attempts.saturating_add(1);
                if query_attempts > query_success_limit {
                    None
                } else {
                    expected_query_depth = expected_query_depth.saturating_add(1);
                    Some(expected_query_depth)
                }
            } else {
                bulk_attempts = bulk_attempts.saturating_add(1);
                if bulk_attempts > bulk_success_limit {
                    None
                } else {
                    expected_bulk_depth = expected_bulk_depth.saturating_add(1);
                    Some(expected_bulk_depth)
                }
            };
            if let Some(expected_depth) = expected_depth {
                wait_for_queue_admission(&observer, clock.as_ref(), class, expected_depth)?;
            }
            workers.push(worker);
        }
    }
    workers
        .into_iter()
        .map(|worker| {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("embedding_qualification_queue_worker_panicked"))?
        })
        .collect()
}

fn wait_for_queue_admission(
    observer: &PerUserEmbeddingClient,
    clock: &dyn AwakeMonotonicClock,
    class: &str,
    expected_depth: u64,
) -> Result<()> {
    let started = clock.now_ns();
    loop {
        let snapshot = observer
            .observe()?
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_queue_owner_absent"))?;
        let actual_depth = match class {
            "query" => snapshot.scheduler.query_depth,
            "bulk" => snapshot.scheduler.bulk_depth,
            _ => bail!("embedding_qualification_queue_class_invalid"),
        };
        if actual_depth >= expected_depth {
            return Ok(());
        }
        if elapsed(clock, started) >= QUEUE_SETUP_TIMEOUT {
            bail!("embedding_qualification_queue_admission_timeout:{class}");
        }
        clock.sleep(POLL);
    }
}

struct QueueOperation {
    runtime: SidecarRuntimeConfig,
    transport: crate::embedding_server_transport::NativeEmbeddingClientTransport,
    clock: Arc<dyn AwakeMonotonicClock>,
    project_identity_sha256: String,
    class: &'static str,
    ordinal: u32,
    correlation_id: String,
    measured_input: Option<String>,
    submitted_tx: std::sync::mpsc::SyncSender<u64>,
}

fn run_queue_operation(operation: QueueOperation) -> Result<WorkerQueueOperation> {
    let QueueOperation {
        runtime,
        transport,
        clock,
        project_identity_sha256,
        class,
        ordinal,
        correlation_id,
        measured_input,
        submitted_tx,
    } = operation;
    let mut stream = match transport.connect(Duration::from_secs(2))? {
        crate::embedding_server_transport::NativeConnectOutcome::Connected(stream) => stream,
        crate::embedding_server_transport::NativeConnectOutcome::NoOwner => {
            bail!("embedding_server_absent")
        }
        crate::embedding_server_transport::NativeConnectOutcome::OwnerUnresponsive => {
            bail!("embedding_server_owner_unresponsive")
        }
    };
    let transport_identity = stream.identity().clone();
    EmbeddingServerStream::set_read_timeout(&stream, Some(Duration::from_secs(2)))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(Duration::from_secs(2)))?;
    let compatibility = EmbeddingCompatibility::current(runtime.embedding.allow_cpu);
    let hello_id = format!("{correlation_id}-hello");
    write_protocol_frame(
        &mut stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: hello_id.clone(),
            compatibility: compatibility.clone(),
            operation: client_hello_operation("activate", &transport),
        },
        &[],
    )?;
    let (hello, hello_payload): (EmbeddingProtocolResponse, Vec<u8>) =
        read_protocol_frame(&mut stream)?;
    if !hello_payload.is_empty() || hello.request_id != hello_id || hello.error.is_some() {
        bail!("embedding_qualification_queue_hello_invalid");
    }
    let hello_snapshot = match hello.result {
        Some(EmbeddingResult::Hello { snapshot, .. }) => *snapshot,
        _ => bail!("embedding_qualification_queue_hello_missing"),
    };
    authenticate_snapshot(&hello_snapshot, &transport_identity)?;
    let scope_seed = runtime
        .project_identity
        .as_ref()
        .map(|identity| format!("{}:{}", identity.project_id, identity.workspace_id))
        .unwrap_or_else(|| runtime.namespace.clone());
    let scope_id = sha256_bytes(scope_seed.as_bytes());
    let deadline_ms = 120_000;
    let operation = match class {
        "query" => EmbeddingOperation::EmbedQuery {
            scope_id,
            deadline_ms,
            retry_after_ms: 100,
            cancel_token: None,
            input: measured_input
                .clone()
                .unwrap_or_else(|| format!("qualification-queue-{ordinal}")),
        },
        "bulk" => EmbeddingOperation::EmbedDocuments {
            scope_id,
            deadline_ms,
            retry_after_ms: 100,
            cancel_token: None,
            inputs: vec![
                measured_input.unwrap_or_else(|| format!("qualification-queue-{ordinal}")),
            ],
        },
        _ => bail!("embedding_qualification_queue_class_invalid"),
    };
    let submitted_ns = clock.now_ns();
    write_protocol_frame(
        &mut stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: correlation_id.clone(),
            compatibility,
            operation,
        },
        &[],
    )?;
    submitted_tx
        .send(submitted_ns)
        .context("publish qualification queue submission")?;
    EmbeddingServerStream::set_read_timeout(&stream, Some(Duration::from_millis(deadline_ms)))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(Duration::from_millis(deadline_ms)))?;
    let (response, payload): (EmbeddingProtocolResponse, Vec<u8>) =
        read_protocol_frame(&mut stream)?;
    if response.request_id != correlation_id
        || response.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || response.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || (response.result.is_some() == response.error.is_some())
    {
        bail!("embedding_qualification_queue_response_invalid");
    }
    let status = if response.error.is_some() {
        "failed"
    } else {
        "ok"
    };
    Ok(WorkerQueueOperation {
        correlation_id,
        project_identity_sha256,
        class: class.into(),
        ordinal,
        submission_batch: 0,
        submitted_ns,
        completed_ns: clock.now_ns(),
        native_completion_sequence: None,
        status: status.into(),
        error: response.error,
        response_payload_bytes: payload.len() as u64,
        transport_identity,
        hello_snapshot,
    })
}
