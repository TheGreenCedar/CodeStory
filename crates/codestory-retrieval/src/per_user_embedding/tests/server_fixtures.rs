use super::super::{
    AwakeMonotonicClock, EmbeddingServerBindOutcome, EmbeddingServerStream,
    EmbeddingServerTransport, EmbeddingTransportFailure, EmbeddingTransportIdentity,
};
use super::transport_fixtures::{MemoryStream, TestClock};
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub(super) struct WatchdogTransport {
    pub(super) clock: Arc<TestClock>,
    pub(super) fail_stops: AtomicUsize,
}

impl EmbeddingServerTransport for WatchdogTransport {
    fn bind(&self) -> std::result::Result<EmbeddingServerBindOutcome, EmbeddingTransportFailure> {
        Err(EmbeddingTransportFailure {
            code: "test".into(),
            message: "not used".into(),
        })
    }

    fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
        self.clock.clone()
    }

    fn fail_stop(&self, _reason_code: &str) {
        self.fail_stops.fetch_add(1, Ordering::AcqRel);
    }
}

pub(super) struct PollingStream {
    pub(super) inner: MemoryStream,
    pub(super) pending_reads: usize,
}

impl Read for PollingStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.pending_reads != 0 {
            self.pending_reads -= 1;
            return Err(io::Error::new(io::ErrorKind::TimedOut, "poll"));
        }
        self.inner.read(buffer)
    }
}

impl Write for PollingStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl EmbeddingServerStream for PollingStream {
    fn transport_identity(&self) -> &EmbeddingTransportIdentity {
        self.inner.transport_identity()
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_write_timeout(timeout)
    }

    fn peer_is_alive(&self) -> io::Result<bool> {
        Ok(true)
    }

    fn shutdown(&self) -> io::Result<()> {
        Ok(())
    }
}
