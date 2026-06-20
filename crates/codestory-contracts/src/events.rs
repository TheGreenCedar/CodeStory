//! Runtime event contracts and in-process fanout.
//!
//! Events are lightweight status signals for progress, warnings, and telemetry.
//! They are not durable audit records; consumers that need complete evidence
//! should use the corresponding DTO, log, or artifact contract instead.

use crossbeam_channel::{Receiver, Sender, unbounded};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

pub mod telemetry;

pub use telemetry::{
    CommandLifecycle, CommandTelemetry, command_failure, command_start, command_success,
    new_correlation_id,
};

/// User-visible runtime status event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    IndexingStarted { file_count: usize },
    IndexingProgress { current: usize, total: usize },
    IndexingComplete { duration_ms: u64 },
    ShowWarning { message: String },
    StatusUpdate { message: String },
}

/// In-process fanout bus for runtime events.
///
/// Publishing is best-effort: closed subscribers are dropped, and send failures
/// are intentionally not surfaced to producers.
#[derive(Clone)]
pub struct EventBus {
    tx: Sender<Event>,
    subscribers: Arc<Mutex<Vec<Sender<Event>>>>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, ingress_rx) = unbounded::<Event>();
        let subscribers = Arc::new(Mutex::new(Vec::<Sender<Event>>::new()));
        let subscribers_for_thread = Arc::clone(&subscribers);

        std::thread::spawn(move || {
            while let Ok(event) = ingress_rx.recv() {
                if let Ok(mut sinks) = subscribers_for_thread.lock() {
                    sinks.retain(|sink| sink.send(event.clone()).is_ok());
                }
            }
        });

        Self { tx, subscribers }
    }

    pub fn receiver(&self) -> Receiver<Event> {
        let (tx, rx) = unbounded();
        if let Ok(mut sinks) = self.subscribers.lock() {
            sinks.push(tx);
        }
        rx
    }

    pub fn publish(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_bus_fanout() {
        let bus = EventBus::new();
        let a = bus.receiver();
        let b = bus.receiver();

        bus.publish(Event::StatusUpdate {
            message: "fanout".to_string(),
        });

        assert!(matches!(
            a.recv().expect("receive a"),
            Event::StatusUpdate { message } if message == "fanout"
        ));
        assert!(matches!(
            b.recv().expect("receive b"),
            Event::StatusUpdate { message } if message == "fanout"
        ));
    }
}
