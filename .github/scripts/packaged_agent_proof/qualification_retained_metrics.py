"""Retained scenario, nonclaim, and measurement verification."""

from __future__ import annotations

from pathlib import Path

from .contract_primitives import (
    require_exact_keys,
    require_nonempty_string,
    require_positive_int,
    require_sha256,
)
from .foundation import (
    LOWER_TIER_NONCLAIMS,
    MIN_RETRIEVAL_QUALITY_REPEATS,
    RELEASE_QUALITY_CORPUS_ID,
    REQUIRED_SERVER_SCENARIOS,
    RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
    require,
)
from .qualification_retained_types import (
    RetainedMeasurementBinding,
    RetainedMetric,
    RetainedQualificationContract,
)


def _verified_scenario_artifact_names(scenario_id: str, artifacts: object) -> set[str]:
    require(
        isinstance(artifacts, list) and artifacts,
        f"scenario {scenario_id} has no retained artifacts",
    )
    names = set()
    for artifact in artifacts:
        require(
            isinstance(artifact, dict), f"scenario {scenario_id} artifact is malformed"
        )
        name = require_nonempty_string(
            artifact.get("name"),
            f"scenario {scenario_id} artifact name",
        )
        require(
            Path(name).name == name and Path(name).suffix == ".json",
            f"scenario {scenario_id} artifact name is not a safe JSON basename",
        )
        require_sha256(
            artifact.get("sha256"), f"scenario {scenario_id} artifact sha256"
        )
        names.add(name)
    return names


def _verify_scenarios(contract: RetainedQualificationContract) -> None:
    scenarios = contract.evidence.scenarios
    scenario_contracts = contract.measurement_contract["measurement_protocol"][
        "scenario_contracts"
    ]
    require(
        set(scenarios) == REQUIRED_SERVER_SCENARIOS,
        "retained qualification scenario set is incomplete",
    )
    for scenario_id in sorted(REQUIRED_SERVER_SCENARIOS):
        scenario = scenarios.get(scenario_id)
        require(isinstance(scenario, dict), f"scenario {scenario_id} is malformed")
        require(
            scenario.get("status") == "pass", f"scenario {scenario_id} did not pass"
        )
        assertions = scenario.get("assertions")
        require(
            isinstance(assertions, dict),
            f"scenario {scenario_id} omitted assertions",
        )
        required_assertions = set(scenario_contracts[scenario_id]["required"])
        require(
            set(assertions) == required_assertions,
            f"scenario {scenario_id} assertions do not match the preregistered contract",
        )
        failed = sorted(
            name for name, passed in assertions.items() if passed is not True
        )
        require(
            not failed,
            f"scenario {scenario_id} has failed assertions: " + ", ".join(failed),
        )
        artifact_names = _verified_scenario_artifact_names(
            scenario_id,
            scenario.get("artifacts"),
        )
        if scenario_id in {"server_crash", "worker_stall"}:
            require(
                "publication-fault-external.raw.json" in artifact_names,
                f"{scenario_id} scenario omitted separately hashed publication-fence evidence",
            )


def _verify_nonclaims(contract: RetainedQualificationContract) -> None:
    nonclaims = contract.evidence.lower_tier_nonclaims
    require(
        set(nonclaims) == LOWER_TIER_NONCLAIMS,
        "retained qualification nonclaim set is incomplete",
    )
    for claim, record in nonclaims.items():
        require(isinstance(record, dict), f"nonclaim {claim} is malformed")
        require(
            record.get("claimed") is False,
            f"lower-tier evidence incorrectly claims {claim}",
        )
        require_nonempty_string(record.get("reason"), f"nonclaim {claim} reason")


def _normalized_retained_metrics(
    contract: RetainedQualificationContract,
) -> tuple[RetainedMetric, ...]:
    metrics = contract.evidence.metrics
    protocol = contract.measurement_contract["measurement_protocol"]
    required_metrics = set(protocol["required_metrics"])
    require(
        set(metrics) == required_metrics,
        "retained qualification metric set is incomplete",
    )
    thresholds = contract.measurement_contract["constant_set"][
        "qualification_thresholds"
    ]
    metric_contracts = protocol["metric_contracts"]
    normalized = []
    for metric, result in metrics.items():
        require(isinstance(result, dict), f"metric {metric} is malformed")
        require(
            result.get("status") == "pass",
            f"metric {metric} did not pass its frozen threshold",
        )
        require(
            result.get("unit") == metric_contracts[metric]["unit"],
            f"metric {metric} used the wrong unit",
        )
        value = result.get("value")
        require(
            isinstance(value, (int, float)) and not isinstance(value, bool),
            f"metric {metric} value is not numeric",
        )
        threshold = result.get("threshold")
        require(
            threshold == thresholds[metric]
            and isinstance(threshold, (int, float))
            and not isinstance(threshold, bool),
            f"metric {metric} threshold does not match the frozen constant set",
        )
        comparison = metric_contracts[metric]["comparison"]
        require(
            result.get("comparison") == comparison,
            f"metric {metric} used the wrong comparison",
        )
        raw_evidence = result.get("raw_evidence")
        require(
            isinstance(raw_evidence, dict),
            (
                "retrieval quality metric omitted raw evidence"
                if metric == "retrieval_quality"
                else f"metric {metric} omitted its raw measurement artifact"
            ),
        )
        normalized.append(
            RetainedMetric(metric, value, threshold, comparison, raw_evidence)
        )
    return tuple(normalized)


