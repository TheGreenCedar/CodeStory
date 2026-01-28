use crate::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub message: String,
    pub file_id: Option<NodeId>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub is_fatal: bool,
    pub index_step: IndexStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IndexStep {
    Collection,
    Indexing,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrorFilter {
    /// Show only fatal errors
    pub fatal_only: bool,
    /// Show only errors from the indexing step (vs collection)
    pub indexed_only: bool,
}
