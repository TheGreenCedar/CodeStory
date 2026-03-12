use crate::{FileInfo, ProjectionFlushBreakdown, StorageError, Store};
use codestory_contracts::graph::{
    AccessKind, CallableProjectionState, Edge, Node, NodeId, Occurrence,
};

pub struct ProjectionStore<'a> {
    storage: &'a mut Store,
}

pub struct ProjectionBatch<'a> {
    pub files: &'a [FileInfo],
    pub nodes: &'a [Node],
    pub edges: &'a [Edge],
    pub occurrences: &'a [Occurrence],
    pub component_access: &'a [(NodeId, AccessKind)],
    pub callable_projection_states: &'a [CallableProjectionState],
}

impl<'a> ProjectionStore<'a> {
    pub(crate) fn new(storage: &'a mut Store) -> Self {
        Self { storage }
    }

    pub fn get_callable_projection_states_for_file(
        &self,
        file_node_id: NodeId,
    ) -> Result<Vec<CallableProjectionState>, StorageError> {
        self.storage
            .get_callable_projection_states_for_file(file_node_id.0)
    }

    pub fn flush_projection_batch(
        &mut self,
        batch: ProjectionBatch<'_>,
    ) -> Result<ProjectionFlushBreakdown, StorageError> {
        self.storage
            .flush_projection_batch(crate::storage_impl::ProjectionBatch {
                files: batch.files,
                nodes: batch.nodes,
                edges: batch.edges,
                occurrences: batch.occurrences,
                component_access: batch.component_access,
                callable_projection_states: batch.callable_projection_states,
            })
    }
}
