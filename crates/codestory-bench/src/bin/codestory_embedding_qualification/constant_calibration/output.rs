use super::request::ConstantCalibrationPackage;
use crate::qualification::request::{
    QualificationContracts, QualificationRuntime, QualificationSource,
};
use crate::qualification::scenarios::artifact::{
    ConstantCalibrationRunArtifact, RawServerIdentity,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct ConstantCalibrationRawOutput {
    pub(super) schema_version: u32,
    pub(super) source: QualificationSource,
    pub(super) package: ConstantCalibrationPackage,
    pub(super) contracts: QualificationContracts,
    pub(super) runtime: QualificationRuntime,
    pub(super) request_sha256: String,
    pub(super) calibration_runs: Vec<ConstantCalibrationRunSummary>,
}

#[derive(Debug, Serialize)]
pub(super) struct ConstantCalibrationRunSummary {
    pub(super) run_index: u32,
    pub(super) measurements: ConstantCalibrationMeasurementsSummary,
    pub(super) server_identities: Vec<RawServerIdentity>,
    pub(super) backend: String,
    pub(super) policy: String,
    pub(super) model_sha256: String,
    pub(super) materialized_reused: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct ConstantCalibrationMeasurementsSummary {
    pub(super) artifact: String,
    pub(super) metric_count: u64,
    pub(super) sample_count: u64,
}

impl ConstantCalibrationRunSummary {
    pub(super) fn from_artifact(
        artifact: &ConstantCalibrationRunArtifact,
        artifact_name: String,
    ) -> Self {
        Self {
            run_index: artifact.run_index(),
            measurements: ConstantCalibrationMeasurementsSummary {
                artifact: artifact_name,
                metric_count: artifact.metric_count(),
                sample_count: artifact.sample_count(),
            },
            server_identities: artifact.server_identities().to_vec(),
            backend: artifact.backend().into(),
            policy: artifact.policy().into(),
            model_sha256: artifact.model_sha256().into(),
            materialized_reused: artifact.materialized_reused(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConstantCalibrationMeasurementsSummary, ConstantCalibrationRawOutput,
        ConstantCalibrationRunSummary,
    };
    use crate::qualification::constant_calibration::request::ConstantCalibrationPackage;
    use crate::qualification::request::{
        QualificationContracts, QualificationRuntime, QualificationSource,
    };
    use crate::qualification::scenarios::artifact::RawServerIdentity;

    #[test]
    fn output_schema_contains_three_constant_runs_and_no_qualification_surfaces() {
        let run = |run_index| ConstantCalibrationRunSummary {
            run_index,
            measurements: ConstantCalibrationMeasurementsSummary {
                artifact: format!("constant-calibration-run-{run_index}.raw.json"),
                metric_count: 9,
                sample_count: 9,
            },
            server_identities: vec![RawServerIdentity {
                server_instance_id: format!("server-{run_index}"),
                process_start_id: format!("boot:{run_index}"),
                load_generation: 1,
            }],
            backend: "metal".into(),
            policy: "accelerated".into(),
            model_sha256: "a".repeat(64),
            materialized_reused: run_index > 1,
        };
        let output = ConstantCalibrationRawOutput {
            schema_version: 1,
            source: QualificationSource {
                commit: "b".repeat(40),
                tree: "c".repeat(40),
                tracked_dirty: false,
            },
            package: ConstantCalibrationPackage {
                archive_sha256: "d".repeat(64),
                executable_sha256: "e".repeat(64),
                asset_target: "macos-arm64".into(),
                release_version: "0.16.3".into(),
                model_sha256: "a".repeat(64),
            },
            contracts: QualificationContracts {
                protocol_sha256: "1".repeat(64),
                constant_set_sha256: "2".repeat(64),
                measurement_protocol_sha256: "3".repeat(64),
            },
            runtime: QualificationRuntime {
                engine_policy: "accelerated".into(),
                expected_backend: "metal".into(),
                offline: true,
                matrix_cell_id: "protected_macos_arm64_metal".into(),
                cache_state: "reused".into(),
                residency_state: "resident".into(),
            },
            request_sha256: "f".repeat(64),
            calibration_runs: vec![run(1), run(2), run(3)],
        };
        let value = serde_json::to_value(output).expect("serialize output");
        assert_eq!(
            value
                .as_object()
                .expect("output object")
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            [
                "calibration_runs",
                "contracts",
                "package",
                "request_sha256",
                "runtime",
                "schema_version",
                "source",
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(
            value["calibration_runs"]
                .as_array()
                .expect("calibration runs")
                .len(),
            3
        );
        assert!(value.get("scenarios").is_none());
        assert!(value.get("qualification_thresholds").is_none());
    }
}