def _verify_retrieval_quality_metric(
    contract: RetainedQualificationContract,
    metric: RetainedMetric,
) -> None:
    raw_evidence = metric.raw_evidence
    require_exact_keys(
        raw_evidence,
        {
            "artifact",
            "evaluation_contract",
            "source_commit",
            "source_tree",
            "corpus_id",
            "holdout_manifest_set_sha256",
            "repeats",
            "row_count",
            "passing_row_count",
            "publishable_packet_pass_rate",
        },
        "retrieval quality retained raw evidence",
    )
    artifact = raw_evidence["artifact"]
    require(isinstance(artifact, dict), "retrieval quality raw artifact is malformed")
    require_exact_keys(artifact, {"name", "sha256"}, "retrieval quality raw artifact")
    require(
        artifact["name"] == "packet-runtime-summary.json",
        "retrieval quality raw artifact name is invalid",
    )
    require_sha256(artifact["sha256"], "retrieval quality raw artifact sha256")
    require(
        raw_evidence["evaluation_contract"] == RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
        "retrieval quality retained evaluation contract changed",
    )
    require(
        raw_evidence["source_commit"] == contract.evidence.source["commit"]
        and raw_evidence["source_tree"] == contract.evidence.source["tree"],
        "retrieval quality retained source identity is stale",
    )
    require(
        require_positive_int(raw_evidence["repeats"], "retrieval quality repeats")
        == MIN_RETRIEVAL_QUALITY_REPEATS,
        "retrieval quality retained the wrong repeat count",
    )
    require(
        raw_evidence["corpus_id"] == RELEASE_QUALITY_CORPUS_ID,
        "retrieval quality retained the wrong holdout corpus",
    )
    require_sha256(
        raw_evidence["holdout_manifest_set_sha256"],
        "retrieval quality holdout manifest set sha256",
    )
    row_count = require_positive_int(
        raw_evidence["row_count"],
        "retrieval quality row count",
    )
    require(
        require_positive_int(
            raw_evidence["passing_row_count"],
            "retrieval quality passing row count",
        )
        == row_count,
        "retrieval quality retained a failing row",
    )
    pass_rate = raw_evidence["publishable_packet_pass_rate"]
    require(
        isinstance(pass_rate, (int, float)) and not isinstance(pass_rate, bool),
        "retrieval quality pass rate is not numeric",
    )
    require(
        pass_rate == metric.value,
        "retrieval quality metric does not match its raw evidence",
    )


def _verify_measurement_metric(metric: RetainedMetric) -> None:
    require_exact_keys(
        metric.raw_evidence,
        {"name", "sha256"},
        f"metric {metric.name} raw measurement artifact",
    )
    expected_artifact_name = (
        "total-codestory-process-memory.raw.json"
        if metric.name == "total_codestory_process_memory"
        else "measurements.raw.json"
    )
    require(
        metric.raw_evidence["name"] == expected_artifact_name,
        f"metric {metric.name} used the wrong raw measurement artifact",
    )
    require_sha256(
        metric.raw_evidence["sha256"],
        f"metric {metric.name} raw artifact sha256",
    )


def _verify_metrics(
    contract: RetainedQualificationContract,
) -> tuple[RetainedMetric, ...]:
    metrics = _normalized_retained_metrics(contract)
    for metric in metrics:
        if metric.name == "retrieval_quality":
            _verify_retrieval_quality_metric(contract, metric)
        else:
            _verify_measurement_metric(metric)
        passed = {
            "equal": metric.value == metric.threshold,
            "greater_than_or_equal": metric.value >= metric.threshold,
            "less_than_or_equal": metric.value <= metric.threshold,
        }[metric.comparison]
        require(
            passed,
            f"metric {metric.name} value failed its frozen comparison",
        )
    return metrics


def _verify_measurement_binding(
    contract: RetainedQualificationContract,
) -> RetainedMeasurementBinding:
    _verify_scenarios(contract)
    _verify_nonclaims(contract)
    metrics = _verify_metrics(contract)
    retained = contract.evidence
    return RetainedMeasurementBinding(
        retained.scenarios,
        retained.lower_tier_nonclaims,
        metrics,
    )
