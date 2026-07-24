//! Per-user embedding server lifecycle, request handling, and engine ownership.

use super::{
    EmbeddingExecutableIdentity, EmbeddingServerProtocolSnapshot, EmbeddingServerTransport,
    PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS,
};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

mod connection;
mod frame;
mod lifecycle;
mod operation;
mod response;
mod state;

#[cfg(test)]
pub(in crate::per_user_embedding) use connection::{
    serve_embedding_connection, serve_embedding_connection_at_handler_capacity,
};
#[cfg(test)]
pub(in crate::per_user_embedding) use frame::{IncrementalProtocolFrameReader, ProtocolFramePoll};
#[cfg(test)]
pub(in crate::per_user_embedding) use lifecycle::reap_finished_connection_handlers;
pub use lifecycle::run_per_user_embedding_server;
#[cfg(test)]
pub(in crate::per_user_embedding) use operation::{ServerRequestDeadline, cancel_if_peer_dead};
#[cfg(test)]
pub(in crate::per_user_embedding) use response::{
    configure_server_operation_timeout, failure_response, protocol_error, success_response,
};
pub(in crate::per_user_embedding) use state::PerUserEmbeddingServerState;
#[cfg(test)]
pub(in crate::per_user_embedding) use state::{
    ServerCancellationAuth, ServerLeaseActivity, ServerRequestGuard, ServerRequestRegistration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingServerBudgets {
    pub idle_timeout: Duration,
    pub native_no_progress: Duration,
    pub watchdog_poll: Duration,
}

impl EmbeddingServerBudgets {
    /// Values generated from the checked-in constant set. Its draft section
    /// is used only while package qualification remains fail-closed.
    pub const fn current() -> Self {
        Self {
            idle_timeout: Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS),
            native_no_progress: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_HARD_NATIVE_NO_PROGRESS_MS,
            ),
            watchdog_poll: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_WATCHDOG_CADENCE_MS,
            ),
        }
    }
}

pub struct PerUserEmbeddingServerConfig {
    pub transport: Arc<dyn EmbeddingServerTransport>,
    pub engine_cache_root: PathBuf,
    pub executable: EmbeddingExecutableIdentity,
    pub allow_cpu: bool,
    pub budgets: EmbeddingServerBudgets,
    pub protocol: EmbeddingServerProtocolSnapshot,
}

impl fmt::Debug for PerUserEmbeddingServerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PerUserEmbeddingServerConfig")
            .field("engine_cache_root", &self.engine_cache_root)
            .field("executable", &self.executable)
            .field("allow_cpu", &self.allow_cpu)
            .field("budgets", &self.budgets)
            .field("protocol", &self.protocol)
            .finish_non_exhaustive()
    }
}
