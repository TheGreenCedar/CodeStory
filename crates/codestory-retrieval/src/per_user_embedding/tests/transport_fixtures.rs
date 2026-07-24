use super::super::{
    AwakeMonotonicClock, EmbeddingCompatibility, EmbeddingOperation, EmbeddingProtocolError,
    EmbeddingProtocolRequest, EmbeddingProtocolResponse, EmbeddingResult,
    EmbeddingServerClockSnapshot, EmbeddingServerStream, EmbeddingTransportIdentity,
    encode_vectors, failure_response, protocol_error, success_response,
};
use super::identities::{
    decode_test_frame, encode_test_frame, test_capacity, test_engine_identity, test_snapshot,
    test_transport_identity,
};
use crate::embedding_contract::RETRIEVAL_EMBEDDING_DIM;
use std::io::{self, Cursor, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug)]
pub(super) struct TestClock {
    pub(super) now: AtomicU64,
}

impl TestClock {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            now: AtomicU64::new(1),
        })
    }
}

impl AwakeMonotonicClock for TestClock {
    fn now_ns(&self) -> u64 {
        self.now.load(Ordering::Acquire)
    }

    fn sleep(&self, duration: Duration) {
        self.now.fetch_add(
            duration.as_nanos().max(1).min(u128::from(u64::MAX)) as u64,
            Ordering::AcqRel,
        );
    }

    fn snapshot(&self) -> EmbeddingServerClockSnapshot {
        EmbeddingServerClockSnapshot {
            domain: "awake_monotonic".into(),
            api: "test_clock".into(),
            boot_id: "test-boot".into(),
            resolution_ns: 1,
        }
    }
}

pub(super) struct MemoryStream {
    pub(super) identity: EmbeddingTransportIdentity,
    pub(super) input: Cursor<Vec<u8>>,
    pub(super) output: Arc<Mutex<Vec<u8>>>,
    pub(super) finished_deliveries: Arc<AtomicUsize>,
    pub(super) read_timeouts: Arc<Mutex<Vec<Option<Duration>>>>,
    pub(super) write_timeouts: Arc<Mutex<Vec<Option<Duration>>>>,
    pub(super) alive: bool,
    pub(super) exit_codes: Mutex<Vec<Option<u32>>>,
}

pub(super) struct MemoryStreamFixture {
    pub(super) stream: MemoryStream,
    pub(super) output: Arc<Mutex<Vec<u8>>>,
    pub(super) finished_deliveries: Arc<AtomicUsize>,
    pub(super) read_timeouts: Arc<Mutex<Vec<Option<Duration>>>>,
    pub(super) write_timeouts: Arc<Mutex<Vec<Option<Duration>>>>,
}

impl MemoryStream {
    pub(super) fn new(input: Vec<u8>, alive: bool) -> (Self, Arc<Mutex<Vec<u8>>>) {
        let fixture = Self::with_delivery_tracking(input, alive);
        (fixture.stream, fixture.output)
    }

    pub(super) fn with_delivery_tracking(input: Vec<u8>, alive: bool) -> MemoryStreamFixture {
        let output = Arc::new(Mutex::new(Vec::new()));
        let finished_deliveries = Arc::new(AtomicUsize::new(0));
        let read_timeouts = Arc::new(Mutex::new(Vec::new()));
        let write_timeouts = Arc::new(Mutex::new(Vec::new()));
        MemoryStreamFixture {
            stream: Self {
                identity: test_transport_identity(),
                input: Cursor::new(input),
                output: Arc::clone(&output),
                finished_deliveries: Arc::clone(&finished_deliveries),
                read_timeouts: Arc::clone(&read_timeouts),
                write_timeouts: Arc::clone(&write_timeouts),
                alive,
                exit_codes: Mutex::new(vec![None]),
            },
            output,
            finished_deliveries,
            read_timeouts,
            write_timeouts,
        }
    }
}

impl Read for MemoryStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.input.read(buffer)
    }
}

impl Write for MemoryStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl EmbeddingServerStream for MemoryStream {
    fn transport_identity(&self) -> &EmbeddingTransportIdentity {
        &self.identity
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeouts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(timeout);
        Ok(())
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.write_timeouts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(timeout);
        Ok(())
    }

    fn peer_is_alive(&self) -> io::Result<bool> {
        Ok(self.alive)
    }

    fn peer_exit_code(&self) -> io::Result<Option<u32>> {
        let mut exit_codes = self
            .exit_codes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if exit_codes.len() > 1 {
            return Ok(exit_codes.remove(0));
        }
        Ok(exit_codes.first().copied().flatten())
    }

