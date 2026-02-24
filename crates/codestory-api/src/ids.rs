use crate::errors::ApiError;
use codestory_core as core;
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn to_core(&self) -> Result<core::NodeId, ApiError> {
        let raw = self.0.trim();
        let parsed = raw
            .parse::<i64>()
            .map_err(|_| ApiError::invalid_argument(format!("Invalid NodeId: {raw}")))?;
        Ok(core::NodeId(parsed))
    }
}

impl From<core::NodeId> for NodeId {
    fn from(value: core::NodeId) -> Self {
        Self(value.0.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type)]
#[serde(transparent)]
pub struct EdgeId(pub String);

impl EdgeId {
    pub fn to_core(&self) -> Result<core::EdgeId, ApiError> {
        let raw = self.0.trim();
        let parsed = raw
            .parse::<i64>()
            .map_err(|_| ApiError::invalid_argument(format!("Invalid EdgeId: {raw}")))?;
        Ok(core::EdgeId(parsed))
    }
}

impl From<core::EdgeId> for EdgeId {
    fn from(value: core::EdgeId) -> Self {
        Self(value.0.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id_round_trip() {
        let core_id = core::NodeId(123);
        let api_id = NodeId::from(core_id);
        assert_eq!(api_id.0, "123");

        let parsed = api_id.to_core().expect("NodeId should parse");
        assert_eq!(parsed.0, 123);
    }

    #[test]
    fn test_node_id_invalid_returns_api_error() {
        let api_id = NodeId("not_a_number".to_string());
        let err = api_id.to_core().expect_err("Expected parse error");
        assert_eq!(err.code, "invalid_argument");
    }

    #[test]
    fn test_edge_id_round_trip() {
        let core_id = core::EdgeId(456);
        let api_id = EdgeId::from(core_id);
        assert_eq!(api_id.0, "456");

        let parsed = api_id.to_core().expect("EdgeId should parse");
        assert_eq!(parsed.0, 456);
    }

    #[test]
    fn test_edge_id_invalid_returns_api_error() {
        let api_id = EdgeId("nope".to_string());
        let err = api_id.to_core().expect_err("Expected parse error");
        assert_eq!(err.code, "invalid_argument");
    }
}
