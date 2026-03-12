use crate::{StorageError, Store};
use codestory_contracts::graph::{Edge, Node, NodeId, Occurrence};

pub struct GraphStore<'a> {
    storage: &'a Store,
}

impl<'a> GraphStore<'a> {
    pub(crate) fn new(storage: &'a Store) -> Self {
        Self { storage }
    }

    pub fn nodes(&self) -> Result<Vec<Node>, StorageError> {
        self.storage.get_nodes()
    }

    pub fn edges_for_node(&self, node_id: NodeId) -> Result<Vec<Edge>, StorageError> {
        self.storage.get_edges_for_node_id(node_id)
    }

    pub fn occurrences_for_node(&self, node_id: NodeId) -> Result<Vec<Occurrence>, StorageError> {
        self.storage.get_occurrences_for_node(node_id)
    }
}
