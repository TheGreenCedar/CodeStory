"""Measurement and metric retention for qualification production."""

from __future__ import annotations

import os
import sys
from dataclasses import dataclass

from .archive import normalized_backend
from .contracts import (
    canonical_sha256,
    require_nonempty_string,
)
from .foundation import require
from .qualification import qualification_measurement_artifact
from .qualification_production import (
    QualificationExternalEvidence,
    QualificationProducerContext,
    QualificationRunnerEvidence,
)
from .runtime import metric_passes, retain_five_process_memory_evidence


@dataclass(frozen=True)
class QualificationMeasurementEvidence:
    measurement: dict
    memory: dict
    timing: dict
    host: dict
    metrics: dict[str, dict]


def _qualification_measurement_sources(
    context: QualificationProducerContext,
    runner: QualificationRunnerEvidence,
) -> tuple[dict, dict]:
    measurement = qualification_measurement_artifact(
        context.artifact_root,
        runner.output["measurements"],
        contracts=context.contracts,
        measurement_contract=context.measurement_contract,
        target=context.manifest["asset_target"],
        proof_tier=context.args.proof_tier,
        matrix_cell_id=runner.matrix_cell_id,
        expected_policy=context.args.engine_policy,
        expected_backend=runner.expected_backend,
        forbidden_values=context.forbidden_values,
    )
    memory = retain_five_process_memory_evidence(
        context.artifact_root,
        context.runtime.get("_memory_observations"),
        source=context.manifest["source"],
        package=context.package,
        contracts=context.contracts,
        protocol=context.measurement_contract["measurement_protocol"],
        target=context.manifest["asset_target"],
        proof_tier=context.args.proof_tier,
        matrix_cell_id=runner.matrix_cell_id,
        expected_policy=context.args.engine_policy,
        expected_backend=runner.expected_backend,
        forbidden_values=context.forbidden_values,
    )
    return measurement, memory


def _qualification_host(
    context: QualificationProducerContext,
    runner: QualificationRunnerEvidence,
    measurement: dict,
) -> dict:
    identity = context.runtime["identity"]
    cache_state = (
        "reused"
        if context.runtime["materialization"]["reused_on_rejoin"] is True
        else "materialized"
    )
    residency_state = require_nonempty_string(
        identity["embedding_engine_residency"],
        "runtime engine residency",
    )
    platform = (
        f"{sys.platform}:{os.uname().machine}"
        if hasattr(os, "uname")
        else sys.platform
    )
    return {
        "fingerprint": canonical_sha256(
            {
                "platform": platform,
                "target": context.manifest["asset_target"],
                "account_id": context.runtime["same_account"]["account_id"],
                "backend": normalized_backend(runner.expected_backend),
                "policy": context.args.engine_policy,
            }
        ),
        "platform": platform,
        "target": context.manifest["asset_target"],
        "matrix_cell_id": runner.matrix_cell_id,
        "host_class": runner.matrix_cell["host_class"],
        "accelerator_claim": runner.matrix_cell["accelerator_claim"],
        "backend": identity["embedding_backend"],
        "policy": context.args.engine_policy,
        "cache_state": cache_state,
        "residency_state": residency_state,
        "unplanned_suspend": measurement["unplanned_suspend"],
    }


def _qualification_metric_value(
    metric: str,
    *,
    external: QualificationExternalEvidence,
    measurement: dict,
    memory: dict,
) -> float | int | None:
    if metric == "retrieval_quality":
        return (
            external.retrieval_quality["publishable_packet_pass_rate"]
            if external.retrieval_quality is not None
            else None
        )
    if metric == "total_codestory_process_memory":
        return memory["value"]
    return measurement["values"][metric]


def _qualification_raw_metric_evidence(
    metric: str,
    *,
    external: QualificationExternalEvidence,
    measurement: dict,
    memory: dict,
) -> dict:
    if metric == "retrieval_quality":
        require(
            external.retrieval_quality is not None,
            "qualification retrieval quality omitted publishable packet evidence",
        )
        return external.retrieval_quality
    if metric == "total_codestory_process_memory":
        return memory["artifact"]
    return measurement["artifact"]


def _retained_qualification_metric(
    metric: str,
    *,
    context: QualificationProducerContext,
    external: QualificationExternalEvidence,
    measurement: dict,
    memory: dict,
) -> dict:
    protocol = context.measurement_contract["measurement_protocol"]
    contract = protocol["metric_contracts"][metric]
    value = _qualification_metric_value(
        metric,
        external=external,
        measurement=measurement,
        memory=memory,
    )
    if metric == "retrieval_quality" and value is None:
        require(
            context.args.proof_tier == "calibration",
            "qualification retrieval quality omitted publishable packet evidence",
        )
        return {
            "status": "not_measured",
            "unit": contract["unit"],
            "value": None,
            "reason": (
                "calibration omitted the separately produced exact-head "
                "publishable packet artifact"
            ),
        }
    require(
        isinstance(value, (int, float)) and not isinstance(value, bool),
        f"qualification metric {metric} is not numeric",
    )
    raw_evidence = _qualification_raw_metric_evidence(
        metric,
        external=external,
        measurement=measurement,
        memory=memory,
    )
    if context.args.proof_tier == "calibration":
        return {
            "status": "calibration",
            "unit": contract["unit"],
            "value": value,
            "raw_evidence": raw_evidence,
        }
    threshold = context.measurement_contract["constant_set"][
        "qualification_thresholds"
    ][metric]
    require(
        isinstance(threshold, (int, float)) and not isinstance(threshold, bool),
        f"qualification metric {metric} has no frozen threshold",
    )
    comparison = contract["comparison"]
    require(
        metric_passes(value, threshold, comparison),
        f"qualification metric {metric} failed its frozen threshold",
    )
    return {
        "status": "pass",
        "unit": contract["unit"],
        "value": value,
        "threshold": threshold,
        "comparison": comparison,
        "raw_evidence": raw_evidence,
    }


def collect_qualification_measurements(
    context: QualificationProducerContext,
    runner: QualificationRunnerEvidence,
    external: QualificationExternalEvidence,
) -> QualificationMeasurementEvidence:
    measurement, memory = _qualification_measurement_sources(context, runner)
    timing = {
        "clock_domain": "awake_monotonic",
        "cross_process_timestamp_subtraction": False,
        "unplanned_suspend": measurement["unplanned_suspend"],
        "constants_frozen_before_run": (
            context.measurement_contract["constant_set"]["status"] == "frozen"
        ),
        "constant_set_sha256": context.contracts["constant_set_sha256"],
    }
    metrics = {
        metric: _retained_qualification_metric(
            metric,
            context=context,
            external=external,
            measurement=measurement,
            memory=memory,
        )
        for metric in sorted(
            context.measurement_contract["measurement_protocol"][
                "required_metrics"
            ]
        )
    }
    return QualificationMeasurementEvidence(
        measurement,
        memory,
        timing,
        _qualification_host(context, runner, measurement),
        metrics,
    )
