//! Shared JSON contract between the external qualification driver and packaged CLI worker.

use super::{
    EmbeddingCapacityPressureWire, EmbeddingEngineIdentity, EmbeddingProtocolError,
    EmbeddingProtocolResponse, EmbeddingQualificationParameters, EmbeddingQualificationResult,
    EmbeddingServerClockSnapshot, EmbeddingServerSnapshot, EmbeddingTransportIdentity,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Version 2 adds the measurement-span contract: measurement workers stamp the
/// measurement protocol's declared start/end instants themselves and return
/// them as an exclusive `measurement` payload. Both the packaged worker and
/// the external driver reject any other version, so a cross-version
/// producer/consumer mix fails closed instead of silently recording the
/// whole-worker window as a metric sample.
pub const EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationWorkerRequest {
    pub schema_version: u32,
    pub nonce_sha256: String,
    pub executable_sha256: String,
    pub project: PathBuf,
    pub operation: String,
    pub parameters: EmbeddingQualificationParameters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_gate: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_gate_timeout_ms: Option<u64>,
    /// Measurement workloads only: the protocol workload id that seeds the
    /// deterministic `sha256_counter_utf8_v1` input generator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_id: Option<String>,
    /// Measurement workloads only: the 1-based repeat ordinal that seeds the
    /// deterministic input generator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// `measure_busy_retry` only: the private-directory file the worker
    /// creates once the typed capacity retry was validated, signalling the
    /// driver to release the held classes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_marker: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationWorkerOutput {
    pub schema_version: u32,
    pub pid: u32,
    pub process_start_id: String,
    pub executable_sha256: String,
    pub executable_version: String,
    pub project_identity_sha256: String,
    pub clock: EmbeddingServerClockSnapshot,
    pub started_ns: u64,
    pub finished_ns: u64,
    pub inclusive_clock_api: String,
    pub inclusive_started_ns: u64,
    pub inclusive_finished_ns: u64,
    pub boot_id_started: String,
    pub boot_id_finished: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<EmbeddingQualificationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_exchange: Option<EmbeddingQualificationWorkerProtocolExchange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_operations: Option<Vec<EmbeddingQualificationWorkerQueueOperation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_identity: Option<EmbeddingEngineIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<EmbeddingQualificationWorkerMeasurement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EmbeddingQualificationWorkerError>,
}

/// One measurement span stamped at the measurement protocol's declared start
/// and end instants, on the worker's own awake-monotonic clock (the same
/// clock as `EmbeddingQualificationWorkerOutput::clock`). The
/// suspend-inclusive readings and boot ids are sampled at the same instants
/// as the awake readings because the checker requires the suspend witness to
/// cover exactly the recorded phase timestamps; the whole-worker readings in
/// the output cannot serve a sub-window.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationWorkerMeasurementSpan {
    pub awake_started_ns: u64,
    pub awake_finished_ns: u64,
    pub inclusive_started_ns: u64,
    pub inclusive_finished_ns: u64,
    pub boot_id_started: String,
    pub boot_id_finished: String,
}

