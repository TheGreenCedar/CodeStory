use crate::NodeKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeType {
    pub kind: NodeKind,
    pub bundle_info: Option<BundleInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleInfo {
    pub is_bundle: bool,
    pub bundle_id: Option<i64>,
    pub layout_vertical: bool,
    pub connected_count: usize,
}
