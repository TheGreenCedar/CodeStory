use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IndexingPhaseTimings {
    pub parse_index_ms: u32,
    pub projection_flush_ms: u32,
    pub edge_resolution_ms: u32,
    pub error_flush_ms: u32,
    pub cleanup_ms: u32,
    pub cache_refresh_ms: Option<u32>,
    pub unresolved_calls_start: u32,
    pub unresolved_imports_start: u32,
    pub resolved_calls: u32,
    pub resolved_imports: u32,
    pub unresolved_calls_end: u32,
    pub unresolved_imports_end: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "data")]
pub enum AppEventPayload {
    // Use u32 so TS can safely represent these as `number` without BigInt.
    IndexingStarted {
        file_count: u32,
    },
    IndexingProgress {
        current: u32,
        total: u32,
    },
    IndexingComplete {
        duration_ms: u32,
        phase_timings: IndexingPhaseTimings,
    },
    IndexingFailed {
        error: String,
    },
    StatusUpdate {
        message: String,
    },
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
