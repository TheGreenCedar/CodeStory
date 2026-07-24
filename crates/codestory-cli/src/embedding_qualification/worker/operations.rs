use super::contracts::{WorkerError, WorkerProtocolExchange, WorkerQueueOperation};
use super::gate::{POLL, elapsed, project_identity_sha256, qualification_request_id, sha256_bytes};
use super::protocol::{
    authenticate_snapshot, client_hello_operation, connect_until, read_protocol_frame,
    run_protocol_exchange_on_stream, run_raw_protocol_exchange_with_input, write_protocol_frame,
};
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingClientTransport, EmbeddingCompatibility, EmbeddingOperation,
    EmbeddingProtocolRequest, EmbeddingProtocolResponse, EmbeddingQualificationOperationResult,
    EmbeddingQualificationParameters, EmbeddingQualificationResult, EmbeddingResult,
    EmbeddingServerStream, PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
    PER_USER_EMBEDDING_PROTOCOL_V1, PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS,
    PerUserEmbeddingClient, SidecarRuntimeConfig,
};
use std::sync::Arc;
use std::time::Duration;

const ANTI_IDLE_PROTOCOL_DEADLINE_MS: u64 = 90_000;
const CLIENT_DEATH_LEASE_HOLD_MS: u64 = 600_000;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const QUEUE_SETUP_TIMEOUT: Duration = Duration::from_secs(60);
const QUALIFICATION_QUEUE_CAPACITY: u64 = 64;
const MIXED_QUEUE_COUNT: u32 = QUALIFICATION_QUEUE_CAPACITY as u32 + 1;
const OWNER_ABSENCE_GRACE: Duration = Duration::from_secs(30);

pub(super) fn wait_for_owner_absence(
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

pub(super) fn run_dead_client_load(
    runtime: &SidecarRuntimeConfig,
    parameters: EmbeddingQualificationParameters,
    clock: &dyn AwakeMonotonicClock,
) -> Result<()> {
    if parameters.query_count == 0
        || parameters.bulk_count == 0
        || parameters.documents_per_bulk == 0
        || parameters.hold_ms != CLIENT_DEATH_LEASE_HOLD_MS
    {
        bail!("embedding_qualification_dead_client_parameters_invalid");
    }
    let client = PerUserEmbeddingClient::for_runtime(runtime)?;
    let _lease = client.acquire_residency_lease()?;
    let input = "q".repeat(parameters.input_bytes.max(1) as usize);
    let documents = (0..parameters.documents_per_bulk)
        .map(|index| format!("{index}:{input}"))
        .collect::<Vec<_>>();
    let mut workers = Vec::new();
    for _ in 0..parameters.query_count {
        let runtime = runtime.clone();
        let input = input.clone();
        workers.push(
            std::thread::Builder::new()
                .name("codestory-dead-client-query".into())
                .spawn(move || {
                    // Keep an admitted request alive until this process is
                    // terminated. The product client's short deadline would
                    // otherwise start cancellation watchers and make their
                    // retry traffic the pressure under test.
                    let transport = match crate::embedding_server_transport::NativeEmbeddingClientTransport::capture() {
                        Ok(transport) => transport,
                        Err(_) => return,
                    };
                    let clock = EmbeddingClientTransport::clock(&transport);
                    let _ = run_raw_protocol_exchange_with_input(
                        &runtime,
                        clock.as_ref(),
                        "query",
                        ANTI_IDLE_PROTOCOL_DEADLINE_MS,
                        Some(input),
                    );
                })?,
        );
    }
    for _ in 0..parameters.bulk_count {
        let runtime = runtime.clone();
        let input = documents.join("\n");
        workers.push(
            std::thread::Builder::new()
                .name("codestory-dead-client-bulk".into())
                .spawn(move || {
                    let transport = match crate::embedding_server_transport::NativeEmbeddingClientTransport::capture() {
                        Ok(transport) => transport,
                        Err(_) => return,
                    };
                    let clock = EmbeddingClientTransport::clock(&transport);
                    let _ = run_raw_protocol_exchange_with_input(
                        &runtime,
                        clock.as_ref(),
                        "bulk",
                        ANTI_IDLE_PROTOCOL_DEADLINE_MS,
                        Some(input),
                    );
                })?,
        );
    }
    loop {
        std::hint::black_box(&workers);
        clock.sleep(Duration::from_secs(1));
    }
}

pub(super) fn run_queue_load(
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
                    run_queue_operation(
                        &runtime,
                        transport,
                        worker_clock.as_ref(),
                        &project_identity,
                        class,
                        ordinal,
                        correlation_id,
                        None,
                        submitted_tx,
                    )
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

pub(super) fn wait_for_queue_admission(
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

pub(super) fn run_queue_operation(
    runtime: &SidecarRuntimeConfig,
    transport: crate::embedding_server_transport::NativeEmbeddingClientTransport,
    clock: &dyn AwakeMonotonicClock,
    project_identity_sha256: &str,
    class: &str,
    ordinal: u32,
    correlation_id: String,
    measured_input: Option<String>,
    submitted_tx: std::sync::mpsc::SyncSender<u64>,
) -> Result<WorkerQueueOperation> {
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
        project_identity_sha256: project_identity_sha256.into(),
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

pub(super) fn run_activate_probe(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
) -> Result<WorkerError> {
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let mut stream = match transport.connect(Duration::from_secs(2))? {
        crate::embedding_server_transport::NativeConnectOutcome::Connected(stream) => stream,
        crate::embedding_server_transport::NativeConnectOutcome::NoOwner => {
            bail!("embedding_server_absent")
        }
        crate::embedding_server_transport::NativeConnectOutcome::OwnerUnresponsive => {
            bail!("embedding_server_owner_unresponsive")
        }
    };
    let identity = stream.identity();
    if !identity.peer_verified
        || identity.peer_pid.is_none()
        || identity
            .peer_process_start_id
            .as_deref()
            .is_none_or(str::is_empty)
    {
        bail!("embedding_qualification_activate_peer_unverified");
    }
    EmbeddingServerStream::set_read_timeout(&stream, Some(Duration::from_secs(2)))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(Duration::from_secs(2)))?;
    let request_id = qualification_request_id("activate-probe", clock.now_ns());
    write_protocol_frame(
        &mut stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: request_id.clone(),
            compatibility: EmbeddingCompatibility::current(runtime.embedding.allow_cpu),
            operation: client_hello_operation("activate", &transport),
        },
        &[],
    )?;
    let (response, payload): (EmbeddingProtocolResponse, Vec<u8>) =
        read_protocol_frame(&mut stream)?;
    if !payload.is_empty()
        || response.request_id != request_id
        || response.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || response.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || response.result.is_some()
    {
        bail!("embedding_qualification_activate_response_invalid");
    }
    let error = response
        .error
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_activate_error_missing"))?;
    Ok(WorkerError {
        code: error.code,
        message_head: error.message.chars().take(128).collect(),
        retry_class: error.retry_class,
        retry_after_ms: error.retry_after_ms,
        retry_condition: error.retry_condition,
        capacity: error.capacity,
    })
}

pub(super) fn run_cold_race_protocol_exchange(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
) -> Result<WorkerProtocolExchange> {
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let spawn_attempt = transport.spawn_exact_current_exe()?;
    let stream = connect_until(
        &transport,
        clock,
        Duration::from_secs(15),
        Some(&spawn_attempt),
    )?;
    run_protocol_exchange_on_stream(
        runtime,
        clock,
        "query",
        ANTI_IDLE_PROTOCOL_DEADLINE_MS,
        None,
        &transport,
        stream,
    )
}
