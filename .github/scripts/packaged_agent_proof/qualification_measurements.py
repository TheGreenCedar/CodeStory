"""Measurement artifact normalization for qualification evidence."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path

from .foundation import (
    EXTERNAL_QUALIFICATION_METRICS,
    TARGET_CONTRACTS,
    require,
)
from .contracts import (
    qualification_measurement_sample_value,
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
    require_opaque_identifier,
    require_positive_int,
    selected_qualification_matrix_cell,
)
from .qualification_documents import (
    PrivateJsonArtifact,
    PrivateJsonMessages,
    _private_json_artifact,
)


@dataclass(frozen=True)
class QualificationMeasurementSummary:
    name: str
    metric_count: int
    sample_count: int


@dataclass(frozen=True)
class MeasurementValidationContract:
    contracts: dict
    protocol: dict
    metric_contracts: dict
    phase_boundaries: dict
    matrix_cell_id: str
    matrix_cell: dict
    expected_policy: str
    expected_backend: str
    raw_metric_names: frozenset[str]
    allowed_awake_apis: frozenset[str]
    inclusive_api: str
    maximum_suspend_ns: int


@dataclass(frozen=True)
class QualificationMeasurementSample:
    sample_id: str
    server_identity: tuple[str, str, int]
    value: float | int


@dataclass(frozen=True)
class QualificationMetricMeasurement:
    name: str
    samples: tuple[QualificationMeasurementSample, ...]
    value: float | int


@dataclass(frozen=True)
class QualificationMeasurementEvidence:
    summary: QualificationMeasurementSummary
    document: PrivateJsonArtifact
    validation: MeasurementValidationContract
    metrics: tuple[QualificationMetricMeasurement, ...]

    @property
    def values(self) -> dict[str, float | int]:
        return {metric.name: metric.value for metric in self.metrics}

    @property
    def sample_count(self) -> int:
        return sum(len(metric.samples) for metric in self.metrics)


def _normalized_measurement_summary(
    summary: object,
) -> QualificationMeasurementSummary:
    require(
        isinstance(summary, dict),
        "qualification measurement summary is malformed",
    )
    require_exact_keys(
        summary,
        {"artifact", "metric_count", "sample_count"},
        "qualification measurement summary",
    )
    name = require_nonempty_string(
        summary["artifact"],
        "qualification measurement artifact",
    )
    relative = Path(name)
    require(
        not relative.is_absolute()
        and len(relative.parts) == 1
        and relative.name == name
        and name == "measurements.raw.json",
        "qualification measurement artifact must be measurements.raw.json",
    )
    return QualificationMeasurementSummary(
        name,
        require_nonnegative_int(
            summary["metric_count"],
            "qualification measurement summary metric_count",
        ),
        require_nonnegative_int(
            summary["sample_count"],
            "qualification measurement summary sample_count",
        ),
    )


def _qualification_measurement_document(
    artifact_root: Path,
    summary: object,
    *,
    contracts: dict,
    forbidden_values: list[str],
) -> tuple[QualificationMeasurementSummary, PrivateJsonArtifact]:
    normalized_summary = _normalized_measurement_summary(summary)
    document = _private_json_artifact(
        artifact_root,
        normalized_summary.name,
        forbidden_values=forbidden_values,
        messages=PrivateJsonMessages(
            missing_or_unsafe=(
                "qualification measurement artifact is missing or unsafe"
            ),
            escaped="qualification measurement artifact is missing or unsafe",
            leaked=(
                "qualification measurement artifact leaked private request material"
            ),
            invalid_json="qualification measurement artifact is not valid JSON",
            non_object="qualification measurement artifact must be an object",
        ),
    )
    payload = document.payload
    require_exact_keys(
        payload,
        {"schema_version", "contracts", "external_metrics", "metrics"},
        "qualification measurement artifact",
    )
    require(
        payload["schema_version"] == 2,
        "qualification measurement schema is unsupported",
    )
    require(
        payload["contracts"] == contracts,
        "qualification measurements used stale contracts",
    )
    require(
        payload["external_metrics"] == sorted(EXTERNAL_QUALIFICATION_METRICS),
        "qualification measurements changed the externally owned metric set",
    )
    return normalized_summary, document


def _measurement_validation_contract(
    *,
    contracts: dict,
    measurement_contract: dict,
    target: str,
    proof_tier: str,
    matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
) -> MeasurementValidationContract:
    protocol = measurement_contract["measurement_protocol"]
    clock_policy = protocol["clock_policy"]
    target_os = TARGET_CONTRACTS[target]["target_os"]
    suspend_contract = clock_policy["suspend_detection"]
    return MeasurementValidationContract(
        contracts=contracts,
        protocol=protocol,
        metric_contracts=protocol["metric_contracts"],
        phase_boundaries=protocol["phase_boundaries"],
        matrix_cell_id=matrix_cell_id,
        matrix_cell=selected_qualification_matrix_cell(
            protocol,
            cell_id=matrix_cell_id,
            target=target,
            proof_tier=proof_tier,
            expected_policy=expected_policy,
            expected_backend=expected_backend,
        ),
        expected_policy=expected_policy,
        expected_backend=expected_backend,
        raw_metric_names=frozenset(
            set(protocol["required_metrics"]) - EXTERNAL_QUALIFICATION_METRICS
        ),
        allowed_awake_apis=frozenset(
            clock_policy["platform_apis"][target_os]
        ),
        inclusive_api=suspend_contract["platform_apis"][target_os],
        maximum_suspend_ns=require_nonnegative_int(
            suspend_contract["maximum_inclusive_minus_awake_ns"],
            "measurement suspend-detection tolerance",
        ),
    )


_MEASUREMENT_SAMPLE_FIELDS = {
    "sample_id",
    "repeat",
    "matrix_cell_id",
    "workload_id",
    "cache_state",
    "residency_state",
    "process",
    "server_identity",
    "clock",
    "start",
    "end",
    "operands",
    "suspend_witness",
}


def _qualification_measurement_sample(
    metric: str,
    sample: object,
    *,
    sample_index: int,
    validation: MeasurementValidationContract,
) -> QualificationMeasurementSample:
    field = f"qualification measurement {metric} sample {sample_index}"
    require(isinstance(sample, dict), f"{field} is malformed")
    require_exact_keys(sample, _MEASUREMENT_SAMPLE_FIELDS, field)
    sample_id = require_opaque_identifier(
        sample["sample_id"],
        f"qualification measurement {metric} sample_id",
    )
    require(
        sample["repeat"] == sample_index + 1,
        f"qualification measurement {metric} repeat sequence is not exact",
    )
    require(
        sample["matrix_cell_id"] == validation.matrix_cell_id,
        f"qualification measurement {metric} used the wrong host/package matrix cell",
    )
    require(
        sample["workload_id"]
        == validation.protocol["workloads"][metric]["workload_id"],
        f"qualification measurement {metric} used the wrong workload",
    )
    require(
        sample["cache_state"] == validation.matrix_cell["cache_state"]
        and sample["residency_state"]
        == validation.matrix_cell["residency_state"],
        f"qualification measurement {metric} changed cache or residency state",
    )
    server_identity = sample["server_identity"]
    require(
        isinstance(server_identity, dict),
        f"qualification measurement {metric} server identity is malformed",
    )
    require_exact_keys(
        server_identity,
        {"server_instance_id", "process_start_id", "load_generation"},
        f"qualification measurement {metric} server identity",
    )
    identity = (
        require_opaque_identifier(
            server_identity["server_instance_id"],
            f"qualification measurement {metric} server_instance_id",
        ),
        require_nonempty_string(
            server_identity["process_start_id"],
            f"qualification measurement {metric} server process_start_id",
        ),
        require_positive_int(
            server_identity["load_generation"],
            f"qualification measurement {metric} server load_generation",
        ),
    )
    value = qualification_measurement_sample_value(
        metric,
        sample,
        contracts=validation.contracts,
        phase_boundaries=validation.phase_boundaries,
        allowed_awake_apis=set(validation.allowed_awake_apis),
        inclusive_api=validation.inclusive_api,
        maximum_suspend_ns=validation.maximum_suspend_ns,
        expected_policy=validation.expected_policy,
        expected_backend=validation.expected_backend,
    )
    return QualificationMeasurementSample(sample_id, identity, value)


def _qualification_metric_measurement(
    metric: str,
    record: object,
    *,
    validation: MeasurementValidationContract,
) -> QualificationMetricMeasurement:
    require(
        isinstance(record, dict),
        f"qualification measurement {metric} is malformed",
    )
    require_exact_keys(
        record,
        {"unit", "samples"},
        f"qualification measurement {metric}",
    )
    require(
        record["unit"] == validation.metric_contracts[metric]["unit"],
        f"qualification measurement {metric} used the wrong unit",
    )
    samples = record["samples"]
    sample_policy = validation.protocol["metric_sampling"][metric]
    require(
        isinstance(samples, list)
        and len(samples) == sample_policy["sample_count"],
        f"qualification measurement {metric} sample count changed",
    )
    normalized = tuple(
        _qualification_measurement_sample(
            metric,
            sample,
            sample_index=index,
            validation=validation,
        )
        for index, sample in enumerate(samples)
    )
    sample_ids = [sample.sample_id for sample in normalized]
    require(
        len(set(sample_ids)) == len(sample_ids),
        f"qualification measurement {metric} duplicated a sample id",
    )
    identities = [sample.server_identity for sample in normalized]
    if sample_policy.get("independence") == "distinct_server_instance_per_sample":
        require(
            len({identity[:2] for identity in identities}) == len(normalized),
            f"qualification measurement {metric} repeats did not use distinct server instances",
        )
    else:
        require(
            len(set(identities)) == 1,
            f"qualification measurement {metric} changed server identity within its repeated block",
        )
    values = [sample.value for sample in normalized]
    aggregate = {
        "maximum": max,
        "minimum": min,
        "exact": lambda raw: raw[0],
    }[sample_policy["aggregation"]](values)
    return QualificationMetricMeasurement(metric, normalized, aggregate)


def _qualification_measurements(
    payload: dict,
    *,
    validation: MeasurementValidationContract,
) -> tuple[QualificationMetricMeasurement, ...]:
    metrics = payload["metrics"]
    require(
        isinstance(metrics, dict)
        and set(metrics) == validation.raw_metric_names,
        "qualification measurements did not contain exactly the 12 product-path metrics",
    )
    return tuple(
        _qualification_metric_measurement(
            metric,
            metrics[metric],
            validation=validation,
        )
        for metric in sorted(validation.raw_metric_names)
    )


def _verify_measurement_summary(evidence: QualificationMeasurementEvidence) -> None:
    require(
        evidence.summary.metric_count == len(evidence.metrics),
        "qualification measurement metric count is stale",
    )
    require(
        evidence.summary.sample_count == evidence.sample_count,
        "qualification measurement sample count is stale",
    )


def qualification_measurement_artifact(
    artifact_root: Path,
    summary: object,
    *,
    contracts: dict,
    measurement_contract: dict,
    target: str,
    proof_tier: str,
    matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
    forbidden_values: list[str],
) -> dict:
    normalized_summary, document = _qualification_measurement_document(
        artifact_root,
        summary,
        contracts=contracts,
        forbidden_values=forbidden_values,
    )
    validation = _measurement_validation_contract(
        contracts=contracts,
        measurement_contract=measurement_contract,
        target=target,
        proof_tier=proof_tier,
        matrix_cell_id=matrix_cell_id,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
    )
    evidence = QualificationMeasurementEvidence(
        normalized_summary,
        document,
        validation,
        _qualification_measurements(
            document.payload,
            validation=validation,
        ),
    )
    _verify_measurement_summary(evidence)
    return {
        "artifact": {
            "name": document.name,
            "sha256": hashlib.sha256(document.payload_bytes).hexdigest(),
        },
        "values": evidence.values,
        "unplanned_suspend": False,
        "matrix_cell_id": matrix_cell_id,
        "payload": document.payload,
    }
