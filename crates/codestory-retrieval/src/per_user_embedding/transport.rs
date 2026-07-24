//! Transport contracts, executable identity, and frozen client budgets.

use super::EmbeddingServerClockSnapshot;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingTransportIdentity {
    pub endpoint_namespace_id: String,
    pub lifetime_authority_id: String,
    pub listener_id: String,
    pub peer_verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_process_start_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingConnectIntent {
    Activate,
    Observe,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code}: {message}")]
pub struct EmbeddingTransportFailure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct EmbeddingSpawnAttempt {
    generation: u64,
    state: Arc<Mutex<EmbeddingSpawnAttemptState>>,
}

#[derive(Debug)]
enum EmbeddingSpawnAttemptState {
    Pending,
    Succeeded,
    Failed(EmbeddingTransportFailure),
}

impl EmbeddingSpawnAttempt {
    pub fn new(generation: u64) -> Self {
        debug_assert_ne!(generation, 0);
        Self {
            generation,
            state: Arc::new(Mutex::new(EmbeddingSpawnAttemptState::Pending)),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn record_failure(&self, failure: EmbeddingTransportFailure) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*state, EmbeddingSpawnAttemptState::Pending) {
            *state = EmbeddingSpawnAttemptState::Failed(failure);
        }
    }

    pub fn record_success(&self) {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            EmbeddingSpawnAttemptState::Succeeded;
    }

    pub fn failure(&self) -> Option<EmbeddingTransportFailure> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*state {
            EmbeddingSpawnAttemptState::Failed(failure) => Some(failure.clone()),
            EmbeddingSpawnAttemptState::Pending | EmbeddingSpawnAttemptState::Succeeded => None,
        }
    }
}

pub trait EmbeddingServerStream: Read + Write + Send {
    fn transport_identity(&self) -> &EmbeddingTransportIdentity;
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    /// Returns false once the authenticated peer process has exited. This is
    /// deliberately process-liveness only; it never inspects project state.
    fn peer_is_alive(&self) -> io::Result<bool>;
    /// Returns the authenticated peer process exit code when the platform can
    /// prove that the retained process identity has exited. A live peer and a
    /// platform without exit-code support both return `None`.
    fn peer_exit_code(&self) -> io::Result<Option<u32>> {
        Ok(None)
    }
    /// Completes transport-specific delivery of the final response before the
    /// server tears the connection down. Transports whose close preserves
    /// unread bytes need no additional work.
    fn finish_response_delivery(&self) -> io::Result<()> {
        Ok(())
    }
    fn shutdown(&self) -> io::Result<()>;
}

pub enum EmbeddingConnectOutcome {
    Connected(Box<dyn EmbeddingServerStream>),
    NoOwner,
    OwnerUnresponsive(EmbeddingTransportFailure),
}

impl fmt::Debug for EmbeddingConnectOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connected(stream) => formatter
                .debug_tuple("Connected")
                .field(stream.transport_identity())
                .finish(),
            Self::NoOwner => formatter.write_str("NoOwner"),
            Self::OwnerUnresponsive(error) => formatter
                .debug_tuple("OwnerUnresponsive")
                .field(error)
                .finish(),
        }
    }
}

pub trait AwakeMonotonicClock: Send + Sync {
    fn now_ns(&self) -> u64;
    fn sleep(&self, duration: Duration);
    fn snapshot(&self) -> EmbeddingServerClockSnapshot;
}

pub trait EmbeddingClientTransport: Send + Sync {
    fn connect(
        &self,
        intent: EmbeddingConnectIntent,
        budget: Duration,
        spawn_attempt: Option<&EmbeddingSpawnAttempt>,
    ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure>;
    fn spawn_exact_current_exe(
        &self,
    ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure>;
    fn clock(&self) -> Arc<dyn AwakeMonotonicClock>;
    fn executable_identity(&self) -> EmbeddingExecutableIdentity;
    fn budgets(&self) -> EmbeddingClientBudgets;
}

pub trait EmbeddingServerListener: Send + Sync {
    fn accept(
        &self,
        timeout: Duration,
    ) -> std::result::Result<Option<Box<dyn EmbeddingServerStream>>, EmbeddingTransportFailure>;
    fn identity(&self) -> &EmbeddingTransportIdentity;
    fn close(&self) -> std::result::Result<(), EmbeddingTransportFailure>;
}

pub enum EmbeddingServerBindOutcome {
    Bound(Box<dyn EmbeddingServerListener>),
    AlreadyOwned,
}

impl fmt::Debug for EmbeddingServerBindOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bound(listener) => formatter
                .debug_tuple("Bound")
                .field(listener.identity())
                .finish(),
            Self::AlreadyOwned => formatter.write_str("AlreadyOwned"),
        }
    }
}

pub trait EmbeddingServerTransport: Send + Sync {
    fn bind(&self) -> std::result::Result<EmbeddingServerBindOutcome, EmbeddingTransportFailure>;
    fn clock(&self) -> Arc<dyn AwakeMonotonicClock>;
    fn fail_stop(&self, reason_code: &str);
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingExecutableIdentity {
    pub pid: u32,
    pub process_start_id: String,
    pub executable_sha256: String,
    pub executable_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingClientBudgets {
    pub connect: Duration,
    pub spawn: Duration,
    pub retry_after: Duration,
    pub query_request: Duration,
    pub bulk_request: Duration,
}

impl EmbeddingClientBudgets {
    /// Values generated from the checked-in constant set. Its draft section
    /// is used only while package qualification remains fail-closed.
    pub const fn current() -> Self {
        Self {
            connect: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_CONNECT_TIMEOUT_MS,
            ),
            spawn: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_SPAWN_CONVERGENCE_TIMEOUT_MS,
            ),
            retry_after: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_RETRY_AFTER_MS,
            ),
            query_request: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_QUERY_REQUEST_DEADLINE_MS,
            ),
            bulk_request: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS,
            ),
        }
    }
}