    fn finish_response_delivery(&self) -> io::Result<()> {
        self.finished_deliveries.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn shutdown(&self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub(super) enum ScriptOutcome {
    Success,
    Loss,
    HelloLoss,
    Capacity,
    TimedBulk {
        hello_delay: Duration,
        exchange_delay: Duration,
        lose_exchange: bool,
    },
    Blocking {
        request_started: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
    },
}

pub(super) struct ScriptStream {
    pub(super) identity: EmbeddingTransportIdentity,
    pub(super) writes: Vec<u8>,
    pub(super) reads: Cursor<Vec<u8>>,
    pub(super) outcome: ScriptOutcome,
    pub(super) compatibility: EmbeddingCompatibility,
    pub(super) read_gate: Option<Arc<AtomicBool>>,
}

impl ScriptStream {
    pub(super) fn new(outcome: ScriptOutcome, compatibility: EmbeddingCompatibility) -> Self {
        Self {
            identity: test_transport_identity(),
            writes: Vec::new(),
            reads: Cursor::new(Vec::new()),
            outcome,
            compatibility,
            read_gate: None,
        }
    }

    pub(super) fn prepare_response(&mut self) -> io::Result<()> {
        let request: EmbeddingProtocolRequest =
            decode_test_frame(&self.writes).map_err(io::Error::other)?;
        self.writes.clear();
        let Some((response, payload)) = self.response_for_request(request)? else {
            self.reads = Cursor::new(Vec::new());
            return Ok(());
        };
        self.reads = Cursor::new(encode_test_frame(&response, &payload));
        Ok(())
    }

    pub(super) fn response_for_request(
        &mut self,
        request: EmbeddingProtocolRequest,
    ) -> io::Result<Option<(EmbeddingProtocolResponse, Vec<u8>)>> {
        let request_id = request.request_id;
        match request.operation {
            EmbeddingOperation::Hello { .. } => self.hello_response(&request_id),
            EmbeddingOperation::EmbedQuery { .. } => self.query_response(&request_id),
            EmbeddingOperation::EmbedDocuments { inputs, .. } => {
                self.documents_response(&request_id, inputs.len())
            }
            EmbeddingOperation::Cancel { .. } => self.cancel_response(&request_id),
            EmbeddingOperation::Snapshot => Ok(Some((
                success_response(
                    &request_id,
                    EmbeddingResult::Snapshot {
                        snapshot: Box::new(test_snapshot()),
                        lease: None,
                        identity: None,
                    },
                ),
                Vec::new(),
            ))),
            _ => Ok(Some((
                failure_response(
                    &request_id,
                    protocol_error("test_operation_unsupported", "unsupported test operation"),
                ),
                Vec::new(),
            ))),
        }
    }

    pub(super) fn hello_response(
        &self,
        request_id: &str,
    ) -> io::Result<Option<(EmbeddingProtocolResponse, Vec<u8>)>> {
        if matches!(self.outcome, ScriptOutcome::HelloLoss) {
            return Ok(None);
        }
        if let ScriptOutcome::TimedBulk { hello_delay, .. } = self.outcome {
            thread::sleep(hello_delay);
        }
        Ok(Some((
            success_response(
                request_id,
                EmbeddingResult::Hello {
                    compatibility_sha256: self.compatibility.digest().map_err(io::Error::other)?,
                    snapshot: Box::new(test_snapshot()),
                },
            ),
            Vec::new(),
        )))
    }

    pub(super) fn query_response(
        &mut self,
        request_id: &str,
    ) -> io::Result<Option<(EmbeddingProtocolResponse, Vec<u8>)>> {
        match self.outcome.clone() {
            ScriptOutcome::Loss => Ok(None),
            ScriptOutcome::Capacity => Ok(Some((
                failure_response(
                    request_id,
                    EmbeddingProtocolError {
                        code: "embedding_capacity".into(),
                        message: "query queue is full".into(),
                        retry_class: "after_capacity_change".into(),
                        retry_after_ms: 10,
                        retry_condition: "a live request completes".into(),
                        capacity: Some(test_capacity()),
                    },
                ),
                Vec::new(),
            ))),
            ScriptOutcome::Success => {
                let mut vector = vec![0.0_f32; RETRIEVAL_EMBEDDING_DIM];
                vector[0] = 1.0;
                Ok(Some((
                    success_response(
                        request_id,
                        EmbeddingResult::Vectors {
                            rows: 1,
                            columns: RETRIEVAL_EMBEDDING_DIM as u32,
                            encoding: "f32_le".into(),
                            identity: Box::new(test_engine_identity()),
                        },
                    ),
                    encode_vectors(&[vector]).map_err(io::Error::other)?,
                )))
            }
            ScriptOutcome::Blocking {
                request_started,
                cancelled,
            } => {
                request_started.store(true, Ordering::Release);
                self.read_gate = Some(cancelled);
                Ok(Some((
                    failure_response(
                        request_id,
                        EmbeddingProtocolError {
                            code: "embedding_cancelled".into(),
                            message: "the active request was cancelled".into(),
                            retry_class: "none".into(),
                            retry_after_ms: 0,
                            retry_condition: "the caller starts a new request".into(),
                            capacity: None,
                        },
                    ),
                    Vec::new(),
                )))
            }
            ScriptOutcome::HelloLoss => Err(io::Error::other("query reached hello-loss stream")),
            ScriptOutcome::TimedBulk { .. } => {
                Err(io::Error::other("query reached timed bulk stream"))
            }
        }
    }

    pub(super) fn documents_response(
        &self,
        request_id: &str,
        input_count: usize,
    ) -> io::Result<Option<(EmbeddingProtocolResponse, Vec<u8>)>> {
        let ScriptOutcome::TimedBulk {
            exchange_delay,
            lose_exchange,
            ..
        } = self.outcome
        else {
            return Err(io::Error::other("documents reached non-bulk stream"));
        };
        thread::sleep(exchange_delay);
        if lose_exchange {
            return Ok(None);
        }
        let vectors = (0..input_count)
            .map(|_| {
                let mut vector = vec![0.0_f32; RETRIEVAL_EMBEDDING_DIM];
                vector[0] = 1.0;
                vector
            })
            .collect::<Vec<_>>();
        Ok(Some((
            success_response(
                request_id,
                EmbeddingResult::Vectors {
                    rows: input_count as u32,
                    columns: RETRIEVAL_EMBEDDING_DIM as u32,
                    encoding: "f32_le".into(),
                    identity: Box::new(test_engine_identity()),
                },
            ),
            encode_vectors(&vectors).map_err(io::Error::other)?,
        )))
    }

    pub(super) fn cancel_response(
        &self,
        request_id: &str,
    ) -> io::Result<Option<(EmbeddingProtocolResponse, Vec<u8>)>> {
        let ScriptOutcome::Blocking { cancelled, .. } = self.outcome.clone() else {
            return Err(io::Error::other("unexpected cancellation request"));
        };
        cancelled.store(true, Ordering::Release);
        Ok(Some((
            success_response(request_id, EmbeddingResult::Cancelled),
            Vec::new(),
        )))
    }
}

impl Read for ScriptStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while self
            .read_gate
            .as_ref()
            .is_some_and(|gate| !gate.load(Ordering::Acquire))
        {
            thread::sleep(Duration::from_millis(1));
        }
        self.reads.read(buffer)
    }
}

impl Write for ScriptStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.writes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.prepare_response()
    }
}

