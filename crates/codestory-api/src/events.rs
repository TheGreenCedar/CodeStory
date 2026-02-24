use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "data")]
pub enum AppEventPayload {
    // Use u32 so TS can safely represent these as `number` without BigInt.
    IndexingStarted { file_count: u32 },
    IndexingProgress { current: u32, total: u32 },
    IndexingComplete { duration_ms: u32 },
    IndexingFailed { error: String },
    StatusUpdate { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_payload_serializes_with_type_and_data() {
        let ev = AppEventPayload::IndexingStarted { file_count: 3 };
        let v = serde_json::to_value(ev).expect("serialize");
        assert_eq!(v["type"], "IndexingStarted");
        assert_eq!(v["data"]["file_count"], 3);
    }
}
