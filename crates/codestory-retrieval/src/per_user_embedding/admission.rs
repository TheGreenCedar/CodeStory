//! Bounded front-door admission by embedding request class.

use codestory_llama_sys::{
    EMBEDDING_BULK_QUEUE_CAPACITY, EMBEDDING_QUERY_QUEUE_CAPACITY, EmbeddingRequestClass,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ServerRequestAdmissionDepths {
    pub(super) query: usize,
    pub(super) bulk: usize,
}

impl ServerRequestAdmissionDepths {
    pub(super) fn depth(self, request_class: EmbeddingRequestClass) -> usize {
        match request_class {
            EmbeddingRequestClass::Query => self.query,
            EmbeddingRequestClass::Bulk => self.bulk,
        }
    }

    pub(super) fn capacity(request_class: EmbeddingRequestClass) -> usize {
        match request_class {
            EmbeddingRequestClass::Query => EMBEDDING_QUERY_QUEUE_CAPACITY,
            EmbeddingRequestClass::Bulk => EMBEDDING_BULK_QUEUE_CAPACITY,
        }
    }

    fn increment(&mut self, request_class: EmbeddingRequestClass) {
        match request_class {
            EmbeddingRequestClass::Query => self.query += 1,
            EmbeddingRequestClass::Bulk => self.bulk += 1,
        }
    }

    fn decrement(&mut self, request_class: EmbeddingRequestClass) {
        match request_class {
            EmbeddingRequestClass::Query => self.query = self.query.saturating_sub(1),
            EmbeddingRequestClass::Bulk => self.bulk = self.bulk.saturating_sub(1),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct ServerRequestAdmission {
    depths: Mutex<ServerRequestAdmissionDepths>,
}

impl ServerRequestAdmission {
    pub(super) fn try_acquire(
        self: &Arc<Self>,
        request_class: EmbeddingRequestClass,
        active_execution: bool,
    ) -> std::result::Result<ServerRequestAdmissionPermit, ServerRequestAdmissionDepths> {
        let mut depths = self
            .depths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let in_flight_capacity = ServerRequestAdmissionDepths::capacity(request_class)
            .saturating_add(usize::from(active_execution));
        if depths.depth(request_class) >= in_flight_capacity {
            return Err(*depths);
        }
        depths.increment(request_class);
        Ok(ServerRequestAdmissionPermit {
            inner: Arc::new(ServerRequestAdmissionPermitInner {
                admission: Arc::clone(self),
                request_class,
                released: AtomicBool::new(false),
            }),
        })
    }

    pub(super) fn snapshot(&self) -> ServerRequestAdmissionDepths {
        *self
            .depths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn release(&self, request_class: EmbeddingRequestClass) {
        self.depths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .decrement(request_class);
    }
}

#[derive(Debug, Clone)]
pub(super) struct ServerRequestAdmissionPermit {
    inner: Arc<ServerRequestAdmissionPermitInner>,
}

impl ServerRequestAdmissionPermit {
    pub(super) fn release(&self) {
        if !self.inner.released.swap(true, Ordering::AcqRel) {
            self.inner.admission.release(self.inner.request_class);
        }
    }
}

impl Drop for ServerRequestAdmissionPermit {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Debug)]
struct ServerRequestAdmissionPermitInner {
    admission: Arc<ServerRequestAdmission>,
    request_class: EmbeddingRequestClass,
    released: AtomicBool,
}