impl EmbeddingServerStream for ScriptStream {
    fn transport_identity(&self) -> &EmbeddingTransportIdentity {
        &self.identity
    }

    fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    fn peer_is_alive(&self) -> io::Result<bool> {
        Ok(true)
    }

    fn shutdown(&self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) struct StallingHelloStream {
    pub(super) identity: EmbeddingTransportIdentity,
    pub(super) read_timeout: Mutex<Option<Duration>>,
    pub(super) observed_read_timeout: Arc<Mutex<Option<Duration>>>,
}

impl Read for StallingHelloStream {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        let timeout = self
            .read_timeout
            .lock()
            .expect("stalling Hello read timeout")
            .expect("Hello exchange must configure a read timeout");
        thread::sleep(timeout.saturating_add(Duration::from_millis(5)));
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "scripted initial Hello stall",
        ))
    }
}

impl Write for StallingHelloStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl EmbeddingServerStream for StallingHelloStream {
    fn transport_identity(&self) -> &EmbeddingTransportIdentity {
        &self.identity
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        *self
            .read_timeout
            .lock()
            .expect("stalling Hello read timeout") = timeout;
        *self
            .observed_read_timeout
            .lock()
            .expect("observed Hello read timeout") = timeout;
        Ok(())
    }

    fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    fn peer_is_alive(&self) -> io::Result<bool> {
        Ok(true)
    }

    fn shutdown(&self) -> io::Result<()> {
        Ok(())
    }
}