/// Result of one measurement operation: the declared-instant span plus the
/// evidence the driver needs to record the sample (server identity witness,
/// per-metric operands, and busy-retry pressure evidence).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationWorkerMeasurement {
    pub span: EmbeddingQualificationWorkerMeasurementSpan,
    /// Server identity witness observed by the worker for this sample.
    pub snapshot: EmbeddingServerSnapshot,
    /// Engine identity validated inside the measured exchange (frame and
    /// residency measurements).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_identity: Option<EmbeddingEngineIdentity>,
    /// Request id of the measured protocol exchange (frame measurements), for
    /// driver-side token accounting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Documents validated inside the measured window (bulk frame
    /// measurements).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_documents: Option<u64>,
    /// The typed capacity pressure that opened the busy-retry window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure: Option<EmbeddingCapacityPressureWire>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationWorkerProtocolExchange {
    pub request_id: String,
    pub submitted_ns: u64,
    pub finished_ns: u64,
    pub transport_identity: EmbeddingTransportIdentity,
    pub hello_snapshot: EmbeddingServerSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_snapshot: Option<EmbeddingServerSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<EmbeddingProtocolResponse>,
    pub response_payload_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_transport_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationWorkerError {
    pub code: String,
    pub message_head: String,
    pub retry_class: String,
    pub retry_after_ms: u64,
    pub retry_condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<EmbeddingCapacityPressureWire>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationWorkerQueueOperation {
    pub correlation_id: String,
    pub project_identity_sha256: String,
    pub class: String,
    pub ordinal: u32,
    #[serde(default)]
    pub submission_batch: u32,
    pub submitted_ns: u64,
    pub completed_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_completion_sequence: Option<u64>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EmbeddingProtocolError>,
    pub response_payload_bytes: u64,
    pub transport_identity: EmbeddingTransportIdentity,
    pub hello_snapshot: EmbeddingServerSnapshot,
}

#[cfg(test)]
mod tests {
    use super::{
        EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION, EmbeddingQualificationParameters,
        EmbeddingQualificationWorkerError, EmbeddingQualificationWorkerMeasurementSpan,
        EmbeddingQualificationWorkerOutput, EmbeddingQualificationWorkerRequest,
        EmbeddingServerClockSnapshot,
    };
    use serde_json::{Value, json};
    use std::path::PathBuf;

    #[test]
    fn worker_request_round_trips_one_strict_schema() {
        let request = EmbeddingQualificationWorkerRequest {
            schema_version: EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
            nonce_sha256: "a".repeat(64),
            executable_sha256: "b".repeat(64),
            project: PathBuf::from("/private/project"),
            operation: "query".into(),
            parameters: EmbeddingQualificationParameters {
                query_count: 1,
                bulk_count: 0,
                documents_per_bulk: 0,
                input_bytes: 32,
                hold_ms: 0,
            },
            start_gate: None,
            start_gate_timeout_ms: None,
            workload_id: None,
            repeat: None,
            retry_marker: None,
        };

        let encoded = serde_json::to_value(&request).expect("serialize worker request");
        assert_eq!(
            encoded
                .as_object()
                .expect("request object")
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            [
                "executable_sha256",
                "nonce_sha256",
                "operation",
                "parameters",
                "project",
                "schema_version",
            ]
            .into_iter()
            .collect()
        );
        let decoded: EmbeddingQualificationWorkerRequest =
            serde_json::from_value(encoded.clone()).expect("deserialize worker request");
        assert_eq!(decoded.operation, request.operation);
        assert_eq!(decoded.project, request.project);

        let mut incompatible = encoded;
        incompatible
            .as_object_mut()
            .expect("request object")
            .insert("unexpected".into(), Value::Bool(true));
        assert!(
            serde_json::from_value::<EmbeddingQualificationWorkerRequest>(incompatible).is_err(),
            "the shared worker contract must reject fields unknown to either executable"
        );
    }

    #[test]
    fn worker_output_round_trips_one_payload_shape() {
        let output = EmbeddingQualificationWorkerOutput {
            schema_version: EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
            pid: 42,
            process_start_id: "process-start".into(),
            executable_sha256: "b".repeat(64),
            executable_version: "0.16.0".into(),
            project_identity_sha256: "c".repeat(64),
            clock: EmbeddingServerClockSnapshot {
                domain: "awake_monotonic".into(),
                api: "test".into(),
                boot_id: "boot".into(),
                resolution_ns: 1,
            },
            started_ns: 10,
            finished_ns: 20,
            inclusive_clock_api: "inclusive-test".into(),
            inclusive_started_ns: 9,
            inclusive_finished_ns: 21,
            boot_id_started: "boot".into(),
            boot_id_finished: "boot".into(),
            result: None,
            protocol_exchange: None,
            queue_operations: None,
            engine_identity: None,
            measurement: None,
            error: Some(EmbeddingQualificationWorkerError {
                code: "capacity".into(),
                message_head: "busy".into(),
                retry_class: "bounded".into(),
                retry_after_ms: 25,
                retry_condition: "capacity_available".into(),
                capacity: None,
            }),
        };

        let encoded = serde_json::to_value(&output).expect("serialize worker output");
        assert_eq!(encoded["error"]["code"], json!("capacity"));
        for omitted in [
            "result",
            "protocol_exchange",
            "queue_operations",
            "engine_identity",
            "measurement",
        ] {
            assert!(
                encoded.get(omitted).is_none(),
                "absent worker payload {omitted} must remain omitted"
            );
        }
        let decoded: EmbeddingQualificationWorkerOutput =
            serde_json::from_value(encoded).expect("deserialize worker output");
        assert_eq!(
            decoded.error.expect("worker error").retry_after_ms,
            output.error.expect("source worker error").retry_after_ms
        );
    }

    #[test]
    fn measurement_request_fields_round_trip_and_stay_strict() {
        let request = EmbeddingQualificationWorkerRequest {
            schema_version: EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
            nonce_sha256: "a".repeat(64),
            executable_sha256: "b".repeat(64),
            project: PathBuf::from("/private/project"),
            operation: "measure_busy_retry".into(),
            parameters: EmbeddingQualificationParameters {
                query_count: 65,
                bulk_count: 0,
                documents_per_bulk: 0,
                input_bytes: 256,
                hold_ms: 0,
            },
            start_gate: None,
            start_gate_timeout_ms: None,
            workload_id: Some("saturated_query_65th_retry_v1".into()),
            repeat: Some(2),
            retry_marker: Some(PathBuf::from("/private/dir/marker.json")),
        };
        let encoded = serde_json::to_value(&request).expect("serialize measure request");
        assert_eq!(
            encoded["workload_id"],
            json!("saturated_query_65th_retry_v1")
        );
        assert_eq!(encoded["repeat"], json!(2));
        let decoded: EmbeddingQualificationWorkerRequest =
            serde_json::from_value(encoded).expect("deserialize measure request");
        assert_eq!(decoded.workload_id, request.workload_id);
        assert_eq!(decoded.repeat, request.repeat);
        assert_eq!(decoded.retry_marker, request.retry_marker);
    }

    #[test]
    fn measurement_span_round_trips_the_exact_witness_shape() {
        let span = EmbeddingQualificationWorkerMeasurementSpan {
            awake_started_ns: 10,
            awake_finished_ns: 20,
            inclusive_started_ns: 9,
            inclusive_finished_ns: 21,
            boot_id_started: "boot".into(),
            boot_id_finished: "boot".into(),
        };
        let encoded = serde_json::to_value(&span).expect("serialize measurement span");
        assert_eq!(
            encoded
                .as_object()
                .expect("span object")
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            [
                "awake_started_ns",
                "awake_finished_ns",
                "inclusive_started_ns",
                "inclusive_finished_ns",
                "boot_id_started",
                "boot_id_finished",
            ]
            .into_iter()
            .collect()
        );
        let mut incompatible = encoded;
        incompatible
            .as_object_mut()
            .expect("span object")
            .insert("unexpected".into(), Value::Bool(true));
        assert!(
            serde_json::from_value::<EmbeddingQualificationWorkerMeasurementSpan>(incompatible)
                .is_err(),
            "the measurement span must reject fields unknown to either executable"
        );
    }
}
