use crate::Store;
use crate::{LlmSymbolDoc, LlmSymbolDocStats, StorageError};
use codestory_contracts::graph::NodeId;

pub struct SearchDocStore<'a> {
    storage: &'a mut Store,
}

impl<'a> SearchDocStore<'a> {
    pub(crate) fn new(storage: &'a mut Store) -> Self {
        Self { storage }
    }

    pub fn get_all(&self) -> Result<Vec<LlmSymbolDoc>, StorageError> {
        self.storage.get_all_llm_symbol_docs()
    }

    pub fn get_stats(&self) -> Result<LlmSymbolDocStats, StorageError> {
        self.storage.get_llm_symbol_doc_stats()
    }

    pub fn get_by_node_ids(&self, node_ids: &[NodeId]) -> Result<Vec<LlmSymbolDoc>, StorageError> {
        self.storage.get_llm_symbol_docs_by_node_ids(node_ids)
    }

    pub fn delete_for_file(&mut self, file_node_id: NodeId) -> Result<(), StorageError> {
        let storage = &mut *self.storage;
        storage
            .delete_llm_symbol_docs_for_file(file_node_id)
            .map(|_| ())
    }

    pub fn upsert_batch(&mut self, docs: &[LlmSymbolDoc]) -> Result<(), StorageError> {
        let storage = &mut *self.storage;
        storage.upsert_llm_symbol_docs_batch(docs)
    }
}
