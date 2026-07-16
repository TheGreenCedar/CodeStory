use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A simple cancellation token for cooperative cancellation.
#[derive(Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Use an existing shared flag as this token's cancellation source.
    ///
    /// Runtime adapters use this to bridge host-owned request cancellation
    /// without exposing indexer types to the host or CLI layer.
    pub fn from_shared_flag(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }

    /// Signal cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Check if cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}
