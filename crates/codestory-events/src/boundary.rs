use anyhow::Result;
use codestory_core::NodeId;
use crossbeam_channel::{unbounded, Receiver, Sender};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::{ActivationOrigin, RefreshMode};

pub type EventStream = Receiver<AppEvent>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshWorkspaceCmd {
    pub path: PathBuf,
    pub mode: RefreshMode,
    pub correlation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFileCmd {
    pub path: PathBuf,
    pub correlation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivateNodeCmd {
    pub id: NodeId,
    pub origin: ActivationOrigin,
    pub correlation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AppCommand {
    RefreshWorkspace(RefreshWorkspaceCmd),
    DeleteFile(DeleteFileCmd),
    ActivateNode(ActivateNodeCmd),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileProjectionRemovedEvt {
    pub canonical_file_node_id: NodeId,
    pub removed_node_count: usize,
    pub removed_edge_count: usize,
    pub removed_occurrence_count: usize,
    pub path: Option<PathBuf>,
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeUpsertedEvt {
    pub node_id: NodeId,
    pub file_node_id: Option<NodeId>,
    pub kind: String,
    pub path: Option<PathBuf>,
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexBatchFlushedEvt {
    pub flushed_nodes: usize,
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationFailedEvt {
    pub command: String,
    pub reason: String,
    pub correlation_id: Option<String>,
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AppEvent {
    FileProjectionRemoved(FileProjectionRemovedEvt),
    NodeUpserted(NodeUpsertedEvt),
    IndexBatchFlushed(IndexBatchFlushedEvt),
    OperationFailed(OperationFailedEvt),
}

pub trait EventBusBoundary {
    fn publish_command(&self, command: AppCommand) -> Result<()>;
    fn subscribe_events(&self) -> EventStream;
}

#[derive(Clone)]
pub struct InMemoryBoundary {
    command_tx: Sender<AppCommand>,
    command_rx: Receiver<AppCommand>,
    event_tx: Sender<AppEvent>,
    event_rx: Receiver<AppEvent>,
}

impl Default for InMemoryBoundary {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryBoundary {
    pub fn new() -> Self {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();

        Self {
            command_tx,
            command_rx,
            event_tx,
            event_rx,
        }
    }

    pub fn command_receiver(&self) -> Receiver<AppCommand> {
        self.command_rx.clone()
    }

    pub fn publish_event(&self, event: AppEvent) -> Result<()> {
        self.event_tx
            .send(event)
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}

impl EventBusBoundary for InMemoryBoundary {
    fn publish_command(&self, command: AppCommand) -> Result<()> {
        self.command_tx
            .send(command)
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    fn subscribe_events(&self) -> EventStream {
        self.event_rx.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_bus_roundtrip() {
        let bus = InMemoryBoundary::new();
        let cmd = AppCommand::ActivateNode(ActivateNodeCmd {
            id: NodeId(123),
            origin: ActivationOrigin::Search,
            correlation_id: "corr-1".to_string(),
        });

        bus.publish_command(cmd.clone()).expect("publish command");
        let received = bus.command_receiver().recv().expect("receive command");

        match received {
            AppCommand::ActivateNode(inner) => {
                assert_eq!(inner.id, NodeId(123));
                assert_eq!(inner.origin, ActivationOrigin::Search);
            }
            _ => panic!("unexpected command variant"),
        }

        let event = AppEvent::IndexBatchFlushed(IndexBatchFlushedEvt {
            flushed_nodes: 42,
            correlation_id: Some("corr-1".to_string()),
        });
        bus.publish_event(event.clone()).expect("publish event");
        let received = bus.subscribe_events().recv().expect("receive event");
        assert!(matches!(
            received,
            AppEvent::IndexBatchFlushed(IndexBatchFlushedEvt {
                flushed_nodes: 42,
                ..
            })
        ));
    }
}
